use prod_code_mcp::wrap_return::{wrap_polyglot, Wrapper};
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
async fn test_wrap_return_typescript_promise() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math.ts",
            r#"export function calculate(x: number): number {
    return x * 2;
}
"#,
        ),
        (
            "client.ts",
            r#"import { calculate } from "./math";

export async function run() {
    const a = calculate(5);
    const b = calculate(10).toString();
    return a;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math.ts");
    let client_file = root.join("client.ts");
    let gw = fake_gateway().await;

    let res = wrap_polyglot(
        gw.addr(),
        &root,
        &math_file,
        Some("calculate"),
        None,
        None,
        Wrapper::Promise,
        None,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.function, "calculate");
    assert_eq!(res.was, "number");
    assert_eq!(res.now, "Promise<number>");
    assert_eq!(res.propagated, 2);
    assert!(res.blocked.is_empty());
    assert!(res.applied);

    let math_content = fs::read_to_string(&math_file).unwrap();
    assert!(math_content.contains("export async function calculate(x: number): Promise<number> {"));

    let client_content = fs::read_to_string(&client_file).unwrap();
    assert!(client_content.contains("const a = await calculate(5);"));
    assert!(client_content.contains("const b = (await calculate(10)).toString();"));
}

#[tokio::test]
async fn test_wrap_return_typescript_blocked_caller_refusal() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math.ts",
            r#"export function calculate(x: number): number {
    return x * 2;
}
"#,
        ),
        (
            "client.ts",
            r#"import { calculate } from "./math";

export function syncRun() {
    return calculate(5);
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math.ts");
    let gw = fake_gateway().await;

    let err = wrap_polyglot(
        gw.addr(),
        &root,
        &math_file,
        Some("calculate"),
        None,
        None,
        Wrapper::Promise,
        None,
        true,
        false,
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("1 call site(s) cannot propagate"));

    // With force: true, it applies
    let res = wrap_polyglot(
        gw.addr(),
        &root,
        &math_file,
        Some("calculate"),
        None,
        None,
        Wrapper::Promise,
        None,
        true,
        true,
    )
    .await
    .unwrap();

    assert!(res.applied);
    assert_eq!(res.blocked.len(), 1);
}

#[tokio::test]
async fn test_wrap_return_typescript_option() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "user.ts",
            r#"export function getUser(id: number): string {
    return "user_" + id;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let user_file = root.join("user.ts");
    let gw = fake_gateway().await;

    let res = wrap_polyglot(
        gw.addr(),
        &root,
        &user_file,
        Some("getUser"),
        None,
        None,
        Wrapper::Option,
        None,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.was, "string");
    assert_eq!(res.now, "string | null");
    assert!(res.applied);

    let user_content = fs::read_to_string(&user_file).unwrap();
    assert!(user_content.contains("export function getUser(id: number): string | null {"));
}

#[tokio::test]
async fn test_wrap_return_python_option() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "db.py",
            r#"def find_user(user_id: int) -> str:
    return "User"
"#,
        ),
        (
            "service.py",
            r#"from db import find_user

def get_user_name(uid: int) -> Optional[str]:
    return find_user(uid)
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let db_file = root.join("db.py");
    let gw = fake_gateway().await;

    let res = wrap_polyglot(
        gw.addr(),
        &root,
        &db_file,
        Some("find_user"),
        None,
        None,
        Wrapper::Option,
        None,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.was, "str");
    assert_eq!(res.now, "Optional[str]");
    assert_eq!(res.propagated, 1);
    assert!(res.applied);

    let db_content = fs::read_to_string(&db_file).unwrap();
    assert!(db_content.contains("def find_user(user_id: int) -> Optional[str]:"));
}

#[tokio::test]
async fn test_wrap_return_python_result() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math_ops.py",
            r#"def divide(a: int, b: int) -> int:
    return a // b
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math_ops.py");
    let gw = fake_gateway().await;

    let res = wrap_polyglot(
        gw.addr(),
        &root,
        &math_file,
        Some("divide"),
        None,
        None,
        Wrapper::Result,
        Some("ZeroDivisionError"),
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.was, "int");
    assert_eq!(res.now, "Result[int, ZeroDivisionError]");
    assert!(res.applied);

    let math_content = fs::read_to_string(&math_file).unwrap();
    assert!(math_content.contains("def divide(a: int, b: int) -> Result[int, ZeroDivisionError]:"));
    assert!(math_content.contains("return Ok(a // b)"));
}

#[tokio::test]
async fn test_wrap_return_cpp_optional() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.hpp",
            r#"int calculate(int x);
"#,
        ),
        (
            "calc.cpp",
            r#"int calculate(int x) {
    return x * 2;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let calc_cpp = root.join("calc.cpp");
    let calc_hpp = root.join("calc.hpp");
    let gw = fake_gateway().await;

    let res = wrap_polyglot(
        gw.addr(),
        &root,
        &calc_cpp,
        Some("calculate"),
        None,
        None,
        Wrapper::Option,
        None,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.was, "int");
    assert_eq!(res.now, "std::optional<int>");
    assert!(res.applied);

    let cpp_content = fs::read_to_string(&calc_cpp).unwrap();
    assert!(cpp_content.contains("std::optional<int> calculate(int x) {"));

    let hpp_content = fs::read_to_string(&calc_hpp).unwrap();
    assert!(hpp_content.contains("std::optional<int> calculate(int x);"));
}

#[tokio::test]
async fn test_wrap_return_swift_optional_and_result() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "Fetcher.swift",
            r#"func fetch(id: Int) -> String {
    return "Item"
}
"#,
        ),
        (
            "Parser.swift",
            r#"func parse(data: String) -> Int {
    return 42
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let fetcher_file = root.join("Fetcher.swift");
    let parser_file = root.join("Parser.swift");
    let gw = fake_gateway().await;

    let res_opt = wrap_polyglot(
        gw.addr(),
        &root,
        &fetcher_file,
        Some("fetch"),
        None,
        None,
        Wrapper::Option,
        None,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res_opt.now, "String?");
    assert!(res_opt.applied);
    let fetcher_content = fs::read_to_string(&fetcher_file).unwrap();
    assert!(fetcher_content.contains("func fetch(id: Int) -> String? {"));

    let res_res = wrap_polyglot(
        gw.addr(),
        &root,
        &parser_file,
        Some("parse"),
        None,
        None,
        Wrapper::Result,
        None,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res_res.now, "Result<Int, Error>");
    assert!(res_res.applied);
    let parser_content = fs::read_to_string(&parser_file).unwrap();
    assert!(parser_content.contains("func parse(data: String) -> Result<Int, Error> {"));
    assert!(parser_content.contains("return .success(42)"));
}

#[tokio::test]
async fn test_wrap_return_go_result_and_pointer() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "store.go",
            r#"package main

func Load(key string) string {
    return "val"
}
"#,
        ),
        (
            "handler.go",
            r#"package main

func Handle(k string) (string, error) {
    return Load(k)
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let store_file = root.join("store.go");
    let gw = fake_gateway().await;

    let res = wrap_polyglot(
        gw.addr(),
        &root,
        &store_file,
        Some("Load"),
        None,
        None,
        Wrapper::Result,
        None,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.was, "string");
    assert_eq!(res.now, "(string, error)");
    assert_eq!(res.propagated, 1);
    assert!(res.applied);

    let store_content = fs::read_to_string(&store_file).unwrap();
    assert!(store_content.contains("func Load(key string) (string, error) {"));
    assert!(store_content.contains("return \"val\", nil"));
}

#[tokio::test]
async fn test_wrap_return_already_wrapped_refusals() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math.ts",
            r#"export async function getPromise(): Promise<number> {
    return 1;
}
export function getNullable(): string | null {
    return null;
}
"#,
        ),
        (
            "db.py",
            r#"def get_opt() -> Optional[int]:
    return None
"#,
        ),
        (
            "store.go",
            r#"package main

func GetErr() (string, error) {
    return "", nil
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math.ts");
    let db_file = root.join("db.py");
    let store_file = root.join("store.go");
    let gw = fake_gateway().await;

    let err1 = wrap_polyglot(
        gw.addr(),
        &root,
        &math_file,
        Some("getPromise"),
        None,
        None,
        Wrapper::Promise,
        None,
        false,
        false,
    )
    .await
    .unwrap_err();
    assert!(err1.to_string().contains("already returns a `Promise`"));

    let err2 = wrap_polyglot(
        gw.addr(),
        &root,
        &math_file,
        Some("getNullable"),
        None,
        None,
        Wrapper::Option,
        None,
        false,
        false,
    )
    .await
    .unwrap_err();
    assert!(err2.to_string().contains("already returns an `Option`"));

    let err3 = wrap_polyglot(
        gw.addr(),
        &root,
        &db_file,
        Some("get_opt"),
        None,
        None,
        Wrapper::Option,
        None,
        false,
        false,
    )
    .await
    .unwrap_err();
    assert!(err3.to_string().contains("already returns an `Option`"));

    let err4 = wrap_polyglot(
        gw.addr(),
        &root,
        &store_file,
        Some("GetErr"),
        None,
        None,
        Wrapper::Result,
        None,
        false,
        false,
    )
    .await
    .unwrap_err();
    assert!(err4.to_string().contains("already returns a `Result`"));
}
