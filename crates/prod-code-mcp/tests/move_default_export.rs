/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Moving a TypeScript `export default` keeps every default import working (#1431):
//! - a mixed `import Foo, { other }` is split, `other` staying at the source;
//! - an aliased `import LocalName` follows the move;
//! - the source's own remaining calls import the moved function by default.
//!
//! The project typechecks with tsc after the move.

use prod_code_mcp::move_item::move_item;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};

#[tokio::test]
async fn every_default_import_follows_a_moved_default_export() {
    let ws = Workspace::new(&[
        (
            "src/source.ts",
            "export const other = 1;\nexport default function Foo(): number { return 2; }\nexport function useFoo(): number { return Foo(); }\n",
        ),
        (
            "src/mixed.ts",
            "import Foo, { other } from \"./source\";\nexport const a = Foo() + other;\n",
        ),
        (
            "src/alias.ts",
            "import LocalName from \"./source\";\nexport const b = LocalName();\n",
        ),
        (
            "src/plain.ts",
            "import Foo from \"./source\";\nexport const c = Foo();\n",
        ),
    ]);
    let root = ws.root().to_path_buf();
    let gateway = ScriptedGateway::start(|method, _params| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await;
    let source = root.join("src/source.ts");
    let helpers = root.join("src/helpers.ts");
    let res = move_item(gateway.addr(), &root, &source, 2, 25, &helpers, true, false)
        .await
        .expect("the move");
    assert_eq!(res.symbol, "Foo");
    assert!(res.applied);

    let read = |rel: &str| std::fs::read_to_string(root.join(rel)).unwrap();
    assert!(read("src/helpers.ts").contains("export default function Foo()"));
    let src = read("src/source.ts");
    assert!(src.contains("import Foo from \"./helpers\";"), "{src}");
    assert!(!src.contains("{ Foo }"), "{src}");
    let mixed = read("src/mixed.ts");
    assert!(
        mixed.contains("import { other } from \"./source\";"),
        "{mixed}"
    );
    assert!(mixed.contains("import Foo from \"./helpers\";"), "{mixed}");
    assert!(
        !mixed.contains("{ Foo }") && !mixed.contains("import Foo, {"),
        "{mixed}"
    );
    let alias = read("src/alias.ts");
    assert!(
        alias.contains("import LocalName from \"./helpers\";"),
        "{alias}"
    );
    let plain = read("src/plain.ts");
    assert!(plain.contains("import Foo from \"./helpers\";"), "{plain}");

    let tsc = std::process::Command::new("tsc")
        .args([
            "--noEmit",
            "--strict",
            "--target",
            "es2022",
            "--module",
            "es2022",
            "src/source.ts",
            "src/helpers.ts",
            "src/mixed.ts",
            "src/alias.ts",
            "src/plain.ts",
        ])
        .current_dir(&root)
        .output()
        .expect("tsc runs");
    assert!(
        tsc.status.success(),
        "{}{}",
        String::from_utf8_lossy(&tsc.stdout),
        String::from_utf8_lossy(&tsc.stderr)
    );
}
