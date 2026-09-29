use prod_code_mcp::invert_boolean::invert_polyglot;
use prod_code_testkit::{answers, ScriptedGateway, Workspace};
use std::fs;

const CARGO_TOML: &str = "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

async fn fake_gateway() -> ScriptedGateway {
    ScriptedGateway::start(|method, _params| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await
}

#[tokio::test]
async fn test_invert_boolean_typescript_multi_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math.ts",
            r#"export function isValid(x: number): boolean {
    return x > 0;
}
"#,
        ),
        (
            "client.ts",
            r#"import { isValid } from "./math";

export function check() {
    const a = isValid(5);
    const b = !isValid(10);
    const c = isValid(15).toString();
    return a && b && c;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math.ts");
    let client_file = root.join("client.ts");
    let gw = fake_gateway().await;

    let res = invert_polyglot(
        gw.addr(),
        &root,
        &math_file,
        None,
        None,
        Some("isValid"),
        "isInvalid",
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.was, "isValid");
    assert_eq!(res.now, "isInvalid");
    assert_eq!(res.negated, 2);
    assert_eq!(res.cancelled, 1);
    assert!(res.applied);

    let math_content = fs::read_to_string(&math_file).unwrap();
    assert!(math_content.contains("export function isInvalid(x: number): boolean {"));
    assert!(math_content.contains("return !(x > 0);"));

    let client_content = fs::read_to_string(&client_file).unwrap();
    assert!(client_content.contains("import { isInvalid } from \"./math\";"));
    assert!(client_content.contains("!isInvalid(5)"));
    assert!(client_content.contains("isInvalid(10)"));
    assert!(client_content.contains("(!isInvalid(15)).toString()"));
}

#[tokio::test]
async fn test_invert_boolean_python_multi_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math_mod.py",
            r#"def is_valid(x: int) -> bool:
    """Check validity."""
    if x == 0:
        return False
    return x > 0
"#,
        ),
        (
            "client.py",
            r#"from math_mod import is_valid

def run():
    a = is_valid(5)
    b = not is_valid(10)
    return a and b
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math_mod.py");
    let client_file = root.join("client.py");
    let gw = fake_gateway().await;

    let res = invert_polyglot(
        gw.addr(),
        &root,
        &math_file,
        None,
        None,
        Some("is_valid"),
        "is_invalid",
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.was, "is_valid");
    assert_eq!(res.now, "is_invalid");
    assert_eq!(res.negated, 1);
    assert_eq!(res.cancelled, 1);
    assert!(res.applied);

    let math_content = fs::read_to_string(&math_file).unwrap();
    assert!(math_content.contains("def is_invalid(x: int) -> bool:"));
    assert!(math_content.contains("\"\"\"Check validity.\"\"\""));
    assert!(math_content.contains("return True"));
    assert!(math_content.contains("return not (x > 0)"));

    let client_content = fs::read_to_string(&client_file).unwrap();
    assert!(client_content.contains("from math_mod import is_invalid"));
    assert!(client_content.contains("not is_invalid(5)"));
    assert!(client_content.contains("b = is_invalid(10)"));
}

#[tokio::test]
async fn test_invert_boolean_cpp_multi_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math.cpp",
            r#"bool isValid(int x) {
    return x > 0;
}
"#,
        ),
        (
            "client.cpp",
            r#"bool isValid(int x);

int run() {
    return isValid(10) ? 1 : 0;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math.cpp");
    let client_file = root.join("client.cpp");
    let gw = fake_gateway().await;

    let res = invert_polyglot(
        gw.addr(),
        &root,
        &math_file,
        None,
        None,
        Some("isValid"),
        "isInvalid",
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.was, "isValid");
    assert_eq!(res.now, "isInvalid");
    assert_eq!(res.negated, 1);
    assert!(res.applied);

    let math_content = fs::read_to_string(&math_file).unwrap();
    assert!(math_content.contains("bool isInvalid(int x) {"));
    assert!(math_content.contains("return !(x > 0);"));

    let client_content = fs::read_to_string(&client_file).unwrap();
    assert!(client_content.contains("bool isInvalid(int x);"));
    assert!(client_content.contains("!isInvalid(10)"));
}

#[tokio::test]
async fn test_invert_boolean_swift_multi_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math.swift",
            r#"func isValid(x: Int) -> Bool {
    return x > 0
}
"#,
        ),
        (
            "client.swift",
            r#"func test() -> Bool {
    return isValid(x: 10)
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math.swift");
    let client_file = root.join("client.swift");
    let gw = fake_gateway().await;

    let res = invert_polyglot(
        gw.addr(),
        &root,
        &math_file,
        None,
        None,
        Some("isValid"),
        "isInvalid",
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.was, "isValid");
    assert_eq!(res.now, "isInvalid");
    assert_eq!(res.negated, 1);
    assert!(res.applied);

    let math_content = fs::read_to_string(&math_file).unwrap();
    assert!(math_content.contains("func isInvalid(x: Int) -> Bool {"));
    assert!(math_content.contains("return !(x > 0)"));

    let client_content = fs::read_to_string(&client_file).unwrap();
    assert!(client_content.contains("!isInvalid(x: 10)"));
}

#[tokio::test]
async fn test_invert_boolean_go_multi_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math.go",
            r#"package math

func IsValid(x int) bool {
    return x > 0
}
"#,
        ),
        (
            "client.go",
            r#"package main

import "math"

func run() bool {
    return math.IsValid(10)
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math.go");
    let client_file = root.join("client.go");
    let gw = fake_gateway().await;

    let res = invert_polyglot(
        gw.addr(),
        &root,
        &math_file,
        None,
        None,
        Some("IsValid"),
        "IsInvalid",
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.was, "IsValid");
    assert_eq!(res.now, "IsInvalid");
    assert_eq!(res.negated, 1);
    assert!(res.applied);

    let math_content = fs::read_to_string(&math_file).unwrap();
    assert!(math_content.contains("func IsInvalid(x int) bool {"));
    assert!(math_content.contains("return !(x > 0)"));

    let client_content = fs::read_to_string(&client_file).unwrap();
    assert!(client_content.contains("!math.IsInvalid(10)"));
}

#[tokio::test]
async fn test_invert_boolean_recursive_refused() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math.ts",
            r#"export function isValid(n: number): boolean {
    if (n <= 0) return true;
    return isValid(n - 1);
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math.ts");
    let gw = fake_gateway().await;

    let res = invert_polyglot(
        gw.addr(),
        &root,
        &math_file,
        None,
        None,
        Some("isValid"),
        "isInvalid",
        true,
        false,
    )
    .await;

    assert!(res.is_err());
    let err = res.unwrap_err().to_string();
    assert!(
        err.contains("calls itself; invert a recursive predicate by hand"),
        "{err}"
    );
}

#[tokio::test]
async fn test_invert_boolean_function_used_as_value_refused() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math.ts",
            r#"export function isValid(n: number): boolean {
    return n > 0;
}
"#,
        ),
        (
            "client.ts",
            r#"import { isValid } from "./math";

export function check() {
    const fnRef = isValid;
    return fnRef(5);
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math.ts");
    let gw = fake_gateway().await;

    let res = invert_polyglot(
        gw.addr(),
        &root,
        &math_file,
        None,
        None,
        Some("isValid"),
        "isInvalid",
        true,
        false,
    )
    .await;

    assert!(res.is_err());
    let err = res.unwrap_err().to_string();
    assert!(err.contains("used as a value"), "{err}");
}
