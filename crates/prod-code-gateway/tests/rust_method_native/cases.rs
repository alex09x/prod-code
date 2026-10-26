/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::native::{
    Fixture,
    bridge::{Answer, Bridge},
};
use super::support::{
    artifact, dispatch_identity, observe_probes, position, record_answers, require_methods, runtime,
};
use prod_code_mcp::to_method::{MadeMethod, convert_to_method};

const HOLDER_CONFIG: &str = concat!(
    "struct Config(u32);\n",
    "struct Holder<T>(T);\n",
    "impl Holder<Config> { fn f(h: &Holder<Config>) -> u32 { (h.0).0 + 1 } }\n",
    "fn main() { let a = Holder(Config(6)); let b: Holder<Config> = Holder(Config(40)); ",
    "println!(\"{} {}\", Holder::f(&a), Holder::f(&b)); }\n",
);

const HOLDER_FALLBACKS: &str = concat!(
    "pub struct Config(pub u32);\n",
    "pub struct Holder<T>(pub T);\n",
    "impl Holder<Config> { pub fn f(h: &Holder<Config>) -> u32 { (h.0).0 + 1 } }\n",
    "trait Intercept { fn f(&self) -> u32 { 999 } }\n",
    "mod shadow_head {\n",
    "    pub struct Holder<T>(pub crate::Holder<T>);\n",
    "    impl<T> Holder<T> { pub fn f(&self) -> u32 { 888 } }\n",
    "    impl<T> std::ops::Deref for Holder<T> {\n",
    "        type Target = crate::Holder<T>;\n",
    "        fn deref(&self) -> &Self::Target { &self.0 }\n",
    "    }\n",
    "}\n",
    "mod other { pub struct Config(pub crate::Holder<crate::Config>); }\n",
    "impl Intercept for Holder<other::Config> {}\n",
    "impl std::ops::Deref for Holder<other::Config> {\n",
    "    type Target = Holder<Config>;\n",
    "    fn deref(&self) -> &Self::Target { &(self.0).0 }\n",
    "}\n",
    "fn main() {\n",
    "    let exact = Holder(Config(6));\n",
    "    let head_val = {\n",
    "        use shadow_head::Holder;\n",
    "        let bad_head: Holder<Config> = Holder(crate::Holder(Config(10)));\n",
    "        let _ = bad_head.f();\n",
    "        crate::Holder::f(&bad_head)\n",
    "    };\n",
    "    let arg_val = {\n",
    "        use other::Config;\n",
    "        let bad_arg: Holder<Config> = Holder(Config(Holder(crate::Config(20))));\n",
    "        let _ = bad_arg.f();\n",
    "        crate::Holder::f(&bad_arg)\n",
    "    };\n",
    "    println!(\"{} {} {}\", Holder::f(&exact), head_val, arg_val);\n",
    "}\n",
);

const CONST_SHADOW: &str = concat!(
    "const N: usize = 1;\n",
    "struct S<const K: usize>(usize);\n",
    "impl S<N> { fn f<const N: usize>(c: &S<N>) -> usize { c.0 + N } }\n",
    "fn main() { println!(\"{}\", S::<N>::f::<2>(&S::<2>(5))); }\n",
);

const CONST_MACRO_SHADOW: &str = concat!(
    "macro_rules! cap { ($n:expr) => { [usize; $n] }; }\n",
    "const N: usize = 1;\n",
    "struct S<T>(T);\n",
    "impl S<cap!(N)> { fn f<const N: usize>(c: &S<cap!(N)>) -> usize { c.0[0] + N } }\n",
    "fn main() { println!(\"{}\", S::<[usize; 1]>::f::<2>(&S([5, 8]))); }\n",
);

const CONST_POSITIVE: &str = concat!(
    "macro_rules! fixed { ($ignored:ident) => { [usize; { let K = 1; K }] }; }\n",
    "struct S<const N: usize, T>([usize; N], T);\n",
    "impl<const N: usize> S<N, fixed!(M)> {\n",
    "    fn f<const M: usize>(c: &S<N, fixed!(M)>) -> usize { c.0[0] + c.1[0] + M }\n",
    "}\n",
    "fn main() { let s: S<1, [usize; 1]> = S([4], [2]); println!(\"{}\", S::<1, [usize; 1]>::f::<3>(&s)); }\n",
);

async fn run_positive_case(
    label: &str,
    source: &str,
    decl_needle: (&str, u32),
    impl_needle: (&str, u32),
    fn_needle: &str,
    expected_out: &str,
) -> (Vec<(bool, anyhow::Result<MadeMethod>, String)>, Vec<Answer>) {
    artifact(label, "original.rs", source);
    let before = runtime(label, "original", source);
    assert_eq!(before, expected_out);
    let fixture = Fixture::new(source, "2021").await;
    let bridge = Bridge::bind().await;
    let addr = bridge.addr;
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let client = async {
        observe_probes(
            label,
            &fixture,
            source,
            decl_needle.0,
            decl_needle.1,
            impl_needle.0,
            impl_needle.1,
        )
        .await;
        let file = fixture.workspace.root.join("src/lib.rs");
        let (line, col) = position(source, fn_needle);
        let mut results = Vec::new();
        for apply in [false, true] {
            let result = convert_to_method(
                addr,
                &fixture.workspace.root,
                &file,
                line,
                col,
                apply,
                false,
            )
            .await;
            artifact(
                label,
                &format!("public-{apply}.txt"),
                &format!("{result:#?}\n"),
            );
            if let Ok(plan) = &result {
                artifact(label, &format!("returned-{apply}.rs"), &plan.rewritten[0].1);
                assert_eq!(
                    runtime(label, &format!("returned-{apply}"), &plan.rewritten[0].1),
                    before
                );
            }
            results.push((apply, result, std::fs::read_to_string(&file).unwrap()));
        }
        stop.send(true).unwrap();
        results
    };
    let (results, answers) = tokio::join!(client, bridge.run(&fixture, stopped));
    record_answers(label, &answers);
    (results, answers)
}

#[tokio::test]
async fn generic_owner_holder_config_converts_calls_with_distinct_identities() {
    let (results, answers) = run_positive_case(
        "holder-config",
        HOLDER_CONFIG,
        ("struct Holder", 6),
        ("impl Holder<Config>", 4),
        "f(h:",
        "7 41\n",
    )
    .await;
    let mut returned = None;
    for (apply, result, disk) in results {
        let plan = result.expect("generic owner Holder<Config> conversion must succeed");
        assert_eq!(plan.applied, apply);
        assert_eq!(plan.renamed_uses, 1);
        assert_eq!(plan.rewritten_calls, 2, "{plan:#?}");
        assert!(plan.diagnostics.is_empty(), "{plan:#?}");
        assert!(plan.unmatched.is_empty(), "{plan:#?}");
        assert!(plan.rewritten[0].1.contains("a.f()"));
        assert!(plan.rewritten[0].1.contains("b.f()"));
        assert_eq!(
            disk,
            if apply {
                plan.rewritten[0].1.as_str()
            } else {
                HOLDER_CONFIG
            }
        );
        if let Some(preview) = &returned {
            assert_eq!(preview, &plan.rewritten);
        }
        returned = Some(plan.rewritten.clone());
        dispatch_identity(&answers, &plan.rewritten[0].1, 2);
    }
    require_methods(
        &answers,
        &[
            "textDocument/references",
            "textDocument/typeDefinition",
            "textDocument/definition",
            "textDocument/hover",
        ],
    );
}

#[tokio::test]
async fn generic_owner_holder_fallbacks_preserve_coerced_and_shadowed_calls() {
    let (results, answers) = run_positive_case(
        "holder-fallbacks",
        HOLDER_FALLBACKS,
        ("pub struct Holder", 10),
        ("impl Holder<Config>", 4),
        "f(h:",
        "7 11 21\n",
    )
    .await;
    for (apply, result, disk) in results {
        let plan = result.expect("generic owner fallback conversion must succeed");
        assert_eq!(plan.applied, apply);
        assert_eq!(plan.renamed_uses, 1);
        assert_eq!(plan.rewritten_calls, 1, "{plan:#?}");
        assert_eq!(plan.unchanged.len(), 2, "{plan:#?}");
        assert!(plan.rewritten[0].1.contains("exact.f()"));
        assert!(plan.rewritten[0].1.contains("crate::Holder::f(&bad_head)"));
        assert!(plan.rewritten[0].1.contains("crate::Holder::f(&bad_arg)"));
        assert_eq!(
            disk,
            if apply {
                plan.rewritten[0].1.as_str()
            } else {
                HOLDER_FALLBACKS
            }
        );
        dispatch_identity(&answers, &plan.rewritten[0].1, 1);
    }
}

async fn assert_const_shadow_refused(label: &str, source: &str, needle: &str, expected_out: &str) {
    artifact(label, "original.rs", source);
    assert_eq!(runtime(label, "original", source), expected_out);
    let fixture = Fixture::new(source, "2021").await;
    let bridge = Bridge::bind().await;
    let addr = bridge.addr;
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let client = async {
        observe_probes(label, &fixture, source, "struct S", 6, "impl S", 4).await;
        let file = fixture.workspace.root.join("src/lib.rs");
        let (line, col) = position(source, needle);
        let mut results = Vec::new();
        for apply in [false, true] {
            for force in [false, true] {
                let result = convert_to_method(
                    addr,
                    &fixture.workspace.root,
                    &file,
                    line,
                    col,
                    apply,
                    force,
                )
                .await;
                artifact(
                    label,
                    &format!("public-{apply}-{force}.txt"),
                    &format!("{result:#?}\n"),
                );
                results.push((result, std::fs::read(&file).unwrap()));
            }
        }
        stop.send(true).unwrap();
        results
    };
    let (results, answers) = tokio::join!(client, bridge.run(&fixture, stopped));
    record_answers(label, &answers);
    for (result, bytes) in results {
        assert_eq!(bytes, source.as_bytes());
        let error = result
            .expect_err("method const generic shadowing impl self type must refuse")
            .to_string();
        assert!(error.contains("shadows the impl self type"), "{error}");
    }
}

#[tokio::test]
async fn method_const_generic_shadows_refuse_before_writes_including_force() {
    assert_const_shadow_refused("const-shadow", CONST_SHADOW, "f<const N:", "7\n").await;
    assert_const_shadow_refused(
        "const-macro-shadow",
        CONST_MACRO_SHADOW,
        "f<const N:",
        "7\n",
    )
    .await;
}

#[tokio::test]
async fn unrelated_method_const_generic_with_macro_expansion_converts_and_preserves_output() {
    let (results, answers) = run_positive_case(
        "const-positive",
        CONST_POSITIVE,
        ("struct S", 6),
        ("impl<const N: usize> S", 20),
        "f<const M:",
        "9\n",
    )
    .await;
    for (apply, result, disk) in results {
        let plan = result.expect("unrelated const generic with macro expansion must succeed");
        assert_eq!(plan.applied, apply);
        assert_eq!(plan.renamed_uses, 2);
        assert!(plan.rewritten[0].1.contains("fn f<const M: usize>(&self)"));
        assert!(plan.rewritten[0].1.contains("self.0[0] + self.1[0] + M"));
        assert_eq!(
            disk,
            if apply {
                plan.rewritten[0].1.as_str()
            } else {
                CONST_POSITIVE
            }
        );
    }
    require_methods(&answers, &["rust-analyzer/expandMacro"]);
}
