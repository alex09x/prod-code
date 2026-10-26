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
async fn python_initial_bom_preserves_suite_ownership_and_source_bytes() {
    let owner = "class Box:\n def __init__(self):\n  self.count = 0\n";
    // Reuse one root so the client retains only one workspace watcher.
    let ws = Workspace::new(&[("model.py", owner), ("main.py", "")]);
    let mut failures = Vec::new();
    for (shape, statement, supported) in [
        (
            "false-suite",
            "if False: from model import Box; b = Box(); b.count = 2",
            false,
        ),
        (
            "true-suite",
            "if True: from model import Box; b = Box(); b.count = 2\nassert b.count == 2",
            false,
        ),
        (
            "continued-suite",
            "\\\nif False: from model import Box; b = Box(); b.count = 2",
            false,
        ),
        (
            "match-suite",
            "match 1:\n case[1]: from model import Box; b = Box(); b.count = 2",
            false,
        ),
        (
            "standalone-semicolon",
            "from model import Box; b = Box(); b.count = 2\nassert b.count == 2",
            true,
        ),
        (
            "standalone",
            "from model import Box\nb = Box()\nb.count = 2\nassert b.count == 2",
            true,
        ),
        (
            "indented",
            "from model import Box\nb = Box()\nif True:\n b.count = 2\nassert b.count == 2",
            true,
        ),
        (
            "string-bom-before-suite",
            "label = '\u{feff}'\nif False: from model import Box; b = Box(); b.count = 2",
            false,
        ),
    ] {
        for force in [false, true] {
            let name = format!("python-bom-{shape}-force-{force}");
            let body = format!("\u{feff}{statement}\nprint('BOM ownership preserved')\n");
            ws.write("model.py", owner);
            ws.write("main.py", &body);
            for (file, source) in [("model.py", owner), ("main.py", body.as_str())] {
                emit(format!(
                    "BOM_SOURCE {}\n",
                    json!({"case": name, "phase": "input", "file": file, "source": source})
                ));
            }
            assert!(run(&ws, "python3", &["main.py"]).status.success());
            let decl = spot(owner, "self.count = 0", "count");
            let mut refs = vec![
                ("model.py", decl),
                ("main.py", spot(&body, "b.count = 2", "count")),
            ];
            if body.contains("assert b.count") {
                refs.push(("main.py", spot(&body, "assert b.count", "count")));
            }
            emit(format!(
                "BOM_REFERENCES {}\n",
                json!({"case": name, "declaration": ["model.py", decl], "references": refs})
            ));
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
                let returned: Vec<_> = plan
                    .rewritten
                    .iter()
                    .map(|(file, source)| {
                        json!({"file": std::path::Path::new(file).file_name().unwrap().to_str().unwrap(), "source": source})
                    })
                    .collect();
                emit(format!(
                    "BOM_PLAN {}\n",
                    json!({"case": name, "apply": apply, "applied": plan.applied,
                        "unmatched": plan.unmatched, "returned": returned})
                ));
                if plan.applied != (apply && supported) || plan.unmatched.is_empty() != supported {
                    failures.push(format!("{name} plan apply={apply}"));
                }
                for (file, original) in [("model.py", owner), ("main.py", body.as_str())] {
                    let source = ws.read(file);
                    emit(format!(
                        "BOM_SOURCE {}\n",
                        json!({"case": name, "phase": if apply { "applied" } else { "preview" },
                            "file": file, "source": source})
                    ));
                    if (!apply || !supported) && source.as_bytes() != original.as_bytes() {
                        failures.push(format!("{name} {file} bytes changed apply={apply}"));
                    }
                    if file == "main.py" && !source.starts_with('\u{feff}') {
                        failures.push(format!("{name} lost initial BOM apply={apply}"));
                    }
                }
            }
            if !run(&ws, "python3", &["main.py"]).status.success() {
                failures.push(format!("{name} runtime"));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}
