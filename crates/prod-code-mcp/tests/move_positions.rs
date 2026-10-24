/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::move_item::move_item;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};

const ITEM: &str = "pub struct DexOracleState {\n    pub value: u32,\n}\n";
const USES: &str = "\npub struct Holder {\n    pub state: MARKDexOracleState,\n}\n\npub fn make(value: u32) -> MARKDexOracleState {\n    MARKDexOracleState { value }\n}\n";

fn positions(text: &str) -> Vec<(u32, u32)> {
    text.lines()
        .enumerate()
        .filter_map(|(line, text)| {
            let at = text.find("DexOracleState")?;
            Some((
                line as u32 + 1,
                text[..at].encode_utf16().count() as u32 + 1,
            ))
        })
        .collect()
}

// The real analyzer/gateway matrix is in the gateway's move_views integration test.
// This table isolates the planner's position conversion, including unchanged callers.
#[tokio::test]
async fn move_position_encoding_table() {
    for (case, mark, ending) in [
        ("ascii", "", "\n"),
        ("utf16", "/* 😀 */ ", "\n"),
        ("crlf", "", "\r\n"),
        ("utf16_crlf", "/* 😀 */ ", "\r\n"),
    ] {
        let uses = USES.replace("MARK", mark);
        let original = format!("{ITEM}{uses}").replace('\n', ending);
        let caller = uses
            .replace("DexOracleState", "crate::dex_oracle::DexOracleState")
            .replace('\n', ending);
        let ws = Workspace::new(&[
            (
                "Cargo.toml",
                "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
            ),
            ("src/lib.rs", "pub mod dex_oracle;\npub mod caller;\n"),
            ("src/dex_oracle.rs", &original),
            ("src/caller.rs", &caller),
        ]);
        let source = ws.path("src/dex_oracle.rs");
        let target = ws.path("src/dex_oracle_types.rs");
        let source_refs = positions(&original).into_iter().skip(1).collect::<Vec<_>>();
        let caller_refs = positions(&caller);
        let source_path = source.clone();
        let caller_path = ws.path("src/caller.rs");
        let remote = ScriptedGateway::start(move |method, _| match method {
            "textDocument/documentSymbol" => {
                serde_json::json!([answers::document_symbol("DexOracleState", 23, 1, 3, 12)])
            }
            "textDocument/references" => {
                let mut refs = answers::locations(&source_path, &source_refs);
                refs.as_array_mut().unwrap().extend(
                    answers::locations(&caller_path, &caller_refs)
                        .as_array()
                        .unwrap()
                        .clone(),
                );
                refs
            }
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => serde_json::Value::Null,
        })
        .await;
        let dry = move_item(
            remote.addr(),
            &ws.root(),
            &source,
            1,
            12,
            &target,
            false,
            false,
        )
        .await
        .unwrap();
        assert_eq!(ws.read("src/dex_oracle.rs"), original, "{case}");
        assert!(!target.exists(), "{case}");
        let applied = move_item(
            remote.addr(),
            &ws.root(),
            &source,
            1,
            12,
            &target,
            true,
            false,
        )
        .await
        .unwrap_or_else(|e| panic!("{case}: {e:#}"));
        assert!(dry.unmatched.is_empty(), "{case}: {:?}", dry.unmatched);
        assert_eq!(dry.rewritten, applied.rewritten, "{case}");
        assert_eq!(
            ws.read("src/dex_oracle.rs"),
            uses.replacen(
                "pub struct Holder",
                "use crate::dex_oracle_types::DexOracleState;\npub struct Holder",
                1,
            ),
            "{case}"
        );
        assert_eq!(ws.read("src/dex_oracle_types.rs"), ITEM, "{case}");
        assert_eq!(
            ws.read("src/caller.rs"),
            caller.replace(
                "crate::dex_oracle::DexOracleState",
                "crate::dex_oracle_types::DexOracleState",
            ),
            "{case}"
        );
        assert_eq!(
            ws.read("src/lib.rs"),
            "pub mod dex_oracle;\npub mod caller;\npub mod dex_oracle_types;\n",
            "{case}"
        );
    }
}

#[tokio::test]
async fn move_genuinely_stale_reference_still_refuses_every_write() {
    let source = format!("{ITEM}pub fn unrelated() {{}}\n");
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        ("src/lib.rs", "pub mod dex_oracle;\n"),
        ("src/dex_oracle.rs", &source),
    ]);
    let file = ws.path("src/dex_oracle.rs");
    let path = file.clone();
    let remote = ScriptedGateway::start(move |method, _| match method {
        "textDocument/documentSymbol" => {
            serde_json::json!([answers::document_symbol("DexOracleState", 23, 1, 3, 12)])
        }
        "textDocument/references" => answers::locations(&path, &[(4, 8)]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await;
    let target = ws.path("src/dex_oracle_types.rs");
    let expected = "src/dex_oracle.rs:1:8 (the analyzer places `DexOracleState` here, but the file says otherwise)";
    let dry = move_item(
        remote.addr(),
        &ws.root(),
        &file,
        1,
        12,
        &target,
        false,
        false,
    )
    .await
    .unwrap();
    assert_eq!(dry.unmatched, [expected]);
    for force in [false, true] {
        let error = move_item(
            remote.addr(),
            &ws.root(),
            &file,
            1,
            12,
            &target,
            true,
            force,
        )
        .await
        .unwrap_err()
        .to_string();
        assert_eq!(
            error,
            format!(
                "1 reference(s) to `DexOracleState` were not rewritten; nothing was written:\n  {expected}"
            )
        );
        assert_eq!(ws.read("src/dex_oracle.rs"), source);
        assert_eq!(ws.read("src/lib.rs"), "pub mod dex_oracle;\n");
        assert!(!target.exists());
    }
}
