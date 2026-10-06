/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::loop_to_iterator::loop_to_iterator_polyglot;
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
async fn test_loop_to_iterator_typescript() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.ts",
            r#"export function total(prices: number[]): number {
    let sum = 0;
    for (const p of prices) {
        sum += p * 2;
    }
    return sum;
}

export function evens(numbers: number[]): number {
    let count = 0;
    for (const n of numbers) {
        if (n % 2 === 0) {
            count += 1;
        }
    }
    return count;
}

export function adults(users: { age: number; name: string }[]): string[] {
    const out = [];
    for (const u of users) {
        if (u.age >= 18) {
            out.push(u.name);
        }
    }
    return out;
}

export function findAdmin(users: { isAdmin: boolean; name: string }[]): any {
    let found = null;
    for (const u of users) {
        if (u.isAdmin) {
            found = u.name;
            break;
        }
    }
    return found;
}

export function hasAdmin(users: { isAdmin: boolean }[]): boolean {
    let hasAny = false;
    for (const u of users) {
        if (u.isAdmin) {
            hasAny = true;
            break;
        }
    }
    return hasAny;
}

export function allActive(users: { isActive: boolean }[]): boolean {
    let allValid = true;
    for (const u of users) {
        if (!u.isActive) {
            allValid = false;
            break;
        }
    }
    return allValid;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("calc.ts");
    let gw = fake_gateway().await;

    // 1. Sum
    let res = loop_to_iterator_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("total"),
        None,
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("const sum = prices.reduce((acc, p) => acc + (p * 2), 0);"));

    // 2. Count
    let res = loop_to_iterator_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("evens"),
        None,
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("const count = numbers.filter(n => n % 2 === 0).length;"));

    // 3. Collect
    let res = loop_to_iterator_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("adults"),
        None,
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("const out = users.filter(u => u.age >= 18).map(u => u.name);"));

    // 4. Find
    let res = loop_to_iterator_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("findAdmin"),
        None,
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("const found = (() => { let matched = false; const found = users.find(u => { const yes = u.isAdmin; if (yes) matched = true; return yes; }); return matched ? [found].map(u => u.name)[0] ?? null : null; })();"));

    // 5. Any
    let res = loop_to_iterator_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("hasAdmin"),
        None,
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("const hasAny = users.some(u => u.isAdmin);"));

    // 6. All
    let res = loop_to_iterator_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("allActive"),
        None,
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("const allValid = users.every(u => u.isActive);"));
}

#[tokio::test]
async fn test_loop_to_iterator_python() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.py",
            r#"def total(prices):
    total = 0
    for p in prices:
        total += p * 2
    return total

def evens(numbers):
    count = 0
    for n in numbers:
        if n % 2 == 0:
            count += 1
    return count

def adults(users):
    out = []
    for u in users:
        if u.age >= 18:
            out.append(u.name)
    return out

def find_admin(users):
    found = None
    for u in users:
        if u.is_admin:
            found = u.name
            break
    return found

def has_admin(users):
    has_any = False
    for u in users:
        if u.is_admin:
            has_any = True
            break
    return has_any

def all_active(users):
    all_match = True
    for u in users:
        if not u.is_active:
            all_match = False
            break
    return all_match
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("calc.py");
    let gw = fake_gateway().await;

    // 1. Sum
    let res = loop_to_iterator_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("total"),
        None,
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("total = sum(p * 2 for p in prices)"));

    // 2. Count
    let res = loop_to_iterator_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("evens"),
        None,
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("count = sum(1 for n in numbers if n % 2 == 0)"));

    // 3. Collect
    let res = loop_to_iterator_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("adults"),
        None,
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("out = [u.name for u in users if u.age >= 18]"));

    // 4. Find
    let res = loop_to_iterator_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("find_admin"),
        None,
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("found = next((u.name for u in users if u.is_admin), None)"));

    // 5. Any
    let res = loop_to_iterator_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("has_admin"),
        None,
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("has_any = any(u.is_admin for u in users)"));

    // 6. All
    let res = loop_to_iterator_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("all_active"),
        None,
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("all_match = all(u.is_active for u in users)"));
}

#[tokio::test]
async fn test_loop_to_iterator_swift() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.swift",
            r#"func total(prices: [Int]) -> Int {
    var sum = 0
    for p in prices {
        sum += p * 2
    }
    return sum
}

func evens(numbers: [Int]) -> Int {
    var count = 0
    for n in numbers {
        if n % 2 == 0 {
            count += 1
        }
    }
    return count
}

func adults(users: [User]) -> [String] {
    var out: [String] = []
    for u in users {
        if u.age >= 18 {
            out.append(u.name)
        }
    }
    return out
}

func findAdmin(users: [User]) -> String? {
    var found: String? = nil
    for u in users {
        if u.isAdmin {
            found = u.name
            break
        }
    }
    return found
}

func hasAdmin(users: [User]) -> Bool {
    var hasAny = false
    for u in users {
        if u.isAdmin {
            hasAny = true
            break
        }
    }
    return hasAny
}

func allActive(users: [User]) -> Bool {
    var allValid = true
    for u in users {
        if !u.isActive {
            allValid = false
            break
        }
    }
    return allValid
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("calc.swift");
    let gw = fake_gateway().await;

    // 1. Sum
    let res = loop_to_iterator_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("total"),
        None,
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("let sum = prices.reduce(0) { _acc, p in _acc + (p * 2) }"));

    // 2. Count
    let res = loop_to_iterator_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("evens"),
        None,
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("let count = numbers.filter { n in n % 2 == 0 }.count"));

    // 3. Collect
    let res = loop_to_iterator_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("adults"),
        None,
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("let out = users.filter { u in u.age >= 18 }.map { u in u.name }"));

    // 4. Find
    let res = loop_to_iterator_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("findAdmin"),
        None,
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(
        content.contains("let found = users.first(where: { u in u.isAdmin }).map { u in u.name }")
    );

    // 5. Any
    let res = loop_to_iterator_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("hasAdmin"),
        None,
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("let hasAny = users.contains(where: { u in u.isAdmin })"));

    // 6. All
    let res = loop_to_iterator_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("allActive"),
        None,
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("let allValid = users.allSatisfy { u in u.isActive }"));
}

#[tokio::test]
async fn test_loop_to_iterator_cpp() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.cpp",
            r#"int total(const std::vector<int>& prices) {
    int sum = 0;
    for (const auto& p : prices) {
        sum += p * 2;
    }
    return sum;
}

int evens(const std::vector<int>& numbers) {
    int count = 0;
    for (const auto& n : numbers) {
        if (n % 2 == 0) {
            count += 1;
        }
    }
    return count;
}

bool has_admin(const std::vector<User>& users) {
    bool has_any = false;
    for (const auto& u : users) {
        if (u.is_admin) {
            has_any = true;
            break;
        }
    }
    return has_any;
}

bool all_active(const std::vector<User>& users) {
    bool all_valid = true;
    for (const auto& u : users) {
        if (!u.is_active) {
            all_valid = false;
            break;
        }
    }
    return all_valid;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("calc.cpp");
    let gw = fake_gateway().await;

    // 1. Sum
    let res = loop_to_iterator_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("total"),
        None,
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("auto&& __prod_code_range = (prices);"));
    assert!(content.contains("int sum = std::accumulate(__prod_code_range.begin(), __prod_code_range.end(), static_cast<int>(0), [](auto _acc, const auto& p) { return _acc + (p * 2); });"));

    // 2. Count
    let res = loop_to_iterator_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("evens"),
        None,
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("const auto count = std::count_if(__prod_code_range_1.begin(), __prod_code_range_1.end(), [](const auto& n) { return n % 2 == 0; });"));

    // 3. Any
    let res = loop_to_iterator_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("has_admin"),
        None,
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("const bool has_any = std::any_of(__prod_code_range_2.begin(), __prod_code_range_2.end(), [](const auto& u) { return u.is_admin; });"));

    // 4. All
    let res = loop_to_iterator_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("all_active"),
        None,
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("const bool all_valid = std::all_of(__prod_code_range_3.begin(), __prod_code_range_3.end(), [](const auto& u) { return u.is_active; });"));
}

#[tokio::test]
async fn test_loop_to_iterator_go() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.go",
            r#"package main

func total(prices []int) int {
    sum := 0
    for _, p := range prices {
        sum += p * 2
    }
    return sum
}

func evens(numbers []int) int {
    count := 0
    for _, n := range numbers {
        if n % 2 == 0 {
            count += 1
        }
    }
    return count
}

func hasAdmin(users []User) bool {
    hasAny := false
    for _, u := range users {
        if u.IsAdmin {
            hasAny = true
            break
        }
    }
    return hasAny
}

func allActive(users []User) bool {
    allValid := true
    for _, u := range users {
        if !u.IsActive {
            allValid = false
            break
        }
    }
    return allValid
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("calc.go");
    let gw = fake_gateway().await;

    // 1. Sum
    let res = loop_to_iterator_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("total"),
        None,
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains(
        "sum := func() int { s := 0; for _, p := range prices { s += p * 2 }; return s }()"
    ));

    // 2. Count
    let res = loop_to_iterator_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("evens"),
        None,
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("count := func() int { c := 0; for _, n := range numbers { if n % 2 == 0 { c++ } }; return c }()"));

    // 3. Any
    let res = loop_to_iterator_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("hasAdmin"),
        None,
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("hasAny := func() bool { for _, u := range users { if u.IsAdmin { return true } }; return false }()"));

    // 4. All
    let res = loop_to_iterator_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("allActive"),
        None,
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("allValid := func() bool { for _, u := range users { if !(u.IsActive) { return false } }; return true }()"));
}

#[tokio::test]
async fn test_loop_to_iterator_safety_refusals() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "refusals.ts",
            r#"function withContinue(xs: number[]): number {
    let sum = 0;
    for (const x of xs) {
        if (x < 0) continue;
        sum += x;
    }
    return sum;
}

function withReturn(xs: number[]): number {
    let sum = 0;
    for (const x of xs) {
        if (x === 0) return 0;
        sum += x;
    }
    return sum;
}

function nonZero(xs: number[]): number {
    let sum = 10;
    for (const x of xs) {
        sum += x;
    }
    return sum;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("refusals.ts");
    let gw = fake_gateway().await;

    // Continue refusal
    let err = loop_to_iterator_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("withContinue"),
        None,
        None,
        false,
        false,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("continue"));

    // Return refusal
    let err = loop_to_iterator_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("withReturn"),
        None,
        None,
        false,
        false,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("return"));

    // Non-zero refusal
    let err = loop_to_iterator_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("nonZero"),
        None,
        None,
        false,
        false,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("not zero"));
}
