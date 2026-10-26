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

async fn binding_case(body: &str, init: &str, expected: usize, count: usize, kept: &str) {
    let source = format!(
        "struct Store {{}}\nimpl Store {{\nfn limit(&self) -> usize {{ {body} }}\n}}\nfn main() {{ assert_eq!(Store {{}}.limit(), {expected}); }}\n"
    );
    binding_source_case(&source, "base * 8", init, count, kept).await;
}

async fn binding_source_case(source: &str, selection: &str, init: &str, count: usize, kept: &str) {
    let ws = fixture(source);
    assert!(execute(&ws.root(), "binding identity original").0);
    let mut failures = Vec::new();
    for force in [false, true] {
        for apply in [false, true] {
            ws.write("src/main.rs", source);
            match promote_initialized(&ws, selection, init, true, apply, force).await {
                Ok(result) => {
                    returned(&ws, &result);
                    let changed = ws.read("src/main.rs");
                    let (ok, _) = execute(&ws.root(), "binding identity returned");
                    if !ok || result.replaced != count || !changed.contains(kept) {
                        failures.push(format!("force={force} apply={apply}: {result:?}"));
                    }
                }
                Err(e) => {
                    let unchanged = ws.read("src/main.rs") == source;
                    println!("binding refusal byte_equal={unchanged}");
                    assert!(unchanged);
                    failures.push(format!("force={force} apply={apply}: {e}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn raw_identifiers_keep_canonical_binding_identity() {
    // Both spellings denote the same Rust name, including in a braced closure.
    binding_case(
        "let base = 8; let a = base * 8; let f = |r#base: usize| { base * 8 }; a + f(2)",
        "64",
        80,
        1,
        "{ base * 8 }",
    )
    .await;
    let source = "struct Store {}\nimpl Store {\nfn limit(&self) -> usize { let base = 8; let f = |base: usize| r#base * 8; let a = r#base * 8; f(2) + a }\n}\nfn main() { assert_eq!(Store {}.limit(), 80); }\n";
    binding_source_case(source, "r#base * 8", "16", 1, "let a = r#base * 8;").await;
    binding_case(
        "let r#base = 8; let a = base * 8; let f = || { base * 8 }; a + f()",
        "64",
        128,
        2,
        "let f = || { self.cap }",
    )
    .await;
}

#[tokio::test]
async fn unrelated_imports_keep_known_constant_binding_identity() {
    // Retain the original shadowing fixture, adding only unrelated imports.
    for import in [
        "use std::fmt::Debug;",
        "use std::{fmt::{self, Debug}, cmp::max as maximum};",
        "use std::fmt::Debug as r#Other;",
        "use std::fmt::Debug as _;",
    ] {
        let source = format!(
            "{import}\nconst FACTOR: usize = 8;\nstruct Store {{}}\nimpl Store {{\nfn limit(&self) -> usize {{\nlet base = 8;\nlet a = base * FACTOR;\nlet b = base * FACTOR;\nlet base = 2;\nlet c = base * FACTOR;\nlet d = {{ let base = 3; base * FACTOR }};\na + b + c + d\n}}\n}}\nfn main() {{ assert_eq!(Store {{}}.limit(), 168); }}\n"
        );
        binding_source_case(&source, "base * FACTOR", "64", 2, "let c = base * FACTOR;").await;
    }
}

#[tokio::test]
async fn imports_of_the_referenced_name_still_do_not_prove_identity() {
    for import in [
        "use values::FACTOR;",
        "use values::OTHER as r#FACTOR;",
        "use values::*;",
    ] {
        let source = format!(
            "mod values {{ pub const FACTOR: usize = 2; pub const OTHER: usize = 2; }}\nconst FACTOR: usize = 8;\nstruct Store {{}}\nimpl Store {{\nfn limit(&self) -> usize {{ let base = 8; let a = base * FACTOR; let b = base * FACTOR; let c = {{ {import} base * FACTOR }}; a + b + c }}\n}}\nfn main() {{ assert_eq!(Store {{}}.limit(), 144); }}\n"
        );
        binding_source_case(&source, "base * FACTOR", "64", 2, "base * FACTOR }").await;
    }
}

#[tokio::test]
async fn selected_unbraced_closure_parameter_does_not_escape() {
    binding_case(
        "let base = 8; let f = |base: usize| base * 8; let a = base * 8; f(2) + a",
        "16",
        80,
        1,
        "let a = base * 8;",
    )
    .await;
}

#[tokio::test]
async fn closure_parameter_duplicates_are_retained() {
    binding_case(
        "let base = 8; let f = |base: usize| (base * 8) + (base * 8); let a = base * 8; f(2) + a",
        "16",
        96,
        2,
        "let a = base * 8;",
    )
    .await;
}

#[tokio::test]
async fn captured_outer_bindings_cross_closures_and_blocks() {
    binding_case(
        "let base = 8; let f = || base * 8; let a = base * 8; let g = || { base * 8 }; let b = { base * 8 }; f() + a + g() + b",
        "64",
        256,
        4,
        "let f = || self.cap;",
    )
    .await;
}

#[tokio::test]
async fn captured_outer_binding_stays_separate_from_later_shadows() {
    binding_case(
        "let base = 8; let f = || base * 8; let g = |base: usize| base * 8; let base = 3; let a = base * 8; f() + g(2) + a",
        "64",
        104,
        1,
        "let a = base * 8;",
    )
    .await;
}

#[tokio::test]
async fn direct_unary_not_operands_support_single_and_all_modes() {
    let mut failures = Vec::new();
    for all in [false, true] {
        let prefix = if all { "let selected = 64;" } else { "" };
        let source = format!(
            "struct Store {{}}\nimpl Store {{\nfn limit(&self) -> usize {{ {prefix} !64 }}\n}}\nfn main() {{ assert_eq!(Store {{}}.limit(), !64usize); }}\n"
        );
        let ws = fixture(&source);
        assert!(execute(&ws.root(), "direct not original").0);
        for force in [false, true] {
            for apply in [false, true] {
                ws.write("src/main.rs", &source);
                match promote(&ws, "64", all, apply, force).await {
                    Ok(result) => {
                        returned(&ws, &result);
                        let changed = ws.read("src/main.rs");
                        let (ok, _) = execute(&ws.root(), "direct not returned");
                        if !ok
                            || result.replaced != if all { 2 } else { 1 }
                            || !changed.contains("!self.cap")
                        {
                            failures
                                .push(format!("all={all} force={force} apply={apply}: {result:?}"));
                        }
                    }
                    Err(e) => {
                        let unchanged = ws.read("src/main.rs") == source;
                        println!("direct not refusal byte_equal={unchanged}");
                        assert!(unchanged);
                        failures.push(format!("all={all} force={force} apply={apply}: {e}"));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn direct_not_macro_payloads_still_refuse_byte_preservingly() {
    for body in ["keep!(64)", "crate::keep!{64}", "keep![64]"] {
        let source = format!(
            "#[macro_export] macro_rules! keep {{ ($e:expr) => {{ !$e }}; }}\nstruct Store {{}}\nimpl Store {{\nfn limit(&self) -> usize {{ {body} }}\n}}\nfn main() {{ assert_eq!(Store {{}}.limit(), !64usize); }}\n"
        );
        let ws = fixture(&source);
        assert!(execute(&ws.root(), "not macro original").0);
        for all in [false, true] {
            for force in [false, true] {
                assert!(promote(&ws, "64", all, true, force).await.is_err());
                let unchanged = ws.read("src/main.rs") == source;
                println!("not macro all={all} force={force} byte_equal={unchanged}");
                assert!(unchanged);
            }
        }
    }
}

#[tokio::test]
async fn qualified_duplicates_keep_the_same_resolution_scope() {
    let source = "struct Store {}\nimpl Store {\nfn limit(&self) -> usize { let a = std::cmp::max(8, 8); let b = std::cmp::max(8, 8); a + b }\n}\nfn main() { assert_eq!(Store {}.limit(), 16); }\n";
    binding_source_case(source, "std::cmp::max(8, 8)", "8", 2, "let b = self.cap;").await;
}

#[tokio::test]
async fn method_duplicates_require_the_same_trait_environment() {
    let modules = "mod left { pub trait Mode { fn scale(self) -> usize; } impl Mode for usize { fn scale(self) -> usize { 8 } } }\nmod right { pub trait Mode { fn scale(self) -> usize; } impl Mode for usize { fn scale(self) -> usize { 2 } } }\n";
    let source = format!(
        "{modules}struct Store {{}}\nimpl Store {{\nfn limit(&self) -> usize {{ let base = 0usize; let a = {{ use left::Mode; base.scale() }}; let b = {{ use right::Mode; base.scale() }}; a + b }}\n}}\nfn main() {{ assert_eq!(Store {{}}.limit(), 10); }}\n"
    );
    binding_source_case(
        &source,
        "base.scale()",
        "8",
        1,
        "use right::Mode; base.scale()",
    )
    .await;
}

#[tokio::test]
async fn additional_method_duplicates_in_one_trait_environment() {
    // A method in one unchanged trait environment retains replace-all behavior.
    let source = "trait Mode { fn scale(self) -> usize; } impl Mode for usize { fn scale(self) -> usize { 8 } }\nstruct Store {}\nimpl Store {\nfn limit(&self) -> usize { let base = 0usize; let a = base.scale(); let b = { base.scale() }; a + b }\n}\nfn main() { assert_eq!(Store {}.limit(), 16); }\n";
    binding_source_case(&source, "base.scale()", "8", 2, "let b = { self.cap }").await;
}

#[tokio::test]
async fn escaping_closure_duplicates_do_not_acquire_self_captures() {
    let source = "struct Store {}\nimpl Store {\nfn limit(&self) -> Box<dyn Fn() -> usize> { let base = 8; let selected = base * 8; Box::new(move || { base * 8 }) }\n}\nfn main() { let f = Store {}.limit(); assert_eq!(f(), 64); }\n";
    binding_source_case(source, "base * 8", "64", 1, "move || { base * 8 }").await;
}

#[tokio::test]
async fn additional_unbraced_escaping_closure() {
    let source = "struct Store {}\nimpl Store {\nfn limit(&self) -> Box<dyn Fn() -> usize> { let base = 8; let selected = base * 8; Box::new(move || base * 8) }\n}\nfn main() { let f = Store {}.limit(); assert_eq!(f(), 64); }\n";
    binding_source_case(source, "base * 8", "64", 1, "move || base * 8").await;
}

#[tokio::test]
async fn additional_locally_bound_escaping_closure() {
    // The closure may escape through a local binding rather than its initializer.
    let source = "struct Store {}\nimpl Store {\nfn limit(&self) -> Box<dyn Fn() -> usize> { let base = 8; let selected = base * 8; let f = move || { base * 8 }; Box::new(f) }\n}\nfn main() { let f = Store {}.limit(); assert_eq!(f(), 64); }\n";
    binding_source_case(source, "base * 8", "64", 1, "move || { base * 8 }").await;
}

#[tokio::test]
async fn local_move_closure_duplicates_still_replace() {
    binding_case(
        "let base = 8; let selected = base * 8; let f = move || { base * 8 }; selected + f()",
        "64",
        128,
        2,
        "move || { self.cap }",
    )
    .await;
}

#[tokio::test]
async fn escaping_async_move_duplicates_do_not_acquire_self_captures() {
    let poll = "fn run<F: std::future::Future<Output = usize> + ?Sized>(mut f: std::pin::Pin<Box<F>>) -> usize { let waker = std::task::Waker::noop(); let mut cx = std::task::Context::from_waker(waker); match f.as_mut().poll(&mut cx) { std::task::Poll::Ready(v) => v, std::task::Poll::Pending => panic!(), } }\n";
    for body in [
        "let base = 8; let selected = base * 8; let _ = selected; Box::pin(async move { base * 8 })",
        "let base = 8; let selected = base * 8; let _ = selected; let fut = async move { base * 8 }; Box::pin(fut)",
    ] {
        let source = format!(
            "{poll}struct Store {{}}\nimpl Store {{\nfn limit(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = usize>>> {{ {body} }}\n}}\nfn main() {{ assert_eq!(run(Store {{}}.limit()), 64); }}\n"
        );
        binding_source_case(&source, "base * 8", "64", 1, "async move { base * 8 }").await;
    }
}

#[tokio::test]
async fn local_awaited_futures_and_selected_async_blocks_still_replace() {
    let poll = "fn block_on<F: std::future::Future<Output = usize>>(f: F) -> usize { let mut f = std::pin::pin!(f); let waker = std::task::Waker::noop(); let mut cx = std::task::Context::from_waker(waker); match f.as_mut().poll(&mut cx) { std::task::Poll::Ready(v) => v, std::task::Poll::Pending => panic!(), } }\n";
    let awaited = format!(
        "{poll}struct Store {{}}\nimpl Store {{\nasync fn calc(&self) -> usize {{ let base = 8; let selected = base * 8; let now = (async move {{ base * 8 }}).await; let fut = async move {{ base * 8 }}; selected + now + fut.await }}\n}}\nfn main() {{ assert_eq!(block_on(Store {{}}.calc()), 192); }}\n"
    );
    binding_source_case(
        &awaited,
        "base * 8",
        "64",
        3,
        "let fut = async move { self.cap };",
    )
    .await;
    let same_async = format!(
        "{poll}struct Store {{}}\nimpl Store {{\nfn limit(&self) -> usize {{ let base = 8; let a = block_on(async move {{ (base * 8) + (base * 8) }}); let escaped: std::pin::Pin<Box<dyn std::future::Future<Output = usize>>> = Box::pin(async move {{ base * 8 }}); a + block_on(escaped) }}\n}}\nfn main() {{ assert_eq!(Store {{}}.limit(), 192); }}\n"
    );
    binding_source_case(
        &same_async,
        "base * 8",
        "64",
        2,
        "Box::pin(async move { base * 8 })",
    )
    .await;
}

#[tokio::test]
async fn same_scope_imported_constants_keep_replace_all_behavior() {
    for import in [
        "use values::FACTOR;",
        "use values::OTHER as FACTOR;",
        "use values::OTHER as r#FACTOR;",
        "use values::*;",
    ] {
        let source = format!(
            "mod values {{ pub const FACTOR: usize = 8; pub const OTHER: usize = 8; }}\nmod shadow {{ pub const FACTOR: usize = 2; }}\n{import}\nstruct Store {{}}\nimpl Store {{\nfn limit(&self) -> usize {{ let base = 8; let a = base * FACTOR; let b = base * FACTOR; let c = {{ use shadow::FACTOR; base * FACTOR }}; let base = 3; let d = base * FACTOR; let e = {{ const FACTOR: usize = 3; base * FACTOR }}; a + b + c + d + e }}\n}}\nfn main() {{ assert_eq!(Store {{}}.limit(), 177); }}\n"
        );
        binding_source_case(&source, "base * FACTOR", "64", 2, "let b = self.cap;").await;
    }
}

#[tokio::test]
async fn statement_macros_in_trait_scopes_preserve_dispatch_without_confusing_nested_scopes() {
    let prelude = "trait Outer { fn scale(&self) -> usize; }\nimpl Outer for usize { fn scale(&self) -> usize { 8 } }\nmod inner { pub trait Inner { fn scale(self) -> usize; } impl Inner for usize { fn scale(self) -> usize { 2 } } }\nmacro_rules! use_inner { () => { use inner::Inner; }; }\n";
    let shadowed = format!(
        "{prelude}struct Store {{}}\nimpl Store {{\nfn limit(&self) -> usize {{ let a = 0usize.scale(); let b = {{ use_inner!(); 0usize.scale() }}; a + b }}\n}}\nfn main() {{ assert_eq!(Store {{}}.limit(), 10); }}\n"
    );
    binding_source_case(
        &shadowed,
        "0usize.scale()",
        "8",
        1,
        "use_inner!(); 0usize.scale()",
    )
    .await;
    let nested_unrelated = format!(
        "{prelude}struct Store {{}}\nimpl Store {{\nfn limit(&self) -> usize {{ let a = 0usize.scale(); let nested = {{ use_inner!(); 2usize }}; let b = 0usize.scale(); a + nested + b }}\n}}\nfn main() {{ assert_eq!(Store {{}}.limit(), 18); }}\n"
    );
    binding_source_case(
        &nested_unrelated,
        "0usize.scale()",
        "8",
        2,
        "let b = self.cap;",
    )
    .await;
}
