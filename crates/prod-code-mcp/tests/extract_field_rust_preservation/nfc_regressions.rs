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

async fn check(source: &str, selection: &str, count: usize, kept: &str) {
    let ws = fixture(source);
    assert!(execute(&ws.root(), "NFC original").0);
    let mut failures = Vec::new();
    for force in [false, true] {
        for apply in [false, true] {
            ws.write("src/main.rs", source);
            let result = promote_initialized(&ws, selection, "64", true, apply, force)
                .await
                .unwrap();
            returned(&ws, &result);
            let changed = ws.read("src/main.rs");
            let (ok, report) = execute(&ws.root(), "NFC returned");
            if !ok || result.replaced != count || !changed.contains(kept) {
                failures.push(format!(
                    "force={force} apply={apply} replaced={} expected={count}\n{report}",
                    result.replaced
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn nfc_shadow_store_report_preserves_80() {
    // Exact complete Store counterexample in review-recovered-4297.json.
    let source = "struct Store {}\nimpl Store {\nfn limit(&self) -> usize { let café = 8; let a = café * 8; let b = { let cafe\u{301} = 2; café * 8 }; a + b }\n}\nfn main() { assert_eq!(Store {}.limit(), 80); }\n";
    check(source, "café * 8", 1, "let cafe\u{301} = 2; café * 8").await;
}

#[tokio::test]
async fn nfc_raw_declarations_and_references_keep_shadow_identity() {
    for (outer, inner, reference) in [
        ("r#café", "r#cafe\u{301}", "café"),
        ("cafe\u{301}", "café", "r#cafe\u{301}"),
    ] {
        let source = format!(
            "struct Store {{}}\nimpl Store {{\nfn limit(&self) -> usize {{ let {outer} = 8; let a = {reference} * 8; let b = {{ let {inner} = 2; {reference} * 8 }}; a + b }}\n}}\nfn main() {{ assert_eq!(Store {{}}.limit(), 80); }}\n"
        );
        check(
            &source,
            &format!("{reference} * 8"),
            1,
            &format!("let {inner} = 2; {reference} * 8"),
        )
        .await;
    }
    // Selecting the actual inner declaration must not resolve past it either.
    let source = "struct Store {}\nimpl Store {\nfn limit(&self) -> usize { let café = 2; let a = { let cafe\u{301} = 8; café * 8 }; let b = café * 8; a + b }\n}\nfn main() { assert_eq!(Store {}.limit(), 80); }\n";
    check(source, "café * 8", 1, "let b = café * 8;").await;
}

#[tokio::test]
async fn nfc_parameter_loop_and_match_shadows_keep_identity() {
    for body in [
        "let café = 8; let a = café * 8; let f = |cafe\u{301}: usize| { café * 8 }; a + f(2)",
        "let café = 8; let a = café * 8; let b = match 2 { cafe\u{301} => café * 8 }; a + b",
        "let café = 8; let a = café * 8; let mut b = 0; for cafe\u{301} in [2] { b += café * 8; } a + b",
        "let café = 8; let a = café * 8; let b = if let Some(cafe\u{301}) = Some(2) { café * 8 } else { 0 }; a + b",
    ] {
        let source = format!(
            "struct Store {{}}\nimpl Store {{\nfn limit(&self) -> usize {{ {body} }}\n}}\nfn main() {{ assert_eq!(Store {{}}.limit(), 80); }}\n"
        );
        check(&source, "café * 8", 1, "café * 8").await;
    }
}

#[tokio::test]
async fn nfc_items_and_imports_shadow_outer_declarations() {
    for item in [
        "const cafe\u{301}: usize = 2;",
        "use values::cafe\u{301};",
        "use values::{cafe\u{301}};",
        "use values::OTHER as r#cafe\u{301};",
    ] {
        let source = format!(
            "mod values {{ pub const café: usize = 2; pub const OTHER: usize = 2; }}\nconst café: usize = 8;\nstruct Store {{}}\nimpl Store {{\nfn limit(&self) -> usize {{ let a = café * 8; let b = {{ {item} café * 8 }}; a + b }}\n}}\nfn main() {{ assert_eq!(Store {{}}.limit(), 80); }}\n"
        );
        check(&source, "café * 8", 1, &format!("{item} café * 8")).await;
    }
}

#[tokio::test]
async fn nfc_capture_uses_are_checked_before_replacing() {
    for (declaration, reference) in [("café", "cafe\u{301}"), ("r#cafe\u{301}", "café")] {
        let source = format!(
            "struct Store {{}}\nimpl Store {{\nfn limit(&self) -> Box<dyn Fn() -> usize> {{ let base = 8; let selected = base * 8; let {declaration} = move || {{ base * 8 }}; Box::new({reference}) }}\n}}\nfn main() {{ let f = Store {{}}.limit(); assert_eq!(f(), 64); }}\n"
        );
        check(&source, "base * 8", 1, "move || { base * 8 }").await;
    }
    let source = "fn run<F: std::future::Future<Output = usize> + ?Sized>(mut f: std::pin::Pin<Box<F>>) -> usize { let waker = std::task::Waker::noop(); let mut cx = std::task::Context::from_waker(waker); match f.as_mut().poll(&mut cx) { std::task::Poll::Ready(v) => v, _ => panic!() } }\nstruct Store {}\nimpl Store {\nfn limit(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = usize>>> { let base = 8; let selected = base * 8; let café = async move { base * 8 }; Box::pin(cafe\u{301}) }\n}\nfn main() { assert_eq!(run(Store {}.limit()), 64); }\n";
    check(source, "base * 8", 1, "async move { base * 8 }").await;
}

#[tokio::test]
async fn nfc_same_binding_and_local_capture_positive_controls() {
    for body in [
        "let r#cafe\u{301} = 8; let a = café * 8; let b = { café * 8 }; a + b",
        "let base = 8; let selected = base * 8; let café = move || { base * 8 }; selected + cafe\u{301}()",
    ] {
        let source = format!(
            "struct Store {{}}\nimpl Store {{\nfn limit(&self) -> usize {{ {body} }}\n}}\nfn main() {{ assert_eq!(Store {{}}.limit(), 128); }}\n"
        );
        let selection = if body.contains("base") {
            "base * 8"
        } else {
            "café * 8"
        };
        check(&source, selection, 2, "self.cap").await;
    }
    let imported = "mod values { pub const café: usize = 8; }\nuse values::cafe\u{301};\nstruct Store {}\nimpl Store {\nfn limit(&self) -> usize { let a = café * 8; let b = café * 8; a + b }\n}\nfn main() { assert_eq!(Store {}.limit(), 128); }\n";
    check(imported, "café * 8", 2, "use values::cafe\u{301};").await;
}

#[tokio::test]
async fn nfc_keys_do_not_use_compatibility_normalization() {
    // These two identifiers are NFKC-equivalent but distinct under Rust's NFC.
    let source = "struct Store {}\nimpl Store {\nfn limit(&self) -> usize { let ff = 8; let a = ff * 8; let b = { let ﬀ = 2; let _ = ﬀ; ff * 8 }; a + b }\n}\nfn main() { assert_eq!(Store {}.limit(), 128); }\n";
    check(source, "ff * 8", 2, "let ﬀ = 2; let _ = ﬀ; self.cap").await;
}
