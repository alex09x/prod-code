/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[path = "make_static_rust_preservation/guards.rs"]
mod guards;
#[path = "make_static_rust_preservation/raw_identifiers.rs"]
mod raw_identifiers;
#[path = "make_static_rust_preservation/support.rs"]
mod support;

use support::*;

#[tokio::test]
async fn owning_receivers_preserve_consumption_and_drop_by_refusing() {
    let mut violations = Vec::new();
    for (receiver, init, call, reference) in [
        ("self", "S", "value.consume()", "consume();"),
        ("mut self", "S", "value.consume()", "consume();"),
        (
            "self: Box<Self>",
            "Box::new(S)",
            "value.consume()",
            "consume();",
        ),
        ("self", "S", "S::consume(value)", "consume(value)"),
    ] {
        assert!(!prod_code_mcp::make_static::receiver_has_effects("value"));
        let source = owning_source(receiver, init, call);
        println!("OWNING {receiver}, {call}: original source\n{source}");
        assert_eq!(run(&source, "original"), "method\ndrop\nafter\n");
        for (apply, force) in [(false, false), (false, true), (true, false), (true, true)] {
            let ws = workspace(&source);
            let remote = gateway(references(&ws, &source, &[reference])).await;
            match change(&ws, remote, apply, force).await {
                Ok(plan) => {
                    println!(
                        "OWNING {receiver}, apply={apply}, force={force}:\n{}",
                        plan.render(12000)
                    );
                    let rewritten = &plan.rewritten[0].1;
                    println!("transformed source\n{rewritten}");
                    let observed = run(rewritten, "transformed");
                    violations.push(format!("{receiver}, {call}, apply={apply}, force={force}: accepted, runtime {observed:?}"));
                }
                Err(err) => {
                    println!("OWNING {receiver}, apply={apply}, force={force}: {err:#}");
                    assert!(err.to_string().contains("owning receiver"), "{err:#}");
                    untouched(&ws, &source);
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "unsafe ownership rewrites:\n{}",
        violations.join("\n")
    );
}

#[tokio::test]
async fn inherent_generic_headers_ignore_comments_and_higher_ranked_bounds() {
    let mut violations = Vec::new();
    for header in [
        "impl<T /* for a type */> S<T>",
        "impl<T: for <'a> Fn(&'a str)> S<T>",
        "impl<T> S<T> where T: for <'a> Fn(&'a str)",
        "impl<T> /* for a type */ S<T>",
        "impl<T /* outer /* for nested */ for comment */> S<T>",
    ] {
        let source = format!(
            "struct S<T>(T);\n{header} {{\n    fn consume(&self) {{ println!(\"method\"); }}\n}}\nfn main() {{\n    let value = S(|_: &str| {{}});\n    value.consume();\n}}\n"
        );
        println!("INHERENT {header}: original source\n{source}");
        assert_eq!(run(&source, "original inherent"), "method\n");
        let ws = workspace(&source);
        let remote = gateway(references(&ws, &source, &["consume();"])).await;
        match change(&ws, remote, false, false).await {
            Ok(plan) => {
                assert_eq!(plan.owner, "S");
                assert_eq!(plan.rewritten_calls, 1);
                println!("INHERENT preview:\n{}", plan.render(12000));
                assert!(plan.rewritten[0].1.contains("S::consume()"));
                untouched(&ws, &source);
            }
            Err(err) => violations.push(format!("{header}: {err:#}")),
        }
        // A generic owner with no inferable arguments at the rewritten call cannot compile;
        // use a no-call fixture to verify the header/declaration independently with rustc.
        let no_call = source.replace("    value.consume();", "    let _ = value;");
        let no_call_ws = workspace(&no_call);
        let remote = gateway(serde_json::json!([])).await;
        match change(&no_call_ws, remote, true, false).await {
            Ok(plan) => {
                assert!(plan.applied);
                let rewritten = no_call_ws.read("src/main.rs");
                assert!(rewritten.contains("fn consume()"));
                println!("INHERENT applied source\n{rewritten}");
                assert_eq!(run(&rewritten, "transformed inherent"), "");
            }
            Err(err) => violations.push(format!("{header} (apply): {err:#}")),
        }
    }
    assert!(
        violations.is_empty(),
        "valid inherent impls refused:\n{}",
        violations.join("\n")
    );
}

#[tokio::test]
async fn inherent_generic_calls_compile_when_arguments_determine_the_type() {
    for header in [
        "impl<T /* for a type */> S<T>",
        "impl<T: for <'a> Fn(&'a str)> S<T>",
        "impl<T> S<T> where T: for <'a> Fn(&'a str)",
    ] {
        let source = format!(
            "struct S<T>(T);\n{header} {{\n    fn consume(&self, _: T) {{ println!(\"method\"); }}\n}}\nfn noop(_: &str) {{}}\nfn main() {{\n    let value = S(noop as fn(&str));\n    value.consume(noop as fn(&str));\n}}\n"
        );
        assert_eq!(run(&source, header), "method\n");
        let ws = workspace(&source);
        let remote = gateway(references(&ws, &source, &["consume(noop"])).await;
        let plan = change(&ws, remote, false, false).await.unwrap();
        untouched(&ws, &source);
        let expected = source
            .replace("&self, ", "")
            .replace("value.consume(", "S::consume(");
        assert_eq!(plan.rewritten[0].1, expected);
        println!("INHERENT with inferred arguments:\n{}", plan.render(12000));
        let applied = change(&ws, remote, true, false).await.unwrap();
        assert!(applied.applied);
        assert_eq!(ws.read("src/main.rs"), expected);
        assert_eq!(run(&expected, "transformed inferred inherent"), "method\n");
    }
}

#[tokio::test]
async fn borrowed_receivers_keep_dot_and_path_calls_working() {
    for receiver in [
        "&self",
        "&mut self",
        "&'a self",
        "self: &Self",
        "self: &mut Self",
        "self: &'a Self",
        "mut self: &'a mut Self",
        "self: (&Self)",
    ] {
        let mutable = receiver.contains("mut");
        let borrow = if mutable { "&mut value" } else { "&value" };
        let source = format!(
            "struct S;\nimpl Drop for S {{ fn drop(&mut self) {{ println!(\"drop\"); }} }}\nimpl S {{\n    fn consume<'a>({receiver}) {{ println!(\"method\"); }}\n}}\nfn main() {{\n    let mut value = S;\n    value.consume();\n    S::consume({borrow});\n    let _still_available = value;\n}}\n"
        );
        assert_eq!(run(&source, receiver), "method\nmethod\ndrop\n");
        let ws = workspace(&source);
        let remote = gateway(references(&ws, &source, &["consume();", "consume(&"])).await;
        let plan = change(&ws, remote, false, false).await.unwrap();
        assert_eq!(plan.receiver, receiver);
        assert_eq!(plan.rewritten_calls, 2);
        assert!(plan.blocked.is_empty());
        assert!(plan.unmatched.is_empty());
        assert!(!plan.applied);
        untouched(&ws, &source);
        let expected = source
            .replace(&format!("consume<'a>({receiver})"), "consume<'a>()")
            .replace("value.consume()", "S::consume()")
            .replace(&format!("S::consume({borrow})"), "S::consume()");
        assert_eq!(plan.rewritten[0].1, expected);
        let applied = change(&ws, remote, true, false).await.unwrap();
        assert!(applied.applied);
        assert_eq!(ws.read("src/main.rs"), expected);
        assert_eq!(
            run(&expected, &format!("transformed {receiver}")),
            "method\nmethod\ndrop\n"
        );
    }
}
