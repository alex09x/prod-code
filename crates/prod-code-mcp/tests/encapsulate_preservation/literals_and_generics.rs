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
async fn cpp_numeric_separators_preserve_fraction_and_exponent_boundaries() {
    let owner = "class Box {\npublic:\n    double count;\n};\n";
    for (literal, read) in [
        ("0x1.A'BCp0", false),
        ("0x1.A'BCp0", true),
        (".5'0", true),
        ("1.0e+1'0", true),
        ("1.0e-1'0", true),
        ("0x1.ABp+1'0", true),
    ] {
        let check = if read {
            format!("if (b.count != {literal}) return 1; ")
        } else {
            String::new()
        };
        let body = format!(
            "#include \"box.hpp\"\nint main() {{ Box b; b.count = {literal}; {check}return 0; }}\n"
        );
        let ws = Workspace::new(&[("box.hpp", owner), ("main.cpp", &body)]);
        assert!(
            run(&ws, "c++", &["-std=c++17", "main.cpp", "-o", "probe"])
                .status
                .success()
        );
        assert!(run(&ws, "./probe", &[]).status.success());
        let mut refs = vec![("main.cpp", spot(&body, "b.count =", "count"))];
        if read {
            refs.push(("main.cpp", spot(&body, "b.count !=", "count")));
        }
        let gw = gateway(
            &ws,
            "Box",
            "count",
            ("box.hpp", spot(owner, "double count;", "count")),
            &refs,
        )
        .await;
        let result = encapsulate_polyglot(
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
            "CPP NUMERIC {literal} read={read} applied={} unmatched={:?}\nEMITTED box.hpp:\n{}\nEMITTED main.cpp:\n{}\n",
            result.applied,
            result.unmatched,
            ws.read("box.hpp"),
            ws.read("main.cpp")
        ));
        assert!(
            run(&ws, "c++", &["-std=c++17", "main.cpp", "-o", "probe"])
                .status
                .success(),
            "generated C++ must compile with force"
        );
        assert!(run(&ws, "./probe", &[]).status.success());
        assert!(result.applied, "{:?}", result.unmatched);
        assert!(ws.read("main.cpp").contains(&format!("({literal});")));
    }
}

#[tokio::test]
async fn typescript_generic_comma_refuses_before_writes_even_force() {
    let owner = "class Box {\n    count: number = 1;\n}\n";
    for expression in [
        "choose<number, string>()",
        "choose<Value, Label>()",
        "choose<Array<number>, string>()",
        "(choose)<number, string>()",
        "choose<(x: number, y: string) => number, string>()",
        "choose<number,\n string>()",
    ] {
        let body = format!("const b = new Box();\nb.count = {expression};\n");
        let ws = Workspace::new(&[("owner.ts", owner), ("main.ts", &body)]);
        let refs = [("main.ts", spot(&body, "b.count =", "count"))];
        let gw = gateway(
            &ws,
            "Box",
            "count",
            ("owner.ts", spot(owner, "count: number", "count")),
            &refs,
        )
        .await;
        for apply in [false, true] {
            let result = encapsulate_polyglot(
                gw.addr(),
                &ws.root(),
                &ws.path("owner.ts"),
                Some("Box"),
                "count",
                None,
                apply,
                true,
            )
            .await;
            emit(format!(
                "TS GENERIC {expression} apply={apply} result={result:?}\n"
            ));
            assert!(
                result.is_err(),
                "ambiguous generic boundary must refuse rather than emit malformed source"
            );
            assert!(format!("{:#}", result.unwrap_err()).contains("TypeScript type-argument"));
            assert_eq!(ws.read("owner.ts"), owner);
            assert_eq!(ws.read("main.ts"), body);
        }
    }
}

#[tokio::test]
async fn typescript_ordinary_rhs_remains_supported() {
    let owner = "class Box {\n    count: number = 1;\n}\n";
    let body = "const b = new Box();\nb.count = Number(1 < 2);\nb.count = Math.max(b.count, 3);\nif (b.count !== 3) throw Error('ordinary RHS changed');\nconsole.log('ordinary TypeScript RHS=3');\n";
    let ws = Workspace::new(&[("owner.ts", owner), ("main.ts", body)]);
    ws.write(
        "probe.js",
        &format!("{}{body}", owner.replace(": number", "")),
    );
    assert!(run(&ws, "node", &["probe.js"]).status.success());
    std::fs::remove_file(ws.path("probe.js")).unwrap();
    let refs = [
        "b.count = Number",
        "b.count = Math",
        "b.count, 3",
        "b.count !==",
    ]
    .map(|anchor| ("main.ts", spot(body, anchor, "count")));
    let gw = gateway(
        &ws,
        "Box",
        "count",
        ("owner.ts", spot(owner, "count: number", "count")),
        &refs,
    )
    .await;
    let result = encapsulate_polyglot(
        gw.addr(),
        &ws.root(),
        &ws.path("owner.ts"),
        Some("Box"),
        "count",
        None,
        true,
        false,
    )
    .await
    .unwrap();
    emit(format!(
        "TS ORDINARY applied={} unmatched={:?}\nEMITTED owner.ts:\n{}\nEMITTED main.ts:\n{}\n",
        result.applied,
        result.unmatched,
        ws.read("owner.ts"),
        ws.read("main.ts")
    ));
    assert!(result.applied, "{:?}", result.unmatched);
    assert!(
        ws.read("main.ts")
            .contains("b.setCount(Math.max(b.getCount(), 3));")
    );
    // Only generated annotations/modifiers are erased for real Node execution.
    // This is not evidence from a TypeScript compiler or a live semantic server.
    let generated = ws
        .read("owner.ts")
        .replace("private ", "")
        .replace("public ", "")
        .replace(": number", "")
        .replace(": void", "");
    ws.write("probe.js", &format!("{generated}{}", ws.read("main.ts")));
    assert!(run(&ws, "node", &["probe.js"]).status.success());
}
