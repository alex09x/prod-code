/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::*;
use prod_code_mcp::encapsulate_field::{encapsulate, encapsulate_polyglot};
use prod_code_testkit::Workspace;
use serde_json::json;

fn fixture(name: &str, phase: &str, file: &str, source: &str) {
    emit(format!(
        "CONTEXT_SOURCE {}\n",
        json!({
            "case": name, "phase": phase, "file": file, "source": source
        })
    ));
}

#[tokio::test]
async fn cpp_rhs_keeps_enclosing_function_context() {
    let owner = "#include <string>\nclass Box {\npublic:\n std::string count;\n};\n";
    let body = "#include \"box.hpp\"\n#include <iostream>\nvoid update(Box& b) { b.count = __func__; }\nint main() { Box b; update(b); std::cout << b.count << '\\n'; return b.count == \"update\" ? 0 : 1; }\n";
    let mut failures = Vec::new();
    for force in [false, true] {
        let name = format!("cpp-function-context-force-{force}");
        let ws = Workspace::new(&[("box.hpp", owner), ("main.cpp", body)]);
        for (file, source) in [("box.hpp", owner), ("main.cpp", body)] {
            fixture(&name, "input", file, source);
        }
        assert!(
            run(&ws, "c++", &["-std=c++17", "main.cpp", "-o", "probe"])
                .status
                .success()
        );
        assert!(run(&ws, "./probe", &[]).status.success());
        let refs = ["b.count = __func__", "<< b.count", "b.count =="]
            .map(|anchor| ("main.cpp", spot(body, anchor, "count")));
        emit(format!("CONTEXT_REFERENCES {name} {refs:?}\n"));
        let gw = gateway(
            &ws,
            "Box",
            "count",
            ("box.hpp", spot(owner, "std::string count;", "count")),
            &refs,
        )
        .await;
        let plan = encapsulate_polyglot(
            gw.addr(),
            &ws.root(),
            &ws.path("box.hpp"),
            Some("Box"),
            "count",
            None,
            true,
            force,
        )
        .await
        .unwrap();
        emit(format!(
            "CONTEXT_PLAN {name} applied={} unmatched={:?}\n",
            plan.applied, plan.unmatched
        ));
        for file in ["box.hpp", "main.cpp"] {
            fixture(&name, "emitted", file, &ws.read(file));
        }
        let compiled = run(&ws, "c++", &["-std=c++17", "main.cpp", "-o", "probe"])
            .status
            .success();
        let runtime = compiled && run(&ws, "./probe", &[]).status.success();
        if !plan.applied || !plan.unmatched.is_empty() || !runtime {
            failures.push(name);
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
async fn rust_rhs_unconditionally_initializes_locals_and_target() {
    let owner = "pub struct Box {\n pub count: i32,\n}\nimpl Box { pub fn new() -> Self { Self { count: 0 } } }\n";
    let mut failures = Vec::new();
    for (shape, body) in [
        (
            "local",
            "mod model;\nfn main() { let mut b = model::Box::new(); let answer; b.count = { answer = 7; answer }; assert_eq!(answer, 7); assert_eq!(b.count, 7); println!(\"answer=7\"); }\n",
        ),
        (
            "target",
            "mod model;\nfn main() { let mut boxes = [model::Box::new()]; let index: usize; boxes[index].count = { index = 0; 7 }; assert_eq!(boxes[0].count, 7); println!(\"target initialized by RHS\"); }\n",
        ),
    ] {
        for force in [false, true] {
            let name = format!("rust-init-{shape}-force-{force}");
            let ws = Workspace::new(&[("model.rs", owner), ("main.rs", body)]);
            for (file, source) in [("model.rs", owner), ("main.rs", body)] {
                fixture(&name, "input", file, source);
            }
            assert!(
                run(&ws, "rustc", &["--edition=2024", "main.rs", "-o", "probe"])
                    .status
                    .success()
            );
            assert!(run(&ws, "./probe", &[]).status.success());
            let refs = vec![
                ("model.rs", spot(owner, "Self { count:", "count")),
                ("main.rs", spot(body, ".count =", "count")),
                ("main.rs", spot(body, ".count, 7", "count")),
            ];
            emit(format!("CONTEXT_REFERENCES {name} {refs:?}\n"));
            let decl = spot(owner, "pub count:", "count");
            let gw = gateway(&ws, "Box", "count", ("model.rs", decl), &refs).await;
            let plan = encapsulate(
                gw.addr(),
                &ws.root(),
                &ws.path("model.rs"),
                decl.0,
                decl.1,
                None,
                true,
                force,
            )
            .await
            .unwrap();
            emit(format!(
                "CONTEXT_PLAN {name} applied={} unmatched={:?}\n",
                plan.applied, plan.unmatched
            ));
            for file in ["model.rs", "main.rs"] {
                fixture(&name, "emitted", file, &ws.read(file));
            }
            let compiled = run(&ws, "rustc", &["--edition=2024", "main.rs", "-o", "probe"])
                .status
                .success();
            let runtime = compiled && run(&ws, "./probe", &[]).status.success();
            if !plan.applied || !plan.unmatched.is_empty() || !runtime {
                failures.push(name);
            }
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
async fn python_inline_suite_refuses_without_writes_and_standalone_applies() {
    let owner = "class Box:\n def __init__(self):\n  self.count = 0\n";
    // Reuse one root: client workspace watchers outlive individual runtimes.
    let ws = Workspace::new(&[("owner.py", owner), ("main.py", "")]);
    let mut failures = Vec::new();
    for (shape, statement, supported) in [
        (
            "false-suite",
            "if False: pass; b.count = 2\nassert b.count == 0",
            false,
        ),
        (
            "true-suite",
            "if True: pass; b.count = 2\nassert b.count == 2",
            false,
        ),
        (
            "direct-suite",
            "if False: b.count = 2\nassert b.count == 0",
            false,
        ),
        (
            "multiline-suite",
            "if (\n False\n): pass; b.count = 2\nassert b.count == 0",
            false,
        ),
        (
            "continued-suite",
            "if False: pass; \\\n b.count = 2\nassert b.count == 0",
            false,
        ),
        (
            "continued-suite-semicolon-lf",
            "if False: \\\n pass; b.count = 2\nassert b.count == 0",
            false,
        ),
        (
            "continued-suite-semicolon-crlf",
            "if False: \\\r\n pass; b.count = 2\r\nassert b.count == 0",
            false,
        ),
        (
            "comment-backslash-before-suite-lf",
            "# comment \\\nif False: pass; b.count = 2\nassert b.count == 0",
            false,
        ),
        (
            "comment-backslash-before-suite-crlf",
            "# comment \\\r\nif False: pass; b.count = 2\r\nassert b.count == 0",
            false,
        ),
        (
            "leading-continuation-suite-lf",
            "\\\nif False: pass; b.count = 2\nassert b.count == 0",
            false,
        ),
        (
            "leading-continuation-suite-crlf",
            "\\\r\nif False: pass; b.count = 2\r\nassert b.count == 0",
            false,
        ),
        (
            "leading-continuation-match-lf",
            "\\\nmatch 1:\n case 1: pass; b.count = 2\nassert b.count == 2",
            false,
        ),
        (
            "leading-continuation-match-crlf",
            "\\\r\nmatch 1:\r\n case 1: pass; b.count = 2\r\nassert b.count == 2",
            false,
        ),
        (
            "leading-continuation-case",
            "match 1:\n \\\n case 1: pass; b.count = 2\nassert b.count == 2",
            false,
        ),
        (
            "pattern-lambda-default-guard",
            "match 1:\n case _ if lambda x=1: False: pass; b.count = 2\nassert b.count == 2",
            false,
        ),
        (
            "pattern-nested-lambda-default-guard",
            "match 1:\n case _ if lambda f=lambda x=1: x: False: pass; b.count = 2\nassert b.count == 2",
            false,
        ),
        (
            "leading-continuation-semicolon",
            "\\\npass; b.count = 2\nassert b.count == 2",
            true,
        ),
        (
            "leading-continuation-lambda-prefix",
            "\\\nf = lambda x=2: x; b.count = 2\nassert b.count == f()",
            true,
        ),
        ("standalone", "b.count = 2\nassert b.count == 2", true),
        (
            "indented",
            "if True:\n b.count = 2\nassert b.count == 2",
            true,
        ),
        (
            "commented-header",
            "if True: # suite comment\n b.count = 2\nassert b.count == 2",
            true,
        ),
        (
            "comment-backslash-on-header-lf",
            "if True: # comment \\\n b.count = 2\nassert b.count == 2",
            true,
        ),
        (
            "comment-backslash-on-header-crlf",
            "if True: # comment \\\r\n b.count = 2\r\nassert b.count == 2",
            true,
        ),
        ("semicolon", "pass; b.count = 2\nassert b.count == 2", true),
        (
            "combining-keyword-prefix",
            "if\u{301} = 1; b.count = 2\nassert b.count == if\u{301} + 1",
            true,
        ),
        (
            "middle-dot-keyword-prefix",
            "for\u{b7} = 1; b.count = 2\nassert b.count == for\u{b7} + 1",
            true,
        ),
        (
            "connector-keyword-prefix",
            "async\u{203f} = 1; b.count = 2\nassert b.count == async\u{203f} + 1",
            true,
        ),
        (
            "lambda-prefix",
            "f = lambda x: x; b.count = 2\nassert b.count == f(2)",
            true,
        ),
        (
            "soft-keyword-prefix",
            "case={'value': 2}; b.count = 2\nassert b.count == case['value']",
            true,
        ),
        (
            "soft-keyword-call",
            "def case(value): return value\ncase('a:b'); b.count = 2\nassert b.count == 2",
            true,
        ),
        (
            "soft-keyword-index",
            "case = {}\ncase['a:b'] = lambda: 1; b.count = 2\nassert b.count == 2",
            true,
        ),
        (
            "soft-keyword-attribute-annotation",
            "case = Box()\ncase.value: int = 1; b.count = 2\nassert b.count == 2",
            true,
        ),
        (
            "soft-keyword-subscript-annotation",
            "case = {}\ncase[0]: int = 1; b.count = 2\nassert b.count == 2",
            true,
        ),
        (
            "pattern-sequence-no-space",
            "match [0]:\n case[0]: value: int = 1; b.count = 2\nassert b.count == 2",
            false,
        ),
        (
            "soft-keyword-annotation-in-pattern-body",
            "match 1:\n case 1:\n  case = {}\n  case[0]: int = 1; b.count = 2\nassert b.count == 2",
            true,
        ),
        (
            "pattern-continued-match-header",
            "match (\n    [0]\n):\n # intervening comment\n case[0]: pass; b.count = 2\nassert b.count == 2",
            false,
        ),
        (
            "pattern-suite",
            "match 1:\n case 1: pass; b.count = 2\nassert b.count == 2",
            false,
        ),
        (
            "pattern-nested-colons",
            "match {'a:b': 1}:\n case {'a:b': value} if value == 1: pass; b.count = 2\nassert b.count == 2",
            false,
        ),
    ] {
        let body = format!(
            "from owner import Box\nb = Box()\n{statement}\nprint('suite ownership preserved')\n"
        );
        for force in [false, true] {
            let name = format!("python-{shape}-force-{force}");
            ws.write("owner.py", owner);
            ws.write("main.py", &body);
            for (file, source) in [("owner.py", owner), ("main.py", body.as_str())] {
                fixture(&name, "input", file, source);
            }
            assert!(run(&ws, "python3", &["main.py"]).status.success());
            let refs = [
                ("owner.py", spot(owner, "self.count =", "count")),
                ("main.py", spot(&body, "b.count = 2", "count")),
                ("main.py", spot(&body, "assert b.count", "count")),
            ];
            emit(format!("CONTEXT_REFERENCES {name} {refs:?}\n"));
            let gw = gateway(
                &ws,
                "Box",
                "count",
                ("owner.py", spot(owner, "self.count =", "count")),
                &refs,
            )
            .await;
            for apply in [false, true] {
                let plan = encapsulate_polyglot(
                    gw.addr(),
                    &ws.root(),
                    &ws.path("owner.py"),
                    Some("Box"),
                    "count",
                    None,
                    apply,
                    force,
                )
                .await
                .unwrap();
                emit(format!(
                    "CONTEXT_PLAN {name} apply={apply} applied={} unmatched={:?}\n",
                    plan.applied, plan.unmatched
                ));
                if plan.applied != (apply && supported) || (supported != plan.unmatched.is_empty())
                {
                    failures.push(name.clone());
                }
                if (!apply || !supported)
                    && (ws.read("owner.py") != owner || ws.read("main.py") != body)
                {
                    failures.push(format!("{name} bytes changed apply={apply}"));
                }
            }
            for file in ["owner.py", "main.py"] {
                fixture(&name, "emitted", file, &ws.read(file));
            }
            if !run(&ws, "python3", &["main.py"]).status.success() {
                failures.push(name);
            }
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}
