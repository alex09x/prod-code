use prod_code_mcp::encapsulate_field::encapsulate_polyglot;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::fs;
use std::path::Path;

const CARGO_TOML: &str = "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

async fn fake_gateway() -> ScriptedGateway {
    ScriptedGateway::start(|method, _params| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await
}

async fn semantic_gateway(
    owner_file: &Path,
    owner: &str,
    field: &str,
    sources: &[(&Path, &str)],
) -> ScriptedGateway {
    let owner_uri = url::Url::from_file_path(owner_file).unwrap().to_string();
    let symbols = serde_json::json!([{
        "name": owner,
        "kind": 5,
        "selectionRange": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": owner.len() } },
        "children": [{
            "name": field,
            "kind": 8,
            "selectionRange": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": field.len() } }
        }]
    }]);
    let mut locations = Vec::new();
    for (path, source) in sources {
        let uri = url::Url::from_file_path(path).unwrap().to_string();
        for (line_number, line) in source.lines().enumerate() {
            for needle in [format!(".{field}"), format!("->{field}")] {
                let mut offset = 0;
                while let Some(relative) = line[offset..].find(&needle) {
                    let start = offset + relative + needle.len() - field.len();
                    let character = line[..start].encode_utf16().count() as u64;
                    locations.push(serde_json::json!({
                        "uri": uri.clone(),
                        "range": { "start": { "line": line_number, "character": character } }
                    }));
                    offset = start + field.len();
                }
            }
            if *path == owner_file
                && owner_file.extension().and_then(|ext| ext.to_str()) == Some("cpp")
                && line.trim_start().starts_with("return ")
                && let Some(start) = line.find(field)
            {
                let character = line[..start].encode_utf16().count() as u64;
                locations.push(serde_json::json!({
                    "uri": uri.clone(),
                    "range": { "start": { "line": line_number, "character": character } }
                }));
            }
        }
    }
    let references = serde_json::Value::Array(locations);
    ScriptedGateway::start(move |method, params| match method {
        "textDocument/documentSymbol"
            if params["textDocument"]["uri"].as_str() == Some(owner_uri.as_str()) =>
        {
            symbols.clone()
        }
        "textDocument/references"
            if params["textDocument"]["uri"].as_str() == Some(owner_uri.as_str()) =>
        {
            references.clone()
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await
}

#[tokio::test]
async fn test_encapsulate_field_refuses_unverified_typescript_references() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "user.ts",
            r#"export class User {
    name: string;
    age: number;

    constructor(name: string, age: number) {
        this.name = name;
        this.age = age;
    }

    display(): string {
        return this.name;
    }
}
"#,
        ),
        (
            "client.ts",
            r#"import { User } from "./user";

export function handleUser(user: User) {
    user.name = "Bob";
    console.log(user.name);
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let user_file = root.join("user.ts");
    let client_file = root.join("client.ts");
    let user_before = fs::read_to_string(&user_file).unwrap();
    let client_before = fs::read_to_string(&client_file).unwrap();
    let gw = fake_gateway().await;

    let res = encapsulate_polyglot(
        gw.addr(),
        &root,
        &user_file,
        Some("User"),
        "name",
        None,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.owner, "User");
    assert_eq!(res.field, "name");
    assert_eq!(res.ty, "string");
    assert!(!res.applied);
    assert!(!res.unmatched.is_empty());
    assert_eq!(fs::read_to_string(&user_file).unwrap(), user_before);
    assert_eq!(fs::read_to_string(&client_file).unwrap(), client_before);
}

#[tokio::test]
async fn test_encapsulate_field_typescript_multi_file_semantic_references() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "user.ts",
            r#"export class User {
    name: string;
    age: number;

    constructor(name: string, age: number) {
        this.name = name;
        this.age = age;
    }

    display(): string {
        return this.name;
    }
}
"#,
        ),
        (
            "client.ts",
            r#"import { User } from "./user";

export function handleUser(user: User) {
    user.name = "Bob";
    console.log(user.name);
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let user_file = root.join("user.ts");
    let client_file = root.join("client.ts");
    let user_text = fs::read_to_string(&user_file).unwrap();
    let client_text = fs::read_to_string(&client_file).unwrap();
    let gw = semantic_gateway(
        &user_file,
        "User",
        "name",
        &[(&user_file, &user_text), (&client_file, &client_text)],
    )
    .await;

    let res = encapsulate_polyglot(
        gw.addr(),
        &root,
        &user_file,
        Some("User"),
        "name",
        None,
        true,
        false,
    )
    .await
    .unwrap();

    assert!(res.unmatched.is_empty(), "{:?}", res.unmatched);
    assert!(res.applied);
    assert_eq!(res.reads, 1);
    assert_eq!(res.writes, 1);
    assert_eq!(res.rewritten.len(), 2);
    assert!(
        fs::read_to_string(&client_file)
            .unwrap()
            .contains("user.setName(\"Bob\");")
    );
    assert!(
        fs::read_to_string(&client_file)
            .unwrap()
            .contains("console.log(user.getName());")
    );
}

#[tokio::test]
async fn test_encapsulate_field_javascript_emits_valid_private_accessors() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "user.js",
            "export class User {\n    name = \"\";\n\n    constructor(name) {\n        this.name = name;\n    }\n\n    display() {\n        return this.name;\n    }\n}\n",
        ),
    ]);
    let root = ws.root().to_path_buf();
    let user_file = root.join("user.js");
    let gw = fake_gateway().await;
    let res = encapsulate_polyglot(
        gw.addr(),
        &root,
        &user_file,
        Some("User"),
        "name",
        None,
        true,
        false,
    )
    .await
    .unwrap();

    assert!(res.applied);
    let result = fs::read_to_string(&user_file).unwrap();
    assert!(result.contains("#name = \"\";"));
    assert!(result.contains("getName() {"));
    assert!(result.contains("setName(name) {"));
    assert!(result.contains("this.#name = name;"));
    assert!(!result.contains("private _name"));
    assert!(!result.contains(": void"));
}

#[tokio::test]
async fn test_encapsulate_field_refuses_ambiguous_same_name_accesses() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        ("user.ts", "export class User {\n    name: string;\n}\n"),
        (
            "client.ts",
            "export function demo(user: User, order: Order) {\n    console.log(user.name, order.name, \"user.name\");\n}\n",
        ),
    ]);
    let root = ws.root().to_path_buf();
    let user_file = root.join("user.ts");
    let client_file = root.join("client.ts");
    let user_before = fs::read_to_string(&user_file).unwrap();
    let client_before = fs::read_to_string(&client_file).unwrap();
    let user_uri = url::Url::from_file_path(&user_file).unwrap().to_string();
    let client_uri = url::Url::from_file_path(&client_file).unwrap().to_string();
    let symbols = serde_json::json!([{
        "name": "User",
        "kind": 5,
        "children": [{
            "name": "name",
            "kind": 8,
            "selectionRange": { "start": { "line": 1, "character": 4 }, "end": { "line": 1, "character": 8 } }
        }]
    }]);
    let references = serde_json::json!([
        { "uri": client_uri, "range": { "start": { "line": 1, "character": 21 } } }
    ]);
    let request_uri = user_uri.clone();
    let gw = ScriptedGateway::start(move |method, params| match method {
        "textDocument/documentSymbol"
            if params["textDocument"]["uri"].as_str() == Some(request_uri.as_str()) =>
        {
            symbols.clone()
        }
        "textDocument/references"
            if params["textDocument"]["uri"].as_str() == Some(request_uri.as_str()) =>
        {
            references.clone()
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await;

    let res = encapsulate_polyglot(
        gw.addr(),
        &root,
        &user_file,
        Some("User"),
        "name",
        None,
        true,
        false,
    )
    .await
    .unwrap();

    assert!(!res.applied);
    assert!(!res.unmatched.is_empty());
    assert_eq!(fs::read_to_string(&user_file).unwrap(), user_before);
    assert_eq!(fs::read_to_string(&client_file).unwrap(), client_before);
}

#[tokio::test]
async fn test_encapsulate_field_python_multi_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "account.py",
            r#"class Account:
    def __init__(self, balance: int):
        self.balance = balance

    def describe(self) -> str:
        return f"Balance: {self.balance}"
"#,
        ),
        (
            "transfer.py",
            r#"def deposit(acc):
    acc.balance = 100
    print(acc.balance)
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let account_file = root.join("account.py");
    let transfer_file = root.join("transfer.py");
    let account_text = fs::read_to_string(&account_file).unwrap();
    let transfer_text = fs::read_to_string(&transfer_file).unwrap();
    let gw = semantic_gateway(
        &account_file,
        "Account",
        "balance",
        &[
            (&account_file, &account_text),
            (&transfer_file, &transfer_text),
        ],
    )
    .await;

    let res = encapsulate_polyglot(
        gw.addr(),
        &root,
        &account_file,
        Some("Account"),
        "balance",
        None,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.owner, "Account");
    assert_eq!(res.field, "balance");
    assert!(res.applied);

    let account_content = fs::read_to_string(&account_file).unwrap();
    assert!(account_content.contains("self._balance = balance"));
    assert!(account_content.contains("def get_balance(self) -> int:"));
    assert!(account_content.contains("return self._balance"));
    assert!(account_content.contains("def set_balance(self, balance: int) -> None:"));
    assert!(account_content.contains("self._balance = balance"));

    let transfer_content = fs::read_to_string(&transfer_file).unwrap();
    assert!(transfer_content.contains("acc.set_balance(100)"));
    assert!(transfer_content.contains("print(acc.get_balance())"));
}

#[tokio::test]
async fn test_encapsulate_field_cpp_multi_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "widget.cpp",
            r#"class Widget {
public:
    int count;

    int total() const {
        return count;
    }
};
"#,
        ),
        (
            "main.cpp",
            r#"void run(Widget& w) {
    w.count = 42;
    int c = w.count;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let widget_file = root.join("widget.cpp");
    let main_file = root.join("main.cpp");
    let widget_text = fs::read_to_string(&widget_file).unwrap();
    let main_text = fs::read_to_string(&main_file).unwrap();
    let gw = semantic_gateway(
        &widget_file,
        "Widget",
        "count",
        &[(&widget_file, &widget_text), (&main_file, &main_text)],
    )
    .await;

    let res = encapsulate_polyglot(
        gw.addr(),
        &root,
        &widget_file,
        Some("Widget"),
        "count",
        Some(true),
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.owner, "Widget");
    assert_eq!(res.field, "count");
    assert_eq!(res.ty, "int");
    assert!(res.applied);

    let widget_content = fs::read_to_string(&widget_file).unwrap();
    assert!(widget_content.contains("int count_;"));
    assert!(widget_content.contains("int get_count() const {"));
    assert!(widget_content.contains("return count_;"));
    assert!(widget_content.contains("void set_count(int count) {"));
    assert!(widget_content.contains("count_ = count;"));

    let main_content = fs::read_to_string(&main_file).unwrap();
    assert!(main_content.contains("w.set_count(42);"));
    assert!(main_content.contains("int c = w.get_count();"));
}

#[tokio::test]
async fn test_encapsulate_field_swift_multi_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "person.swift",
            r#"class Person {
    var title: String

    init(title: String) {
        self.title = title
    }
}
"#,
        ),
        (
            "app.swift",
            r#"func update(p: Person) {
    p.title = "Dr"
    print(p.title)
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let person_file = root.join("person.swift");
    let app_file = root.join("app.swift");
    let person_text = fs::read_to_string(&person_file).unwrap();
    let app_text = fs::read_to_string(&app_file).unwrap();
    let gw = semantic_gateway(
        &person_file,
        "Person",
        "title",
        &[(&person_file, &person_text), (&app_file, &app_text)],
    )
    .await;

    let res = encapsulate_polyglot(
        gw.addr(),
        &root,
        &person_file,
        Some("Person"),
        "title",
        None,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.owner, "Person");
    assert_eq!(res.field, "title");
    assert!(res.applied);

    let person_content = fs::read_to_string(&person_file).unwrap();
    assert!(person_content.contains("private var _title: String"));
    assert!(person_content.contains("func getTitle() -> String {"));
    assert!(person_content.contains("return _title"));
    assert!(person_content.contains("func setTitle(_ title: String) {"));
    assert!(person_content.contains("_title = title"));

    let app_content = fs::read_to_string(&app_file).unwrap();
    assert!(app_content.contains("p.setTitle(\"Dr\")"));
    assert!(app_content.contains("print(p.getTitle())"));
}

#[tokio::test]
async fn test_encapsulate_field_go_multi_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "entity.go",
            r#"package entity

type Entity struct {
	ID string
}
"#,
        ),
        (
            "main.go",
            r#"package main

import "entity"

func Run(e *entity.Entity) {
	e.ID = "123"
	println(e.ID)
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let entity_file = root.join("entity.go");
    let main_file = root.join("main.go");
    let gw = fake_gateway().await;

    let err = encapsulate_polyglot(
        gw.addr(),
        &root,
        &entity_file,
        Some("Entity"),
        "ID",
        None,
        true,
        false,
    )
    .await
    .expect_err("exported Go fields must keep serialization and public API behavior");
    assert!(
        err.to_string().contains("exported Go field `ID`"),
        "{err:#}"
    );
    assert!(
        fs::read_to_string(&entity_file)
            .unwrap()
            .contains("ID string")
    );
    assert!(
        fs::read_to_string(&main_file)
            .unwrap()
            .contains("e.ID = \"123\"")
    );
}

#[tokio::test]
async fn test_encapsulate_field_via_execute_tool() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "service.ts",
            r#"export class Server {
    port: number;
    host: string;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let service_file = root.join("service.ts");
    let gw = fake_gateway().await;

    let args = serde_json::json!({
        "path": "service.ts",
        "field": "port",
        "apply": true,
    });

    let tool_res =
        prod_code_mcp::tools::execute_tool(gw.addr(), &root, "code_encapsulate_field", args)
            .await
            .unwrap();

    assert!(!tool_res.is_error);
    let content = fs::read_to_string(&service_file).unwrap();
    assert!(content.contains("private _port: number;"));
    assert!(content.contains("public getPort(): number"));
    assert!(content.contains("public setPort(port: number): void"));
}
