/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::migrate::migrate_caller_annotations;
use crate::parameter_object::Language;

#[test]
fn test_ts_caller_migration() {
    let ts = r#"export function handleUser(svc: UserService, other: UserService) {
    svc.getUser("123");
    svc.deleteUser("123");
    other.getUser("456");
    other.internalDb();
}
"#;
    let methods = vec!["getUser".to_string(), "deleteUser".to_string()];
    let (out, migrations) = migrate_caller_annotations(
        ts,
        "UserService",
        "IUserService",
        &methods,
        Language::TypeScript,
    );

    assert_eq!(migrations.len(), 1);
    assert_eq!(migrations[0].var_name, "svc");
    assert!(out.contains("svc: IUserService"));
    assert!(out.contains("other: UserService"));
}

#[test]
fn test_go_caller_migration() {
    let go = r#"package user

func ProcessUser(svc *UserService) {
    svc.GetUser("1")
}

func AuditUser(svc *UserService) {
    _ = svc.db
}
"#;
    let methods = vec!["GetUser".to_string()];
    let (out, migrations) =
        migrate_caller_annotations(go, "UserService", "UserReader", &methods, Language::Go);

    assert_eq!(migrations.len(), 1);
    assert_eq!(migrations[0].var_name, "svc");
    assert!(out.contains("func ProcessUser(svc UserReader) {"));
    assert!(out.contains("func AuditUser(svc *UserService) {"));
}

#[test]
fn test_python_caller_migration() {
    let py = r#"def transfer(acc: AccountService, other: AccountService):
    acc.deposit(100.0)
    acc.withdraw(50.0)
    other.secret_key()
"#;
    let methods = vec!["deposit".to_string(), "withdraw".to_string()];
    let (out, migrations) = migrate_caller_annotations(
        py,
        "AccountService",
        "AccountProtocol",
        &methods,
        Language::Python,
    );

    assert_eq!(migrations.len(), 1);
    assert_eq!(migrations[0].var_name, "acc");
    assert!(out.contains("acc: AccountProtocol"));
    assert!(out.contains("other: AccountService"));
}

#[test]
fn test_cpp_caller_migration() {
    let cpp = r#"double calculate(const Shape& s, const Shape& other) {
    double a = s.area();
    double b = other.id;
    return a;
}
double calculate_copy(Shape s) {
    return s.area();
}
"#;
    let methods = vec!["area".to_string()];
    let (out, migrations) =
        migrate_caller_annotations(cpp, "Shape", "IShape", &methods, Language::Cpp);

    assert_eq!(migrations.len(), 1);
    assert_eq!(migrations[0].var_name, "s");
    assert!(out.contains("double calculate(const IShape& s, const Shape& other) {"));
    assert!(out.contains("double calculate_copy(Shape s) {"));
}

#[test]
fn test_swift_caller_migration() {
    let swift = r#"func executePayment(service: PaymentService, other: PaymentService) {
    _ = service.pay(amount: 100.0)
    print(other.secret)
}
"#;
    let methods = vec!["pay".to_string()];
    let (out, migrations) = migrate_caller_annotations(
        swift,
        "PaymentService",
        "PaymentProtocol",
        &methods,
        Language::Swift,
    );

    assert_eq!(migrations.len(), 1);
    assert_eq!(migrations[0].var_name, "service");
    assert!(out.contains("service: PaymentProtocol"));
    assert!(out.contains("other: PaymentService"));
}

#[test]
fn test_rust_caller_migration() {
    let rust = r#"fn print_area(r: &Rect, other: &Rect) -> f64 {
    let a = r.area();
    let b = other.w;
    a
}
"#;
    let methods = vec!["area".to_string()];
    let (out, migrations) =
        migrate_caller_annotations(rust, "Rect", "Measure", &methods, Language::Rust);

    assert_eq!(migrations.len(), 1);
    assert_eq!(migrations[0].var_name, "r");
    assert!(out.contains("r: &impl Measure"));
    assert!(out.contains("other: &Rect"));
}
