/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::{bytes, record, run};
use prod_code_mcp::replace_constructor::{
    ReplaceMode::{Builder, Factory},
    replace_constructor_with_builder, replace_constructor_with_factory,
};
use prod_code_testkit::{ScriptedGateway, Workspace};
use serde_json::json;

#[tokio::test]
async fn aggregate_cross_file_self_reference() {
    let main = "mod other;\npub struct Config { pub value: i32 }\nfn main() { println!(\"{:?}\", other::values()); }\n";
    let other = "mod unrelated {\n pub struct Config { pub value: i32 }\n impl Config {\n  pub fn new(value: i32) -> Self { let mut r = Self { value: 0 }; r.value = value + 10; r }\n  pub fn make() -> Self { Self { value: 9 } }\n }\n}\nimpl crate::Config { pub fn make() -> Self { Self { value: 2 } } }\npub fn values() -> (i32, i32) { (crate::Config::make().value, unrelated::Config::make().value) }\n";
    for mode in [Factory, Builder] {
        let ws = Workspace::new(&[
            ("main.rs", main),
            ("other.rs", other),
            (
                "Cargo.toml",
                "[package]\nname='fixture'\nversion='0.1.0'\nedition='2021'\n",
            ),
        ]);
        let root = ws.root();
        let before_compile = run(
            &root,
            &["rustc", "--edition=2021", "main.rs", "-o", "program"],
        );
        assert_eq!(before_compile["exit"], 0, "{before_compile}");
        let before = run(&root, &["./program"]);
        let at = other.find("impl crate::Config").unwrap() + "impl crate::".len();
        let line = other[..at].bytes().filter(|&b| b == b'\n').count();
        let col = other[..at]
            .rsplit('\n')
            .next()
            .unwrap()
            .encode_utf16()
            .count();
        let uri = url::Url::from_file_path(root.join("other.rs"))
            .unwrap()
            .to_string();
        let gateway = ScriptedGateway::start(move |method, _| match method {
            "textDocument/references" => json!([{"uri":uri,"range":{"start":{"line":line,"character":col},"end":{"line":line,"character":col+6}}}]),
            "textDocument/diagnostic" => json!({"kind":"full","items":[]}),
            _ => serde_json::Value::Null,
        }).await;
        let result = match mode {
            Factory => {
                replace_constructor_with_factory(
                    gateway.addr(),
                    &root,
                    &root.join("main.rs"),
                    "Config",
                    None,
                    true,
                    true,
                    None,
                )
                .await
            }
            Builder => {
                replace_constructor_with_builder(
                    gateway.addr(),
                    &root,
                    &root.join("main.rs"),
                    "Config",
                    None,
                    true,
                    true,
                    None,
                )
                .await
            }
        };
        let after_main = std::fs::read_to_string(root.join("main.rs")).unwrap();
        let after_other = std::fs::read_to_string(root.join("other.rs")).unwrap();
        let after_compile = run(
            &root,
            &["rustc", "--edition=2021", "main.rs", "-o", "program"],
        );
        let after = if after_compile["exit"] == 0 {
            run(&root, &["./program"])
        } else {
            serde_json::Value::Null
        };
        record(
            &format!("aggregate-cross-file-{mode}"),
            &json!({
                "input_main":bytes(&root, main), "input_other":bytes(&root, other),
                "output_main":bytes(&root, &after_main), "output_other":bytes(&root, &after_other),
                "reference_fixture":{"file":"other.rs","line":line,"character":col,"length":6},
                "scripted_transport":"exact selected-owner reference; scripted diagnostics, force=true; no live analyzer certification",
                "before_compile":before_compile,"before":before,"after_compile":after_compile,"after":after,
                "public_result":result.as_ref().map(|r| json!({"count":r.instantiations_rewritten,"blocked":r.blocked})).unwrap_or_else(|e| json!({"error":format!("{e:#}")}))
            }),
        );
        let result = result.unwrap();
        assert_eq!(result.instantiations_rewritten, 1);
        assert_eq!(after_compile["exit"], 0, "{after_compile}");
        assert_eq!(before["exit"], 0);
        assert_eq!(after["exit"], 0);
        assert_eq!(before["stdout"], after["stdout"]);
        assert!(after_other.contains("Self { value: 9 }"));
        assert!(after_other.contains("Self { value: 0 }"));
    }
}
