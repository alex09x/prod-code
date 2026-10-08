/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::fs;
use std::path::Path;

use prod_code_mcp::extract_field::extract_polyglot;
use prod_code_mcp::extract_field::types::ExtractedField;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};

const CARGO_TOML: &str = "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

async fn fake_gateway() -> ScriptedGateway {
    ScriptedGateway::start(|method, _params| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await
}

async fn extract(
    root: &Path,
    file: &Path,
    start: (u32, u32),
    end: (u32, u32),
    name: &str,
    ty: Option<&str>,
    replace_all: bool,
) -> anyhow::Result<ExtractedField> {
    let gateway = fake_gateway().await;
    extract_polyglot(
        gateway.addr(),
        root,
        file,
        start,
        end,
        name,
        ty,
        None,
        replace_all,
        true,
        false,
    )
    .await
}

#[tokio::test]
async fn extract_field_replace_all_does_not_rewrite_expression_prefixes() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.py",
            "class Calc:\n    def compute(self):\n        c = 5\n        exact_a = 2 + 3\n        exact_b = 2 + 3\n        larger = 2 + 3 * c\n        return exact_a + exact_b + larger\n",
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("calc.py");

    let result = extract(&root, &file, (4, 19), (4, 24), "magic", Some("int"), true)
        .await
        .unwrap();

    assert_eq!(result.replaced, 2);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("exact_a = self.magic"), "{content}");
    assert!(content.contains("exact_b = self.magic"), "{content}");
    assert!(content.contains("larger = 2 + 3 * c"), "{content}");
}

#[tokio::test]
async fn extract_field_replace_all_keeps_complete_lower_precedence_subexpressions() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.py",
            "B = 3\nC = 5\nA = 2\nclass Calc:\n    def compute(self):\n        exact_a = B * C\n        exact_b = B * C\n        combined = A + B * C\n        return exact_a + exact_b + combined\n",
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("calc.py");

    let result = extract(&root, &file, (6, 19), (6, 24), "product", Some("int"), true)
        .await
        .unwrap();

    assert_eq!(result.replaced, 3);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("combined = A + self.product"), "{content}");
}

#[tokio::test]
async fn extract_field_refuses_selected_expression_prefixes() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.py",
            "class Calc:\n    def compute(self):\n        c = 5\n        larger = 2 + 3 * c\n",
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("calc.py");

    let error = extract(&root, &file, (4, 18), (4, 23), "magic", Some("int"), false)
        .await
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("does not cover a complete expression")
    );
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("larger = 2 + 3 * c"), "{content}");
}

#[tokio::test]
async fn extract_field_replace_all_does_not_rewrite_typescript_expression_prefixes() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.ts",
            "class Calc {\n  compute(): number {\n    const c = 5;\n    const exactA = 2 + 3;\n    const exactB = 2 + 3;\n    const larger = 2 + 3 * c;\n    return exactA + exactB + larger;\n  }\n}\n",
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("calc.ts");

    let result = extract(
        &root,
        &file,
        (4, 20),
        (4, 25),
        "magic",
        Some("number"),
        true,
    )
    .await
    .unwrap();

    assert_eq!(result.replaced, 2);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("const exactA = this.magic"), "{content}");
    assert!(content.contains("const exactB = this.magic"), "{content}");
    assert!(content.contains("const larger = 2 + 3 * c;"), "{content}");
}

#[tokio::test]
async fn extract_field_replace_all_does_not_rewrite_top_level_comma_tuple_prefix() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.py",
            "class Calc:\n    def compute(self):\n        tuple_a = 1, 2\n        tuple_b = 1, 2, 3\n        return tuple_a, tuple_b\n",
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("calc.py");

    let result = extract(&root, &file, (3, 19), (3, 23), "pair", None, true)
        .await
        .unwrap();

    assert_eq!(result.replaced, 1);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("tuple_a = self.pair"), "{content}");
    assert!(content.contains("tuple_b = 1, 2, 3"), "{content}");
}

#[tokio::test]
async fn extract_field_rejects_partial_python_comparison_chain() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.py",
            "class Calc:\n    def compute(self):\n        a = 1\n        b = 2\n        c = 3\n        res = a < b < c\n        return res\n",
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("calc.py");

    let error = extract(&root, &file, (6, 15), (6, 20), "cmp", None, false)
        .await
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("does not cover a complete expression")
    );
}

#[tokio::test]
async fn extract_field_rejects_partial_python_conditional_expression() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.py",
            "class Calc:\n    def compute(self, flag, other):\n        a = 1\n        b = 2\n        c = 3\n        val = a if flag else b if other else c\n        return val\n",
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("calc.py");

    let error = extract(&root, &file, (6, 15), (6, 31), "cond", None, false)
        .await
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("does not cover a complete expression")
    );
}

#[tokio::test]
async fn extract_field_supports_python_floor_division() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.py",
            "class Calc:\n    def compute(self):\n        val = 10 // 3\n        return val\n",
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("calc.py");

    let result = extract(&root, &file, (3, 15), (3, 22), "div", None, true)
        .await
        .unwrap();

    assert_eq!(result.replaced, 1);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("val = self.div"), "{content}");
}

#[tokio::test]
async fn extract_field_accepts_conditions_after_if_and_while_keywords() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.py",
            "class Calc:\n    def compute(self):\n        if 1 + 2 > 3:\n            return True\n        return False\n",
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("calc.py");

    let result = extract(&root, &file, (3, 12), (3, 21), "cond", None, true)
        .await
        .unwrap();

    assert_eq!(result.replaced, 1);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("if self.cond:"), "{content}");
}

#[tokio::test]
async fn extract_field_replace_all_does_not_rewrite_unary_minus_before_python_exponentiation() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.py",
            "A = 2\nB = 3\nclass Calc:\n    def compute(self):\n        exact = -A\n        power = -A ** B\n        return exact, power\n",
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("calc.py");

    let result = extract(&root, &file, (5, 17), (5, 19), "neg", None, true)
        .await
        .unwrap();

    assert_eq!(result.replaced, 1);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("exact = self.neg"), "{content}");
    assert!(content.contains("power = -A ** B"), "{content}");
}

#[tokio::test]
async fn extract_field_replace_all_rewrites_postfix_attribute_before_python_exponentiation() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.py",
            "class Item:\n    val = 2\nITEM = Item()\nclass Calc:\n    def compute(self, exp):\n        exact = ITEM.val\n        power = ITEM.val ** exp\n        return exact, power\n",
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("calc.py");

    let result = extract(&root, &file, (6, 17), (6, 25), "cached", None, true)
        .await
        .unwrap();

    assert_eq!(result.replaced, 2);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("exact = self.cached"), "{content}");
    assert!(content.contains("power = self.cached ** exp"), "{content}");
}
