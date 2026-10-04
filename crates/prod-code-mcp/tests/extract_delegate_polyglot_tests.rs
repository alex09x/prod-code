use prod_code_mcp::extract_delegate::extract_delegate_polyglot;
use prod_code_testkit::{answers, ScriptedGateway, Workspace};
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
    fields: &[&str],
    sources: &[(&Path, &str)],
) -> ScriptedGateway {
    let owner_uri = url::Url::from_file_path(owner_file).unwrap().to_string();
    let children: Vec<serde_json::Value> = fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            serde_json::json!({
                "name": field,
                "kind": 8,
                "selectionRange": { "start": { "line": 0, "character": index }, "end": { "line": 0, "character": index + field.len() } }
            })
        })
        .collect();
    let symbols = serde_json::json!([{
        "name": owner,
        "kind": 5,
        "selectionRange": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": owner.len() } },
        "children": children
    }]);
    let mut references_by_field = Vec::new();
    for field in fields {
        let mut locations = Vec::new();
        for (path, source) in sources {
            let uri = url::Url::from_file_path(path).unwrap().to_string();
            for (line_number, line) in source.lines().enumerate() {
                for needle in [format!(".{field}"), format!("->{field}")] {
                    let mut offset = 0;
                    while let Some(relative) = line[offset..].find(&needle) {
                        let start = offset + relative + needle.len() - field.len();
                        let character = line[..start].encode_utf16().count();
                        locations.push(serde_json::json!({
                            "uri": uri.clone(),
                            "range": { "start": { "line": line_number, "character": character } }
                        }));
                        offset = start + field.len();
                    }
                }
            }
        }
        references_by_field.push(serde_json::Value::Array(locations));
    }
    ScriptedGateway::start(move |method, params| match method {
        "textDocument/documentSymbol"
            if params["textDocument"]["uri"].as_str() == Some(owner_uri.as_str()) =>
        {
            symbols.clone()
        }
        "textDocument/references"
            if params["textDocument"]["uri"].as_str() == Some(owner_uri.as_str()) =>
        {
            let index = params["position"]["character"].as_u64().unwrap_or(0) as usize;
            references_by_field
                .get(index)
                .cloned()
                .unwrap_or_else(|| serde_json::json!([]))
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await
}

#[tokio::test]
async fn test_extract_delegate_typescript_multi_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "account.ts",
            r#"export class Account {
    street: string;
    city: string;
    balance: number;

    address(): string {
        return `${this.street}, ${this.city}`;
    }

    deposit(amount: number): void {
        this.balance += amount;
    }
}
"#,
        ),
        (
            "client.ts",
            r#"import { Account } from "./account";

export function printDetails(acc: Account) {
    console.log(acc.street);
    console.log(acc.city);
    console.log(acc.address());
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let account_file = root.join("account.ts");
    let client_file = root.join("client.ts");
    let account_text = fs::read_to_string(&account_file).unwrap();
    let client_text = fs::read_to_string(&client_file).unwrap();
    let gw = semantic_gateway(
        &account_file,
        "Account",
        &["street", "city"],
        &[(&account_file, &account_text), (&client_file, &client_text)],
    )
    .await;

    let res = extract_delegate_polyglot(
        gw.addr(),
        &root,
        &account_file,
        Some("Account"),
        None,
        None,
        &["street".to_string(), "city".to_string()],
        &["address".to_string()],
        "AddressInfo",
        "addressInfo",
        true,
        false,
        None,
    )
    .await
    .unwrap();

    assert_eq!(res.helper, "AddressInfo");
    assert_eq!(res.field, "addressInfo");
    assert!(res.applied);
    assert_eq!(res.accesses, 2); // acc.street and acc.city in client.ts

    let account_content = fs::read_to_string(&account_file).unwrap();
    assert!(account_content.contains(
        "addressInfo: AddressInfo = new AddressInfo(undefined as any, undefined as any);"
    ));
    assert!(account_content.contains("class AddressInfo {"));
    assert!(account_content.contains("street: string;"));
    assert!(account_content.contains("city: string;"));
    assert!(account_content.contains("return this.addressInfo.address();"));
    assert!(account_content.contains("deposit(amount: number)"));

    let client_content = fs::read_to_string(&client_file).unwrap();
    assert!(client_content.contains("console.log(acc.addressInfo.street);"));
    assert!(client_content.contains("console.log(acc.addressInfo.city);"));
    assert!(client_content.contains("console.log(acc.address());"));
}

#[tokio::test]
async fn test_extract_delegate_python_multi_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "person.py",
            r#"class Person:
    def __init__(self, street: str, city: str, age: int):
        self.street = street
        self.city = city
        self.age = age

    def address(self) -> str:
        return f"{self.street}, {self.city}"

    def birthday(self):
        self.age += 1
"#,
        ),
        (
            "app.py",
            r#"from person import Person

def run(p: Person):
    print(p.street)
    print(p.city)
    print(p.address())
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let person_file = root.join("person.py");
    let app_file = root.join("app.py");
    let person_text = fs::read_to_string(&person_file).unwrap();
    let app_text = fs::read_to_string(&app_file).unwrap();
    let gw = semantic_gateway(
        &person_file,
        "Person",
        &["street", "city"],
        &[(&person_file, &person_text), (&app_file, &app_text)],
    )
    .await;

    let res = extract_delegate_polyglot(
        gw.addr(),
        &root,
        &person_file,
        Some("Person"),
        None,
        None,
        &["street".to_string(), "city".to_string()],
        &["address".to_string()],
        "Address",
        "addr",
        true,
        false,
        None,
    )
    .await
    .unwrap();

    assert_eq!(res.helper, "Address");
    assert_eq!(res.field, "addr");
    assert!(res.applied);
    assert_eq!(res.accesses, 2);

    let person_content = fs::read_to_string(&person_file).unwrap();
    assert!(person_content.contains("self.addr = Address(street, city)"));
    assert!(person_content.contains("class Address:"));
    assert!(person_content.contains("return self.addr.address()"));

    let app_content = fs::read_to_string(&app_file).unwrap();
    assert!(app_content.contains("print(p.addr.street)"));
    assert!(app_content.contains("print(p.addr.city)"));
    assert!(app_content.contains("print(p.address())"));
}

#[tokio::test]
async fn test_extract_delegate_cpp_refuses_unqualified_owner_field_uses() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "account.hpp",
            r#"#include <string>

class Account {
public:
    std::string street;
    std::string city;
    int balance;

    std::string address() {
        return street + ", " + city;
    }

    void deposit(int amt) {
        balance += amt;
    }
};
"#,
        ),
        (
            "client.cpp",
            r#"#include "account.hpp"
#include <iostream>

void inspect(Account& acc, Account* ptr) {
    std::cout << acc.street << std::endl;
    std::cout << ptr->city << std::endl;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let account_file = root.join("account.hpp");
    let client_file = root.join("client.cpp");
    let account_before = fs::read_to_string(&account_file).unwrap();
    let client_before = fs::read_to_string(&client_file).unwrap();
    let account_text = fs::read_to_string(&account_file).unwrap();
    let client_text = fs::read_to_string(&client_file).unwrap();
    let gw = semantic_gateway(
        &account_file,
        "Account",
        &["street", "city"],
        &[(&account_file, &account_text), (&client_file, &client_text)],
    )
    .await;

    let error = extract_delegate_polyglot(
        gw.addr(),
        &root,
        &account_file,
        Some("Account"),
        None,
        None,
        &["street".to_string(), "city".to_string()],
        &["address".to_string()],
        "Location",
        "loc",
        true,
        false,
        None,
    )
    .await
    .unwrap_err();

    assert!(format!("{error:#}").contains("unqualified moved-field uses need semantic resolution"));
    assert_eq!(fs::read_to_string(account_file).unwrap(), account_before);
    assert_eq!(fs::read_to_string(client_file).unwrap(), client_before);
}

#[tokio::test]
async fn test_extract_delegate_swift_multi_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "user.swift",
            r#"public class User {
    var street: String
    var city: String
    var role: String

    func address() -> String {
        return "\(street), \(city)"
    }
}
"#,
        ),
        (
            "client.swift",
            r#"func handleUser(u: User) {
    print(u.street)
    print(u.city)
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let user_file = root.join("user.swift");
    let client_file = root.join("client.swift");
    let user_text = fs::read_to_string(&user_file).unwrap();
    let client_text = fs::read_to_string(&client_file).unwrap();
    let gw = semantic_gateway(
        &user_file,
        "User",
        &["street", "city"],
        &[(&user_file, &user_text), (&client_file, &client_text)],
    )
    .await;

    let res = extract_delegate_polyglot(
        gw.addr(),
        &root,
        &user_file,
        Some("User"),
        None,
        None,
        &["street".to_string(), "city".to_string()],
        &["address".to_string()],
        "Address",
        "addr",
        true,
        false,
        None,
    )
    .await
    .unwrap();

    assert_eq!(res.helper, "Address");
    assert_eq!(res.field, "addr");
    assert!(res.applied);
    assert_eq!(res.accesses, 2);

    let user_content = fs::read_to_string(&user_file).unwrap();
    assert!(user_content.contains("var addr: Address"));
    assert!(user_content.contains("struct Address {"));
    assert!(user_content.contains("return addr.address()"));

    let client_content = fs::read_to_string(&client_file).unwrap();
    assert!(client_content.contains("print(u.addr.street)"));
    assert!(client_content.contains("print(u.addr.city)"));
}

#[tokio::test]
async fn test_extract_delegate_refuses_unresolved_go_literals_from_another_package() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "server.go",
            r#"package server

type Server struct {
	Host string
	Port int
	MaxClients int
}

func (s *Server) Address() string {
	return s.Host
}
"#,
        ),
        (
            "main.go",
            r#"package main

import "server"

func run() {
	srv := &server.Server{
		Host: "localhost",
		Port: 8080,
		MaxClients: 100,
	}
	println(srv.Host)
	println(srv.Port)
	println(srv.Address())
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let server_file = root.join("server.go");
    let main_file = root.join("main.go");
    let server_before = fs::read_to_string(&server_file).unwrap();
    let main_before = fs::read_to_string(&main_file).unwrap();
    let gw = fake_gateway().await;

    let err = extract_delegate_polyglot(
        gw.addr(),
        &root,
        &server_file,
        Some("Server"),
        None,
        None,
        &["Host".to_string(), "Port".to_string()],
        &["Address".to_string()],
        "Endpoint",
        "endpoint",
        true,
        false,
        None,
    )
    .await
    .expect_err("unresolved package-qualified Go literals must not be rewritten");
    assert!(format!("{err:#}").contains("Go literals need package-aware semantic resolution"), "{err:#}");
    assert_eq!(fs::read_to_string(&server_file).unwrap(), server_before);
    assert_eq!(fs::read_to_string(&main_file).unwrap(), main_before);
}

#[tokio::test]
async fn test_extract_delegate_safety_checks_refused() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "model.ts",
            r#"export class Model {
    id: string;
    title: string;
    score: number;

    display(): string {
        return `${this.id}: ${this.title} (${this.score})`;
    }
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let model_file = root.join("model.ts");
    let gw = fake_gateway().await;

    // 1. Moved method `display` uses `score`, which is not in moved fields `id`, `title`
    let err = extract_delegate_polyglot(
        gw.addr(),
        &root,
        &model_file,
        Some("Model"),
        None,
        None,
        &["id".to_string(), "title".to_string()],
        &["display".to_string()],
        "Metadata",
        "meta",
        false,
        false,
        None,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("uses `score`, which does not move"));

    // 2. Non-existent field
    let err2 = extract_delegate_polyglot(
        gw.addr(),
        &root,
        &model_file,
        Some("Model"),
        None,
        None,
        &["nonexistent".to_string()],
        &[],
        "Metadata",
        "meta",
        false,
        false,
        None,
    )
    .await
    .unwrap_err();
    assert!(err2.to_string().contains("has no field `nonexistent`"));

    // 3. Non-existent method
    let err3 = extract_delegate_polyglot(
        gw.addr(),
        &root,
        &model_file,
        Some("Model"),
        None,
        None,
        &["id".to_string()],
        &["nonexistentMethod".to_string()],
        "Metadata",
        "meta",
        false,
        false,
        None,
    )
    .await
    .unwrap_err();
    assert!(err3.to_string().contains("has no method `nonexistentMethod`"));

    // 4. Delegate field already exists
    let err4 = extract_delegate_polyglot(
        gw.addr(),
        &root,
        &model_file,
        Some("Model"),
        None,
        None,
        &["id".to_string()],
        &[],
        "Metadata",
        "title", // already exists on Model
        false,
        false,
        None,
    )
    .await
    .unwrap_err();
    assert!(err4.to_string().contains("already has a field `title`"));
}

#[tokio::test]
async fn test_extract_delegate_via_mcp_execute_tool() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "service.py",
            r#"class Service:
    def __init__(self, host: str, port: int, name: str):
        self.host = host
        self.port = port
        self.name = name

    def url(self) -> str:
        return f"{self.host}:{self.port}"
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let service_file = root.join("service.py");
    let gw = fake_gateway().await;

    let args = serde_json::json!({
        "path": "service.py",
        "symbol": "Service",
        "fields": ["host", "port"],
        "methods": ["url"],
        "name": "Endpoint",
        "field": "endpoint",
        "apply": true,
    });

    let tool_res = prod_code_mcp::tools::execute_tool(
        gw.addr(),
        &root,
        "code_extract_delegate",
        args,
    )
    .await
    .unwrap();

    assert!(!tool_res.is_error);
    let content = fs::read_to_string(&service_file).unwrap();
    assert!(content.contains("self.endpoint = Endpoint(host, port)"));
    assert!(content.contains("class Endpoint:"));
    assert!(content.contains("return self.endpoint.url()"));
}
