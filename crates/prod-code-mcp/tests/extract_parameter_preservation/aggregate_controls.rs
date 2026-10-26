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

#[tokio::test]
async fn python_single_selection_survives_unknown_other_statements() {
    for (other, expected) in [("1+2 if True else 0", "13\n"), ("int(f'{1+2}')", "13\n")] {
        let source = format!(
            "def target():\n    a = 1 + 2\n    b = {other}\n    return a + b\nprint(target())\n"
        );
        let ws = workspace(&source, "py");
        let result = change_typed(
            &ws,
            &source,
            "py",
            "select:1 + 2",
            false,
            true,
            false,
            false,
            "int",
        )
        .await
        .unwrap();
        assert_eq!(result.replaced, 1);
        let generated = ws.read("main.py");
        assert!(generated.contains(other));
        let customized = generated.replace("print(target(1 + 2))", "print(target(10))");
        assert_eq!(run(&customized, "py"), expected);
    }
}

#[tokio::test]
async fn python_static_strings_and_ordinary_normalized_occurrences() {
    for literal in [
        "'80'",
        "f\"80\"",
        "f'{{80}}'",
        "r'80'",
        "b'80'",
        "fr'''80'''",
        "F'80'",
    ] {
        let source = format!(
            "def target():\n    a = 80\n    b = {literal}\n    c = 80\n    return (a + c, b)\nprint(target())\n"
        );
        let original = run(&source, "py");
        let ws = workspace(&source, "py");
        let result = change_typed(
            &ws,
            &source,
            "py",
            "select:80",
            true,
            true,
            false,
            false,
            "int",
        )
        .await
        .unwrap();
        assert_eq!(result.replaced, 2);
        let generated = ws.read("main.py");
        assert!(generated.contains(&format!("b = {literal}")));
        assert_eq!(run(&generated, "py"), original);
        let customized = generated.replace("print(target(80))", "print(target(10))");
        assert_eq!(run(&customized, "py"), original.replacen("160", "20", 1));
    }
}

#[tokio::test]
async fn cpp_raw_prefixes_and_custom_delimiters_remain_opaque() {
    for prefix in ["R", "u8R", "uR", "UR", "LR"] {
        // The payload includes a false empty-delimiter closer and embedded numeric text.
        let literal = format!("{prefix}\"custom(v\"80\")\"still)custom\"");
        let source = format!(
            "#include <iostream>\nint target() {{\nint a = 80; auto s = {literal}; return a + (s[2] == '8');\n}}\nint main() {{ std::cout << target() << '\\n'; }}\n"
        );
        assert_eq!(run(&source, "cpp"), "81\n");
        let ws = workspace(&source, "cpp");
        let result = change_typed(&ws, &source, "cpp", "80;", true, true, false, false, "int")
            .await
            .unwrap();
        assert_eq!(result.replaced, 1);
        let generated = ws.read("main.cpp");
        assert!(generated.contains(&literal));
        assert_eq!(run(&generated, "cpp"), "81\n");
        assert_eq!(
            run(&generated.replace("target(80)", "target(10)"), "cpp"),
            "11\n"
        );
    }
}

#[tokio::test]
async fn go_new_reused_and_initializer_bindings() {
    for (parameters, body, arguments, count, expected) in [
        (
            "x int",
            "a := x + 1; x, y := x, 0; b := x+1; _ = y; return a+b",
            "x",
            2,
            "10\n",
        ),
        (
            "x, z int",
            "a := x + 1; x, y := x, 0; b := x+1; _ = y; _ = z; return a+b",
            "x, 0",
            2,
            "10\n",
        ),
        (
            "x int",
            "a := x + 1; x,\n y := x, 0; b := x+1; _ = y; return a+b",
            "x",
            2,
            "10\n",
        ),
        (
            "x int",
            "a := x + 1; b := 0; { x := x+1; b = x+1; _ = x }; return a+b",
            "x",
            2,
            "11\n",
        ),
        (
            "x int",
            "a := x + 1; b := 0; { x := 10; x, y := 20, 0; b = x+1; _ = x; _ = y }; return a+b",
            "x",
            1,
            "26\n",
        ),
        (
            "x int",
            "a := x + 1; b := 0; { x := 10; x,\n y := 20, 0; b = x+1; _ = x; _ = y }; return a+b",
            "x",
            1,
            "26\n",
        ),
        (
            "x int",
            "a := x + 1\nb := 0\n{\nx := 10\nb = x+1\n_ = x\n}\nreturn a+b",
            "x",
            1,
            "16\n",
        ),
        (
            "x int",
            "a := x + 1; b := 0; { x := 10; _ = x }; b = x+1; return a+b",
            "x",
            2,
            "10\n",
        ),
    ] {
        let source = format!(
            "package main\nimport \"fmt\"\nfunc target({parameters}) int {{\n{body}\n}}\nfunc main() {{ x := 4; fmt.Println(target({arguments})) }}\n"
        );
        // Reused names can change their value; keep the source and generated outputs for comparison.
        let original = run(&source, "go");
        let ws = workspace(&source, "go");
        let result = change(&ws, &source, "go", "select:x + 1", true, true, false, false)
            .await
            .unwrap();
        let generated = ws.read("main.go");
        println!("GO BINDING CONTROL original={original:?} generated\n{generated}");
        assert_eq!(result.replaced, count);
        assert_eq!(run(&generated, "go"), expected);
        assert_eq!(expected, original);
    }
}

#[tokio::test]
async fn go_uncertain_short_scope_refuses_before_writes() {
    let source = "package main\nimport \"fmt\"\nfunc target(x int) int {\na := x + 1; if x := 10; x > 0 { a += x+1 }; return a\n}\nfunc main() { x := 4; fmt.Println(target(x)) }\n";
    assert_eq!(run(source, "go"), "16\n");
    let ws = workspace(source, "go");
    for force in [false, true] {
        let error = change(&ws, source, "go", "select:x + 1", true, true, force, false)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("incomplete Go binding coverage"));
        assert_eq!(ws.read("main.go"), source);
    }
}
