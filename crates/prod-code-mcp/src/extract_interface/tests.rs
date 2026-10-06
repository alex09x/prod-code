/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::languages::*;

#[test]
fn test_extract_interface_ts() {
    let ts = r#"export class UserService {
    public async getUser(id: string): Promise<User> {
        return fetchUser(id);
    }

    public deleteUser(id: string): boolean {
        return true;
    }
}
"#;
    let (res, methods) = extract_interface_ts(ts, "UserService", "IUserService", &[]).unwrap();
    assert_eq!(methods, vec!["getUser", "deleteUser"]);
    assert!(res.contains("export interface IUserService {"));
    assert!(res.contains("getUser(id: string): Promise<User>;"));
    assert!(res.contains("deleteUser(id: string): boolean;"));
    assert!(res.contains("export class UserService implements IUserService {"));
}

#[test]
fn test_extract_interface_go() {
    let go = r#"package user

type UserService struct {
    db DB
}

func (s *UserService) GetUser(id string) (*User, error) {
    return nil, nil
}

func (s *UserService) DeleteUser(id string) error {
    return nil
}
"#;
    let (res, methods) =
        extract_interface_go(go, "UserService", "UserReader", &["GetUser".to_string()]).unwrap();
    assert_eq!(methods, vec!["GetUser"]);
    assert!(res.contains("type UserReader interface {"));
    assert!(res.contains("GetUser(id string) (*User, error)"));
    let iface = &res[..res.find("type UserService").unwrap()];
    assert!(!iface.contains("DeleteUser"));
    assert!(res.contains("type UserService struct {"));
}

#[test]
fn test_extract_interface_python() {
    let py = r#"class AccountService:
    def deposit(self, amount: float) -> bool:
        return True

    def withdraw(self, amount: float) -> bool:
        return True
"#;
    let (res, methods) =
        extract_interface_python(py, "AccountService", "AccountProtocol", &[]).unwrap();
    assert_eq!(methods, vec!["deposit", "withdraw"]);
    assert!(res.contains("from typing import Protocol"));
    assert!(res.contains("class AccountProtocol(Protocol):"));
    assert!(res.contains("def deposit(self, amount: float) -> bool:\n        ..."));
    assert!(res.contains("class AccountService(AccountProtocol):"));
}

#[test]
fn test_extract_interface_cpp() {
    let cpp = r#"class Shape {
public:
    double area() const {
        return 0.0;
    }

    double perimeter() const {
        return 0.0;
    }
};
"#;
    let (res, methods) = extract_interface_cpp(cpp, "Shape", "IShape", &[]).unwrap();
    assert_eq!(methods, vec!["area", "perimeter"]);
    assert!(res.contains("class IShape {"));
    assert!(res.contains("virtual double area() const = 0;"));
    assert!(res.contains("class Shape : public IShape {"));
}

#[test]
fn test_extract_interface_swift() {
    let swift = r#"struct PaymentService {
    func pay(amount: Double) -> Bool {
        return true
    }

    func refund(transactionId: String) -> Bool {
        return true
    }
}
"#;
    let (res, methods) =
        extract_interface_swift(swift, "PaymentService", "PaymentProtocol", &[]).unwrap();
    assert_eq!(methods, vec!["pay", "refund"]);
    assert!(res.contains("protocol PaymentProtocol {"));
    assert!(res.contains("func pay(amount: Double) -> Bool"));
    assert!(res.contains("struct PaymentService: PaymentProtocol {"));
}
