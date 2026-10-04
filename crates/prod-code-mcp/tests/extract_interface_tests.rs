use prod_code_mcp::extract_interface::extract_interface_impl;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
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
async fn test_extract_interface_typescript() {
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
        true,
        true,
        false,
        None,
    )
    .await
    .unwrap();

    assert_eq!(res.type_name, "UserService");
    assert_eq!(res.interface_name, "IUserService");
    assert!(res.methods.contains(&"getUser".to_string()));
    assert!(res.methods.contains(&"saveUser".to_string()));
    assert!(res.applied);

    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("export interface IUserService {"));
    assert!(content.contains("getUser(id: string): User;"));
    assert!(content.contains("saveUser(user: User): boolean;"));
    assert!(content.contains("export class UserService implements IUserService {"));
}

#[tokio::test]
async fn test_extract_interface_go() {
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
        true,
        true,
        false,
        None,
    )
    .await
    .unwrap();

    assert_eq!(res.type_name, "UserService");
    assert_eq!(res.interface_name, "UserReader");
    assert_eq!(res.methods, vec!["GetUser"]);
    assert!(res.applied);

    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("type UserReader interface {"));
    assert!(content.contains("GetUser(id string) (*User, error)"));
    assert!(!res.diff.contains("SaveUser"));
    assert!(content.contains("type UserService struct {"));
}

#[tokio::test]
async fn test_extract_interface_python() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "order.py",
            r#"class OrderService:
    def create_order(self, item_id: str, quantity: int) -> str:
        return "order_123"

    def cancel_order(self, order_id: str) -> bool:
        return True
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("order.py");
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
        true,
        true,
        false,
        None,
    )
    .await
    .unwrap();

    assert_eq!(res.type_name, "OrderService");
    assert_eq!(res.interface_name, "OrderProtocol");
    assert!(res.applied);

    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("from typing import Protocol"));
    assert!(content.contains("class OrderProtocol(Protocol):"));
    assert!(
        content
            .contains("def create_order(self, item_id: str, quantity: int) -> str:\n        ...")
    );
    assert!(content.contains("class OrderService(OrderProtocol):"));
}

#[tokio::test]
async fn test_extract_interface_cpp() {
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
        true,
        true,
        false,
        None,
    )
    .await
    .unwrap();

    assert_eq!(res.type_name, "Shape");
    assert_eq!(res.interface_name, "IShape");
    assert!(res.applied);

    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("class IShape {"));
    assert!(content.contains("virtual double area() const = 0;"));
    assert!(content.contains("class Shape : public IShape {"));
}

#[tokio::test]
async fn test_extract_interface_swift() {
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
        true,
        true,
        false,
        None,
    )
    .await
    .unwrap();

    assert_eq!(res.type_name, "Repository");
    assert_eq!(res.interface_name, "RepositoryProtocol");
    assert!(res.applied);

    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("protocol RepositoryProtocol {"));
    assert!(content.contains("func fetch(id: Int) -> String"));
    assert!(content.contains("struct Repository: RepositoryProtocol {"));
}

#[tokio::test]
async fn test_extract_interface_dry_run() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "service.ts",
            r#"export class UserService {
    public getUser(id: string): User {
        return findUser(id);
    }
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("service.ts");
    let original = fs::read_to_string(&file).unwrap();
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
        true,
        false, // dry-run
        false,
        None,
    )
    .await
    .unwrap();

    assert!(!res.applied);
    assert!(!res.diff.is_empty());
    let current = fs::read_to_string(&file).unwrap();
    assert_eq!(current, original);
}

#[tokio::test]
async fn test_extract_interface_not_found_error() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "empty.ts",
            r#"export const x = 42;
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("empty.ts");
    let gw = fake_gateway().await;

    let err = extract_interface_impl(
        gw.addr(),
        &root,
        &file,
        "NonExistentClass",
        "INonExistent",
        &[],
        1,
        1,
        true,
        true,
        false,
        None,
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("not found"));
}

#[tokio::test]
async fn extract_interface_rejects_javascript_without_writing_typescript_syntax() {
    let source = "export class UserService {\n    getUser() { return {}; }\n}\n";
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("service.js", source)]);
    let root = ws.root().to_path_buf();
    let file = root.join("service.js");
    let gw = fake_gateway().await;

    let err = extract_interface_impl(
        gw.addr(),
        &root,
        &file,
        "UserService",
        "IUserService",
        &[],
        1,
        1,
        false,
        true,
        false,
        None,
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("does not support JavaScript"));
    assert_eq!(fs::read_to_string(file).unwrap(), source);
}

#[tokio::test]
async fn extract_interface_rejects_compile_verification_for_typescript() {
    let source = "export class UserService {\n    getUser(): string { return \"user\"; }\n}\n";
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("service.ts", source)]);
    let root = ws.root().to_path_buf();
    let file = root.join("service.ts");
    let gw = fake_gateway().await;

    let err = extract_interface_impl(
        gw.addr(),
        &root,
        &file,
        "UserService",
        "IUserService",
        &[],
        1,
        1,
        false,
        true,
        false,
        Some("compile"),
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("only supported for Rust"));
    assert_eq!(fs::read_to_string(file).unwrap(), source);
}

#[tokio::test]
async fn extract_interface_propagates_validation_transport_failure_before_apply() {
    let source = "export class UserService {\n    getUser(): string { return \"user\"; }\n}\n";
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("service.ts", source)]);
    let root = ws.root().to_path_buf();
    let file = root.join("service.ts");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let remote = listener.local_addr().unwrap();
    drop(listener);

    let err = extract_interface_impl(
        remote,
        &root,
        &file,
        "UserService",
        "IUserService",
        &[],
        1,
        1,
        false,
        true,
        false,
        None,
    )
    .await
    .unwrap_err();

    assert!(
        err.to_string().to_lowercase().contains("connect"),
        "{err:#}"
    );
    assert_eq!(fs::read_to_string(file).unwrap(), source);
}
