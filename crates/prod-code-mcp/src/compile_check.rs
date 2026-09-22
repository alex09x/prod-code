//! Asking the compiler, not the analyzer, whether a proposed edit builds — before it is written.
//!
//! The overlay check every write tool runs is fast and good at what it models, and silent about
//! an unresolved type or a module path that does not exist (#63). For a change that introduces a
//! name, that silence is exactly where it goes wrong. So a caller that wants the compiler's word
//! gets it: the proposed files become one hypothesis in a shadow of the workspace copy, `cargo
//! check` runs there against the warm target directory, and nothing in the checkout is touched
//! unless it passes.

use crate::shadow::{HypothesisEdit, HypothesisSpec, relative_edit_path, run_shadow};
use anyhow::Result;
use std::net::SocketAddr;
use std::path::Path;

/// The compiler's opinion of a proposed set of files.
#[derive(Debug, Clone)]
pub struct CompileVerdict {
    pub passed: bool,
    /// One line per error, `file:line:col: message`, in the compiler's order.
    pub errors: Vec<String>,
    pub duration_ms: u64,
}

impl CompileVerdict {
    /// How the verdict reads at the end of a tool's report.
    pub fn render(&self) -> String {
        if self.passed {
            return format!(
                "\nthe compiler accepts the result too: `cargo check` in a shadow of the workspace, \
                 {} ms\n",
                self.duration_ms
            );
        }
        let mut out = format!(
            "\nthe compiler rejects the result (`cargo check` in a shadow of the workspace, {} ms):\n",
            self.duration_ms
        );
        for e in &self.errors {
            out.push_str(&format!("  {e}\n"));
        }
        if self.errors.is_empty() {
            out.push_str("  (it failed without an error line this could read; see the build)\n");
        }
        out
    }
}

/// The error lines of `cargo check --message-format=short`: `path:line:col: error[CODE]: text`.
///
/// Warnings are not errors, and the summary lines cargo prints at the end (`error: could not
/// compile ...`) name no position, so both are left out.
pub fn short_errors(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| {
            let Some((position, rest)) = line.split_once(": error") else {
                return false;
            };
            // A position is `path:line:col`, with the last two numeric.
            let mut parts = position.rsplitn(3, ':');
            let col = parts.next().unwrap_or("");
            let row = parts.next().unwrap_or("");
            !rest.is_empty()
                && !col.is_empty()
                && col.chars().all(|c| c.is_ascii_digit())
                && !row.is_empty()
                && row.chars().all(|c| c.is_ascii_digit())
        })
        .map(str::to_string)
        .collect()
}

/// Runs `cargo check` against the workspace with `files` in place of what is on disk.
pub async fn check(
    remote: SocketAddr,
    root: &Path,
    files: &[(String, String)],
) -> Result<CompileVerdict> {
    let mut edits = Vec::with_capacity(files.len());
    for (path, text) in files {
        edits.push(HypothesisEdit {
            relative_path: relative_edit_path(root, Path::new(path))?,
            text: Some(text.clone()),
        });
    }
    let spec = HypothesisSpec {
        name: "proposed".to_string(),
        edits,
    };
    let command = [
        "cargo",
        "check",
        "--workspace",
        "--all-targets",
        "--message-format=short",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let outcome = run_shadow(
        remote,
        root,
        None,
        &[spec],
        command,
        Vec::new(),
        600,
        1,
        64 * 1024,
    )
    .await?;
    let result = outcome
        .results
        .first()
        .ok_or_else(|| anyhow::anyhow!("the shadow run returned no result"))?;
    Ok(CompileVerdict {
        passed: result.passed(),
        errors: short_errors(&result.output),
        duration_ms: result.duration_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_positioned_error_lines_are_errors() {
        let output = "\
crates/x/src/lib.rs:12:5: error[E0422]: cannot find struct, variant or union type `Opts` in this scope
crates/x/src/lib.rs:3:9: warning: unused import: `std::fmt`
error: could not compile `x` (lib) due to 1 previous error
crates/x/src/other.rs:40:1: error: expected item, found `}`
";
        assert_eq!(
            short_errors(output),
            vec![
                "crates/x/src/lib.rs:12:5: error[E0422]: cannot find struct, variant or union type `Opts` in this scope",
                "crates/x/src/other.rs:40:1: error: expected item, found `}`",
            ]
        );
        assert!(short_errors("").is_empty());
    }

    #[test]
    fn the_verdict_says_which_check_and_what_it_found() {
        let ok = CompileVerdict {
            passed: true,
            errors: Vec::new(),
            duration_ms: 1840,
        };
        assert!(ok.render().contains("the compiler accepts the result too"));
        assert!(ok.render().contains("1840 ms"));

        let broken = CompileVerdict {
            passed: false,
            errors: vec!["src/lib.rs:12:5: error[E0422]: cannot find struct `Opts`".into()],
            duration_ms: 2100,
        };
        let text = broken.render();
        assert!(text.contains("the compiler rejects the result"), "{text}");
        assert!(text.contains("E0422"), "{text}");

        let silent = CompileVerdict {
            passed: false,
            errors: Vec::new(),
            duration_ms: 10,
        };
        assert!(silent.render().contains("without an error line"));
    }
}
