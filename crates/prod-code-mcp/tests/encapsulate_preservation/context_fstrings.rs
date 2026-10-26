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
use prod_code_mcp::encapsulate_field::encapsulate_polyglot;
use prod_code_testkit::Workspace;
use serde_json::json;

#[tokio::test]
async fn python_fstring_logical_lines_preserve_suite_ownership() {
    let owner = "class Box:\n def __init__(self):\n  self.count = 0\n";
    let mut failures = Vec::new();
    for (shape, statement, supported) in [
        (
            "conditional-expression",
            "s = f\"\"\"{1\nif True else 2}\"\"\"; b.count = 2\nassert b.count == 2",
            true,
        ),
        (
            "conditional-suite",
            "if f\"\"\"{0\n+ 1}\"\"\" == \"no\": pass; b.count = 2\nassert b.count == 0",
            false,
        ),
        (
            "nested-conditional-expression",
            "s = f\"\"\"{f'''{1\nif True else 2}'''}\"\"\"; b.count = 2\nassert b.count == 2",
            true,
        ),
        (
            "nested-conditional-suite",
            "if f\"\"\"{f'''{0\n+ 1}'''}\"\"\" == \"no\": pass; b.count = 2\nassert b.count == 0",
            false,
        ),
        (
            "format-comment-marker-before-suite",
            "s = f\"\"\"{1:#x}\"\"\"\nif False: pass; b.count = 2\nassert b.count == 0",
            false,
        ),
        (
            "format-quote-before-standalone",
            "s = f\"\"\"{1:'>3}\"\"\"\nb.count = 2\nassert b.count == 2",
            true,
        ),
    ] {
        for force in [false, true] {
            let name = format!("python-fstring-{shape}-force-{force}");
            let body = format!(
                "from model import Box\nb = Box()\n{statement}\nprint('fstring preserved')\n"
            );
            let ws = Workspace::new(&[("model.py", owner), ("main.py", &body)]);
            for (file, source) in [("model.py", owner), ("main.py", body.as_str())] {
                emit(format!(
                    "CONTEXT_SOURCE {}\n",
                    json!({"case": name, "phase": "input", "file": file, "source": source})
                ));
            }
            assert!(run(&ws, "python3", &["main.py"]).status.success());
            let decl = spot(owner, "self.count = 0", "count");
            let refs = [
                ("model.py", decl),
                ("main.py", spot(&body, "b.count = 2", "count")),
                ("main.py", spot(&body, "assert b.count", "count")),
            ];
            emit(format!("CONTEXT_REFERENCES {name} {refs:?}\n"));
            let gw = gateway(&ws, "Box", "count", ("model.py", decl), &refs).await;
            for apply in [false, true] {
                let plan = encapsulate_polyglot(
                    gw.addr(),
                    &ws.root(),
                    &ws.path("model.py"),
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
                if plan.applied != (apply && supported) || plan.unmatched.is_empty() != supported {
                    failures.push(format!("{name} apply={apply}"));
                }
                if (!apply || !supported)
                    && (ws.read("model.py") != owner || ws.read("main.py") != body)
                {
                    failures.push(format!("{name} bytes changed apply={apply}"));
                }
            }
            for file in ["model.py", "main.py"] {
                emit(format!(
                    "CONTEXT_SOURCE {}\n",
                    json!({"case": name, "phase": "emitted", "file": file, "source": ws.read(file)})
                ));
            }
            if !run(&ws, "python3", &["main.py"]).status.success() {
                failures.push(format!("{name} runtime"));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}
