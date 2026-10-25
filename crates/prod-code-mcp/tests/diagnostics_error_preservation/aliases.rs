/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;
use prod_code_mcp::diagnostics::{parse_items, set_aside_preexisting};

#[tokio::test]
async fn supported_language_aliases_preserve_harmless_comment_shifts() {
    let mut failures = Vec::new();
    for (extensions, before, code) in [
        (
            &["ts", "mts", "cts", "tsx", "js", "mjs", "cjs", "jsx"][..],
            "function first() {\n    missing();\n}\n",
            "2304",
        ),
        (
            &["cpp", "hpp", "cc", "cxx", "hh", "hxx", "ixx"][..],
            "void first() {\n    missing();\n}\n",
            "undeclared_var_use",
        ),
        (
            &["kt", "kts"][..],
            "fun first() {\n    missing();\n}\n",
            "UNRESOLVED_REFERENCE",
        ),
    ] {
        for extension in extensions {
            let path = format!("src/main.{extension}");
            let ws = Workspace::new(&[(&path, before)]);
            let diagnostic = move |text: &str| {
                let line = text
                    .lines()
                    .position(|line| line.contains("missing"))
                    .unwrap() as u32
                    + 1;
                answers::error_at(line, 5, code, "missing")
            };
            let gw = gateway_for(&ws, &path, diagnostic).await;
            for after in [
                format!("// harmless insertion\n{before}"),
                before.replace("    missing", "    // harmless insertion\n    missing"),
            ] {
                let mut filtered = parse_items(&path, &diagnostic(&after));
                set_aside_preexisting(
                    &mut filtered,
                    &after,
                    &parse_items(&path, &diagnostic(before)),
                    before,
                );
                let report = validate_text(gw.addr(), &ws.root(), &ws.path(&path), &after)
                    .await
                    .unwrap();
                let batch = validate_texts(
                    gw.addr(),
                    &ws.root(),
                    &[(ws.path(&path), after.clone())],
                    &[],
                )
                .await
                .unwrap();
                let public = execute_tool(
                    gw.addr(),
                    &ws.root(),
                    "code_validate_edits",
                    json!({"edits": [{"path": path, "new_text": after}]}),
                )
                .await
                .unwrap();
                if !filtered.ok()
                    || !report.ok()
                    || !batch[0].ok()
                    || public.is_error
                    || filtered.preexisting.len() != 1
                    || report.preexisting.len() != 1
                    || batch[0].preexisting.len() != 1
                {
                    failures.push(format!(
                        "{path} ({after:?}): filter={}, report={}, batch={}, public_is_error={}",
                        filtered.ok(),
                        report.ok(),
                        batch[0].ok(),
                        public.is_error
                    ));
                }
                assert_eq!(ws.read(&path), before);
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
