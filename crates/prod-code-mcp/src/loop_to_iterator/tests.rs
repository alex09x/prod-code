/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::loop_to_iterator::helpers::{binding_type, is_zero};
use crate::loop_to_iterator::polyglot::{
    recognise_cpp, recognise_go, recognise_python, recognise_swift, recognise_ts,
};
use crate::loop_to_iterator::rust::{chain, iterator_of, recognise};
use crate::loop_to_iterator::types::AccumulatorLoop;

const LOOPS: &str = "pub fn total(prices: &[u64]) -> u64 {\n    let mut sum = 0;\n    for p in prices {\n        sum += p * 2;\n    }\n    sum\n}\n\npub fn evens(xs: &[i32]) -> usize {\n    let mut n = 0;\n    for x in xs {\n        if x % 2 == 0 {\n            n += 1;\n        }\n    }\n    n\n}\n\npub fn names(users: &[(u32, String)]) -> Vec<String> {\n    let mut out = Vec::new();\n    for (id, name) in users {\n        if *id > 10 {\n            out.push(name.clone());\n        }\n    }\n    out\n}\n";

fn at(line: &str) -> AccumulatorLoop {
    recognise(LOOPS, LOOPS.find(line).unwrap()).unwrap()
}

#[test]
fn a_sum_a_count_and_a_collect_are_recognised() {
    let sum = at("for p in prices");
    assert_eq!(
        chain(&sum, "u64", Some("&[u64]"), false),
        "    let sum: u64 = prices.iter().map(|p| p * 2).sum();"
    );
    assert_eq!(
        &LOOPS[sum.start..sum.end],
        "    let mut sum = 0;\n    for p in prices {\n        sum += p * 2;\n    }"
    );
    let count = at("for x in xs");
    assert_eq!(
        chain(&count, "usize", None, false),
        "    let n: usize = xs.into_iter().filter(|&x| x % 2 == 0).count();"
    );
    let names = at("for (id, name)");
    assert_eq!(
        chain(&names, "Vec<_>", None, true),
        "    let mut out: Vec<_> = users.into_iter().filter_map(|(id, name)| if *id > 10 { Some(name.clone()) } else { None }).collect();"
    );
}

#[test]
fn rust_general_loop_conversion_find_any_all() {
    let find_src = "pub fn find_user(users: &[u32]) -> Option<&u32> {\n    let mut found = None;\n    for u in users {\n        if *u > 10 {\n            found = Some(u);\n            break;\n        }\n    }\n    found\n}\n";
    let l = recognise(find_src, find_src.find("for u in").unwrap()).unwrap();
    assert_eq!(
        chain(&l, "Option<_>", None, false),
        "    let found: Option<_> = users.into_iter().find(|&u| *u > 10);"
    );

    let find_map_src = "pub fn find_user(users: &[u32]) -> Option<u32> {\n    let mut found = None;\n    for u in users {\n        if *u > 10 {\n            found = Some(*u);\n            break;\n        }\n    }\n    found\n}\n";
    let l2 = recognise(find_map_src, find_map_src.find("for u in").unwrap()).unwrap();
    assert_eq!(
        chain(&l2, "Option<_>", None, false),
        "    let found: Option<_> = users.into_iter().find_map(|u| if *u > 10 { Some(*u) } else { None });"
    );

    let any_src = "pub fn has_admin(users: &[bool]) -> bool {\n    let mut has_any = false;\n    for u in users {\n        if *u {\n            has_any = true;\n            break;\n        }\n    }\n    has_any\n}\n";
    let l = recognise(any_src, any_src.find("for u in").unwrap()).unwrap();
    assert_eq!(
        chain(&l, "bool", None, false),
        "    let has_any: bool = users.into_iter().any(|&u| *u);"
    );

    let all_src = "pub fn all_active(users: &[bool]) -> bool {\n    let mut all_match = true;\n    for u in users {\n        if !*u {\n            all_match = false;\n            break;\n        }\n    }\n    all_match\n}\n";
    let l = recognise(all_src, all_src.find("for u in").unwrap()).unwrap();
    assert_eq!(
        chain(&l, "bool", None, false),
        "    let all_match: bool = users.into_iter().all(|&u| !(!*u));"
    );
}

#[test]
fn a_loop_that_does_more_than_accumulate_is_refused() {
    for (from, to, anchor, why) in [
        (
            "        sum += p * 2;\n",
            "        sum += p * 2;\n        if sum > 9 {\n            break;\n        }\n",
            "for p in",
            "`break`",
        ),
        (
            "        sum += p * 2;\n",
            "        sum += p * sum;\n",
            "for p in",
            "more than the one",
        ),
        (
            "    let mut sum = 0;",
            "    let mut sum = 5;",
            "for p in",
            "not zero",
        ),
        (
            "    let mut out = Vec::new();",
            "    let mut out = vec![1];",
            "for (id",
            "does not start empty",
        ),
        (
            "        sum += p * 2;\n",
            "        sum -= p;\n",
            "for p in",
            "is not `sum += …;`",
        ),
    ] {
        let text = LOOPS.replace(from, to);
        let err = recognise(&text, text.find(anchor).unwrap()).unwrap_err();
        assert!(format!("{err:#}").contains(why), "{why}: {err:#}");
    }
}

#[test]
fn the_source_is_iterated_as_the_loop_did() {
    assert_eq!(iterator_of("prices", None), "prices.into_iter()");
    assert_eq!(
        iterator_of("prices", Some("Vec<u64>")),
        "prices.into_iter()"
    );
    assert_eq!(iterator_of("prices", Some("&[u64]")), "prices.iter()");
    assert_eq!(iterator_of("xs", Some("&mut Vec<u8>")), "xs.iter_mut()");
    assert_eq!(iterator_of("&v", None), "v.iter()");
    assert_eq!(
        iterator_of("&mut self.items", None),
        "self.items.iter_mut()"
    );
    assert_eq!(iterator_of("0..n", None), "(0..n)");
    assert_eq!(iterator_of("m.values()", None), "m.values().into_iter()");
    assert_eq!(
        binding_type("```rust\nprices: &[u64]\n```"),
        Some("&[u64]".into())
    );
    assert_eq!(
        binding_type("```rust\nlet mut sum: u64\n```\n---\nno Drop"),
        Some("u64".into())
    );
    assert_eq!(binding_type("```rust\nfn f()\n```"), None);
    assert!(is_zero("0") && is_zero("0.0") && is_zero("0u64") && is_zero("0_i32"));
    assert!(!is_zero("1") && !is_zero("x") && !is_zero("0x10"));
}

#[test]
fn polyglot_ts_recognised() {
    let src = "function total(prices: number[]): number {\n    let sum = 0;\n    for (const p of prices) {\n        sum += p * 2;\n    }\n    return sum;\n}";
    let poly = recognise_ts(src, src.find("for ").unwrap()).unwrap();
    assert_eq!(
        poly.replacement,
        "    const sum = prices.reduce((acc, p) => acc + (p * 2), 0);"
    );

    let cnt_src = "function evens(xs: number[]): number {\n    let count = 0;\n    for (const x of xs) {\n        if (x % 2 === 0) {\n            count += 1;\n        }\n    }\n    return count;\n}";
    let poly = recognise_ts(cnt_src, cnt_src.find("for ").unwrap()).unwrap();
    assert_eq!(
        poly.replacement,
        "    const count = xs.filter(x => x % 2 === 0).length;"
    );

    let find_src = "function findItem(items: string[]): string | null {\n    let found = null;\n    for (const item of items) {\n        if (item.length > 3) {\n            found = item;\n            break;\n        }\n    }\n    return found;\n}";
    let poly = recognise_ts(find_src, find_src.find("for ").unwrap()).unwrap();
    assert_eq!(
        poly.replacement,
        "    const found = items.find(item => item.length > 3) ?? null;"
    );

    let projected_find_src = "function findPrice(prices: number[]): number | null {\n    let found = null;\n    for (const p of prices) {\n        if (p > 0) {\n            found = p * 2;\n            break;\n        }\n    }\n    return found;\n}";
    let poly = recognise_ts(projected_find_src, projected_find_src.find("for ").unwrap()).unwrap();
    assert!(poly.replacement.contains("prices.find(p =>"));
    assert!(!poly.replacement.contains(".filter("));
}

#[test]
fn polyglot_python_recognised() {
    let src = "def total(prices):\n    total = 0\n    for p in prices:\n        total += p * 2\n    return total";
    let poly = recognise_python(src, src.find("for ").unwrap()).unwrap();
    assert_eq!(poly.replacement, "    total = sum(p * 2 for p in prices)");

    let cnt_src = "def evens(xs):\n    count = 0\n    for x in xs:\n        if x % 2 == 0:\n            count += 1\n    return count";
    let poly = recognise_python(cnt_src, cnt_src.find("for ").unwrap()).unwrap();
    assert_eq!(
        poly.replacement,
        "    count = sum(1 for x in xs if x % 2 == 0)"
    );

    let collect_src = "def names(users):\n    out = []\n    for u in users:\n        if u.age > 10:\n            out.append(u.name)\n    return out";
    let poly = recognise_python(collect_src, collect_src.find("for ").unwrap()).unwrap();
    assert_eq!(
        poly.replacement,
        "    out = [u.name for u in users if u.age > 10]"
    );
}

#[test]
fn polyglot_swift_recognised() {
    let src = "func total(prices: [Int]) -> Int {\n    var sum = 0\n    for p in prices {\n        sum += p * 2\n    }\n    return sum\n}";
    let poly = recognise_swift(src, src.find("for ").unwrap()).unwrap();
    assert_eq!(
        poly.replacement,
        "    let sum = prices.reduce(0) { _acc, p in _acc + (p * 2) }"
    );

    let cnt_src = "func evens(xs: [Int]) -> Int {\n    var count = 0\n    for x in xs {\n        if x % 2 == 0 {\n            count += 1\n        }\n    }\n    return count\n}";
    let poly = recognise_swift(cnt_src, cnt_src.find("for ").unwrap()).unwrap();
    assert_eq!(
        poly.replacement,
        "    let count = xs.filter { x in x % 2 == 0 }.count"
    );
}

#[test]
fn swift_conversions_bind_the_loop_element_in_each_closure() {
    let collect = "func doubled(prices: [Int]) -> [Int] {\n    var result: [Int] = []\n    for p in prices {\n        result.append(p * 2)\n    }\n    return result\n}";
    let poly = recognise_swift(collect, collect.find("for ").unwrap()).unwrap();
    assert!(poly.replacement.contains("map { p in p * 2 }"));

    let find = "func findPrice(prices: [Int]) -> Int? {\n    var found: Int? = nil\n    for p in prices {\n        if p > 0 {\n            found = p * 2\n            break;\n        }\n    }\n    return found\n}";
    let poly = recognise_swift(find, find.find("for ").unwrap()).unwrap();
    assert!(
        poly.replacement
            .contains("first(where: { p in p > 0 }).map { p in p * 2 }")
    );

    let any = "func hasPrice(prices: [Int]) -> Bool {\n    var found = false\n    for p in prices {\n        if p > 0 {\n            found = true\n            break;\n        }\n    }\n    return found\n}";
    let poly = recognise_swift(any, any.find("for ").unwrap()).unwrap();
    assert!(poly.replacement.contains("contains(where: { p in p > 0 })"));

    let all = "func allPrices(prices: [Int]) -> Bool {\n    var valid = true\n    for p in prices {\n        if p <= 0 {\n            valid = false\n            break;\n        }\n    }\n    return valid\n}";
    let poly = recognise_swift(all, all.find("for ").unwrap()).unwrap();
    assert!(poly.replacement.contains("allSatisfy { p in !(p <= 0) }"));
}

#[test]
fn polyglot_cpp_recognised() {
    let src = "int total(const std::vector<int>& prices) {\n    int sum = 0;\n    for (const auto& p : prices) {\n        sum += p * 2;\n    }\n    return sum;\n}";
    let poly = recognise_cpp(src, src.find("for ").unwrap()).unwrap();
    assert_eq!(
        poly.replacement,
        "    auto&& __prod_code_range = (prices);\n    int sum = std::accumulate(__prod_code_range.begin(), __prod_code_range.end(), static_cast<int>(0), [](auto _acc, const auto& p) { return _acc + (p * 2); });"
    );

    let any_src = "bool has_even(const std::vector<int>& xs) {\n    bool has_any = false;\n    for (const auto& x : xs) {\n        if (x % 2 == 0) {\n            has_any = true;\n            break;\n        }\n    }\n    return has_any;\n}";
    let poly = recognise_cpp(any_src, any_src.find("for ").unwrap()).unwrap();
    assert_eq!(
        poly.replacement,
        "    auto&& __prod_code_range = (xs);\n    const bool has_any = std::any_of(__prod_code_range.begin(), __prod_code_range.end(), [](const auto& x) { return x % 2 == 0; });"
    );
}

#[test]
fn polyglot_go_recognised() {
    let src = "func total(prices []int) int {\n    sum := 0\n    for _, p := range prices {\n        sum += p * 2\n    }\n    return sum\n}";
    let poly = recognise_go(src, src.find("for ").unwrap()).unwrap();
    assert_eq!(
        poly.replacement,
        "    sum := func() int { s := 0; for _, p := range prices { s += p * 2 }; return s }()"
    );
}

#[test]
fn cpp_conversion_evaluates_range_once_and_preserves_accumulator_type() {
    let src =
        "long long total = 0;\nfor (const auto& value : make_values()) {\n    total += value;\n}";
    let poly = recognise_cpp(src, src.find("for ").unwrap()).unwrap();

    assert_eq!(poly.replacement.matches("make_values()").count(), 1);
    assert!(poly.replacement.contains("static_cast<long long>(0)"));
}

#[test]
fn go_single_range_variable_remains_the_index() {
    let src = "func total(values []int) int {\n    sum := 0\n    for i := range values {\n        sum += i\n    }\n    return sum\n}";
    let poly = recognise_go(src, src.find("for ").unwrap()).unwrap();

    assert!(poly.replacement.contains("for i := range values"));
    assert!(!poly.replacement.contains("for _, i := range values"));
}
