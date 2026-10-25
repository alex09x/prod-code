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
use serde_json::json;

#[test]
fn rendering_ranks_names_filters_fuzzy_matches_and_honors_zero_limit() {
    let hit = |name: &str| SymbolHit {
        path: "/ws/value.rs".into(),
        name: name.into(),
        kind: "Function",
        container: Some("Store".into()),
        line: 2,
        col: 4,
    };
    for (names, limit, expected) in [
        (
            vec!["f_o_o", "fooBar", "foo"],
            1,
            "1 symbol(s) matching `foo`:\n  [Function] Store::foo — value.rs:2:4\n  (1 more only have its letters in order; not shown)",
        ),
        (
            vec!["f_o_o"],
            3,
            "No symbol is named like `foo`; 1 whose names have its letters in order:\n  [Function] Store::f_o_o — value.rs:2:4",
        ),
        (vec!["foo"], 0, "0 symbol(s) matching `foo`:"),
    ] {
        assert_eq!(
            render_symbol_hits(
                Path::new("/ws"),
                "foo",
                &mut names.into_iter().map(hit).collect(),
                limit
            ),
            expected
        );
    }
}

#[tokio::test]
async fn migrate_type_requires_target_and_a_resolvable_declaration() {
    let ws = prod_code_testkit::Workspace::new(&[("empty.txt", "no declarations here")]);
    let remote = "127.0.0.1:1".parse().unwrap();
    for (args, expected) in [
        (
            json!({}),
            "Missing 'to' argument: the type it should become",
        ),
        (json!({"to":"u64"}), "Missing 'path' or 'symbol' argument"),
        (
            json!({"to":"u64","symbol":"Store::missing"}),
            "could not find file declaring symbol `Store::missing`",
        ),
    ] {
        assert_eq!(
            handle_migrate_type(remote, &ws.root(), &args)
                .await
                .unwrap_err()
                .to_string(),
            expected
        );
    }
}

#[tokio::test]
async fn migration_applies_exact_type_change_from_file_or_symbol_lookup() {
    use prod_code_testkit::{ScriptedGateway, Workspace, answers};
    let source = "export function value(amount: number): number {\n    return amount;\n}\n";
    for lookup in [json!({"file":"sample.ts","col":23}), json!({})] {
        let ws = Workspace::new(&[("sample.ts", source)]);
        let gw = ScriptedGateway::start(|method, _| match method {
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => json!(null),
        })
        .await;
        let mut args = lookup;
        args["symbol"] = json!("amount");
        args["to"] = json!("string");
        args["apply"] = json!(true);
        args["transitive"] = json!(false);
        args["convert"] = json!(false);
        args["force"] = json!(false);
        let result = handle_migrate_type(gw.addr(), &ws.root(), &args)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert_eq!(
            ws.read("sample.ts"),
            "export function value(amount: string): number {\n    return amount;\n}\n"
        );
        let crate::protocol::McpContentItem::Text { text } = &result.content[0];
        assert_eq!(
            text.lines().take(4).collect::<Vec<_>>(),
            vec![
                "`amount` (sample.ts)",
                "",
                "- was: `number`",
                "- now: `string`"
            ]
        );
    }
}
