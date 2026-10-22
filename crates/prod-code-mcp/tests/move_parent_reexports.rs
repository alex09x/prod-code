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
use std::process::Command;

#[tokio::test]
async fn move_to_parent_removes_the_child_reexport_and_preserves_callers() {
    for parent_path in ["src/parent.rs", "src/parent/mod.rs"] {
        for reexport in [
            "pub use child::{keep, moved, tail};",
            "pub use child::{moved, keep, tail};",
            "pub use child::{keep, tail, moved};",
            "pub use child::{\n    keep,\n    moved,\n    tail,\n};",
            "pub use self::child::{keep, moved, tail};",
            "pub use child::moved;\npub use child::{keep, tail};",
        ] {
            let parent = format!("pub mod child;\n{reexport}\n");
            let lib = "pub mod parent;\n#[test]\nfn callers() {\n    assert_eq!(crate::parent::moved(), 42);\n    assert_eq!(crate::parent::child::moved(), 42);\n    assert_eq!(crate::parent::keep(), 7);\n    assert_eq!(crate::parent::tail(), 9);\n}\n";
            let child = "pub fn keep() -> u32 { 7 }\npub fn moved() -> u32 { 42 }\npub fn tail() -> u32 { 9 }\n";
            let ws = Workspace::new(&[
                (
                    "Cargo.toml",
                    "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
                ),
                ("src/lib.rs", lib),
                (parent_path, &parent),
                ("src/parent/child.rs", child),
            ]);
            let source = ws.path("src/parent/child.rs");
            let target = ws.path(parent_path);
            let caller = ws.path("src/lib.rs");
            let parent_refs = parent
                .lines()
                .enumerate()
                .filter_map(|(line, text)| {
                    text.find("moved")
                        .map(|col| (line as u32 + 1, col as u32 + 1))
                })
                .collect::<Vec<_>>();
            let caller_refs = lib
                .lines()
                .enumerate()
                .filter_map(|(line, text)| {
                    text.find("moved")
                        .map(|col| (line as u32 + 1, col as u32 + 1))
                })
                .collect::<Vec<_>>();
            let remote = ScriptedGateway::start(move |method, _| match method {
                "textDocument/documentSymbol" => {
                    serde_json::json!([answers::document_symbol("moved", 12, 2, 2, 8)])
                }
                "textDocument/references" => {
                    let mut refs = answers::locations(&target, &parent_refs);
                    refs.as_array_mut().unwrap().extend(
                        answers::locations(&caller, &caller_refs)
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
            let result = move_item(
                remote.addr(),
                &ws.root(),
                &source,
                2,
                8,
                &ws.path(parent_path),
                true,
                false,
            )
            .await
            .unwrap();
            assert!(result.applied);
            let binary = ws.path("fixture-test");
            let compiled = Command::new("rustc")
                .args(["--edition=2021", "--test"])
                .arg(ws.path("src/lib.rs"))
                .arg("-o")
                .arg(&binary)
                .output()
                .unwrap();
            assert!(
                compiled.status.success(),
                "{parent_path}: {reexport}\n{}",
                String::from_utf8_lossy(&compiled.stderr)
            );
            let ran = Command::new(binary).output().unwrap();
            assert!(
                ran.status.success(),
                "{}",
                String::from_utf8_lossy(&ran.stdout)
            );
            assert!(String::from_utf8_lossy(&ran.stdout).contains("1 passed"));
            let rewritten = ws.read(parent_path);
            assert!(rewritten.contains("pub fn moved()"), "{rewritten}");
            assert!(!rewritten.contains(reexport), "{rewritten}");
            assert!(!ws.read("src/parent/child.rs").contains("fn moved"));
            assert!(!ws.read("src/lib.rs").contains("child::moved"));
            assert!(result.imports.iter().any(|note| {
                note.contains(parent_path)
                    && (note.contains("narrowed") || note.contains("dropped"))
            }));
        }
    }
}
