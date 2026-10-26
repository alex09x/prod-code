/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A caller is read as its own file's language (#1407): JSX in a `.js` caller of a TypeScript
//! declaration, and a `<T>x` assertion in a `.ts` caller of a JavaScript one. Both programs are
//! compiled with tsc and run with node before and after, in both force modes.

use super::aggregate_review::snapshot;
use super::support::*;
use prod_code_mcp::signature::{Modifiers, Param, change_with};
use prod_code_testkit::Workspace;
use std::path::Path;

fn compile_and_run(root: &Path, main: &str, out: &str) -> String {
    run(
        root,
        "tsc",
        &[
            main,
            "--jsx",
            "react",
            "--allowJs",
            "--noImplicitAny",
            "false",
            "--target",
            "es2022",
            "--module",
            "commonjs",
            "--outDir",
            "out",
        ],
    );
    let output = run(root, "node", &[out]);
    std::fs::remove_dir_all(root.join("out")).unwrap();
    output
}

async fn reorder(
    library: (&str, &str),
    main: (&str, &str),
    call: &str,
    expected_call: &str,
    stdout: &str,
) {
    for force in [false, true] {
        let ws = Workspace::new(&[library, main]);
        let root = &ws.root();
        let label = format!("mixed-{}-{force}", main.0);
        snapshot(root, main.0, &format!("{label}-before"));
        snapshot(root, library.0, &format!("{label}-before"));
        assert_eq!(compile_and_run(root, main.0, "out/main.js"), stdout);
        let gw = gateway(vec![
            location(root, main.0, main.1, "f }"),
            location(root, main.0, main.1, call),
        ])
        .await;
        let result = change_with(
            gw.addr(),
            root,
            &root.join(library.0),
            1,
            17,
            &[Param::Keep("b".into()), Param::Keep("a".into())],
            &Modifiers::default(),
            true,
            force,
        )
        .await;
        eprintln!("{label} public result: {result:?}");
        let actual = snapshot(root, main.0, &format!("{label}-after"));
        let actual_library = snapshot(root, library.0, &format!("{label}-after"));
        result.unwrap();
        assert_eq!(actual, main.1.replace(call, expected_call), "{label}");
        assert!(actual_library.contains("function f(b"), "{actual_library}");
        assert_eq!(compile_and_run(root, main.0, "out/main.js"), stdout);
    }
}

#[tokio::test]
async fn a_js_caller_of_a_typescript_function_keeps_its_jsx_call() {
    let library = "export function f(a: number, b: number) { return a * 10 + b; }\n";
    let main = "import { f } from './lib';\nconst React = { createElement: (...args) => args };\nconst view = <div><p>ready to\nexport {f(1, 2)}</p></div>;\nconsole.log(JSON.stringify(view));\n";
    reorder(
        ("lib.ts", library),
        ("main.js", main),
        "f(1, 2)",
        "f(2, 1)",
        "[\"div\",null,[\"p\",null,\"ready to export \",12]]\n",
    )
    .await;
}

#[tokio::test]
async fn a_ts_caller_of_a_javascript_function_keeps_its_type_assertion() {
    let library = "export function f(a, b) { return a * 10 + b; }\n";
    let main = "import { f } from './lib';\nconst n = (<number>1);\nconsole.log(f(n, 2));\n";
    reorder(
        ("lib.js", library),
        ("main.ts", main),
        "f(n, 2)",
        "f(2, n)",
        "12\n",
    )
    .await;
}
