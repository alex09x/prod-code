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
use serde_json::json;

pub async fn recovered_parser_regressions() {
    let fixtures = [
        (
            "digit_macro",
            "struct S;\nmacro_rules! make1 { ($i:item) => { $i }; }\nmake1! { impl S { fn value(&self) {} } }\n",
            3,
            true,
        ),
        (
            "comment_visibility",
            "struct S;\nimpl S { pub(crate /* scope */) fn value(&self) {} }\n",
            2,
            false,
        ),
        (
            "array_comparison",
            "use std::marker::PhantomData;\ntrait Marker<A> {}\nimpl Marker<[u8; 1]> for () {}\nstruct Foo<T>(PhantomData<T>);\nimpl<T: Marker<[u8; (1 > 0) as usize]>> Foo<T> { fn value(&self) -> u8 { 1 } }\n",
            5,
            false,
        ),
    ];
    let mut results = Vec::new();
    for (name, source, line, refuse) in fixtures {
        let original = compile_lib(&[("src/lib.rs", source)], "src/lib.rs", &[]);
        assert_eq!(original["exit"], 0, "{name}: {original}");
        let mut matrix = Vec::new();
        for (apply, force) in [(false, false), (false, true), (true, false), (true, true)] {
            let files = [("src/lib.rs", source)];
            let ws = workspace(&files);
            let gw = rustc_diagnostic_gateway(&files, "src/lib.rs", json!([]), &[]).await;
            let result = run_public_extract(
                gw.addr(),
                &ws,
                "src/lib.rs",
                line,
                20,
                &["value"],
                "Value",
                false,
                apply,
                force,
            )
            .await;
            let outcome = match result {
                Ok(plan) => {
                    if !apply {
                        assert_untouched(&ws, &files);
                    }
                    let generated = &plan.rewritten[0].1;
                    json!({"apply": apply, "force": force, "status": "ok",
                        "applied": plan.applied, "diagnostics": plan.diagnostics,
                        "generated_source": generated, "disk_source": ws.read("src/lib.rs"),
                        "compiler": compile_lib(&[("src/lib.rs", generated)], "src/lib.rs", &[])})
                }
                Err(error) => {
                    assert_untouched(&ws, &files);
                    json!({"apply": apply, "force": force, "status": "err",
                        "error": format!("{error:#}"), "disk_source": ws.read("src/lib.rs")})
                }
            };
            matrix.push(outcome);
        }
        record_evidence(json!({"fixture": name, "original_source": source,
            "original_compiler": original, "matrix": matrix,
            "transport": "scripted references; diagnostic callback and compiler fields invoke real rustc"}));
        results.push((name, refuse, matrix));
    }
    if baseline_mode() {
        return;
    }
    for (name, refuse, matrix) in results {
        for outcome in matrix {
            if refuse {
                assert_eq!(outcome["status"], "err", "{name}: {outcome}");
                assert!(
                    outcome["error"]
                        .as_str()
                        .unwrap()
                        .contains("generated inside macros"),
                    "{outcome}"
                );
            } else {
                assert_eq!(outcome["status"], "ok", "{name}: {outcome}");
                assert_eq!(outcome["compiler"]["exit"], 0, "{outcome}");
                assert_eq!(outcome["diagnostics"], json!([]), "{outcome}");
                assert_eq!(outcome["applied"], outcome["apply"], "{outcome}");
                let (inherent, implementation) = match name {
                    "comment_visibility" => ("impl S {", "impl Value for S"),
                    "array_comparison" => (
                        "impl<T: Marker<[u8; (1 > 0) as usize]>> Foo<T> {",
                        "Value<T> for Foo<T>",
                    ),
                    _ => unreachable!(),
                };
                assert_value_extracted(
                    outcome["generated_source"].as_str().unwrap(),
                    inherent,
                    implementation,
                );
            }
        }
    }
    nested_type_and_visibility_controls().await;
}

fn assert_value_extracted(generated: &str, inherent: &str, implementation: &str) {
    assert!(generated.contains("trait Value"), "{generated}");
    assert!(generated.contains(implementation), "{generated}");
    // These fixtures select the only inherent method, so that impl must vanish.
    assert!(!generated.contains(inherent), "{generated}");
    assert_eq!(generated.matches("fn value").count(), 2, "{generated}");
}

async fn nested_type_and_visibility_controls() {
    let source = concat!(
        "use std::marker::PhantomData;\n",
        "trait Marker<A> {}\nimpl<A> Marker<A> for () {}\n",
        "struct Foo<T>(PhantomData<T>);\n",
        "impl<T: Marker<([Vec<(u8, u16)>; (1 > 0) as usize], Option<Vec<u8>>)>, const N: usize> Foo<(T, [u8; N])> where T: Marker<[u8; (1 < 2) as usize]> {\n",
        "    fn value<U: Marker<[[Vec<u8>; (1 >= 0) as usize]; (!0u8 > 0) as usize]>>(&self) -> ([Vec<(u8, u16)>; (1 > 0) as usize], Option<Vec<u8>>) where U: Marker<[u8; (1 << 1 == 2) as usize]> { ([vec![(7, 3)]], None) }\n",
        "}\n",
        "fn main() { let f = Foo::<((), [u8; 2])>(PhantomData); println!(\"{}\", f.value::<()>().0[0][0].0); }\n",
    );
    let before = compile_and_run(&[("src/main.rs", source)], "src/main.rs", &[]);
    assert_eq!(before["compiler_exit"], 0, "{before}");
    assert_eq!(before["runtime_exit"], 0, "{before}");
    assert_eq!(before["stdout"], "7\n", "{before}");
    for (apply, force) in [(false, false), (false, true), (true, false), (true, true)] {
        let files = [("src/main.rs", source)];
        let ws = workspace(&files);
        let gw = rustc_diagnostic_gateway(&files, "src/main.rs", json!([]), &[]).await;
        let plan = run_public_extract(
            gw.addr(),
            &ws,
            "src/main.rs",
            5,
            1,
            &["value"],
            "Value",
            false,
            apply,
            force,
        )
        .await
        .unwrap();
        if !apply {
            assert_untouched(&ws, &files);
        }
        assert!(plan.diagnostics.is_empty(), "{plan:?}");
        let generated = &plan.rewritten[0].1;
        assert_value_extracted(
            generated,
            source.lines().nth(4).unwrap(),
            "Value<T, N> for Foo<(T, [u8; N])>",
        );
        // A trait-qualified call cannot resolve to the original inherent method.
        let qualified = generated.replace(
            "f.value::<()>()",
            "<Foo<((), [u8; 2])> as Value<(), 2>>::value::<()>(&f)",
        );
        assert_ne!(&qualified, generated);
        let after = compile_and_run(&[("src/main.rs", &qualified)], "src/main.rs", &[]);
        record_evidence(
            json!({"fixture": "nested_array_trait_qualified", "apply": apply,
            "force": force, "applied": plan.applied, "original_source": source,
            "generated_source": generated, "trait_qualified_source": qualified,
            "disk_source": ws.read("src/main.rs"), "before": before, "after": after}),
        );
        assert_eq!(plan.applied, apply);
        assert_eq!(after["compiler_exit"], 0, "{after}");
        assert_eq!(after["runtime_exit"], 0, "{after}");
        assert_eq!(after["stdout"], before["stdout"], "{after}");
    }

    // An external caller exercises access after joining scopes, in both source orders.
    for wider in [
        "pub(crate /* outer /* nested */ ) */)",
        "pub(in/* nested /* ) */ end */crate)",
    ] {
        for reverse in [false, true] {
            let narrow = "pub(super /* outer /* nested */ ) */) fn local(&self) {}";
            let wide = format!("{wider} fn wide(&self) {{}}");
            let methods = if reverse {
                format!("{wide}\n{narrow}")
            } else {
                format!("{narrow}\n{wide}")
            };
            let inner = format!("pub struct S;\nimpl S {{\n{methods}\n}}\n");
            let files = [
                ("src/lib.rs", "pub mod outer;\nmod client;\n"),
                ("src/outer/mod.rs", "pub mod inner;\n"),
                ("src/outer/inner.rs", inner.as_str()),
                (
                    "src/client.rs",
                    "use crate::outer::inner::S;\npub fn run() { S.wide(); }\n",
                ),
            ];
            assert_eq!(compile_lib(&files, "src/lib.rs", &[])["exit"], 0);
            for (apply, force) in [(false, false), (false, true), (true, false), (true, true)] {
                let ws = workspace(&files);
                let refs =
                    prod_code_testkit::answers::locations(&ws.path("src/client.rs"), &[(2, 18)]);
                let gw = rustc_diagnostic_gateway(&files, "src/lib.rs", refs, &[]).await;
                let plan = run_public_extract(
                    gw.addr(),
                    &ws,
                    "src/outer/inner.rs",
                    2,
                    1,
                    &["local", "wide"],
                    "Value",
                    false,
                    apply,
                    force,
                )
                .await
                .unwrap();
                if !apply {
                    assert_untouched(&ws, &files);
                }
                assert!(plan.diagnostics.is_empty(), "{plan:?}");
                let mut generated = files
                    .iter()
                    .map(|(p, s)| (p.to_string(), s.to_string()))
                    .collect::<Vec<_>>();
                for (path, source) in &plan.rewritten {
                    let entry = generated
                        .iter_mut()
                        .find(|(p, _)| path.ends_with(p.as_str()))
                        .unwrap();
                    entry.1 = source.clone();
                }
                let pairs = generated
                    .iter()
                    .map(|(p, s)| (p.as_str(), s.as_str()))
                    .collect::<Vec<_>>();
                assert_eq!(compile_lib(&pairs, "src/lib.rs", &[])["exit"], 0);
                assert!(generated[2].1.contains("pub(crate) trait Value"));
                assert!(!generated[2].1.contains("crate::crate"));
            }
        }
    }

    let source = "macro_rules! ty1 { ($t:ty) => { $t } }\nstruct Name<T>(T);\nimpl<T> Name<ty1!{T}> { fn value(&self) {} }\n";
    let files = [("src/lib.rs", source)];
    assert_eq!(compile_lib(&files, "src/lib.rs", &[])["exit"], 0);
    let gw = scripted_gateway(json!([]), prod_code_testkit::answers::no_diagnostics()).await;
    for (apply, force) in [(false, false), (false, true), (true, false), (true, true)] {
        let ws = workspace(&files);
        let err = run_public_extract(
            gw.addr(),
            &ws,
            "src/lib.rs",
            3,
            1,
            &["value"],
            "Value",
            false,
            apply,
            force,
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("macros in an impl header"));
        assert_untouched(&ws, &files);
    }
}
