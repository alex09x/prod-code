use prod_code_mcp::to_method::convert_to_method_polyglot;
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
async fn test_convert_to_method_typescript_multi_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math.ts",
            r#"export class MathUtils {
    val: number = 42;

    static add(m: MathUtils, b: number): number {
        return m.val + b;
    }
}
"#,
        ),
        (
            "client.ts",
            r#"import { MathUtils } from "./math";

export function compute() {
    const m = new MathUtils();
    return MathUtils.add(m, 10);
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math.ts");
    let client_file = root.join("client.ts");
    let gw = fake_gateway().await;

    let res = convert_to_method_polyglot(
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
    assert_eq!(res.renamed_uses, 1);
    assert_eq!(res.rewritten_calls, 1);
    assert!(res.applied);

    let math_content = fs::read_to_string(&math_file).unwrap();
    assert!(math_content.contains("add(b: number): number {"));
    assert!(math_content.contains("return this.val + b;"));

    let client_content = fs::read_to_string(&client_file).unwrap();
    assert!(client_content.contains("m.add(10)"));
}

#[tokio::test]
async fn test_convert_to_method_python_multi_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "formatter.py",
            r#"class Formatter:
    def __init__(self, prefix: str):
        self.prefix = prefix

    @staticmethod
    def format_text(f: Formatter, text: str) -> str:
        return f.prefix + ": " + text
"#,
        ),
        (
            "app.py",
            r#"from formatter import Formatter

def run():
    fmt = Formatter("INFO")
    msg = Formatter.format_text(fmt, "hello world")
    print(msg)
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let formatter_file = root.join("formatter.py");
    let app_file = root.join("app.py");
    let gw = fake_gateway().await;

    let res = convert_to_method_polyglot(
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
    assert_eq!(res.renamed_uses, 1);
    assert_eq!(res.rewritten_calls, 1);
    assert!(res.applied);

    let fmt_content = fs::read_to_string(&formatter_file).unwrap();
    assert!(!fmt_content.contains("@staticmethod"));
    assert!(fmt_content.contains("def format_text(self, text: str) -> str:"));
    assert!(fmt_content.contains("return self.prefix + \": \" + text"));

    let app_content = fs::read_to_string(&app_file).unwrap();
    assert!(app_content.contains("fmt.format_text(\"hello world\")"));
}

#[tokio::test]
async fn test_convert_to_method_cpp_multi_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.h",
            r#"class Calc {
public:
    int base = 10;
    static int multiply(Calc& c, int factor) {
        return c.base * factor;
    }
};
"#,
        ),
        (
            "main.cpp",
            r#"#include "calc.h"

int main() {
    Calc c;
    return Calc::multiply(c, 5);
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let header_file = root.join("calc.h");
    let main_file = root.join("main.cpp");
    let gw = fake_gateway().await;

    let res = convert_to_method_polyglot(
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
    assert_eq!(res.renamed_uses, 1);
    assert_eq!(res.rewritten_calls, 1);
    assert!(res.applied);

    let header_content = fs::read_to_string(&header_file).unwrap();
    assert!(header_content.contains("int multiply(int factor) {"));
    assert!(header_content.contains("return this->base * factor;"));

    let main_content = fs::read_to_string(&main_file).unwrap();
    assert!(main_content.contains("c.multiply(5)"));
}

#[tokio::test]
async fn test_convert_to_method_swift_multi_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "Helper.swift",
            r#"class Helper {
    var prefix: String = "Hello "

    static func combine(h: Helper, name: String) -> String {
        return h.prefix + name
    }
}
"#,
        ),
        (
            "App.swift",
            r#"func test() {
    let h = Helper()
    let s = Helper.combine(h: h, name: "world")
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let helper_file = root.join("Helper.swift");
    let app_file = root.join("App.swift");
    let gw = fake_gateway().await;

    let res = convert_to_method_polyglot(
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
    assert_eq!(res.renamed_uses, 1);
    assert_eq!(res.rewritten_calls, 1);
    assert!(res.applied);

    let helper_content = fs::read_to_string(&helper_file).unwrap();
    assert!(helper_content.contains("func combine(name: String) -> String {"));
    assert!(helper_content.contains("return self.prefix + name"));

    let app_content = fs::read_to_string(&app_file).unwrap();
    assert!(app_content.contains("h.combine(name: \"world\")"));
}

#[tokio::test]
async fn test_convert_to_method_go_multi_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "service.go",
            r#"package main

type Service struct {
    tag string
}

func Execute(s *Service, cmd string) string {
    return s.tag + ":" + cmd
}
"#,
        ),
        (
            "main.go",
            r#"package main

func main() {
    s := &Service{tag: "prod"}
    out := Execute(s, "start")
    println(out)
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let service_file = root.join("service.go");
    let main_file = root.join("main.go");
    let gw = fake_gateway().await;

    let res = convert_to_method_polyglot(
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
    assert_eq!(res.receiver, "(s *Service)");
    assert_eq!(res.rewritten_calls, 1);
    assert!(res.applied);

    let service_content = fs::read_to_string(&service_file).unwrap();
    assert!(service_content.contains("func (s *Service) Execute(cmd string) string {"));

    let main_content = fs::read_to_string(&main_file).unwrap();
    assert!(main_content.contains("s.Execute(\"start\")"));
}

#[tokio::test]
async fn test_convert_to_method_via_execute_tool() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "util.ts",
            r#"export class Util {
    factor: number = 2;

    static helper(u: Util, x: number): number {
        return u.factor * x;
    }
}

export function test() {
    const u = new Util();
    return Util.helper(u, 21);
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
        "code_convert_to_method",
        args,
    )
    .await
    .unwrap();

    assert!(!tool_res.is_error);
    let content = fs::read_to_string(&util_file).unwrap();
    assert!(content.contains("helper(x: number): number {"));
    assert!(content.contains("return this.factor * x;"));
    assert!(content.contains("u.helper(21)"));
}
