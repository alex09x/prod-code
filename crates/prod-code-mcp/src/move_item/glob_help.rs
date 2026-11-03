/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Explain unsupported glob dependencies when the proposed destination has unresolved names.
//! Copying every glob would introduce ambiguous bindings and change unrelated destination code.

use super::types::Move;
use anyhow::{Context, Result};
use ra_ap_syntax::{AstNode, Edition, SourceFile, ast};

fn guidance(source: &str, diagnostics: &[String], destination: &str) -> Option<String> {
    if !diagnostics.iter().any(|d| {
        d.contains(destination)
            && ["unresolved", "cannot find", "not found", "in this scope"]
                .iter()
                .any(|word| d.contains(word))
    }) {
        return None;
    }
    let parsed = SourceFile::parse(source, Edition::Edition2024);
    let imports: Vec<String> = parsed
        .syntax_node()
        .children()
        .filter_map(ast::Use::cast)
        .filter(|item| {
            item.use_tree().is_some_and(|tree| {
                tree.syntax()
                    .descendants()
                    .filter_map(ast::UseTree::cast)
                    .any(|tree| tree.star_token().is_some())
            })
        })
        .map(|item| {
            item.syntax()
                .text()
                .to_string()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .take(4)
        .collect();
    if imports.is_empty() {
        return None;
    }
    Some(format!(
        "The destination has unresolved names and the source imports glob bindings ({}). \
        This move does not resolve or carry dependencies supplied by glob imports. Add only the \
        required explicit imports to `{destination}`, then retry with `apply: false` before \
        applying. The listed globs are possible dependencies, not proof of which binding is missing.",
        imports.join("; ")
    ))
}

pub(super) fn finish(source: &str, result: &mut Move, apply: bool, force: bool) -> Result<()> {
    let help = guidance(source, &result.diagnostics, &result.to);
    if let Some(help) = &help {
        result.left_alone.push(help.clone());
    }
    if apply {
        let written = result.write(force);
        if let Some(help) = help {
            written.with_context(|| help)?;
        } else {
            written?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_1827_unresolved_destination_names_explain_nested_and_plain_globs() {
        let source = "use crate::helpers::*;\nuse crate::{types::*, named::Thing};\nfn moved() { missing(); }\n";
        let errors = vec!["cannot find function `missing` in this scope (src/dest.rs:1:1)".into()];
        let message = guidance(source, &errors, "src/dest.rs").unwrap();
        assert!(message.contains("use crate::helpers::*"));
        assert!(message.contains("types::*"));
        assert!(message.contains("required explicit imports"));
        assert!(message.contains("apply: false"));
        assert!(message.contains("possible dependencies"));
    }

    #[test]
    fn issue_1827_successful_moves_and_unrelated_errors_do_not_blame_glob_imports() {
        assert!(guidance("use crate::helpers::*;", &[], "dest.rs").is_none());
        assert!(
            guidance(
                "use crate::helpers::*;",
                &["mismatched types (dest.rs:1:1)".into()],
                "dest.rs"
            )
            .is_none()
        );
        assert!(
            guidance(
                "use crate::helpers::*;",
                &["unresolved import (other.rs:1:1)".into()],
                "dest.rs"
            )
            .is_none()
        );
        assert!(
            guidance(
                "fn f() { use crate::helpers::*; }",
                &["unresolved import (dest.rs:1:1)".into()],
                "dest.rs"
            )
            .is_none()
        );
    }

    #[test]
    fn issue_1827_apply_refusal_preserves_files_and_reports_actionable_glob_context() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("source.rs");
        let before = "use crate::helpers::*;\nfn moved() {}\n";
        std::fs::write(&path, before).unwrap();
        let mut result = Move {
            symbol: "moved".into(),
            root: temp.path().into(),
            from: "source.rs".into(),
            from_module: "crate".into(),
            to: "dest.rs".into(),
            to_module: "crate::dest".into(),
            new_path: "crate::dest::moved".into(),
            moved_lines: 1,
            rewritten: vec![(path.to_string_lossy().into_owned(), "changed".into())],
            imports: vec![],
            left_alone: vec![],
            unmatched: vec![],
            diagnostics: vec!["unresolved import (dest.rs:1:1)".into()],
            applied: false,
            created: None,
        };
        let error = format!(
            "{:#}",
            finish(before, &mut result, true, false).unwrap_err()
        );
        assert!(
            error.contains("dependencies supplied by glob imports"),
            "{error}"
        );
        assert!(error.contains("nothing was written"), "{error}");
        assert_eq!(std::fs::read_to_string(path).unwrap(), before);
        assert!(!result.applied);
    }
}
