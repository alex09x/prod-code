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
async fn cpp_character_literals_keep_assignment_boundaries() {
    let owner = "class Box {\npublic:\n    int count;\n};\n";
    let body = "#include \"box.hpp\"\nint main() {\n    Box b;\n    b.count = ',';\n    if (b.count != 44) return 1;\n    b.count = '\\'';\n    if (b.count != 39) return 2;\n    b.count = '\\\\';\n    if (b.count != 92) return 3;\n    b.count = ')';\n    if (b.count != 41) return 4;\n    b.count = ';';\n    if (b.count != 59) return 5;\n    b.count = L',';\n    if (b.count != 44) return 6;\n    b.count = 1'000;\n    if (b.count != 1000) return 7;\n    b.count = '\"';\n    if (b.count != 34) return 8;\n    return 0;\n}\n";
    let ws = Workspace::new(&[("box.hpp", owner), ("main.cpp", body)]);
    assert!(
        run(&ws, "c++", &["-std=c++17", "main.cpp", "-o", "probe"])
            .status
            .success()
    );
    assert!(run(&ws, "./probe", &[]).status.success());
    let refs = [
        "count = ','",
        "count != 44) return 1",
        "count = '\\''",
        "count != 39",
        "count = '\\\\'",
        "count != 92",
        "count = ')'",
        "count != 41",
        "count = ';'",
        "count != 59",
        "count = L','",
        "count != 44) return 6",
        "count = 1'000",
        "count != 1000",
        "count = '\"'",
        "count != 34",
    ]
    .map(|anchor| ("main.cpp", spot(body, anchor, "count")));
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
        true,
    )
    .await
    .unwrap();
    emit(format!(
        "CPP CHARACTER applied={} unmatched={:?}\nEMITTED box.hpp:\n{}\nEMITTED main.cpp:\n{}\n",
        plan.applied,
        plan.unmatched,
        ws.read("box.hpp"),
        ws.read("main.cpp")
    ));
    assert!(
        run(&ws, "c++", &["-std=c++17", "main.cpp", "-o", "probe"])
            .status
            .success(),
        "generated C++ must compile even with force"
    );
    assert!(run(&ws, "./probe", &[]).status.success());
    assert!(plan.applied, "{:?}", plan.unmatched);
    // The whole literal reaches the setter, through the long form or a direct call.
    assert!(ws.read("main.cpp").contains("(',');"));
    assert!(ws.read("main.cpp").contains("(1'000);"));
}

#[tokio::test]
async fn javascript_typescript_asi_writes_apply_without_consuming_values() {
    for extension in ["js", "ts"] {
        let owner = "class Box {\n    count = 1\n}\n";
        let body = "const b = new Box()\nb.count = 2\nconst label = 'previous literal' // comment\nb.count = 3\nif (b.count !== 3) throw Error(label)\nconsole.log('ASI count=3')\n";
        let owner_path = format!("owner.{extension}");
        let caller_path = format!("main.{extension}");
        let ws = Workspace::new(&[(&owner_path, owner), (&caller_path, body)]);
        // Both fixtures intentionally use the JavaScript subset of TypeScript.
        ws.write("probe.js", &format!("{owner}{body}"));
        assert!(run(&ws, "node", &["probe.js"]).status.success());
        std::fs::remove_file(ws.path("probe.js")).unwrap();
        let refs = ["count = 2", "count = 3", "count !== 3"]
            .map(|anchor| (caller_path.as_str(), spot(body, anchor, "count")));
        let gw = gateway(
            &ws,
            "Box",
            "count",
            (&owner_path, spot(owner, "count = 1", "count")),
            &refs,
        )
        .await;
        let plan = encapsulate_polyglot(
            gw.addr(),
            &ws.root(),
            &ws.path(&owner_path),
            Some("Box"),
            "count",
            None,
            true,
            true,
        )
        .await
        .unwrap();
        emit(format!(
            "ASI {extension} applied={} unmatched={:?}\nEMITTED {owner_path}:\n{}\nEMITTED {caller_path}:\n{}\n",
            plan.applied,
            plan.unmatched,
            ws.read(&owner_path),
            ws.read(&caller_path)
        ));
        assert!(
            plan.applied,
            "standalone ASI writes stay supported: {:?}",
            plan.unmatched
        );
        assert!(plan.unmatched.is_empty());
        assert!(ws.read(&caller_path).contains("b.setCount(2)\n"));
        assert!(ws.read(&caller_path).contains("b.setCount(3)\n"));
        let mut generated_owner = ws.read(&owner_path);
        if extension == "ts" {
            // Erase only generated TypeScript annotations; this is runtime evidence,
            // not a claim that a TypeScript compiler or real LSP ran.
            generated_owner = generated_owner
                .replace("private ", "")
                .replace("public ", "")
                .replace(": number", "")
                .replace(": void", "");
        }
        ws.write(
            "probe.js",
            &format!("{generated_owner}{}", ws.read(&caller_path)),
        );
        assert!(run(&ws, "node", &["probe.js"]).status.success());
    }
}

#[tokio::test]
async fn javascript_typescript_newlines_inside_consumed_writes_refuse_even_force() {
    for extension in ["js", "ts"] {
        for body in [
            "const b = new Box()\nfunction consume(x) { return x }\nconsume(\n b.count = 2\n)\n",
            "const b = new Box()\nconst value =\n b.count = 2\n",
            "const b = new Box()\nconst assign = () =>\n b.count = 2\nassign()\n",
        ] {
            let owner = "class Box {\n    count = 1\n}\n";
            let owner_path = format!("owner.{extension}");
            let caller_path = format!("main.{extension}");
            let ws = Workspace::new(&[(&owner_path, owner), (&caller_path, body)]);
            ws.write("probe.js", &format!("{owner}{body}"));
            assert!(run(&ws, "node", &["probe.js"]).status.success());
            std::fs::remove_file(ws.path("probe.js")).unwrap();
            let refs = [(caller_path.as_str(), spot(body, "b.count = 2", "count"))];
            let gw = gateway(
                &ws,
                "Box",
                "count",
                (&owner_path, spot(owner, "count = 1", "count")),
                &refs,
            )
            .await;
            let plan = encapsulate_polyglot(
                gw.addr(),
                &ws.root(),
                &ws.path(&owner_path),
                Some("Box"),
                "count",
                None,
                true,
                true,
            )
            .await
            .unwrap();
            emit(format!(
                "CONSUMED {extension} applied={} unmatched={:?}\nEMITTED {caller_path}:\n{}\n",
                plan.applied,
                plan.unmatched,
                ws.read(&caller_path)
            ));
            assert!(!plan.applied, "consumed assignment must refuse");
            assert!(
                plan.unmatched
                    .iter()
                    .any(|s| s.contains("assignment value"))
            );
            assert_eq!(ws.read(&owner_path), owner);
            assert_eq!(ws.read(&caller_path), body);
        }
    }
}
