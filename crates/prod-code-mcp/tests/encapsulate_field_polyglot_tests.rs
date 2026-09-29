use prod_code_mcp::encapsulate_field::encapsulate_polyglot;
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
async fn test_encapsulate_field_typescript_multi_file() {
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
    assert!(res.applied);
    assert_eq!(res.reads, 1); // 1 in client.ts (external read)
    assert_eq!(res.writes, 1); // 1 in client.ts (external write)
    assert_eq!(res.left_in_file, 2); // 2 inside User (constructor and display)

    let user_content = fs::read_to_string(&user_file).unwrap();
    assert!(user_content.contains("private _name: string;"));
    assert!(user_content.contains("public getName(): string {"));
    assert!(user_content.contains("return this._name;"));
    assert!(user_content.contains("public setName(name: string): void {"));
    assert!(user_content.contains("this._name = name;")); // in constructor

    let client_content = fs::read_to_string(&client_file).unwrap();
    assert!(client_content.contains("user.setName(\"Bob\");"));
    assert!(client_content.contains("console.log(user.getName());"));
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
    let gw = fake_gateway().await;

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
    let gw = fake_gateway().await;

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
    let gw = fake_gateway().await;

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

    let res = encapsulate_polyglot(
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
    .unwrap();

    assert_eq!(res.owner, "Entity");
    assert_eq!(res.field, "ID");
    assert!(res.applied);

    let entity_content = fs::read_to_string(&entity_file).unwrap();
    assert!(entity_content.contains("id string"));
    assert!(entity_content.contains("func (e *Entity) ID() string {"));
    assert!(entity_content.contains("return e.id"));
    assert!(entity_content.contains("func (e *Entity) SetID(id string) {"));
    assert!(entity_content.contains("e.id = id"));

    let main_content = fs::read_to_string(&main_file).unwrap();
    assert!(main_content.contains("e.SetID(\"123\")"));
    assert!(main_content.contains("println(e.ID())"));
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

    let tool_res = prod_code_mcp::tools::execute_tool(
        gw.addr(),
        &root,
        "code_encapsulate_field",
        args,
    )
    .await
    .unwrap();

    assert!(!tool_res.is_error);
    let content = fs::read_to_string(&service_file).unwrap();
    assert!(content.contains("private _port: number;"));
    assert!(content.contains("public getPort(): number"));
    assert!(content.contains("public setPort(port: number): void"));
}
