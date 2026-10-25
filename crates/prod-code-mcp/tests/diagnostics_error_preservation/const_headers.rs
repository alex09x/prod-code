/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;
use prod_code_mcp::diagnostics::{parse_items, set_aside_preexisting};

const FUNCTION: &str = "struct Output<const N: usize>;\nfn first() -> Output<{ 1 }>\n{\n    missing();\n    Output\n}\n";
const IMPL: &str = "struct First<const N: usize>;\nstruct Second<const N: usize>;\nimpl First<{ 1 }>\n{\nfn run()\n{\n    missing();\n}\n}\n";
const MULTIPLE: &str = "struct Output<const N: usize, const M: usize>;\nfn first() -> Output<{ 1 }, { 2 }>\n{\n    missing();\n    Output\n}\n";
const EXPRESSIONS: &str = "struct Output<const N: usize>;\nfn first() -> Output<{ if 1 < 2 { 1 } else { 0 } } /* > } */>\n{\n    missing();\n    Output\n}\n";

fn diagnostic(text: &str) -> Value {
    let line = text.lines().position(|s| s.contains("missing();")).unwrap() as u32 + 1;
    let col = text
        .lines()
        .nth(line as usize - 1)
        .unwrap()
        .find("missing")
        .unwrap() as u32
        + 1;
    let mut answer = answers::error_at(line, col, "E0425", "cannot find function `missing`");
    answer["items"][0]["source"] = json!("rust-analyzer");
    answer["items"][0]["range"]["end"] = json!({"line": line - 1, "character": col + 6});
    answer
}

/// Exercise every consumer before asserting, including on a broken implementation.
pub(super) async fn check(label: &str, before: &str, after: &str, preexisting: bool) -> bool {
    let path = "src/lib.rs";
    let ws = workspace(before);
    let mut filtered = parse_items(path, &diagnostic(after));
    set_aside_preexisting(
        &mut filtered,
        after,
        &parse_items(path, &diagnostic(before)),
        before,
    );
    let gw = gateway(&ws, diagnostic).await;
    let single = validate_text(gw.addr(), &ws.root(), &ws.path(path), after)
        .await
        .unwrap();
    let batch = validate_texts(gw.addr(), &ws.root(), &[(ws.path(path), after.into())], &[])
        .await
        .unwrap();
    let public = execute_tool(
        gw.addr(),
        &ws.root(),
        "code_validate_edits",
        json!({"edits": [{"path": path, "new_text": after}]}),
    )
    .await
    .unwrap();
    eprintln!(
        "{label}: expected_preexisting={preexisting} filter={}/{}, single={}/{}, batch={}/{}, public_is_error={}",
        filtered.errors,
        filtered.preexisting.len(),
        single.errors,
        single.preexisting.len(),
        batch[0].errors,
        batch[0].preexisting.len(),
        public.is_error
    );
    let expected_errors = usize::from(!preexisting);
    let expected_old = usize::from(preexisting);
    let correct = [&filtered, &single, &batch[0]].iter().all(|r| {
        r.ok() == preexisting && r.errors == expected_errors && r.preexisting.len() == expected_old
    }) && public.is_error != preexisting;
    let rendered = serde_json::to_string(&public).unwrap();
    assert!(rendered.contains("E0425"), "{label}: {rendered}");
    assert_eq!(ws.read(path), before, "{label}: overlay changed disk");
    correct
}

#[tokio::test]
async fn const_header_owner_changes_keep_errors_blocking() {
    let mut failures = Vec::new();
    for (label, before, after) in [
        (
            "function return",
            FUNCTION.to_owned(),
            FUNCTION.replace("fn first", "fn second"),
        ),
        (
            "impl owner",
            IMPL.to_owned(),
            IMPL.replace("impl First", "impl Second"),
        ),
        (
            "multiple arguments",
            MULTIPLE.to_owned(),
            MULTIPLE.replace("fn first", "fn second"),
        ),
        (
            "nested expressions and lexical trivia",
            EXPRESSIONS.to_owned(),
            EXPRESSIONS.replace("fn first", "fn second"),
        ),
    ] {
        if !check(label, &before, &after, false).await {
            failures.push(label);
        }
    }
    assert!(failures.is_empty(), "authority transferred: {failures:?}");
}

#[tokio::test]
async fn const_header_same_owner_shifts_preserve_baselines() {
    let mut failures = Vec::new();
    for (label, before) in [
        ("function", FUNCTION),
        ("impl", IMPL),
        ("multiple arguments", MULTIPLE),
        ("nested expressions", EXPRESSIONS),
    ] {
        for after in [
            format!("// harmless insertion\n{before}"),
            before.replace(
                "    missing();",
                "    // harmless insertion\n    missing();",
            ),
            before.replace("{ 1 }>", "{ 1 } /* } > */>"),
            before.replace("{ 1 }", "{ 1 } /* outer /* nested */ \" > } */"),
            before.replace("{ 1 }", "{ 1 } // } >\n"),
        ] {
            if !check(label, before, &after, true).await {
                failures.push(label);
            }
        }
    }
    // Completed items/blocks must not become part of the following declaration.
    for (label, prefix, changed) in [
        (
            "completed function",
            "fn previous() {}\n",
            "fn renamed() {}\n",
        ),
        (
            "completed statement",
            "fn outer() {\n    if true {}\n",
            "fn outer() {\n    if false {}\n",
        ),
    ] {
        let suffix = if label == "completed function" {
            ""
        } else {
            "}\n"
        };
        let before = format!("{prefix}{FUNCTION}{suffix}");
        let after = format!("{changed}{FUNCTION}{suffix}");
        if !check(label, &before, &after, true).await {
            failures.push(label);
        }
    }
    let before = "fn outer() {\n    if true {}\n    \"a\" >\n    missing();\n}\n";
    if !check(
        "literal starts statement",
        before,
        &before.replace("true", "false"),
        true,
    )
    .await
    {
        failures.push("literal starts statement");
    }
    assert!(failures.is_empty(), "baseline lost: {failures:?}");
}

#[tokio::test]
async fn const_header_unicode_trivia_preserves_owner_authority() {
    let mut failures = Vec::new();
    // Rust Pattern_White_Space includes direction marks that char::is_whitespace omits.
    for trivia in ['\u{0085}', '\u{200e}', '\u{200f}', '\u{2028}', '\u{2029}'] {
        for source in [FUNCTION, IMPL, MULTIPLE] {
            let before = source.replace("}", &format!("}}{trivia}"));
            let after = before
                .replace("fn first", "fn second")
                .replace("impl First", "impl Second");
            let label = format!("Unicode U+{:04X} owner change", trivia as u32);
            if !check(&label, &before, &after, false).await {
                failures.push(label);
            }
            let label = format!("Unicode U+{:04X} harmless shift", trivia as u32);
            if !check(&label, &before, &format!("// shift\n{before}"), true).await {
                failures.push(label);
            }
            let label = format!("Unicode U+{:04X} inserted trivia", trivia as u32);
            if !check(&label, source, &before, true).await {
                failures.push(label);
            }
        }
    }
    assert!(failures.is_empty(), "Unicode scope mismatch: {failures:?}");
}

#[tokio::test]
async fn const_header_completed_siblings_preserve_baselines() {
    let mut failures = Vec::new();
    for (label, before) in [
        (
            "comma-separated match arms",
            "fn run(value: u8) {\nmatch value {\n    0 => { let a = 1; },\n    _ => {\n    missing();\n    }\n}\n}\n",
        ),
        (
            "comma-separated struct fields",
            "struct Record { first: u8, second: u8 }\nfn run() {\nlet _ = Record {\n    first: { let a = 1; a },\n    second: {\n    missing();\n    0\n    },\n};\n}\n",
        ),
    ] {
        if !check(label, before, &before.replace("a = 1", "a = 2"), true).await {
            failures.push(label);
        }
    }
    assert!(failures.is_empty(), "sibling baseline lost: {failures:?}");
}

const CHAR_OWNER: &str = "struct Tag<const C: char>;\nimpl Tag<'a'>\n{\n    fn run()\n    {\n        missing();\n    }\n}\n";

fn literal_owner(literal: &str) -> String {
    let kind = if literal.starts_with("b'") {
        "u8"
    } else if literal.starts_with('{') {
        "usize"
    } else {
        "char"
    };
    CHAR_OWNER
        .replace("C: char", &format!("C: {kind}"))
        .replace("'a'", literal)
}

#[tokio::test]
async fn literal_header_changed_owners_remain_blocking() {
    let mut failures = Vec::new();
    for (label, from, to) in [
        ("primary char owner", "'a'", "'b'"),
        ("literal brace owner", "'{'", "'}'"),
        ("escaped char owner", "'\\u{61}'", "'\\u{62}'"),
        ("byte char owner", "b'a'", "b'b'"),
        ("string owner", "{ \"a\".len() }", "{ \"b\".len() }"),
        (
            "raw string owner",
            "{ r#\"/* { a } */\"#.len() }",
            "{ r#\"/* { b } */\"#.len() }",
        ),
        (
            "literal interior whitespace",
            "{ \"a b\".len() }",
            "{ \"ab\".len() }",
        ),
    ] {
        let before = literal_owner(from);
        let after = before.replace(from, to);
        if !check(label, &before, &after, false).await {
            failures.push(label);
        }
    }
    assert!(
        failures.is_empty(),
        "literal authority transferred: {failures:?}"
    );
}

#[tokio::test]
async fn literal_header_same_owners_ignore_only_trivia() {
    let mut failures = Vec::new();
    for literal in [
        "'a'",
        "'{'",
        "'\\u{7b}'",
        "b'}'",
        "{ r#\"/* } */\"#.len() }",
        "{ 0x1e + 1 }",
    ] {
        let before = literal_owner(literal);
        for after in [
            format!("// shift\n{before}"),
            before
                .replace("impl Tag<", "impl /* outer /* nested */ } */ Tag < ")
                .replace(">\n", " >\n"),
            before.replace("fn run()", "fn /* comment */ run ( )"),
            before.replace("impl Tag", "impl\u{200e}Tag"),
            before.replace(literal, &format!("/* before */{literal}/* after */")),
            before.replace(" + ", "+"),
        ] {
            if !check(literal, &before, &after, true).await {
                failures.push(literal);
            }
        }
    }
    assert!(failures.is_empty(), "literal baseline lost: {failures:?}");
}

#[tokio::test]
async fn literal_header_token_boundaries_cannot_merge_owners() {
    let before = "struct Tag;\nstruct TraitforTag;\ntrait Trait { fn run(); }\nimpl Trait for Tag\n{\n    fn run()\n    {\n        missing();\n    }\n}\n";
    let after = before.replace("impl Trait for Tag", "impl TraitforTag");
    assert!(check("word boundaries", before, &after, false).await);
    let after = before.replace("Trait for Tag", "Trait/* boundary */for\u{200f}Tag");
    assert!(check("word boundary trivia", before, &after, true).await);
}
