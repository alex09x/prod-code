/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::common::split_top;
use super::polyglot::{extract_param_names_py, owner_region, restructure_cpp};
use super::rust::{argument_names, impl_blocks, parse_struct, restructure, rewrite_literals};
use crate::parameter_object::Language;

const ACCOUNT: &str = "#[derive(Debug, Clone)]\npub struct Account {\n    pub owner: String,\n    pub street: String,\n    pub city: String,\n    balance: u64,\n}\n\nimpl Account {\n    pub fn new(owner: &str, street: &str, city: &str) -> Self {\n        Account {\n            owner: owner.into(),\n            street: street.into(),\n            city: city.into(),\n            balance: 0,\n        }\n    }\n\n    pub fn address(&self) -> String {\n        format!(\"{}, {}\", self.street, self.city)\n    }\n\n    pub fn moves_to(&mut self, city: &str) {\n        self.city = city.to_string();\n    }\n\n    pub fn deposit(&mut self, n: u64) {\n        self.balance += n;\n    }\n}\n";

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn python_delegate_forwarding_preserves_keyword_and_variadic_arguments() {
    assert_eq!(
        extract_param_names_py("self, required, *args, limit=10, flag: bool, **kwargs"),
        ["required", "*args", "limit=limit", "flag=flag", "**kwargs"]
    );
    assert_eq!(
        extract_param_names_py("self, *, limit=10, enabled: bool"),
        ["limit=limit", "enabled=enabled"]
    );
}

#[test]
fn a_struct_and_its_fields_are_read_with_their_attributes() {
    let d = parse_struct(ACCOUNT, ACCOUNT.find("pub struct").unwrap()).unwrap();
    assert_eq!(d.name, "Account");
    assert_eq!(d.derive.as_deref(), Some("#[derive(Debug, Clone)]"));
    let names: Vec<&str> = d.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["owner", "street", "city", "balance"]);
    assert_eq!(split_top("a: HashMap<K, V>, b: (u8, u8)", ',').len(), 2);
    assert_eq!(
        argument_names("pub fn f(&mut self, a: u32, mut b: Vec<(u8, u8)>) -> u8"),
        Some(strings(&["a", "b"]))
    );
    assert_eq!(argument_names("fn g(&self, (a, b): (u8, u8))"), None);
}

#[test]
fn the_fields_and_methods_move_behind_a_forwarding_method() {
    let at = ACCOUNT.find("pub struct").unwrap();
    let fields = strings(&["street", "city"]);
    let methods = strings(&["address", "moves_to"]);
    let out = restructure(ACCOUNT, at, &fields, &methods, "Address", "address").unwrap();
    for expected in [
        "pub struct Account {\n    pub owner: String,\n    pub address: Address,\n    balance: u64,\n}",
        "#[derive(Debug, Clone)]\npub struct Address {\n    pub street: String,\n    pub city: String,\n}",
        "impl Address {\n    pub fn address(&self) -> String {\n        format!(\"{}, {}\", self.street, self.city)\n    }",
        "    pub fn moves_to(&mut self, city: &str) {\n        self.address.moves_to(city)\n    }",
        "    pub fn deposit(&mut self, n: u64) {\n        self.balance += n;\n    }",
    ] {
        assert!(out.contains(expected), "{expected}\n---\n{out}");
    }
    let blocks = impl_blocks(&out, "Account");
    let ranges: Vec<(usize, usize)> = blocks.iter().map(|(s, _, e)| (*s, *e)).collect();
    let lit = rewrite_literals(&out, "Account", &ranges, &fields, "address", "Address").unwrap();
    assert!(
        lit.contains("            owner: owner.into(),\n            address: Address { street: street.into(), city: city.into() },\n            balance: 0,\n"),
        "{lit}"
    );

    // A moved method that uses a field that stays is refused, and so is an unknown name.
    let err = restructure(
        ACCOUNT,
        at,
        &fields,
        &strings(&["deposit"]),
        "Address",
        "address",
    )
    .unwrap_err();
    assert!(format!("{err}").contains("uses `self.balance`"), "{err}");
    let err = restructure(ACCOUNT, at, &strings(&["zip"]), &[], "Address", "address").unwrap_err();
    assert!(format!("{err}").contains("has no field `zip`"), "{err}");
    let err = rewrite_literals(
        "fn f() { let Account { city, .. } = a; }",
        "Account",
        &[],
        &fields,
        "address",
        "Address",
    )
    .unwrap_err();
    assert!(format!("{err}").contains("uses `..`"), "{err}");

    // A tuple or generic struct, and a method that does not borrow its receiver.
    for text in ["pub struct P(u8);\n", "pub struct G<T> {\n    t: T,\n}\n"] {
        let err = parse_struct(text, 0).unwrap_err();
        assert!(format!("{err}").contains("not a plain struct"), "{err}");
    }
    let by_value = ACCOUNT.replace(
        "    pub fn moves_to(&mut self, city: &str) {",
        "    pub fn moves_to(mut self, city: &str) {",
    );
    let err = restructure(&by_value, at, &fields, &methods, "Address", "address").unwrap_err();
    assert!(format!("{err}").contains("does not take `&self`"), "{err}");
}

#[test]
fn cpp_delegate_helper_is_inserted_after_existing_includes() {
    let source = "#include <string>\n\nclass Account {\npublic:\n    std::string city;\n};\n";
    let (rewritten, _) = restructure_cpp(
        source,
        Some("Account"),
        None,
        &strings(&["city"]),
        &[],
        "Location",
        "location",
    )
    .unwrap();
    let include = rewritten.find("#include <string>").unwrap();
    let helper = rewritten.find("class Location").unwrap();
    let owner = rewritten.find("class Account").unwrap();
    assert!(include < helper && helper < owner, "{rewritten}");
}

#[test]
fn cpp_restructure_removes_moved_method_from_owner_body() {
    let source = "#include <string>\n\nclass Account {\npublic:\n    std::string street;\n    std::string city;\n    int balance;\n    std::string address() {\n        return street + \", \" + city;\n    }\n    void deposit(int amt) { balance += amt; }\n};\n";
    let (rewritten, owner) = restructure_cpp(
        source,
        Some("Account"),
        None,
        &strings(&["street", "city"]),
        &strings(&["address"]),
        "Location",
        "location",
    )
    .unwrap();
    let owner_body = owner_region(&rewritten, Language::Cpp, &owner).unwrap();
    let helper_body = owner_region(&rewritten, Language::Cpp, "Location").unwrap();

    assert!(
        helper_body.contains("return street + \", \" + city;"),
        "{rewritten}"
    );
    assert!(
        owner_body.contains("return location.address();"),
        "{rewritten}"
    );
    assert!(
        !owner_body.contains("return street + \", \" + city;"),
        "{rewritten}"
    );
    assert!(owner_body.contains("deposit(int amt)"), "{rewritten}");
}
