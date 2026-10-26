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

async fn constant_duplicates(constants: &str, expression: &str, init: &str, expected: usize) {
    let source = format!(
        "{constants}\nstruct Store {{}}\nimpl Store {{\nfn limit(&self) -> usize {{\nlet a = {expression};\nlet b = {expression};\na + b\n}}\n}}\nfn main() {{ assert_eq!(Store {{}}.limit(), {expected}); }}\n"
    );
    let ws = fixture(&source);
    assert!(execute(&ws.root(), "constant bars before").0);
    let result = promote_initialized(&ws, expression, init, true, false, false)
        .await
        .unwrap();
    returned(&ws, &result);
    assert!(execute(&ws.root(), "constant bars returned").0);
    assert_eq!(result.replaced, 2);
}

#[tokio::test]
async fn bitwise_constant_duplicates_are_not_binding_declarations() {
    constant_duplicates(
        "const LEFT: usize = 64; const RIGHT: usize = 1;",
        "LEFT | RIGHT",
        "65",
        130,
    )
    .await;
}

#[tokio::test]
async fn logical_constant_duplicates_are_not_binding_declarations() {
    constant_duplicates(
        "const LEFT: bool = true; const RIGHT: bool = false;",
        "(LEFT || RIGHT) as usize",
        "1",
        2,
    )
    .await;
}

#[tokio::test]
async fn exhaustive_closure_parameters_stay_blocked_and_rest_parameters_compile() {
    let mut failures = Vec::new();
    for (parameters, call) in [
        ("|Store { entries }|", "unpack(s)"),
        ("move |Store { entries }|", "unpack(s)"),
        ("|(Store { entries },)|", "unpack((s,))"),
        ("|Store { entries }, _other: usize|", "unpack(s, 0)"),
        ("|Store { entries } /* | ) => */|", "unpack(s)"),
        ("&|Store { entries }|", "unpack(s)"),
        ("&mut |Store { entries }|", "unpack(s)"),
        ("|| |Store { entries }|", "unpack()(s)"),
        ("|| || |Store { entries }|", "unpack()()(s)"),
    ] {
        let source = format!(
            "struct Store {{ entries: usize }}\nimpl Store {{\nfn limit(&self) -> usize {{ 64 }}\n}}\nfn main() {{ let s = Store {{ entries: 1 }}; assert_eq!(s.limit(), 64); let unpack = {parameters} entries; assert_eq!({call}, 1); }}\n"
        );
        let ws = fixture(&source);
        assert!(execute(&ws.root(), "closure pattern before").0);
        let result = promote(&ws, "64", false, false, false).await.unwrap();
        // Compile the actual preview even when the scripted diagnostics claim success.
        returned(&ws, &result);
        let (compiled, report) = execute(&ws.root(), "exhaustive closure returned");
        if compiled
            || !report.contains("E0027")
            || result.blocked.len() != 1
            || !result.unmatched.is_empty()
            || result.constructors != 1
            || !ws.read("src/main.rs").contains(parameters)
        {
            failures.push(format!("{parameters}: {result:?}"));
        }
        ws.write("src/main.rs", &source);
        if promote(&ws, "64", false, true, false).await.is_ok() {
            failures.push(format!("applied exhaustive {parameters}"));
        }
        ws.write("src/main.rs", &source);
        let rest = source.replace("entries }", "entries, /* | ) => */ .. }");
        ws.write("src/main.rs", &rest);
        let result = promote(&ws, "64", false, true, false).await.unwrap();
        assert!(result.blocked.is_empty());
        assert_eq!(result.constructors, 1);
        assert!(execute(&ws.root(), "rest closure applied").0);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn closure_parameter_shadowing_keeps_duplicate_bindings_separate() {
    let ws = fixture(
        "struct Store {}\nimpl Store {\nfn limit(&self) -> usize {\nlet base = 8;\nlet a = base * 8;\nlet b = base * 8;\nlet f = |base: usize| base * 8;\na + b + f(2)\n}\n}\nfn main() { assert_eq!(Store {}.limit(), 144); }\n",
    );
    assert!(execute(&ws.root(), "closure shadow before").0);
    let result = promote(&ws, "base * 8", true, false, false).await.unwrap();
    returned(&ws, &result);
    assert!(execute(&ws.root(), "closure shadow returned").0);
    assert_eq!(result.replaced, 2);
    assert!(ws.read("src/main.rs").contains("|base: usize| base * 8"));
}

#[tokio::test]
async fn exhausted_closure_context_stays_unknown_even_under_force() {
    let source = format!(
        "struct Store {{ entries: usize }}\nimpl Store {{\nfn limit(&self) -> usize {{ 64 }}\n}}\nfn main() {{ let s = Store {{ entries: 1 }}; assert_eq!(s.limit(), 64); let unpack = {}|Store {{ entries }}| entries; assert_eq!(unpack{}(s), 1); }}\n",
        "|| ".repeat(9),
        "()".repeat(9)
    );
    let ws = fixture(&source);
    assert!(execute(&ws.root(), "bounded closure before").0);
    let result = promote(&ws, "64", false, false, false).await.unwrap();
    assert_eq!(result.unmatched.len(), 1);
    assert_eq!(result.constructors, 1);
    returned(&ws, &result);
    let (compiled, report) = execute(&ws.root(), "bounded closure returned");
    assert!(!compiled && report.contains("E0027"));
    ws.write("src/main.rs", &source);
    assert!(promote(&ws, "64", false, true, true).await.is_err());
    assert_eq!(ws.read("src/main.rs"), source);
}
