/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::*;
use prod_code_testkit::{ScriptedGateway, answers};
use serde_json::Value;

const DISTINCT: &str = "#![allow(non_camel_case_types)] trait Trait { fn value(&self)->u8; } struct dynTrait; impl dynTrait { fn value(&self)->u8 {1} } impl Trait for dynTrait { fn value(&self)->u8 {2} } struct S<T:?Sized>{value:T} impl S<dyn Trait>{ fn f(c:&S<dynTrait>)->u8{c.value.value()} } fn main(){let x=S{value:dynTrait};println!(\"{}\",S::<dyn Trait>::f(&x));}";

// Only binding/call references and diagnostics are scripted. No type identity or
// overlay dispatch answer is supplied: declaration safety must precede either.
async fn conversion_gateway(
    ws: &prod_code_testkit::Workspace,
    source: &str,
) -> std::net::SocketAddr {
    let binding = position(source, "c:&");
    let use_site = if source.contains("c.value") {
        "c.value"
    } else {
        "c.0"
    };
    let uses = locations(ws, source, &[use_site]);
    let calls = locations(ws, source, &["f(&x)"]);
    ScriptedGateway::start(move |method, params| {
        let pos = (
            params["position"]["line"].as_u64().unwrap_or(0) as u32 + 1,
            params["position"]["character"].as_u64().unwrap_or(0) as u32 + 1,
        );
        match method {
            "textDocument/references" if pos == binding => uses.clone(),
            "textDocument/references" => calls.clone(),
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => Value::Null,
        }
    })
    .await
    .addr()
}

#[tokio::test]
async fn distinct_type_token_boundaries_refuse_before_writes() {
    refuse_types(DISTINCT, "1\n").await;
}

async fn refuse_types(source: &str, expected: &str) {
    assert_eq!(run(source, "type boundary original"), expected);
    let mut failures = Vec::new();
    let ws = workspace(source);
    let remote = conversion_gateway(&ws, source).await;
    for force in [false, true] {
        for apply in [false, true] {
            ws.write("src/main.rs", source);
            let result = convert(&ws, remote, apply, force).await;
            println!("type boundary apply={apply} force={force}: {result:?}");
            match result {
                Err(error) => {
                    assert!(error.to_string().contains("cannot become the receiver"));
                    untouched(&ws, source);
                    assert_eq!(
                        run(&ws.read("src/main.rs"), "type boundary refused"),
                        expected
                    );
                }
                Ok(plan) => {
                    if !apply {
                        untouched(&ws, source);
                    }
                    let output = run(&plan.rewritten[0].1, "type boundary accepted");
                    failures.push(format!(
                        "apply={apply} force={force}: accepted, runtime={output:?}"
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
async fn macro_type_literal_and_punctuation_boundaries_are_not_trivia() {
    for (rules, owner, parameter) in [
        (
            r#"(b $s:literal) => {u8}; ($s:literal) => {u16};"#,
            r#"b "x""#,
            r#"b"x""#,
        ),
        (
            r##"(b $s:literal) => {u8}; ($s:literal) => {u16};"##,
            r##"b r#"x"#"##,
            r##"br#"x"#"##,
        ),
        (
            r#"($s:literal suffix) => {u8}; ($s:literal) => {u16};"#,
            r#""x" suffix"#,
            r#""x"suffix"#,
        ),
        ("(& &) => {u8}; (&&) => {u16};", "& &", "&&"),
        ("([& &]) => {u8}; ([&&]) => {u16};", "[& &]", "[&&]"),
    ] {
        let source = format!(
            "macro_rules! n {{ {rules} }}\nstruct S<T>{{value:T}}\nimpl<T> S<T> {{ fn value(&self)->u8 {{std::mem::size_of_val(&self.value) as u8}} }}\nimpl S<n!({owner})> {{ fn f(c:&S<n!({parameter})>)->u8 {{c.value()}} }}\nfn main() {{let x=S{{value:0u16}};println!(\"{{}}\",S::<n!({owner})>::f(&x));}}"
        );
        refuse_types(&source, "2\n").await;
    }
}

#[tokio::test]
async fn equivalent_trait_objects_and_generic_type_trivia_still_convert() {
    for source in [
        DISTINCT.replace("c:&S<dynTrait>", "c:& S < dyn /* outer /* nested */ comment */ Trait >"),
        "struct S<T>(T);\nimpl<T: Copy> S<T> { fn f(c:& S /* type */ < T >)->T {c.value()} fn value(&self)->T {self.0} } fn main(){let x=S(7);println!(\"{}\",S::f(&x));}".to_string(),
        "struct S<T>(T);\nimpl S<fn() -> u8> { fn f(c:& S < fn /* gap */ ( ) -> u8 >)->u8 {c.value()} fn value(&self)->u8 {(self.0)()} } fn main(){let x:S<fn()->u8>=S(||7);println!(\"{}\",S::f(&x));}".to_string(),
        "struct S<T>(T);\nimpl S<Option<Option<u8>>> { fn f(c:& S < Option < Option < u8 > /* closer */ > >)->u8 {c.value()} fn value(&self)->u8 {self.0.unwrap().unwrap()} } fn main(){let x=S(Some(Some(7)));println!(\"{}\",S::f(&x));}".to_string(),
    ] {
        let expected = if source.contains("dyn Trait") { "2\n" } else { "7\n" };
        equivalent_conversion(&source, expected).await;
    }
}

#[tokio::test]
async fn equivalent_ordinary_type_punctuation_trivia_still_convert() {
    for (owner, parameter, value) in [
        ("&'static u8", " &'static u8", "&7"),
        ("&'static u8", "/* reference */ &'static u8", "&7"),
        ("*const u8", " /* pointer */ *const u8", "&7u8 as *const u8"),
        (
            "(u8,&'static u8)",
            "(u8, /* second */ &'static u8)",
            "(0, &7)",
        ),
        (
            "fn()->&'static u8",
            "fn() -> /* result */ &'static u8",
            "(|| &7u8) as fn()->&'static u8",
        ),
    ] {
        let source = format!(
            "struct S<T>(T);\nimpl S<{owner}> {{ fn f(c:&S<{parameter}>)->u8 {{c.value()}} fn value(&self)->u8 {{7}} }} fn main(){{let x=S({value});println!(\"{{}}\",S::f(&x));}}"
        );
        equivalent_conversion(&source, "7\n").await;
    }
}

#[tokio::test]
async fn equivalent_slice_element_type_trivia_still_convert() {
    for (owner, parameter, value) in [
        (
            "[Option<Option<u8>>]",
            "[Option<Option<u8> /* closer */ >]",
            "S([Some(Some(7))])",
        ),
        ("[&&u8]", "[& /* borrow */ &u8]", "S([&&7])"),
    ] {
        // Unsized slice owners use a reference unsizing coercion at the call.
        let source = format!(
            "struct S<T: ?Sized>(T);\nimpl S<{owner}> {{ fn f(c:&S<{parameter}>)->usize {{c.0.len()}} }} fn main(){{let x={value};println!(\"{{}}\",S::<{owner}>::f(&x));}}"
        );
        equivalent_conversion(&source, "1\n").await;
    }
}

#[test]
fn array_element_trivia_preserves_length_expression_token_boundaries() {
    use prod_code_mcp::to_method::receiver_for;

    for (parameter, owner) in [
        (
            "S<[Option<Option<u8> /* closer */ >; 1]>",
            "S<[Option<Option<u8>>; 1]>",
        ),
        (
            "S<([u8; 1], Option<Option<u8> >)>",
            "S<([u8; 1], Option<Option<u8>>)>",
        ),
    ] {
        assert_eq!(
            receiver_for(&format!("c: &{parameter}"), owner),
            Some(("&self".into(), "c".into()))
        );
    }
    for (parameter, owner) in [
        ("S<[u8; n!(> >)]>", "S<[u8; n!(>>)]>"),
        ("S<[u8; n![> >]]>", "S<[u8; n![>>]]>"),
    ] {
        assert_eq!(receiver_for(&format!("c: &{parameter}"), owner), None);
    }
}

async fn equivalent_conversion(source: &str, expected: &str) {
    assert_eq!(run(source, "equivalent type original"), expected);
    let ws = workspace(source);
    let remote = conversion_gateway(&ws, source).await;
    for force in [false, true] {
        for apply in [false, true] {
            ws.write("src/main.rs", source);
            let plan = convert(&ws, remote, apply, force).await.unwrap();
            println!("equivalent type apply={apply} force={force}: {plan:?}");
            assert_eq!(plan.applied, apply);
            assert_eq!(plan.renamed_uses, 1);
            assert_eq!(
                run(&plan.rewritten[0].1, "equivalent type converted"),
                expected
            );
            if apply {
                assert_eq!(ws.read("src/main.rs"), plan.rewritten[0].1);
            } else {
                untouched(&ws, source);
            }
            assert_eq!(ws.read("Cargo.toml"), CARGO);
            assert_eq!(ws.read("untouched.txt").as_bytes(), b"sentinel\r\n");
        }
    }
}
