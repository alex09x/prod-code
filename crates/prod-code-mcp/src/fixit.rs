//! Applying the compiler's own fixes from a failing `check` or `lint`, without a prompt.
//!
//! rustc and clippy attach suggestions to their diagnostics, and mark each with how sure they
//! are. Only `MachineApplicable` ones are taken: the compiler vouches that applying them keeps
//! the program meaning what the author meant (an unused import removed, a needless `mut`
//! dropped, `&x.clone()` replaced). A suggestion can have several parts (add `(` here and `)`
//! there); its parts are applied together or not at all. The file must still read as it did
//! when the compiler saw it, or the fix is skipped.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// One part of a machine-applicable suggestion: replace `start..end` (bytes) of `file` with
/// `replacement`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edit {
    pub file: String,
    pub start: usize,
    pub end: usize,
    pub line: u64,
    /// The source line the compiler saw at `line`, to tell a stale suggestion from a live one.
    pub line_text: Option<String>,
    pub replacement: String,
}

/// A suggestion the compiler marked `MachineApplicable`, with every part of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fix {
    pub level: String,
    pub code: Option<String>,
    /// The diagnostic's message, then the suggestion's own (`remove the whole `use` item`).
    pub message: String,
    pub edits: Vec<Edit>,
}

fn machine_applicable(span: &serde_json::Value) -> Option<Edit> {
    if span.get("suggestion_applicability")?.as_str()? != "MachineApplicable" {
        return None;
    }
    Some(Edit {
        file: span.get("file_name")?.as_str()?.to_string(),
        start: span.get("byte_start")?.as_u64()? as usize,
        end: span.get("byte_end")?.as_u64()? as usize,
        line: span.get("line_start")?.as_u64()?,
        line_text: span
            .pointer("/text/0/text")
            .and_then(|t| t.as_str())
            .map(str::to_string),
        replacement: span.get("suggested_replacement")?.as_str()?.to_string(),
    })
}

/// The machine-applicable fixes in one `cargo --message-format=json` line: one per suggestion
/// (a child of the diagnostic), with all of its parts.
pub fn parse_fixes(line: &str) -> Vec<Fix> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
        return Vec::new();
    };
    if value.get("reason").and_then(|r| r.as_str()) != Some("compiler-message") {
        return Vec::new();
    }
    let Some(message) = value.get("message") else {
        return Vec::new();
    };
    let level = message
        .get("level")
        .and_then(|l| l.as_str())
        .unwrap_or("")
        .to_string();
    let code = message
        .pointer("/code/code")
        .and_then(|c| c.as_str())
        .map(str::to_string);
    let text = message
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();
    let mut out = Vec::new();
    let mut take = |node: &serde_json::Value, own: Option<&str>| {
        let edits: Vec<Edit> = node
            .get("spans")
            .and_then(|s| s.as_array())
            .into_iter()
            .flatten()
            .filter_map(machine_applicable)
            .collect();
        if !edits.is_empty() {
            out.push(Fix {
                level: level.clone(),
                code: code.clone(),
                message: match own {
                    Some(own) if !own.is_empty() && own != text => format!("{text}: {own}"),
                    _ => text.clone(),
                },
                edits,
            });
        }
    };
    take(message, None);
    for child in message
        .get("children")
        .and_then(|c| c.as_array())
        .into_iter()
        .flatten()
    {
        take(child, child.get("message").and_then(|m| m.as_str()));
    }
    out
}

/// A fix that was applied, or why it was not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Outcome {
    pub file: String,
    pub line: u64,
    pub message: String,
    /// None when applied; otherwise why not.
    pub skipped: Option<String>,
}

/// The new text of every file the fixes touch, and what happened to each fix. A fix whose parts
/// fall outside the workspace, overlap an earlier fix, or no longer match the line the compiler
/// saw is skipped whole.
pub fn plan(root: &Path, fixes: &[Fix]) -> (BTreeMap<PathBuf, String>, Vec<Outcome>) {
    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut taken: BTreeMap<PathBuf, Vec<(usize, usize)>> = BTreeMap::new();
    let mut accepted: Vec<&Fix> = Vec::new();
    let mut outcomes = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for fix in fixes {
        // The same suggestion is reported once per target that compiles the file.
        if !seen.insert(format!("{:?}", fix.edits)) {
            continue;
        }
        let first = &fix.edits[0];
        let outcome = |skipped: Option<String>| Outcome {
            file: first.file.clone(),
            line: first.line,
            message: fix.message.clone(),
            skipped,
        };
        let mut why = None;
        for e in &fix.edits {
            let path = root.join(&e.file);
            if Path::new(&e.file).is_absolute() || e.file.starts_with("..") || !path.is_file() {
                why = Some(format!("{} is not a file of this workspace", e.file));
                break;
            }
            let text = texts
                .entry(path.clone())
                .or_insert_with(|| std::fs::read_to_string(&path).unwrap_or_default());
            let line_now = text.lines().nth(e.line.saturating_sub(1) as usize);
            if e.end > text.len()
                || e.start > e.end
                || !text.is_char_boundary(e.start)
                || !text.is_char_boundary(e.end)
                || e.line_text.as_deref().is_some_and(|t| Some(t) != line_now)
            {
                why = Some("the file changed since the compiler read it".to_string());
                break;
            }
            let spans = taken.entry(path).or_default();
            if spans
                .iter()
                .any(|(s, en)| e.start < *en && *s < e.end.max(e.start + 1))
            {
                why = Some("it overlaps a fix already taken".to_string());
                break;
            }
        }
        if why.is_none() {
            for e in &fix.edits {
                taken
                    .entry(root.join(&e.file))
                    .or_default()
                    .push((e.start, e.end.max(e.start + 1)));
            }
            accepted.push(fix);
        }
        outcomes.push(outcome(why));
    }
    let mut out = BTreeMap::new();
    let mut edits: BTreeMap<PathBuf, Vec<&Edit>> = BTreeMap::new();
    for fix in accepted {
        for e in &fix.edits {
            edits.entry(root.join(&e.file)).or_default().push(e);
        }
    }
    for (path, mut list) in edits {
        let mut text = texts.get(&path).cloned().unwrap_or_default();
        list.sort_by_key(|e| std::cmp::Reverse(e.start));
        for e in list {
            text.replace_range(e.start..e.end, &e.replacement);
        }
        out.insert(path, text);
    }
    (out, outcomes)
}

/// What a fix pass did: the fixes, and the check run again after them.
#[derive(Debug, Clone, Serialize)]
pub struct Fixed {
    pub before: crate::verify::VerifyReport,
    pub outcomes: Vec<Outcome>,
    pub after: Option<crate::verify::VerifyReport>,
}

impl Fixed {
    pub fn render(&self, max_items: usize) -> String {
        let mut out = self.before.render(max_items);
        let applied = self.outcomes.iter().filter(|o| o.skipped.is_none()).count();
        out.push_str(&format!(
            "\nmachine-applicable fixes: {applied} applied, {} skipped\n",
            self.outcomes.len() - applied
        ));
        for o in &self.outcomes {
            match &o.skipped {
                None => out.push_str(&format!("  fixed {}:{}: {}\n", o.file, o.line, o.message)),
                Some(why) => out.push_str(&format!(
                    "  skipped {}:{}: {} ({why})\n",
                    o.file, o.line, o.message
                )),
            }
        }
        if let Some(after) = &self.after {
            out.push_str("\nafter the fixes:\n");
            out.push_str(&after.render(max_items));
        }
        out
    }

    pub fn ok(&self) -> bool {
        self.after.as_ref().unwrap_or(&self.before).ok()
    }
}

/// Runs `kind` (check or lint), applies every machine-applicable fix it reports to the
/// checkout, and runs it again to show what is left.
pub async fn check_and_fix(
    remote: SocketAddr,
    root: &Path,
    hint: Option<&Path>,
    kind: crate::verify::VerifyKind,
    timeout_secs: u64,
) -> Result<Fixed> {
    let before = crate::verify::run_verify(remote, root, hint, kind, None, timeout_secs).await?;
    let (files, outcomes) = plan(root, &before.fixes);
    if files.is_empty() {
        return Ok(Fixed {
            before,
            outcomes,
            after: None,
        });
    }
    crate::refactor::apply_workspace_edit(root, &crate::signature::whole_file_edit(&files))?;
    let after = crate::verify::run_verify(remote, root, hint, kind, None, timeout_secs).await?;
    Ok(Fixed {
        before,
        outcomes,
        after: Some(after),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // An unused import, as cargo reports it: the suggestion is in a child.
    const UNUSED: &str = r#"{"reason":"compiler-message","message":{"level":"warning","code":{"code":"unused_imports"},"message":"unused import: `std::fmt`","spans":[{"file_name":"src/lib.rs","byte_start":4,"byte_end":12,"line_start":1,"is_primary":true,"text":[{"text":"use std::fmt;"}],"suggested_replacement":null,"suggestion_applicability":null}],"children":[{"message":"remove the whole `use` item","spans":[{"file_name":"src/lib.rs","byte_start":0,"byte_end":14,"line_start":1,"is_primary":true,"text":[{"text":"use std::fmt;"}],"suggested_replacement":"","suggestion_applicability":"MachineApplicable"}],"children":[]}]}}"#;

    #[test]
    fn only_machine_applicable_suggestions_are_taken() {
        let fixes = parse_fixes(UNUSED);
        assert_eq!(fixes.len(), 1);
        assert_eq!(
            fixes[0].message,
            "unused import: `std::fmt`: remove the whole `use` item"
        );
        assert_eq!((fixes[0].edits[0].start, fixes[0].edits[0].end), (0, 14));
        let maybe = UNUSED.replace("MachineApplicable", "MaybeIncorrect");
        assert!(parse_fixes(&maybe).is_empty());
        assert!(parse_fixes(r#"{"reason":"build-finished","success":true}"#).is_empty());
        assert!(parse_fixes("not json").is_empty());
    }

    #[test]
    fn a_fix_is_applied_whole_once_and_only_to_the_text_it_was_made_for() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "use std::fmt;\nfn f() {}\n").unwrap();
        let fix = parse_fixes(UNUSED).remove(0);
        // Reported twice (lib and test targets), applied once.
        let (files, outcomes) = plan(dir.path(), &[fix.clone(), fix.clone()]);
        assert_eq!(files[&dir.path().join("src/lib.rs")], "fn f() {}\n");
        assert_eq!(outcomes.len(), 1);
        assert!(outcomes[0].skipped.is_none());

        // Two parts of one suggestion go in together; an overlapping second fix is skipped.
        let parens = Fix {
            edits: vec![
                Edit {
                    start: 3,
                    end: 3,
                    replacement: "(".into(),
                    ..fix.edits[0].clone()
                },
                Edit {
                    start: 12,
                    end: 12,
                    replacement: ")".into(),
                    ..fix.edits[0].clone()
                },
            ],
            ..fix.clone()
        };
        let (files, outcomes) = plan(dir.path(), &[parens, fix.clone()]);
        assert_eq!(
            files[&dir.path().join("src/lib.rs")],
            "use( std::fmt);\nfn f() {}\n"
        );
        assert_eq!(
            outcomes[1].skipped.as_deref(),
            Some("it overlaps a fix already taken")
        );

        // The line is not what the compiler saw, or the file is not the workspace's.
        std::fs::write(dir.path().join("src/lib.rs"), "use std::io;\nfn f() {}\n").unwrap();
        let (files, outcomes) = plan(dir.path(), std::slice::from_ref(&fix));
        assert!(files.is_empty());
        assert_eq!(
            outcomes[0].skipped.as_deref(),
            Some("the file changed since the compiler read it")
        );
        let outside = Fix {
            edits: vec![Edit {
                file: "/registry/src/x.rs".into(),
                ..fix.edits[0].clone()
            }],
            ..fix
        };
        let (_, outcomes) = plan(dir.path(), &[outside]);
        assert!(
            outcomes[0]
                .skipped
                .as_deref()
                .unwrap()
                .contains("not a file of this workspace")
        );
    }

    #[test]
    fn the_report_says_what_was_fixed_skipped_and_left() {
        let report = |exit: i32| crate::verify::VerifyReport {
            kind: crate::verify::VerifyKind::Lint,
            language: "rust".into(),
            command: vec!["cargo".into(), "clippy".into()],
            exit_code: Some(exit),
            timed_out: false,
            duration_ms: 100,
            diagnostics: vec![],
            tests_passed: 0,
            tests_failed: 0,
            failures: vec![],
            tail: String::new(),
            fixes: vec![],
        };
        let outcome = |skipped: Option<&str>| Outcome {
            file: "src/lib.rs".into(),
            line: 1,
            message: "unused import".into(),
            skipped: skipped.map(str::to_string),
        };
        let fixed = Fixed {
            before: report(101),
            outcomes: vec![
                outcome(None),
                outcome(Some("it overlaps a fix already taken")),
            ],
            after: Some(report(0)),
        };
        let text = fixed.render(10);
        for expected in [
            "machine-applicable fixes: 1 applied, 1 skipped",
            "  fixed src/lib.rs:1: unused import",
            "  skipped src/lib.rs:1: unused import (it overlaps a fix already taken)",
            "after the fixes:",
        ] {
            assert!(text.contains(expected), "{expected}\n{text}");
        }
        assert!(fixed.ok());
        let nothing = Fixed {
            after: None,
            ..fixed
        };
        assert!(!nothing.ok() && !nothing.render(10).contains("after the fixes"));
    }
}
