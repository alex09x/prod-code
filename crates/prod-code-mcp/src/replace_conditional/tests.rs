/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::parse::{parse_if_else_block, parse_rust_match, parse_switch_block};
use super::transform::{
    transform_cpp, transform_python, transform_rust, transform_swift, transform_typescript,
};
use super::types::{
    ConditionalBlock, ConditionalBranch, ConditionalKind, returns_from_conditional,
};

#[test]
fn test_ts_switch_replace_conditional() {
    let ts_code = r#"export function getSpeed(type: string): number {
    switch (type) {
        case "EUROPEAN":
            return 10;
        case "AFRICAN":
            return 8;
        default:
            throw new Error("Unknown");
    }
}
"#;
    let block = parse_switch_block(ts_code, 0).unwrap();
    assert_eq!(block.kind, ConditionalKind::Switch);
    assert_eq!(block.discriminator, "type");
    assert_eq!(block.branches.len(), 3);
    assert_eq!(block.branches[0].variant_name, "European");
    assert_eq!(block.branches[1].variant_name, "African");

    let res = transform_typescript(
        ts_code,
        &block,
        "Bird",
        "getSpeed",
        &[],
        Some("number"),
        "bird",
    )
    .unwrap();

    assert!(res.contains("export interface Bird {"));
    assert!(res.contains("getSpeed(): number;"));
    assert!(res.contains("export class EuropeanBird implements Bird {"));
    assert!(res.contains("return 10;"));
    assert!(res.contains("export class AfricanBird implements Bird {"));
    assert!(res.contains("return 8;"));
    assert!(res.contains("return bird.getSpeed();"));
}

#[test]
fn switch_parser_ignores_keywords_in_comments_strings_and_identifiers() {
    let source = r#"function report(kind: string) {
    const switcheroo = "switch (fake) { case 'bad': }";
    // switch (alsoFake) { case "bad": }
    switch (kind) {
        case "real":
            reportReal();
            break;
        default:
            reportOther();
    }
}
"#;
    let block = parse_switch_block(source, 0).unwrap();
    assert_eq!(block.discriminator, "kind");
    assert_eq!(block.branches.len(), 2);
    assert!(
        block
            .branches
            .iter()
            .all(|branch| !branch.body.contains("bad"))
    );
}

#[test]
fn statement_conditional_keeps_following_execution() {
    let source = r#"function report(kind: string) {
    switch (kind) {
        case "real":
            logReal();
            break;
        default:
            logOther();
    }
    cleanup();
}
"#;
    let block = parse_switch_block(source, 0).unwrap();
    let transformed =
        transform_typescript(source, &block, "Reporter", "report", &[], None, "reporter").unwrap();
    assert!(
        transformed.contains("reporter.report();\n    cleanup();"),
        "{transformed}"
    );
    assert!(
        !transformed.contains("return reporter.report();"),
        "{transformed}"
    );
}

#[test]
fn statement_conditionals_in_other_languages_also_preserve_following_execution() {
    let python = "def run(kind):\n    if kind == 'A':\n        log_a()\n    else:\n        log_other()\n    cleanup()\n";
    let python_block = parse_if_else_block(python, 0).unwrap();
    let transformed =
        transform_python(python, &python_block, "Handler", "handle", &[], "handler").unwrap();
    assert!(transformed.contains("handler.handle()"), "{transformed}");
    assert!(transformed.contains("cleanup()"), "{transformed}");
    assert!(
        !transformed.contains("return handler.handle()"),
        "{transformed}"
    );

    let cpp = "void run(int kind) {\n    switch (kind) {\n        case 1: log_a(); break;\n        default: log_other();\n    }\n    cleanup();\n}\n";
    let cpp_block = parse_switch_block(cpp, 0).unwrap();
    let transformed =
        transform_cpp(cpp, &cpp_block, "Handler", "handle", &[], None, "handler").unwrap();
    assert!(transformed.contains("handler.handle()"), "{transformed}");
    assert!(transformed.contains("cleanup();"), "{transformed}");
    assert!(
        !transformed.contains("return handler.handle()"),
        "{transformed}"
    );

    let swift = "func run(kind: Int) {\n    switch kind {\n    case 1: logA()\n    default: logOther()\n    }\n    cleanup()\n}\n";
    let swift_block = parse_switch_block(swift, 0).unwrap();
    let transformed = transform_swift(
        swift,
        &swift_block,
        "Handler",
        "handle",
        &[],
        None,
        "handler",
    )
    .unwrap();
    assert!(transformed.contains("handler.handle()"), "{transformed}");
    assert!(transformed.contains("cleanup()"), "{transformed}");
    assert!(
        !transformed.contains("return handler.handle()"),
        "{transformed}"
    );
}

#[test]
fn test_python_if_elif_replace_conditional() {
    let py_code = r#"def calculate_pay(employee_type, salary, bonus):
    if employee_type == "ENGINEER":
        return salary
    elif employee_type == "MANAGER":
        return salary + bonus
    else:
        raise ValueError("Unknown")
"#;
    let block = parse_if_else_block(py_code, 0).unwrap();
    assert_eq!(block.kind, ConditionalKind::IfElse);
    assert_eq!(block.discriminator, "employee_type");
    assert_eq!(block.branches.len(), 3);

    let res = transform_python(
        py_code,
        &block,
        "Employee",
        "calculate_pay",
        &["salary".to_string(), "bonus".to_string()],
        "employee",
    )
    .unwrap();

    assert!(res.contains("class Employee:"));
    assert!(res.contains("def calculate_pay(self, salary, bonus):"));
    assert!(res.contains("class EngineerEmployee(Employee):"));
    assert!(res.contains("return salary"));
    assert!(res.contains("class ManagerEmployee(Employee):"));
    assert!(res.contains("return salary + bonus"));
    assert!(res.contains("return employee.calculate_pay(salary, bonus)"));
}

#[test]
fn test_cpp_switch_replace_conditional() {
    let cpp_code = r#"double calculateSpeed(BirdType type) {
    switch (type) {
        case EUROPEAN:
            return 10.0;
        case AFRICAN:
            return 8.0;
        default:
            throw std::invalid_argument("Unknown");
    }
}
"#;
    let block = parse_switch_block(cpp_code, 0).unwrap();
    assert_eq!(block.branches.len(), 3);
    let res = transform_cpp(
        cpp_code,
        &block,
        "Bird",
        "getSpeed",
        &[],
        Some("double"),
        "bird",
    )
    .unwrap();
    assert!(res.contains("class Bird {"));
    assert!(res.contains("virtual double getSpeed() = 0;"));
    assert!(res.contains("class EuropeanBird : public Bird {"));
    assert!(res.contains("return 10.0;"));
    assert!(res.contains("return bird.getSpeed();"));
}

#[test]
fn test_swift_switch_replace_conditional() {
    let swift_code = r#"func getSpeed(type: BirdType) -> Double {
    switch type {
    case .european:
        return 10.0
    case .african:
        return 8.0
    default:
        fatalError("Unknown")
    }
}
"#;
    let block = parse_switch_block(swift_code, 0).unwrap();
    assert_eq!(block.branches.len(), 3);
    let res = transform_swift(
        swift_code,
        &block,
        "Bird",
        "getSpeed",
        &[],
        Some("Double"),
        "bird",
    )
    .unwrap();
    assert!(res.contains("protocol Bird {"));
    assert!(res.contains("func getSpeed() -> Double"));
    assert!(res.contains("struct EuropeanBird: Bird {"));
    assert!(res.contains("return bird.getSpeed()"));
}

#[test]
fn test_rust_match_replace_conditional() {
    let rust_code = r#"fn calculate_speed(bird: &BirdType) -> u32 {
    match bird {
        BirdType::European => 10,
        BirdType::African => 8,
        _ => panic!("Unknown"),
    }
}
"#;
    let block = parse_rust_match(rust_code, 0).unwrap();
    assert_eq!(block.discriminator, "bird");
    assert_eq!(block.branches.len(), 3);
    assert_eq!(block.branches[0].variant_name, "European");
    assert_eq!(block.branches[1].variant_name, "African");
    assert_eq!(block.branches[2].variant_name, "Default");

    let res = transform_rust(
        rust_code,
        &block,
        "Bird",
        "get_speed",
        &[],
        Some("u32"),
        "bird",
    )
    .unwrap();
    assert!(res.contains("pub trait Bird {"));
    assert!(res.contains("fn get_speed(&self) -> u32;"));
    assert!(res.contains("pub struct EuropeanBird;"));
    assert!(res.contains("impl Bird for EuropeanBird {"));
    assert!(res.contains("pub struct AfricanBird;"));
    assert!(res.contains("bird.get_speed()"));
}

#[test]
fn test_bare_return_in_conditional_branches() {
    let block = ConditionalBlock {
        kind: ConditionalKind::Switch,
        start_offset: 0,
        end_offset: 50,
        discriminator: "action".to_string(),
        branches: vec![
            ConditionalBranch {
                tag: "Action.Stop".to_string(),
                variant_name: "Stop".to_string(),
                body: "return;\n".to_string(),
                is_default: false,
            },
            ConditionalBranch {
                tag: "default".to_string(),
                variant_name: "Default".to_string(),
                body: "return;".to_string(),
                is_default: true,
            },
        ],
        indent: "".to_string(),
    };
    assert!(returns_from_conditional(&block).unwrap());
}

#[test]
fn test_python_bare_return_in_conditional_branches() {
    let block = ConditionalBlock {
        kind: ConditionalKind::Match,
        start_offset: 0,
        end_offset: 50,
        discriminator: "status".to_string(),
        branches: vec![
            ConditionalBranch {
                tag: "Status.Pending".to_string(),
                variant_name: "Pending".to_string(),
                body: "return".to_string(),
                is_default: false,
            },
            ConditionalBranch {
                tag: "_".to_string(),
                variant_name: "Default".to_string(),
                body: "return\n".to_string(),
                is_default: true,
            },
        ],
        indent: "".to_string(),
    };
    assert!(returns_from_conditional(&block).unwrap());
}
