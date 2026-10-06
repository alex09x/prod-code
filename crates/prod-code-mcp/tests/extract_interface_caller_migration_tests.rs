/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::extract_interface::extract_interface_impl;
use prod_code_mcp::extract_trait::extract_trait_ext;
use prod_code_testkit::{answers, ScriptedGateway, Workspace};
use std::fs;

const CARGO_TOML: &str = "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

async fn fake_gateway() -> ScriptedGateway {
    ScriptedGateway::start(|method, _params| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        "textDocument/references" => serde_json::json!([]),
        _ => serde_json::Value::Null,
    })
    .await
}

#[tokio::test]
async fn test_extract_interface_typescript_caller_migration() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "service.ts",
            r#"export class UserService {
    public getUser(id: string): User {
        return findUser(id);
    }

    public saveUser(user: User): boolean {
        return true;
    }
}

export function renderUser(svc: UserService): string {
    return svc.getUser("123").name;
}

export function debugUser(svc: UserService): void {
    console.log(svc.db);
}
"#,
        ),
        (
            "client.ts",
            r#"import { UserService } from "./service";

export function handleUser(svc: UserService) {
    svc.saveUser(u);
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("service.ts");
    let client_file = root.join("client.ts");
    let gw = fake_gateway().await;

    let res = extract_interface_impl(
        gw.addr(),
        &root,
        &file,
        "UserService",
        "IUserService",
        &[],
        1,
        1,
        true, // migrate_callers
        true, // apply
        false,
        None,
    )
    .await
    .unwrap();

    assert!(res.applied);

    let service_content = fs::read_to_string(&file).unwrap();
    assert!(service_content.contains("export interface IUserService {"));
    assert!(service_content.contains("export function renderUser(svc: IUserService): string {"));
    assert!(service_content.contains("export function debugUser(svc: UserService): void {"));

    let client_content = fs::read_to_string(&client_file).unwrap();
    assert!(client_content.contains("IUserService"));
    assert!(client_content.contains("export function handleUser(svc: IUserService) {"));
}

#[tokio::test]
async fn test_extract_interface_go_caller_migration() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "user.go",
            r#"package user

type UserService struct {
    db DB
}

func (s *UserService) GetUser(id string) (*User, error) {
    return nil, nil
}

func (s *UserService) SaveUser(user *User) error {
    return nil
}

func PrintUser(s *UserService) {
    s.GetUser("1")
}

func CheckDB(s *UserService) {
    _ = s.db
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("user.go");
    let gw = fake_gateway().await;

    let res = extract_interface_impl(
        gw.addr(),
        &root,
        &file,
        "UserService",
        "UserReader",
        &["GetUser".to_string()],
        3,
        1,
        true, // migrate_callers
        true, // apply
        false,
        None,
    )
    .await
    .unwrap();

    assert!(res.applied);

    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("type UserReader interface {"));
    assert!(content.contains("func PrintUser(s UserReader) {"));
    assert!(content.contains("func CheckDB(s *UserService) {"));
}

#[tokio::test]
async fn test_extract_interface_python_caller_migration() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "order.py",
            r#"class OrderService:
    def create_order(self, item_id: str, quantity: int) -> str:
        return "order_123"

    def cancel_order(self, order_id: str) -> bool:
        return True

def process_order(order_svc: OrderService) -> str:
    return order_svc.create_order("item", 2)
"#,
        ),
        (
            "caller.py",
            r#"from order import OrderService

def run(svc: OrderService) -> bool:
    return svc.cancel_order("123")
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("order.py");
    let caller_file = root.join("caller.py");
    let gw = fake_gateway().await;

    let res = extract_interface_impl(
        gw.addr(),
        &root,
        &file,
        "OrderService",
        "OrderProtocol",
        &[],
        1,
        1,
        true, // migrate_callers
        true, // apply
        false,
        None,
    )
    .await
    .unwrap();

    assert!(res.applied);

    let order_content = fs::read_to_string(&file).unwrap();
    assert!(order_content.contains("class OrderProtocol(Protocol):"));
    assert!(order_content.contains("def process_order(order_svc: OrderProtocol) -> str:"));

    let caller_content = fs::read_to_string(&caller_file).unwrap();
    assert!(caller_content.contains("OrderProtocol"));
    assert!(caller_content.contains("def run(svc: OrderProtocol) -> bool:"));
}

#[tokio::test]
async fn test_extract_interface_cpp_caller_migration() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "shape.cpp",
            r#"class Shape {
public:
    double area() const {
        return 0.0;
    }

    double perimeter() const {
        return 0.0;
    }
};

double compute(const Shape& s) {
    return s.area();
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("shape.cpp");
    let gw = fake_gateway().await;

    let res = extract_interface_impl(
        gw.addr(),
        &root,
        &file,
        "Shape",
        "IShape",
        &[],
        1,
        1,
        true, // migrate_callers
        true, // apply
        false,
        None,
    )
    .await
    .unwrap();

    assert!(res.applied);

    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("class IShape {"));
    assert!(content.contains("double compute(const IShape& s) {"));
}

#[tokio::test]
async fn test_extract_interface_swift_caller_migration() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "repo.swift",
            r#"struct Repository {
    func fetch(id: Int) -> String {
        return "item"
    }

    func save(item: String) -> Bool {
        return true
    }
}

func sync(repo: Repository) -> String {
    return repo.fetch(id: 42)
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("repo.swift");
    let gw = fake_gateway().await;

    let res = extract_interface_impl(
        gw.addr(),
        &root,
        &file,
        "Repository",
        "RepositoryProtocol",
        &[],
        1,
        1,
        true, // migrate_callers
        true, // apply
        false,
        None,
    )
    .await
    .unwrap();

    assert!(res.applied);

    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("protocol RepositoryProtocol {"));
    assert!(content.contains("func sync(repo: RepositoryProtocol) -> String {"));
}

#[tokio::test]
async fn test_extract_trait_rust_caller_migration() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        ("src/lib.rs", "pub mod report;\npub mod shapes;\n"),
        (
            "src/shapes.rs",
            r#"pub struct Rect {
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn new(w: f64, h: f64) -> Self {
        Rect { w, h }
    }

    pub fn area(&self) -> f64 {
        self.w * self.h
    }
}
"#,
        ),
        (
            "src/report.rs",
            r#"use crate::shapes::Rect;

pub fn describe(r: &Rect) -> f64 {
    r.area()
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let shapes_file = root.join("src/shapes.rs");
    let report_file = root.join("src/report.rs");
    let gw = fake_gateway().await;

    let res = extract_trait_ext(
        gw.addr(),
        &root,
        &shapes_file,
        6,
        1,
        &["area".to_string()],
        "Measure",
        true, // migrate_callers
        true, // apply
        false,
    )
    .await
    .unwrap();

    assert!(res.applied);

    let report_content = fs::read_to_string(&report_file).unwrap();
    assert!(report_content.contains("pub fn describe(r: &impl Measure) -> f64"));
    assert!(report_content.contains("use crate::shapes::Measure;"));
}

#[tokio::test]
async fn test_extract_interface_migrate_callers_false_preserves_annotations() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "service.ts",
            r#"export class UserService {
    public getUser(id: string): User {
        return findUser(id);
    }
}

export function renderUser(svc: UserService): string {
    return svc.getUser("123").name;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("service.ts");
    let gw = fake_gateway().await;

    let res = extract_interface_impl(
        gw.addr(),
        &root,
        &file,
        "UserService",
        "IUserService",
        &[],
        1,
        1,
        false, // migrate_callers = false
        true,  // apply
        false,
        None,
    )
    .await
    .unwrap();

    assert!(res.applied);

    let service_content = fs::read_to_string(&file).unwrap();
    assert!(service_content.contains("export interface IUserService {"));
    assert!(service_content.contains("export function renderUser(svc: UserService): string {"));
    assert!(!service_content.contains("svc: IUserService"));
}
