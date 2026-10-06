/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::types::{Edit, Fix, Outcome};

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
