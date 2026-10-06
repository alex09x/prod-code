/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

use super::super::polyglot::{
    encapsulate_field_cpp, encapsulate_field_go, encapsulate_field_js, encapsulate_field_py,
    encapsulate_field_swift, encapsulate_field_ts,
};
use super::super::types::Language;

#[test]
fn test_encapsulate_field_ts() {
    let ts = r#"export class UserService {
    public username: string;
    public age: number;

    constructor(username: string, age: number) {
        this.username = username;
        this.age = age;
    }
}

export function testUser(svc: UserService) {
    svc.username = "alice";
    console.log(svc.username);
}
"#;
    let (owner, ty, res, reads, writes, left) =
        encapsulate_field_ts(ts, Some("UserService"), "username").unwrap();
    assert_eq!(owner, "UserService");
    assert_eq!(ty, "string");
    assert_eq!(reads, 1);
    assert_eq!(writes, 1);
    assert_eq!(left, 1);
    assert!(res.contains("private _username: string;"));
    assert!(res.contains("public getUsername(): string {"));
    assert!(res.contains("return this._username;"));
    assert!(res.contains("public setUsername(username: string): void {"));
    assert!(res.contains("this._username = username;"));
    assert!(res.contains("svc.setUsername(\"alice\");"));
    assert!(res.contains("console.log(svc.getUsername());"));
}

#[test]
fn test_encapsulate_field_js_generates_valid_javascript_accessors() {
    assert_eq!(
        Language::from_path(Path::new("user.js")),
        Some(Language::JavaScript)
    );
    assert_eq!(
        Language::from_path(Path::new("user.jsx")),
        Some(Language::JavaScript)
    );
    let js = r#"export class User {
    username = "";

    constructor(username) {
        this.username = username;
    }

    display() {
        return this.username;
    }
}

export function testUser(svc) {
    svc.username = "alice";
    console.log(svc.username);
}
"#;
    let (owner, ty, res, reads, writes, internal) =
        encapsulate_field_js(js, Some("User"), "username").unwrap();
    assert_eq!(owner, "User");
    assert!(ty.is_empty());
    assert_eq!(reads, 1);
    assert_eq!(writes, 1);
    assert_eq!(internal, 2);
    assert!(res.contains("#username = \"\";"));
    assert!(res.contains("getUsername() {"));
    assert!(res.contains("setUsername(username) {"));
    assert!(res.contains("this.#username = username;"));
    assert!(res.contains("svc.setUsername(\"alice\");"));
    assert!(res.contains("console.log(svc.getUsername());"));
    assert!(!res.contains("private _username"));
    assert!(!res.contains(": void"));
}

#[test]
fn test_encapsulate_field_python() {
    let py = r#"class Account:
    def __init__(self, balance: float):
        self.balance = balance

    def deposit(self, amount: float):
        self.balance += amount

def audit_account(acc: Account):
    acc.balance = 100.0
    print(acc.balance)
"#;
    let (owner, _ty, res, reads, writes, left) =
        encapsulate_field_py(py, Some("Account"), "balance").unwrap();
    assert_eq!(owner, "Account");
    assert_eq!(reads, 1);
    assert_eq!(writes, 1);
    assert_eq!(left, 2);
    assert!(res.contains("self._balance = balance"));
    assert!(res.contains("self._balance += amount"));
    assert!(res.contains("def get_balance(self)"));
    assert!(res.contains("return self._balance"));
    assert!(res.contains("def set_balance(self, balance"));
    assert!(res.contains("acc.set_balance(100.0)"));
    assert!(res.contains("print(acc.get_balance())"));
}

#[test]
fn test_encapsulate_field_cpp() {
    let cpp = r#"class User {
public:
    std::string name;
    int age;
};

void update_user(User* u) {
    u->name = "Alice";
    std::cout << u->name << std::endl;
}
"#;
    let (owner, ty, res, reads, writes, _left) =
        encapsulate_field_cpp(cpp, Some("User"), "name", None).unwrap();
    assert_eq!(owner, "User");
    assert_eq!(ty, "std::string");
    assert_eq!(reads, 1);
    assert_eq!(writes, 1);
    assert!(res.contains("const std::string& get_name() const"));
    assert!(res.contains("void set_name(const std::string& name)"));
    assert!(res.contains("std::string name_;"));
    assert!(res.contains("u->set_name(\"Alice\");"));
    assert!(res.contains("std::cout << u->get_name() << std::endl;"));
}

#[test]
fn test_encapsulate_field_swift() {
    let swift = r#"class User {
    var name: String
    var age: Int

    init(name: String, age: Int) {
        self.name = name
        self.age = age
    }
}

func checkUser(u: User) {
    u.name = "Alice"
    print(u.name)
}
"#;
    let (owner, ty, res, reads, writes, left) =
        encapsulate_field_swift(swift, Some("User"), "name").unwrap();
    assert_eq!(owner, "User");
    assert_eq!(ty, "String");
    assert_eq!(reads, 1);
    assert_eq!(writes, 1);
    assert_eq!(left, 1);
    assert!(res.contains("private var _name: String"));
    assert!(res.contains("func getName() -> String"));
    assert!(res.contains("func setName(_ name: String)"));
    assert!(res.contains("u.setName(\"Alice\")"));
    assert!(res.contains("print(u.getName())"));
}

#[test]
fn test_encapsulate_field_go() {
    let go = r#"package user

type User struct {
	Name string
	Age  int
}

func ProcessUser(u *User) {
	u.Name = "Alice"
	println(u.Name)
}
"#;
    let err = encapsulate_field_go(go, Some("User"), "Name").unwrap_err();
    assert!(
        err.to_string().contains("exported Go field `Name`"),
        "{err:#}"
    );
}
