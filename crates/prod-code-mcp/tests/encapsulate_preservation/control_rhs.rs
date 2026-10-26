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

#[tokio::test]
async fn cpp_unbraced_bodies_apply_but_value_consuming_prefixes_refuse() {
    let owner = "class Box {\npublic:\n    int count;\n};\n";
    let mut failures = Vec::new();
    for (statement, supported) in [
        ("if (enabled) b.count = 2;", true),
        ("if (enabled)\n b.count = 2;", true),
        ("if (enabled) b.count = 2; else return 1;", true),
        ("if (!enabled) return 1; else b.count = 2;", true),
        ("while (b.count < 2) b.count = 2;", true),
        ("for (int i = 0; i < 1; ++i) b.count = 2;", true),
        ("do b.count = 2; while (false);", true),
        ("int value = (int) (b.count = 2);", false),
        ("int value = 0; value = b.count = 2;", false),
        ("int value = (int&) b.count = 2;", false),
        ("if ((b.count = 2)) {}", false),
    ] {
        for force in [false, true] {
            let body = format!(
                "#include \"box.hpp\"\nint main() {{ Box b; b.count = 0; bool enabled = true; {statement}\nreturn b.count == 2 ? 0 : 1; }}\n"
            );
            let ws = Workspace::new(&[("box.hpp", owner), ("main.cpp", &body)]);
            assert!(
                run(&ws, "c++", &["-std=c++17", "main.cpp", "-o", "probe"])
                    .status
                    .success()
            );
            assert!(run(&ws, "./probe", &[]).status.success());
            let mut refs = vec![
                ("main.cpp", spot(&body, "b.count = 0", "count")),
                ("main.cpp", spot(&body, "b.count = 2", "count")),
                ("main.cpp", spot(&body, "b.count == 2", "count")),
            ];
            if statement.starts_with("while") {
                refs.push(("main.cpp", spot(&body, "b.count < 2", "count")));
            }
            let gw = gateway(
                &ws,
                "Box",
                "count",
                ("box.hpp", spot(owner, "int count;", "count")),
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
                "CPP CONTROL statement={statement:?} force={force} applied={} unmatched={:?}\nEMITTED box.hpp:\n{}\nEMITTED main.cpp:\n{}\n",
                plan.applied,
                plan.unmatched,
                ws.read("box.hpp"),
                ws.read("main.cpp")
            ));
            let compiled = run(&ws, "c++", &["-std=c++17", "main.cpp", "-o", "probe"])
                .status
                .success();
            let runtime = compiled && run(&ws, "./probe", &[]).status.success();
            if plan.applied != supported || !runtime {
                failures.push(format!(
                    "{statement} force={force} applied={} runtime={runtime}",
                    plan.applied
                ));
            }
            if supported && plan.applied {
                assert!(plan.unmatched.is_empty());
                assert!(ws.read("main.cpp").contains("))(2);"));
            } else if !supported {
                assert!(!plan.unmatched.is_empty());
                assert_eq!(ws.read("main.cpp"), body);
                assert_eq!(ws.read("box.hpp"), owner);
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn python_lambda_and_tuple_rhs_preserve_complete_evaluation_even_force() {
    let owner = "class Box:\n    def __init__(self):\n        self.count = 0\n";
    let mut failures = Vec::new();
    for (rhs, check) in [
        ("lambda x, y: x + y", "assert value(2, 3) == 5"),
        (
            "lambda x=mark(1), y=mark(2): x + y",
            "assert events == [1, 2] and value() == 3",
        ),
        (
            "lambda x=lambda a, b: a + b, y=2: x(3, y)",
            "assert value() == 5",
        ),
        (
            "lambda x, y: (mark(x), mark(y))",
            "assert value(3, 4) == (3, 4) and events == [3, 4]",
        ),
        (
            "mark(1), mark(2)",
            "assert value == (1, 2) and events == [1, 2]",
        ),
        ("mark(1),", "assert value == (1,) and events == [1]"),
        (
            "lambda x, y: x + y, mark(4)",
            "assert value[0](2, 3) == 5 and value[1] == 4 and events == [4]",
        ),
        (
            "(mark(1),\n mark(2))",
            "assert value == (1, 2) and events == [1, 2]",
        ),
        ("lambda x, y=b.count: x + y", "assert value(3) == 3"),
        (
            "b.count, mark(2)",
            "assert value == (0, 2) and events == [2]",
        ),
    ] {
        for force in [false, true] {
            let body = format!(
                "from owner import Box\nb = Box()\nevents = []\ndef mark(x):\n    events.append(x)\n    return x\nb.count = {rhs}\nvalue = b.count\n{check}\nprint('python RHS preserved')\n"
            );
            let ws = Workspace::new(&[("owner.py", owner), ("main.py", &body)]);
            assert!(run(&ws, "python3", &["main.py"]).status.success());
            let mut refs = vec![
                ("owner.py", spot(owner, "self.count =", "count")),
                ("main.py", spot(&body, "b.count =", "count")),
                ("main.py", spot(&body, "value = b.count", "count")),
            ];
            if rhs.contains("b.count") {
                refs.push(("main.py", spot(&body, rhs, "count")));
            }
            let gw = gateway(
                &ws,
                "Box",
                "count",
                ("owner.py", spot(owner, "self.count =", "count")),
                &refs,
            )
            .await;
            let plan = encapsulate_polyglot(
                gw.addr(),
                &ws.root(),
                &ws.path("owner.py"),
                Some("Box"),
                "count",
                None,
                true,
                force,
            )
            .await
            .unwrap();
            emit(format!(
                "PYTHON RHS rhs={rhs:?} force={force} applied={} unmatched={:?}\nEMITTED owner.py:\n{}\nEMITTED main.py:\n{}\n",
                plan.applied,
                plan.unmatched,
                ws.read("owner.py"),
                ws.read("main.py")
            ));
            let runtime = run(&ws, "python3", &["main.py"]).status.success();
            if !plan.applied || !plan.unmatched.is_empty() || !runtime {
                failures.push(format!(
                    "{rhs} force={force} applied={} runtime={runtime}",
                    plan.applied
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn python_chained_rhs_refuses_before_writes_even_force() {
    let owner = "class Box:\n    def __init__(self):\n        self.count = 0\n";
    let body =
        "from owner import Box\nb = Box()\nb.count = other = 2\nassert b.count == other == 2\n";
    let mut failures = Vec::new();
    for force in [false, true] {
        let ws = Workspace::new(&[("owner.py", owner), ("main.py", body)]);
        assert!(run(&ws, "python3", &["main.py"]).status.success());
        let refs = [
            ("owner.py", spot(owner, "self.count =", "count")),
            ("main.py", spot(body, "b.count = other", "count")),
            ("main.py", spot(body, "b.count ==", "count")),
        ];
        let gw = gateway(
            &ws,
            "Box",
            "count",
            ("owner.py", spot(owner, "self.count =", "count")),
            &refs,
        )
        .await;
        let result = encapsulate_polyglot(
            gw.addr(),
            &ws.root(),
            &ws.path("owner.py"),
            Some("Box"),
            "count",
            None,
            true,
            force,
        )
        .await;
        emit(format!(
            "PYTHON CHAIN force={force} result={result:?}\nEMITTED owner.py:\n{}\nEMITTED main.py:\n{}\n",
            ws.read("owner.py"),
            ws.read("main.py")
        ));
        let runtime = run(&ws, "python3", &["main.py"]).status.success();
        if !runtime || result.is_ok() || ws.read("owner.py") != owner || ws.read("main.py") != body
        {
            failures.push(format!("force={force} runtime={runtime} result={result:?}"));
        } else {
            assert!(format!("{:#}", result.unwrap_err()).contains("chained Python assignment"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
