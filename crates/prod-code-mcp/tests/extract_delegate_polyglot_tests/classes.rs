/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{semantic::*, *};

#[tokio::test]
async fn test_extract_delegate_typescript_multi_file() {
    let owner = "export class Account {\n    street: string;\n    city: string;\n    balance: number;\n\n    address(): string {\n        return `${this.street}, ${this.city}`;\n    }\n\n    deposit(amount: number): void {\n        this.balance += amount;\n    }\n}\n";
    let caller = "import { Account } from './account';\nexport function printDetails(acc: Account) {\n    console.log(acc.street);\n    console.log(acc.city);\n    console.log(acc.address());\n}\n";
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        ("account.ts", owner),
        ("client.ts", caller),
    ]);
    let file = ws.path("account.ts");
    let client = ws.path("client.ts");
    let fields = ["street", "city"]
        .iter()
        .map(|name| {
            let mut refs = references(&file, owner, name, &[&format!("this.{name}")]);
            refs.extend(references(&client, caller, name, &[&format!("acc.{name}")]));
            field(&file, owner, name, &format!("{name}: string;"), refs)
        })
        .collect();
    let gw = semantic_gateway(&file, "Account", fields).await;
    let res = extract_delegate_polyglot(
        gw.addr(),
        &ws.root(),
        &file,
        Some("Account"),
        None,
        None,
        &["street".into(), "city".into()],
        &["address".into()],
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
    assert_eq!(res.accesses, 2);
    let owner = fs::read_to_string(file).unwrap();
    for expected in [
        "addressInfo: AddressInfo = new AddressInfo(undefined as any, undefined as any);",
        "class AddressInfo {",
        "street: string;",
        "city: string;",
        "return this.addressInfo.address();",
        "deposit(amount: number)",
    ] {
        assert!(owner.contains(expected), "{expected}: {owner}");
    }
    let caller = fs::read_to_string(client).unwrap();
    for expected in [
        "console.log(acc.addressInfo.street);",
        "console.log(acc.addressInfo.city);",
        "console.log(acc.address());",
    ] {
        assert!(caller.contains(expected), "{expected}: {caller}");
    }
}

#[tokio::test]
async fn test_extract_delegate_python_multi_file() {
    let owner = "class Person:\n    def __init__(self, street: str, city: str, age: int):\n        self.street = street\n        self.city = city\n        self.age = age\n\n    def address(self) -> str:\n        return f\"{self.street}, {self.city}\"\n\n    def birthday(self):\n        self.age += 1\n";
    let caller = "from person import Person\n\ndef run(p: Person):\n    print(p.street)\n    print(p.city)\n    print(p.address())\n";
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        ("person.py", owner),
        ("app.py", caller),
    ]);
    let file = ws.path("person.py");
    let client = ws.path("app.py");
    let fields = ["street", "city"]
        .iter()
        .map(|name| {
            let mut refs = references(&file, owner, name, &[&format!("{{self.{name}}}")]);
            refs.extend(references(&client, caller, name, &[&format!("p.{name}")]));
            field(&file, owner, name, &format!("self.{name} ="), refs)
        })
        .collect();
    let gw = semantic_gateway(&file, "Person", fields).await;
    let res = extract_delegate_polyglot(
        gw.addr(),
        &ws.root(),
        &file,
        Some("Person"),
        None,
        None,
        &["street".into(), "city".into()],
        &["address".into()],
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
    let owner = fs::read_to_string(file).unwrap();
    for expected in [
        "self.addr = Address(street, city)",
        "class Address:",
        "return self.addr.address()",
    ] {
        assert!(owner.contains(expected), "{expected}: {owner}");
    }
    let caller = fs::read_to_string(client).unwrap();
    for expected in [
        "print(p.addr.street)",
        "print(p.addr.city)",
        "print(p.address())",
    ] {
        assert!(caller.contains(expected), "{expected}: {caller}");
    }
}

#[tokio::test]
async fn test_extract_delegate_cpp_multi_file() {
    let owner = "#include <string>\n\nclass Account {\npublic:\n    std::string street;\n    std::string city;\n    int balance;\n\n    std::string address() {\n        return street + \", \" + city;\n    }\n\n    void deposit(int amt) {\n        balance += amt;\n    }\n};\n";
    let caller = "#include \"account.hpp\"\n#include <iostream>\n\nvoid inspect(Account& acc, Account* ptr) {\n    std::cout << acc.street << std::endl;\n    std::cout << ptr->city << std::endl;\n}\n";
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        ("account.hpp", owner),
        ("client.cpp", caller),
    ]);
    let file = ws.path("account.hpp");
    let client = ws.path("client.cpp");
    let fields = [
        ("street", "return street", "acc.street"),
        ("city", "+ city;", "ptr->city"),
    ]
    .iter()
    .map(|(name, moved, external)| {
        let mut refs = references(&file, owner, name, &[moved]);
        refs.extend(references(&client, caller, name, &[external]));
        field(&file, owner, name, &format!("std::string {name};"), refs)
    })
    .collect();
    let gw = semantic_gateway(&file, "Account", fields).await;
    let res = extract_delegate_polyglot(
        gw.addr(),
        &ws.root(),
        &file,
        Some("Account"),
        None,
        None,
        &["street".into(), "city".into()],
        &["address".into()],
        "Location",
        "loc",
        true,
        false,
        None,
    )
    .await
    .unwrap();
    assert_eq!(res.helper, "Location");
    assert_eq!(res.field, "loc");
    assert!(res.applied);
    assert_eq!(res.accesses, 2);
    let owner = fs::read_to_string(file).unwrap();
    for expected in ["Location loc;", "class Location {", "return loc.address();"] {
        assert!(owner.contains(expected), "{expected}: {owner}");
    }
    let caller = fs::read_to_string(client).unwrap();
    assert!(caller.contains("std::cout << acc.loc.street << std::endl;"));
    assert!(caller.contains("std::cout << ptr->loc.city << std::endl;"));
}

#[tokio::test]
async fn test_extract_delegate_swift_multi_file() {
    let owner = "public class User {\n    var street: String\n    var city: String\n    var role: String\n\n    func address() -> String {\n        return \"\\(street), \\(city)\"\n    }\n}\n";
    let caller = "func handleUser(u: User) {\n    print(u.street)\n    print(u.city)\n}\n";
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        ("user.swift", owner),
        ("client.swift", caller),
    ]);
    let file = ws.path("user.swift");
    let client = ws.path("client.swift");
    let fields = ["street", "city"]
        .iter()
        .map(|name| {
            let mut refs = references(&file, owner, name, &[&format!("\\({name})")]);
            refs.extend(references(&client, caller, name, &[&format!("u.{name}")]));
            field(&file, owner, name, &format!("var {name}:"), refs)
        })
        .collect();
    let gw = semantic_gateway(&file, "User", fields).await;
    let res = extract_delegate_polyglot(
        gw.addr(),
        &ws.root(),
        &file,
        Some("User"),
        None,
        None,
        &["street".into(), "city".into()],
        &["address".into()],
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
    let owner = fs::read_to_string(file).unwrap();
    for expected in [
        "var addr: Address",
        "struct Address {",
        "return addr.address()",
        "return \"\\(street), \\(city)\"",
    ] {
        assert!(owner.contains(expected), "{expected}: {owner}");
    }
    let caller = fs::read_to_string(client).unwrap();
    assert!(caller.contains("print(u.addr.street)"));
    assert!(caller.contains("print(u.addr.city)"));
}
