use prod_code_mcp::replace_conditional::replace_conditional_impl;
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
async fn test_replace_conditional_typescript_switch() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "bird.ts",
            r#"export function getSpeed(type: string): number {
    switch (type) {
        case "EUROPEAN":
            return 10;
        case "AFRICAN":
            return 8;
        default:
            throw new Error("Unknown");
    }
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("bird.ts");
    let gw = fake_gateway().await;

    let res = replace_conditional_impl(
        gw.addr(),
        &root,
        &file,
        2,
        5,
        "Bird",
        "getSpeed",
        &[],
        Some("number"),
        Some("bird"),
        true,
        false,
        None,
    )
    .await
    .unwrap();

    assert_eq!(res.base_name, "Bird");
    assert_eq!(res.method_name, "getSpeed");
    assert!(res.variants.contains(&"European".to_string()));
    assert!(res.variants.contains(&"African".to_string()));
    assert!(res.applied);

    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("export interface Bird {"));
    assert!(content.contains("getSpeed(): number;"));
    assert!(content.contains("export class EuropeanBird implements Bird {"));
    assert!(content.contains("return 10;"));
    assert!(content.contains("export class AfricanBird implements Bird {"));
    assert!(content.contains("return 8;"));
    assert!(content.contains("return bird.getSpeed();"));
}

#[tokio::test]
async fn test_replace_conditional_python_if_elif() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "payroll.py",
            r#"def calculate_pay(employee_type, salary, bonus):
    if employee_type == "ENGINEER":
        return salary
    elif employee_type == "MANAGER":
        return salary + bonus
    else:
        raise ValueError("Unknown")
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("payroll.py");
    let gw = fake_gateway().await;

    let res = replace_conditional_impl(
        gw.addr(),
        &root,
        &file,
        2,
        5,
        "Employee",
        "calculate_pay",
        &["salary".to_string(), "bonus".to_string()],
        None,
        Some("employee"),
        true,
        false,
        None,
    )
    .await
    .unwrap();

    assert_eq!(res.base_name, "Employee");
    assert_eq!(res.method_name, "calculate_pay");
    assert!(res.applied);

    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("class Employee:"));
    assert!(content.contains("def calculate_pay(self, salary, bonus):"));
    assert!(content.contains("class EngineerEmployee(Employee):"));
    assert!(content.contains("return salary"));
    assert!(content.contains("class ManagerEmployee(Employee):"));
    assert!(content.contains("return salary + bonus"));
    assert!(content.contains("return employee.calculate_pay(salary, bonus)"));
}

#[tokio::test]
async fn test_replace_conditional_cpp_switch() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "speed.cpp",
            r#"double calculateSpeed(BirdType type) {
    switch (type) {
        case EUROPEAN:
            return 10.0;
        case AFRICAN:
            return 8.0;
        default:
            throw std::invalid_argument("Unknown");
    }
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("speed.cpp");
    let gw = fake_gateway().await;

    let res = replace_conditional_impl(
        gw.addr(),
        &root,
        &file,
        2,
        5,
        "Bird",
        "getSpeed",
        &[],
        Some("double"),
        Some("bird"),
        true,
        false,
        None,
    )
    .await
    .unwrap();

    assert_eq!(res.base_name, "Bird");
    assert!(res.applied);

    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("class Bird {"));
    assert!(content.contains("virtual double getSpeed() = 0;"));
    assert!(content.contains("class EuropeanBird : public Bird {"));
    assert!(content.contains("return 10.0;"));
    assert!(content.contains("return bird.getSpeed();"));
}

#[tokio::test]
async fn test_replace_conditional_swift_switch() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "speed.swift",
            r#"func getSpeed(type: BirdType) -> Double {
    switch type {
    case .european:
        return 10.0
    case .african:
        return 8.0
    default:
        fatalError("Unknown")
    }
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("speed.swift");
    let gw = fake_gateway().await;

    let res = replace_conditional_impl(
        gw.addr(),
        &root,
        &file,
        2,
        5,
        "Bird",
        "getSpeed",
        &[],
        Some("Double"),
        Some("bird"),
        true,
        false,
        None,
    )
    .await
    .unwrap();

    assert_eq!(res.base_name, "Bird");
    assert!(res.applied);

    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("protocol Bird {"));
    assert!(content.contains("func getSpeed() -> Double"));
    assert!(content.contains("struct EuropeanBird: Bird {"));
    assert!(content.contains("return bird.getSpeed()"));
}

#[tokio::test]
async fn test_replace_conditional_rust_match() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "speed.rs",
            r#"fn calculate_speed(bird: &BirdType) -> u32 {
    match bird {
        BirdType::European => 10,
        BirdType::African => 8,
        _ => panic!("Unknown"),
    }
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("speed.rs");
    let gw = fake_gateway().await;

    let res = replace_conditional_impl(
        gw.addr(),
        &root,
        &file,
        2,
        5,
        "Bird",
        "get_speed",
        &[],
        Some("u32"),
        Some("bird"),
        true,
        false,
        None,
    )
    .await
    .unwrap();

    assert_eq!(res.base_name, "Bird");
    assert!(res.applied);

    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("pub trait Bird {"));
    assert!(content.contains("fn get_speed(&self) -> u32;"));
    assert!(content.contains("pub struct EuropeanBird;"));
    assert!(content.contains("impl Bird for EuropeanBird {"));
    assert!(content.contains("pub struct AfricanBird;"));
    assert!(content.contains("bird.get_speed()"));
}

#[tokio::test]
async fn test_replace_conditional_dry_run() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "bird.ts",
            r#"export function getSpeed(type: string): number {
    switch (type) {
        case "EUROPEAN":
            return 10;
        default:
            return 0;
    }
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("bird.ts");
    let original = fs::read_to_string(&file).unwrap();
    let gw = fake_gateway().await;

    let res = replace_conditional_impl(
        gw.addr(),
        &root,
        &file,
        2,
        5,
        "Bird",
        "getSpeed",
        &[],
        Some("number"),
        Some("bird"),
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
async fn test_replace_conditional_no_block_error() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "plain.ts",
            r#"export function calculate(): number {
    const a = 1;
    const b = 2;
    return a + b;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("plain.ts");
    let gw = fake_gateway().await;

    let err = replace_conditional_impl(
        gw.addr(),
        &root,
        &file,
        2,
        5,
        "Calc",
        "calculate",
        &[],
        Some("number"),
        None,
        true,
        false,
        None,
    )
    .await
    .unwrap_err();

    assert!(
        err.to_string()
            .contains("no switch, match, or if-else conditional block found")
    );
}

#[tokio::test]
async fn replace_conditional_requires_an_explicit_polymorphic_receiver() {
    let source = r#"function run(kind: string) {
    switch (kind) {
        case "A":
            logA();
            break;
        default:
            logOther();
    }
}
"#;
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("run.ts", source)]);
    let root = ws.root().to_path_buf();
    let file = root.join("run.ts");
    let gw = fake_gateway().await;

    let err = replace_conditional_impl(
        gw.addr(),
        &root,
        &file,
        2,
        5,
        "Handler",
        "handle",
        &[],
        None,
        None,
        true,
        false,
        None,
    )
    .await
    .unwrap_err();

    assert!(
        err.to_string().contains("`target_var` is required"),
        "{err:#}"
    );
    assert_eq!(fs::read_to_string(file).unwrap(), source);
}
