/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! #1810: the `&name` form, its plan, the borrowed arguments and the declaration's refusals.

use crate::signature::borrow::{
    apply_insertions, borrow_argument, borrowed_indices, declaration_edits,
};
use crate::signature::parse::{parse_declared, parse_param};
use crate::signature::plan::plan;
use crate::signature::types::Param;

fn keep(name: &str) -> Param {
    Param::Keep(name.to_string())
}

fn borrow(name: &str) -> Param {
    Param::Borrow(name.to_string())
}

/// The inside of the parameter list of `fn name`, as the planner hands it on.
fn open_of(text: &str, name: &str) -> usize {
    let at = text.find(&format!("fn {name}")).expect(name);
    at + text[at..].find('(').expect("a parameter list") + 1
}

/// `text` with the parameters at `borrowed` of `fn name` borrowed, or the refusal.
fn borrowed(text: &str, name: &str, borrowed: &[usize]) -> Result<String, String> {
    declaration_edits(text, open_of(text, name), name, borrowed)
        .map(|edits| apply_insertions(text, &edits))
        .map_err(|e| format!("{e:#}"))
}

#[test]
fn issue_1810_parses_a_borrow_and_keeps_the_old_forms() {
    assert_eq!(parse_param("&request").unwrap(), borrow("request"));
    assert_eq!(parse_param(" & request ").unwrap(), borrow("request"));
    assert_eq!(parse_param("request").unwrap(), keep("request"));
    assert_eq!(
        parse_param("request: &Request = &request").unwrap(),
        Param::Add {
            name: "request".into(),
            ty: "&Request".into(),
            value: "&request".into(),
        },
        "the old adding form is still an addition, not a borrow"
    );
    for refused in [
        "&mut request",
        "&&request",
        "&",
        "&request: Request = r",
        "&a b",
    ] {
        assert!(parse_param(refused).is_err(), "{refused}");
    }
    let text = format!("{:#}", parse_param("&mut request").unwrap_err());
    assert!(text.contains("shared borrow"), "{text}");
}

#[test]
fn issue_1810_a_borrow_keeps_every_other_parameter_in_place() {
    let (_, declared) = parse_declared("request: Request, cancellation: &Cancellation");
    assert_eq!(
        borrowed_indices(&declared, &[borrow("request"), keep("cancellation")]).unwrap(),
        vec![0]
    );
    assert_eq!(
        borrowed_indices(&declared, &[borrow("request"), borrow("cancellation")]).unwrap(),
        vec![0, 1]
    );
    let refused = [
        vec![keep("cancellation"), borrow("request")],
        vec![borrow("request")],
        vec![
            borrow("request"),
            keep("cancellation"),
            Param::Add {
                name: "extra".into(),
                ty: "u8".into(),
                value: "0".into(),
            },
        ],
    ];
    for request in refused {
        let text = format!("{:#}", borrowed_indices(&declared, &request).unwrap_err());
        assert!(text.contains("declared order"), "{request:?}: {text}");
        assert!(text.contains("nothing was written"), "{text}");
    }
    let text = format!(
        "{:#}",
        borrowed_indices(&declared, &[borrow("req"), keep("cancellation")]).unwrap_err()
    );
    assert!(text.contains("no parameter named `req`"), "{text}");
}

/// The reported attempt, `request: &Request = &request` with `cancellation` kept, still adds a
/// parameter and drops the old one, so the used-parameter check refuses it as before; the plan
/// never reads a borrow into it.
#[test]
fn issue_1810_the_old_add_and_remove_plan_is_unchanged() {
    let (_, declared) = parse_declared("request: Request, cancellation: &Cancellation");
    let request = [
        parse_param("request: &Request = &request").unwrap(),
        parse_param("cancellation").unwrap(),
    ];
    let planned = plan(&declared, &request).unwrap();
    assert_eq!(
        planned.list,
        ["request: &Request", "cancellation: &Cancellation"]
    );
    assert_eq!(planned.args, [None, Some(1)]);
    assert_eq!(planned.dropped, ["request"]);
    let text = format!("{:#}", plan(&declared, &[borrow("request")]).unwrap_err());
    assert!(text.contains("planned on its own"), "{text}");
}

/// Each borrowed argument is its own text once, behind one `&`: parenthesized only where a
/// prefix `&` would bind to part of it, a trailing comment left outside the parentheses.
#[test]
fn issue_1810_a_borrowed_argument_is_written_once() {
    for (arg, expected) in [
        ("request", "&request"),
        ("make_request()", "&make_request()"),
        ("self.request.clone()", "&self.request.clone()"),
        ("requests[0]", "&requests[0]"),
        ("Request { id: 1 }", "&Request { id: 1 }"),
        ("*boxed", "&*boxed"),
        ("&request", "&&request"),
        ("load()?", "&load()?"),
        ("a + b", "&(a + b)"),
        ("x as u64", "&(x as u64)"),
        ("|| make()", "&(|| make())"),
        ("a..b", "&(a..b)"),
        (
            "if c { make(1) } else { make(2) }",
            "&(if c { make(1) } else { make(2) })",
        ),
        (
            "request // the one we built",
            "&request // the one we built",
        ),
        ("a + b /* sum */", "&(a + b) /* sum */"),
        ("$value", "&($value)"),
    ] {
        let out = borrow_argument(arg);
        assert_eq!(out, expected, "{arg}");
        let code = arg
            .split(" //")
            .next()
            .unwrap()
            .split(" /*")
            .next()
            .unwrap();
        assert_eq!(out.matches(code).count(), 1, "{arg} is written once");
    }
}

#[test]
fn issue_1810_the_declaration_borrows_owned_plain_parameters() {
    let free = "fn run(request: Request, cancellation: &Cancellation) -> u32 { request.id }\n";
    assert_eq!(
        borrowed(free, "run", &[0]).unwrap(),
        "fn run(request: &Request, cancellation: &Cancellation) -> u32 { request.id }\n"
    );
    let method = "impl Engine {\n    pub fn run(&self, request: Request, c: &C) -> &str {\n        \"x\"\n    }\n}\n";
    assert_eq!(
        borrowed(method, "run", &[0]).unwrap(),
        method.replace("request: Request", "request: &Request"),
        "a `&self` receiver decides the result's lifetime"
    );
    let bounds = "fn f(x: impl Read + Send, y: Vec<u8>) -> Box<dyn Fn(&str) -> usize> { todo!() }";
    assert_eq!(
        borrowed(bounds, "f", &[0, 1]).unwrap(),
        "fn f(x: &(impl Read + Send), y: &Vec<u8>) -> Box<dyn Fn(&str) -> usize> { todo!() }",
        "a `Fn(&str)` bound has its own elision scope"
    );
}

#[test]
fn issue_1810_unsupported_declarations_are_refused() {
    for (text, name, why) in [
        ("fn f(mut x: R) {}", "f", "mutable binding"),
        ("fn f(x: &R) {}", "f", "already a reference"),
        ("fn f(x: &mut R) {}", "f", "already a reference"),
        ("fn f((a, b): (u8, u8)) {}", "f", "pattern"),
        ("fn f(#[cfg(test)] x: R) {}", "f", "attribute"),
        ("trait T {\n    fn f(x: R);\n}\n", "f", "trait"),
        (
            "impl T for S {\n    fn f(x: R) {}\n}\n",
            "f",
            "implements a trait",
        ),
        ("extern \"C\" fn f(x: R) {}", "f", "ABI"),
        ("fn f(x: R, s: &str) -> &str { s }", "f", "lifetime elision"),
        (
            "fn f(x: R, y: Item<'_>) -> Item<'_> { y }",
            "f",
            "lifetime elision",
        ),
    ] {
        let text = borrowed(text, name, &[0]).expect_err(text);
        assert!(text.contains(why), "{why}: {text}");
        assert!(text.contains("Nothing was written"), "{text}");
    }
}
