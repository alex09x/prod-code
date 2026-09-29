use prod_code_mcp::make_static::make_static_polyglot;
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
async fn test_make_static_typescript_multi_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math.ts",
            r#"export class MathUtils {
    add(a: number, b: number): number {
        return a + b;
    }
}
"#,
        ),
        (
            "client.ts",
            r#"import { MathUtils } from "./math";

export function compute() {
    const m = new MathUtils();
    return m.add(10, 20);
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math.ts");
    let client_file = root.join("client.ts");
    let gw = fake_gateway().await;

    let res = make_static_polyglot(
        gw.addr(),
        &root,
        &math_file,
        Some("MathUtils"),
        "add",
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.owner, "MathUtils");
    assert_eq!(res.method, "add");
    assert_eq!(res.receiver, "this");
    assert_eq!(res.rewritten_calls, 1);
    assert!(res.applied);

    let math_content = fs::read_to_string(&math_file).unwrap();
    assert!(math_content.contains("static add(a: number, b: number): number {"));

    let client_content = fs::read_to_string(&client_file).unwrap();
    assert!(client_content.contains("MathUtils.add(10, 20)"));
}

#[tokio::test]
async fn test_make_static_python_multi_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "formatter.py",
            r#"class Formatter:
    def format_text(self, prefix: str, text: str) -> str:
        return f"{prefix}: {text}"
"#,
        ),
        (
            "app.py",
            r#"from formatter import Formatter

def run():
    fmt = Formatter()
    msg = fmt.format_text("INFO", "hello world")
    print(msg)
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let formatter_file = root.join("formatter.py");
    let app_file = root.join("app.py");
    let gw = fake_gateway().await;

    let res = make_static_polyglot(
        gw.addr(),
        &root,
        &formatter_file,
        Some("Formatter"),
        "format_text",
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.owner, "Formatter");
    assert_eq!(res.method, "format_text");
    assert_eq!(res.receiver, "self");
    assert_eq!(res.rewritten_calls, 1);
    assert!(res.applied);

    let fmt_content = fs::read_to_string(&formatter_file).unwrap();
    assert!(fmt_content.contains("@staticmethod\n    def format_text(prefix: str, text: str) -> str:"));

    let app_content = fs::read_to_string(&app_file).unwrap();
    assert!(app_content.contains("Formatter.format_text(\"INFO\", \"hello world\")"));
}

#[tokio::test]
async fn test_make_static_cpp_multi_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.h",
            r#"class Calc {
public:
    int multiply(int a, int b) const {
        return a * b;
    }
};
"#,
        ),
        (
            "main.cpp",
            r#"#include "calc.h"

int main() {
    Calc c;
    return c.multiply(4, 5);
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let header_file = root.join("calc.h");
    let main_file = root.join("main.cpp");
    let gw = fake_gateway().await;

    let res = make_static_polyglot(
        gw.addr(),
        &root,
        &header_file,
        Some("Calc"),
        "multiply",
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.owner, "Calc");
    assert_eq!(res.method, "multiply");
    assert_eq!(res.receiver, "*this");
    assert_eq!(res.rewritten_calls, 1);
    assert!(res.applied);

    let header_content = fs::read_to_string(&header_file).unwrap();
    assert!(header_content.contains("static int multiply(int a, int b) {"));

    let main_content = fs::read_to_string(&main_file).unwrap();
    assert!(main_content.contains("Calc::multiply(4, 5)"));
}

#[tokio::test]
async fn test_make_static_swift_multi_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "Helper.swift",
            r#"class Helper {
    func combine(a: String, b: String) -> String {
        return a + b
    }
}
"#,
        ),
        (
            "App.swift",
            r#"func test() {
    let h = Helper()
    let s = h.combine(a: "foo", b: "bar")
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let helper_file = root.join("Helper.swift");
    let app_file = root.join("App.swift");
    let gw = fake_gateway().await;

    let res = make_static_polyglot(
        gw.addr(),
        &root,
        &helper_file,
        Some("Helper"),
        "combine",
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.owner, "Helper");
    assert_eq!(res.method, "combine");
    assert_eq!(res.receiver, "self");
    assert_eq!(res.rewritten_calls, 1);
    assert!(res.applied);

    let helper_content = fs::read_to_string(&helper_file).unwrap();
    assert!(helper_content.contains("static func combine(a: String, b: String) -> String {"));

    let app_content = fs::read_to_string(&app_file).unwrap();
    assert!(app_content.contains("Helper.combine(a: \"foo\", b: \"bar\")"));
}

#[tokio::test]
async fn test_make_static_go_multi_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "service.go",
            r#"package main

type Service struct{}

func (s *Service) Execute(cmd string) string {
    return "cmd:" + cmd
}
"#,
        ),
        (
            "main.go",
            r#"package main

func main() {
    s := &Service{}
    out := s.Execute("list")
    println(out)
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let service_file = root.join("service.go");
    let main_file = root.join("main.go");
    let gw = fake_gateway().await;

    let res = make_static_polyglot(
        gw.addr(),
        &root,
        &service_file,
        Some("Service"),
        "Execute",
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.owner, "Service");
    assert_eq!(res.method, "Execute");
    assert_eq!(res.receiver, "(*Service)");
    assert_eq!(res.rewritten_calls, 1);
    assert!(res.applied);

    let service_content = fs::read_to_string(&service_file).unwrap();
    assert!(service_content.contains("func Execute(cmd string) string {"));

    let main_content = fs::read_to_string(&main_file).unwrap();
    assert!(main_content.contains("Execute(\"list\")"));
}

#[tokio::test]
async fn test_make_static_via_execute_tool() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "util.ts",
            r#"export class Util {
    helper(x: number): number {
        return x * 2;
    }
}

export function test() {
    const u = new Util();
    return u.helper(21);
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let util_file = root.join("util.ts");
    let gw = fake_gateway().await;

    let args = serde_json::json!({
        "path": "util.ts",
        "method": "helper",
        "apply": true,
    });

    let tool_res = prod_code_mcp::tools::execute_tool(
        gw.addr(),
        &root,
        "code_make_static",
        args,
    )
    .await
    .unwrap();

    assert!(!tool_res.is_error);
    let content = fs::read_to_string(&util_file).unwrap();
    assert!(content.contains("static helper(x: number): number {"));
    assert!(content.contains("Util.helper(21)"));
}
