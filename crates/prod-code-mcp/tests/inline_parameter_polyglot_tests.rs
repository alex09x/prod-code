use prod_code_mcp::inline_parameter::inline_parameter_polyglot;
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

fn reference_at(file: &std::path::Path, source: &str, name: &str, occurrence: usize) -> serde_json::Value {
    let at = source
        .match_indices(name)
        .nth(occurrence)
        .expect("reference occurrence")
        .0;
    let before = &source[..at];
    let line = before.bytes().filter(|byte| *byte == b'\n').count() as u32;
    let character = before.rsplit('\n').next().unwrap_or_default().encode_utf16().count() as u32;
    serde_json::json!({
        "uri": url::Url::from_file_path(file).unwrap().to_string(),
        "range": { "start": { "line": line, "character": character } }
    })
}

async fn fake_gateway_with_references(references: Vec<serde_json::Value>) -> ScriptedGateway {
    ScriptedGateway::start(move |method, _params| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        "textDocument/references" => serde_json::Value::Array(references.clone()),
        _ => serde_json::Value::Null,
    })
    .await
}

#[tokio::test]
async fn test_inline_parameter_typescript_multi_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math.ts",
            r#"export function clamp(x: number, max: number): number {
    return Math.min(x, max);
}
"#,
        ),
        (
            "client.ts",
            r#"import { clamp } from "./math";

export function compute() {
    return clamp(10, 100) + clamp(20, 100);
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math.ts");
    let client_file = root.join("client.ts");
    let client_text = fs::read_to_string(&client_file).unwrap();
    let gw = fake_gateway_with_references(vec![
        reference_at(&client_file, &client_text, "clamp", 1),
        reference_at(&client_file, &client_text, "clamp", 2),
    ])
    .await;

    let res = inline_parameter_polyglot(
        gw.addr(),
        &root,
        &math_file,
        None,
        None,
        Some("clamp"),
        Some("max"),
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.function, "clamp");
    assert_eq!(res.parameter, "max");
    assert_eq!(res.value, "100");
    assert_eq!(res.rewritten_calls, 2);
    assert!(res.applied);

    let math_content = fs::read_to_string(&math_file).unwrap();
    assert!(math_content.contains("export function clamp(x: number): number {"));
    assert!(math_content.contains("const max: number = 100;"));

    let client_content = fs::read_to_string(&client_file).unwrap();
    assert!(client_content.contains("clamp(10)"));
    assert!(client_content.contains("clamp(20)"));
}

#[tokio::test]
async fn test_inline_parameter_python_multi_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math_mod.py",
            r#"def clamp(x: int, max: int = 100) -> int:
    """Clamp x to max."""
    return min(x, max)
"#,
        ),
        (
            "client.py",
            r#"from math_mod import clamp

def run():
    return clamp(10, 100) + clamp(20, max=100)
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math_mod.py");
    let client_file = root.join("client.py");
    let client_text = fs::read_to_string(&client_file).unwrap();
    let gw = fake_gateway_with_references(vec![
        reference_at(&client_file, &client_text, "clamp", 1),
        reference_at(&client_file, &client_text, "clamp", 2),
    ])
    .await;

    let res = inline_parameter_polyglot(
        gw.addr(),
        &root,
        &math_file,
        None,
        None,
        Some("clamp"),
        Some("max"),
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.function, "clamp");
    assert_eq!(res.parameter, "max");
    assert_eq!(res.value, "100");
    assert_eq!(res.rewritten_calls, 2);
    assert!(res.applied);

    let math_content = fs::read_to_string(&math_file).unwrap();
    assert!(math_content.contains("def clamp(x: int) -> int:"));
    assert!(math_content.contains("max = 100"));
    assert!(math_content.contains("\"\"\"Clamp x to max.\"\"\""));

    let client_content = fs::read_to_string(&client_file).unwrap();
    assert!(client_content.contains("clamp(10)"));
    assert!(client_content.contains("clamp(20)"));
}

#[tokio::test]
async fn test_inline_parameter_cpp_multi_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math.cpp",
            r#"int clamp(int x, int max) {
    return x > max ? max : x;
}
"#,
        ),
        (
            "client.cpp",
            r#"int clamp(int x, int max);

int run() {
    return clamp(10, 100) + clamp(20, 100);
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math.cpp");
    let client_file = root.join("client.cpp");
    let client_text = fs::read_to_string(&client_file).unwrap();
    let gw = fake_gateway_with_references(vec![
        reference_at(&client_file, &client_text, "clamp", 1),
        reference_at(&client_file, &client_text, "clamp", 2),
    ])
    .await;

    let res = inline_parameter_polyglot(
        gw.addr(),
        &root,
        &math_file,
        None,
        None,
        Some("clamp"),
        Some("max"),
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.function, "clamp");
    assert_eq!(res.parameter, "max");
    assert_eq!(res.value, "100");
    assert_eq!(res.rewritten_calls, 2);
    assert!(res.applied);

    let math_content = fs::read_to_string(&math_file).unwrap();
    assert!(math_content.contains("int clamp(int x) {"));
    assert!(math_content.contains("const int max = 100;"));

    let client_content = fs::read_to_string(&client_file).unwrap();
    assert!(client_content.contains("clamp(10)"));
    assert!(client_content.contains("clamp(20)"));
}

#[tokio::test]
async fn test_inline_parameter_swift_multi_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math.swift",
            r#"func clamp(x: Int, max: Int) -> Int {
    return min(x, max)
}
"#,
        ),
        (
            "client.swift",
            r#"func run() -> Int {
    return clamp(x: 10, max: 100) + clamp(x: 20, max: 100)
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math.swift");
    let client_file = root.join("client.swift");
    let client_text = fs::read_to_string(&client_file).unwrap();
    let gw = fake_gateway_with_references(vec![
        reference_at(&client_file, &client_text, "clamp", 0),
        reference_at(&client_file, &client_text, "clamp", 1),
    ])
    .await;

    let res = inline_parameter_polyglot(
        gw.addr(),
        &root,
        &math_file,
        None,
        None,
        Some("clamp"),
        Some("max"),
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.function, "clamp");
    assert_eq!(res.parameter, "max");
    assert_eq!(res.value, "100");
    assert_eq!(res.rewritten_calls, 2);
    assert!(res.applied);

    let math_content = fs::read_to_string(&math_file).unwrap();
    assert!(math_content.contains("func clamp(x: Int) -> Int {"));
    assert!(math_content.contains("let max: Int = 100"));

    let client_content = fs::read_to_string(&client_file).unwrap();
    assert!(client_content.contains("clamp(x: 10)"));
    assert!(client_content.contains("clamp(x: 20)"));
}

#[tokio::test]
async fn test_inline_parameter_go_multi_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math.go",
            r#"package math

func Clamp(x int, max int) int {
	if x > max {
		return max
	}
	return x
}
"#,
        ),
        (
            "client.go",
            r#"package math

func Run() int {
	return Clamp(10, 100) + Clamp(20, 100)
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math.go");
    let client_file = root.join("client.go");
    let client_text = fs::read_to_string(&client_file).unwrap();
    let gw = fake_gateway_with_references(vec![
        reference_at(&client_file, &client_text, "Clamp", 0),
        reference_at(&client_file, &client_text, "Clamp", 1),
    ])
    .await;

    let res = inline_parameter_polyglot(
        gw.addr(),
        &root,
        &math_file,
        None,
        None,
        Some("Clamp"),
        Some("max"),
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.function, "Clamp");
    assert_eq!(res.parameter, "max");
    assert_eq!(res.value, "100");
    assert_eq!(res.rewritten_calls, 2);
    assert!(res.applied);

    let math_content = fs::read_to_string(&math_file).unwrap();
    assert!(math_content.contains("func Clamp(x int) int {"));
    assert!(math_content.contains("const max = 100"));

    let client_content = fs::read_to_string(&client_file).unwrap();
    assert!(client_content.contains("Clamp(10)"));
    assert!(client_content.contains("Clamp(20)"));
}

#[tokio::test]
async fn test_inline_parameter_differing_args_refused() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math.ts",
            r#"export function clamp(x: number, max: number): number {
    return Math.min(x, max);
}
"#,
        ),
        (
            "client.ts",
            r#"import { clamp } from "./math";

export function compute() {
    return clamp(10, 100) + clamp(20, 200);
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math.ts");
    let client_file = root.join("client.ts");
    let client_text = fs::read_to_string(&client_file).unwrap();
    let gw = fake_gateway_with_references(vec![
        reference_at(&client_file, &client_text, "clamp", 1),
        reference_at(&client_file, &client_text, "clamp", 2),
    ])
    .await;

    let err = inline_parameter_polyglot(
        gw.addr(),
        &root,
        &math_file,
        None,
        None,
        Some("clamp"),
        Some("max"),
        true,
        false,
    )
    .await
    .unwrap_err();

    assert!(
        err.to_string().contains("the calls do not agree on `max`"),
        "{err}"
    );
}

#[tokio::test]
async fn test_inline_parameter_caller_dependent_refused() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math.ts",
            r#"export function clamp(x: number, max: number): number {
    return Math.min(x, max);
}
"#,
        ),
        (
            "client.ts",
            r#"import { clamp } from "./math";

export function compute(limit: number) {
    return clamp(10, limit);
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math.ts");
    let client_file = root.join("client.ts");
    let client_text = fs::read_to_string(&client_file).unwrap();
    let gw = fake_gateway_with_references(vec![reference_at(&client_file, &client_text, "clamp", 1)])
        .await;

    let err = inline_parameter_polyglot(
        gw.addr(),
        &root,
        &math_file,
        None,
        None,
        Some("clamp"),
        Some("max"),
        true,
        false,
    )
    .await
    .unwrap_err();

    assert!(
        err.to_string().contains("may name something of the caller's"),
        "{err}"
    );
}
