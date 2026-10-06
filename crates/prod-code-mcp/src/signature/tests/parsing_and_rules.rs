/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::signature::modifiers::{with_return_type, with_visibility};
use crate::signature::parse::{
    param_span, parameter_at, parse_declared, parse_param, split_params,
};
use crate::signature::plan::{call_site_rule, format_list, plan};
use crate::signature::types::Param;

#[test]
fn a_parameter_is_found_with_its_function_and_the_ones_that_stay() {
    let t = "impl S {\n    pub fn join<T: Into<String>>(&self, a: T, mut b: &str, c: u8) {}\n}\n";
    let (fn_at, name, kept) = parameter_at(t, 2, 51).expect("`b` is a parameter");
    assert_eq!(&t[fn_at..fn_at + 4], "join");
    assert_eq!(name, "b");
    assert_eq!(kept, ["a", "c"]);
    // `a` is one too; the function's name and `self` are not.
    assert_eq!(parameter_at(t, 2, 41).map(|p| p.1).as_deref(), Some("a"));
    assert!(parameter_at(t, 2, 12).is_none());
    assert!(parameter_at(t, 2, 35).is_none());
    let call = "fn f(x: u8) {\n    g(x, 1);\n}\n";
    assert!(
        parameter_at(call, 2, 7).is_none(),
        "an argument is not a parameter"
    );
}

#[test]
fn a_return_type_is_replaced_added_and_removed() {
    let t = "pub fn total(xs: &[u32]) -> u32 {\n    0\n}\n";
    let close = t.find(')').unwrap();
    let (was, out) = with_return_type(t, close, "u64");
    assert_eq!(was, "u32");
    assert!(
        out.starts_with("pub fn total(xs: &[u32]) -> u64 {"),
        "{out}"
    );
    let (_, out) = with_return_type(t, close, "()");
    assert!(out.starts_with("pub fn total(xs: &[u32]) {"), "{out}");
    let none = "fn log(s: &str) {\n}\n";
    let (was, out) = with_return_type(none, none.find(')').unwrap(), "bool");
    assert_eq!(was, "()");
    assert!(out.starts_with("fn log(s: &str) -> bool {"), "{out}");
    let generic = "fn f<T>(x: T) -> Vec<T> where T: Clone {\n}\n";
    let (was, out) = with_return_type(generic, generic.find(')').unwrap(), "Option<T>");
    assert_eq!(was, "Vec<T>");
    assert!(out.contains("-> Option<T> where T: Clone"), "{out}");
}

#[test]
fn a_visibility_is_replaced_added_and_removed() {
    let t = "    pub async fn run() {}\n";
    let at = t.find("run").unwrap();
    let (was, out) = with_visibility(t, at, "pub(crate)").unwrap();
    assert_eq!(was, "pub");
    assert_eq!(out, "    pub(crate) async fn run() {}\n");
    let (was, out) = with_visibility(&out, out.find("run").unwrap(), "private").unwrap();
    assert_eq!(was, "pub(crate)");
    assert_eq!(out, "    async fn run() {}\n");
    let (was, out) = with_visibility(&out, out.find("run").unwrap(), "pub").unwrap();
    assert_eq!(was, "private");
    assert_eq!(out, "    pub async fn run() {}\n");
}

#[test]
fn parses_a_kept_parameter_and_a_new_one() {
    assert_eq!(parse_param(" root ").unwrap(), Param::Keep("root".into()));
    assert_eq!(
        parse_param("budget: usize = 0").unwrap(),
        Param::Add {
            name: "budget".into(),
            ty: "usize".into(),
            value: "0".into()
        }
    );
}

#[test]
fn a_new_parameter_keeps_commas_and_arrows_inside_its_type() {
    let p = parse_param("map: HashMap<String, Vec<u8>> = HashMap::new()").unwrap();
    assert_eq!(
        p,
        Param::Add {
            name: "map".into(),
            ty: "HashMap<String, Vec<u8>>".into(),
            value: "HashMap::new()".into()
        }
    );
    let f = parse_param("f: fn(u32) -> u32 = |x| x").unwrap();
    assert!(matches!(f, Param::Add { ref ty, .. } if ty == "fn(u32) -> u32"));
}

#[test]
fn a_new_parameter_without_an_expression_is_refused() {
    let err = parse_param("budget: usize").unwrap_err().to_string();
    assert!(err.contains("expression"), "{err}");
}

#[test]
fn finds_the_parameter_list_past_generics_and_lifetimes() {
    let text = "fn f<'a, T: Into<String>>(a: &'a str, b: T) -> u32 { 0 }\n";
    let (name, open, close) = param_span(text, 3).unwrap();
    assert_eq!(name, "f");
    assert_eq!(&text[open..close], "a: &'a str, b: T");
}

#[test]
fn splits_parameters_without_splitting_their_types() {
    let params = split_params("a: HashMap<String, u64>, b: (u32, u32), c: impl Fn() -> u32");
    assert_eq!(params.len(), 3);
    assert_eq!(params[2], "c: impl Fn() -> u32");
}

#[test]
fn a_receiver_is_kept_out_of_the_parameters() {
    let (recv, params) = parse_declared("&mut self, file: &Path, text: &str");
    assert_eq!(recv.as_deref(), Some("&mut self"));
    assert_eq!(
        params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        ["file", "text"]
    );
}

#[test]
fn a_reorder_becomes_a_rule_with_a_placeholder_per_argument() {
    let declared = parse_declared("remote: SocketAddr, root: &Path").1;
    let request = [Param::Keep("root".into()), Param::Keep("remote".into())];
    let plan = plan(&declared, &request).unwrap();
    assert!(plan.dropped.is_empty());
    assert_eq!(
        call_site_rule("validate", false, declared.len(), &plan.args, &[]),
        "validate($a0, $a1) ==>> validate($a1, $a0)"
    );
}

#[test]
fn an_added_parameter_is_spelled_out_at_the_call_sites() {
    let declared = parse_declared("path: &Path").1;
    let request = [
        Param::Keep("path".into()),
        Param::Add {
            name: "budget".into(),
            ty: "usize".into(),
            value: "4096".into(),
        },
    ];
    let plan = plan(&declared, &request).unwrap();
    assert_eq!(plan.list, ["path: &Path", "budget: usize"]);
    assert_eq!(
        call_site_rule("read", true, declared.len(), &plan.args, &["4096"]),
        "$recv.read($a0) ==>> $recv.read($a0, 4096)"
    );
}

#[test]
fn a_dropped_parameter_is_reported_by_name() {
    let declared = parse_declared("a: u32, b: u32").1;
    let plan = plan(&declared, &[Param::Keep("a".into())]).unwrap();
    assert_eq!(plan.dropped, ["b"]);
}

#[test]
fn an_unknown_parameter_names_the_ones_there_are() {
    let declared = parse_declared("a: u32, b: u32").1;
    let err = plan(&declared, &[Param::Keep("c".into())])
        .unwrap_err()
        .to_string();
    assert!(err.contains("a, b"), "{err}");
}

#[test]
fn a_multiline_list_stays_multiline() {
    let old = "\n    a: u32,\n    b: u32,\n";
    let out = format_list(old, None, &["b: u32".into(), "a: u32".into()]);
    assert_eq!(out, "\n    b: u32,\n    a: u32,\n");
}
