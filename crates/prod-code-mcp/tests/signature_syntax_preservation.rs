/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! #1287: syntax survives the public signature planner and unforced application.

#[path = "signature_syntax_preservation/expressions.rs"]
mod expressions;
#[path = "signature_syntax_preservation/support.rs"]
mod support;

use prod_code_mcp::signature::{Modifiers, Param, change_with};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use support::{assert_complete, position, run};

async fn replaced_return(old_type: &str, bounds: &str, prelude: &str, body: &str) {
    let source = format!(
        "{prelude}fn foo<T>() -> {old_type}{bounds}{{ {body} }}\nfn main() {{ println!(\"{{}}\", foo::<u8>()); }}\n"
    );
    let ws = Workspace::new(&[("src/main.rs", &source)]);
    let file = ws.root().join("src/main.rs");
    let call = position(&source, "foo::<u8>()");
    let gateway_file = file.clone();
    let gateway = ScriptedGateway::start(move |method, _| match method {
        "textDocument/references" => answers::locations(&gateway_file, &[call]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await;
    let (line, col) = position(&source, "foo<T>");
    let modifiers = Modifiers {
        returns: Some("u64".into()),
        ..Modifiers::default()
    };
    let expected = source.replacen(&format!("-> {old_type}"), "-> u64", 1);
    let plan = change_with(
        gateway.addr(),
        &ws.root(),
        &file,
        line,
        col,
        &[],
        &modifiers,
        false,
        false,
    )
    .await
    .unwrap();
    assert_complete(&plan);
    assert_eq!(plan.returns, Some((old_type.into(), "u64".into())));
    assert_eq!(plan.rewritten[0].1, expected, "bounds must survive exactly");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), source);
    let applied = change_with(
        gateway.addr(),
        &ws.root(),
        &file,
        line,
        col,
        &[],
        &modifiers,
        true,
        false,
    )
    .await
    .unwrap();
    assert_complete(&applied);
    assert!(applied.applied);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), expected);
    assert_eq!(run(&source), "7\n");
    assert_eq!(run(&expected), "7\n");
}

#[tokio::test]
async fn replacing_return_preserves_newline_where_bounds() {
    replaced_return("u32", "\nwhere\n    T: Copy + Default,\n", "", "7").await;
}

#[tokio::test]
async fn replacing_return_preserves_inline_where_bounds() {
    replaced_return("u32", " where T: Copy + Default ", "", "7").await;
}

#[tokio::test]
async fn where_like_type_tokens_and_strings_are_not_bounds() {
    replaced_return(
        "somewhere",
        "\nwhere T: Copy,\n",
        "#[allow(non_camel_case_types)]\ntype somewhere = u32;\n",
        "7",
    )
    .await;
    replaced_return(
        "ty!(\" where { ;\")",
        "\nwhere T: Copy,\n",
        "macro_rules! ty { ($s:literal) => { u32 }; }\n",
        "7",
    )
    .await;
}

#[tokio::test]
async fn combining_marks_in_return_type_identifiers_are_not_where_keywords() {
    // U+0332 is a Rust XID continuation character, but is not alphanumeric.
    for ty in ["Type\u{0332}where", "where\u{0332}"] {
        replaced_return(
            ty,
            "\nwhere T: Copy,\n",
            &format!("#[allow(non_camel_case_types)]\ntype {ty} = u32;\n"),
            "7",
        )
        .await;
    }
}

#[tokio::test]
async fn return_type_macro_comparisons_preserve_the_return_span() {
    for ty in [
        "ty!(1 < 2)",
        "ty![1 > 2]",
        "ty! /* token tree */ { (1 << 2) < 8 }",
        "ty!([1 < 2, 3 > 2])",
        "ty!(Option::<u8>::None)",
        "Identity<ty!(1 < 2)>",
        "Identity<Identity<ty!(1 < 2)>>",
        "Identity<u32>",
    ] {
        replaced_return(
            ty,
            "\nwhere T: Copy,\n",
            "macro_rules! ty { ($e:expr) => { u32 }; }\ntype Identity<T> = T;\n",
            "7",
        )
        .await;
    }
}

async fn visibility_and_async(add: bool) {
    let (head, call) = if add {
        ("fn", "foo()")
    } else {
        ("pub async fn", "foo().await")
    };
    let source = format!(
        "{head} foo() -> u32 {{ 7 }}\nasync fn caller() -> u32 {{ {call} }}\n{}",
        support::EXECUTOR_MAIN,
    );
    let ws = Workspace::new(&[("src/main.rs", &source)]);
    let file = ws.root().join("src/main.rs");
    let site = position(&source, call);
    // The first occurrence is the declaration when adding async; select the caller explicitly.
    let caller_at = source.find("{ foo()").unwrap() + 2;
    let site = if add {
        support::position_at(&source, caller_at)
    } else {
        site
    };
    let gateway_file = file.clone();
    let gateway = ScriptedGateway::start(move |method, _| match method {
        "textDocument/references" => answers::locations(&gateway_file, &[site]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await;
    let (line, col) = position(&source, "foo()");
    let modifiers = Modifiers {
        visibility: Some(if add { "pub" } else { "private" }.into()),
        asyncness: Some(add),
        ..Modifiers::default()
    };
    let plan = change_with(
        gateway.addr(),
        &ws.root(),
        &file,
        line,
        col,
        &[],
        &modifiers,
        false,
        false,
    )
    .await
    .unwrap();
    assert_complete(&plan);
    let expected = if add {
        source
            .replacen("fn foo", "pub async fn foo", 1)
            .replace("{ foo() }", "{ foo().await }")
    } else {
        source
            .replacen("pub async fn foo", "fn foo", 1)
            .replace("{ foo().await }", "{ foo() }")
    };
    assert_eq!(plan.rewritten[0].1, expected);
    assert_eq!(plan.asyncness, Some((!add, add)));
    let applied = change_with(
        gateway.addr(),
        &ws.root(),
        &file,
        line,
        col,
        &[],
        &modifiers,
        true,
        false,
    )
    .await
    .unwrap();
    assert_complete(&applied);
    assert!(applied.applied);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), expected);
    assert_eq!(run(&source), "7\n");
    assert_eq!(run(&expected), "7\n");
}

#[tokio::test]
async fn adding_visibility_and_async_updates_declaration_and_caller() {
    visibility_and_async(true).await;
}

#[tokio::test]
async fn removing_visibility_and_async_updates_declaration_and_caller() {
    visibility_and_async(false).await;
}

#[tokio::test]
async fn async_and_parameter_reordering_still_require_separate_requests() {
    let source = "fn foo(a: u32, b: u32) -> u32 { a + b }\n";
    let ws = Workspace::new(&[("src/main.rs", source)]);
    let file = ws.root().join("src/main.rs");
    let gateway = ScriptedGateway::start(|_, _| panic!("must refuse before queries")).await;
    let error = change_with(
        gateway.addr(),
        &ws.root(),
        &file,
        1,
        4,
        &[Param::Keep("b".into()), Param::Keep("a".into())],
        &Modifiers {
            asyncness: Some(true),
            ..Modifiers::default()
        },
        true,
        false,
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("in two steps"), "{error:#}");
    assert_eq!(std::fs::read_to_string(file).unwrap(), source);
}
