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

#[tokio::test]
async fn patterns_types_signature_and_nested_items_are_preserved() {
    let source = "fn target() -> [i32; 80] {\n    let x = 80;\n    let text = r##\"raw 80 } \\\"\"##;\n    /* outer /* inner */ 80 */\n    let _: [u8; 80] = [0; 80];\n    fn nested() -> i32 { 80 }\n    const C: i32 = 80;\n    let closure = |x: i32| x + 80;\n    let y = match x { 80 => 1, _ => 0 };\n    let larger180 = 180;\n    let _ = (text, nested(), C, closure(0), y, larger180);\n    [x; 80]\n}\nfn main() { println!(\"{}\", target()[0]); }\n";
    let original = run(source, "rs");
    let ws = workspace(source, "rs");
    let plan = change(&ws, source, "rs", "80;\n", true, true, false, false)
        .await
        .unwrap();
    let generated = ws.read("main.rs");
    println!("BOUNDARIES\n{generated}");
    assert_eq!(run(&generated, "rs"), original);
    for unchanged in [
        "-> [i32; 80]",
        "[u8; 80]",
        "r##\"raw 80",
        "/* outer /* inner */ 80 */",
        "fn nested() -> i32 { 80 }",
        "const C: i32 = 80;",
        "|x: i32| x + 80",
        "80 => 1",
        "larger180 = 180",
    ] {
        assert!(
            generated.contains(unchanged),
            "lost {unchanged}: {generated}"
        );
    }
    assert_eq!(plan.replaced, 1);
}

#[tokio::test]
async fn equivalent_expression_nodes_respect_precedence_and_shadowing() {
    let source = "fn target(x: i32) -> i32 {\n    let a = x + 1;\n    let b = x+1;\n    let c = 2 * x + 1;\n    let d = { let x = 10; x + 1 };\n    a + b + c + d\n}\nfn main() { let x = 4; println!(\"{}\", target(x)); }\n";
    let original = run(source, "rs");
    let ws = workspace(source, "rs");
    let plan = change(&ws, source, "rs", "select:x + 1", true, true, false, false)
        .await
        .unwrap();
    assert_eq!(plan.replaced, 2, "{}", plan.rewritten[0].1);
    assert_eq!(run(&ws.read("main.rs"), "rs"), original);
    assert!(ws.read("main.rs").contains("2 * x + 1"));
    assert!(ws.read("main.rs").contains("let x = 10; x + 1"));
}

#[tokio::test]
async fn pattern_bindings_form_distinct_expression_contexts() {
    let source = "fn target(x: i32) -> i32 {\n    let a = x + 1;\n    let b = match Some(10) { Some(x) => x + 1, _ => 0 };\n    let mut c = 0;\n    for x in [5] { c += x + 1; }\n    if let Some(x) = Some(8) { c += x + 1; }\n    a + b + c\n}\nfn main() { let x = 4; println!(\"{}\", target(x)); }\n";
    let original = run(source, "rs");
    let ws = workspace(source, "rs");
    let plan = change(&ws, source, "rs", "select:x + 1", true, true, false, false)
        .await
        .unwrap();
    assert_eq!(plan.replaced, 1, "{}", plan.rewritten[0].1);
    assert_eq!(run(&ws.read("main.rs"), "rs"), original);
}

#[tokio::test]
async fn unmatched_reference_prevents_apply_even_with_force() {
    for (ext, source) in [
        (
            "rs",
            "fn target() -> i32 {\n    80\n}\nfn main() { println!(\"{}\", target()); }\n",
        ),
        (
            "js",
            "function target() {\n    return 80;\n}\nconsole.log(target());\n",
        ),
    ] {
        let ws = workspace(source, ext);
        let needle = if ext == "rs" { "80\n" } else { "80;" };
        let plan = change(&ws, source, ext, needle, true, false, true, true)
            .await
            .unwrap();
        assert_eq!(plan.unmatched.len(), 1);
        for force in [false, true] {
            let error = change(&ws, source, ext, needle, true, true, force, true)
                .await
                .unwrap_err();
            assert!(
                error.to_string().contains("nothing was written"),
                "{error:#}"
            );
            assert_eq!(ws.read(&format!("main.{ext}")), source);
        }
    }
}

#[tokio::test]
async fn selected_non_code_and_partial_tokens_are_refused_without_writes() {
    for source in [
        "fn target() -> i32 {\n    // 80\n    1\n}\nfn main() {}\n",
        "fn target() -> i32 {\n    let _ = \"80px\";\n    1\n}\nfn main() {}\n",
        "fn target() -> [i32; 80] {\n    [1; 80]\n}\nfn main() {}\n",
        "fn target() -> i32 {\n    180\n}\nfn main() {}\n",
    ] {
        for all in [false, true] {
            let ws = workspace(source, "rs");
            let error = change(&ws, source, "rs", "80", all, true, true, false)
                .await
                .unwrap_err();
            assert!(
                error.to_string().contains("complete executable expression"),
                "{error:#}"
            );
            assert_eq!(ws.read("main.rs"), source);
        }
    }
}

#[tokio::test]
async fn utf16_one_line_selection_and_rejected_boundaries_do_not_panic() {
    let source = "/* 🌍 */ fn target() -> i32 { let _ = \"80px\"; 80 + 80 }\nfn main() { println!(\"{}\", target()); }\n";
    let original = run(source, "rs");
    let ws = workspace(source, "rs");
    let plan = change(&ws, source, "rs", "80 +", true, true, false, false)
        .await
        .unwrap();
    assert_eq!(plan.replaced, 2);
    assert_eq!(run(&ws.read("main.rs"), "rs"), original);
    for source in [
        "fn target() -> i32 { 1 }\nfn main() { let _ = 80; }\n",
        "fn target() -> i32 {\n    80\n}\nfn main() {}\n",
    ] {
        let ws = workspace(source, "rs");
        let selection = if source.contains("    80") {
            "select:80\n}\nfn main"
        } else {
            "80"
        };
        assert!(
            change(&ws, source, "rs", selection, true, true, true, false)
                .await
                .is_err()
        );
        assert_eq!(ws.read("main.rs"), source);
    }
}

#[tokio::test]
async fn recursive_argument_occurrences_do_not_overlap_call_edits() {
    let source = "fn target(x: i32) -> i32 {\n    let base = 80;\n    if x == 80 { base } else { target(80) }\n}\nfn main() { println!(\"{}\", target(0)); }\n";
    let original = run(source, "rs");
    let ws = workspace(source, "rs");
    let plan = change(&ws, source, "rs", "80;", true, true, false, false)
        .await
        .unwrap();
    assert_eq!(plan.replaced, 3);
    assert_eq!(run(&ws.read("main.rs"), "rs"), original);
}

#[tokio::test]
async fn replacement_does_not_read_a_shadowing_binding_of_the_new_name() {
    let source = "fn target() -> i32 {\n    let a = 80;\n    let b = { let limit = 7; let _ = limit; 80 };\n    a + b\n}\nfn main() { println!(\"{}\", target()); }\n";
    let original = run(source, "rs");
    let ws = workspace(source, "rs");
    let plan = change(&ws, source, "rs", "80;", true, true, true, false)
        .await
        .unwrap();
    assert_eq!(plan.replaced, 1);
    assert_eq!(run(&ws.read("main.rs"), "rs"), original);
    let source =
        "fn target() -> i32 {\n    let limit = 7;\n    let _ = limit;\n    80\n}\nfn main() {}\n";
    let ws = workspace(source, "rs");
    let error = change(&ws, source, "rs", "80\n", true, true, true, false)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("shadowed"));
    assert_eq!(ws.read("main.rs"), source);
}

#[tokio::test]
async fn generic_call_expressions_remain_extractable() {
    let source = "fn target() -> i32 {\n    item::<80>() + item::<80>()\n}\nfn item<const N: usize>() -> i32 { N as i32 }\nfn main() { println!(\"{}\", target()); }\n";
    let original = run(source, "rs");
    let ws = workspace(source, "rs");
    let plan = change(
        &ws,
        source,
        "rs",
        "select:item::<80>()",
        true,
        true,
        false,
        false,
    )
    .await
    .unwrap();
    assert_eq!(plan.replaced, 2);
    assert_eq!(run(&ws.read("main.rs"), "rs"), original);
}

#[tokio::test]
async fn const_generic_and_qualified_type_tokens_are_preserved() {
    let source = "fn target() -> i32 {\n    let x = 80;\n    let _ = size::<Wrap<80>>();\n    let _ = <Wrap<80> as Size>::size();\n    x + 80\n}\nstruct Wrap<const N: usize>;\ntrait Size { fn size() -> usize; }\nimpl<const N: usize> Size for Wrap<N> { fn size() -> usize { N } }\nfn size<T>() -> usize { std::mem::size_of::<T>() }\nfn main() { println!(\"{}\", target()); }\n";
    let original = run(source, "rs");
    let ws = workspace(source, "rs");
    let plan = change(&ws, source, "rs", "80;", true, true, false, false)
        .await
        .unwrap();
    assert_eq!(plan.replaced, 2, "{}", plan.rewritten[0].1);
    assert_eq!(run(&ws.read("main.rs"), "rs"), original);
    assert!(ws.read("main.rs").contains("size::<Wrap<80>>()"));
    assert!(ws.read("main.rs").contains("<Wrap<80> as Size>"));
}
