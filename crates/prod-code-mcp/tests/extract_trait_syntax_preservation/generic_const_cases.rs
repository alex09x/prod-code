/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::{
    assert_untouched, baseline_mode, compile_and_run, compile_lib, record_evidence,
    run_public_extract, rustc_diagnostic_gateway, scripted_gateway, workspace,
};
use prod_code_testkit::answers;
use serde_json::json;

pub async fn const_comparison_and_negation_preservation() {
    // Exact original fixture for review-comment:4114339898.
    let orig_4114339898 = concat!(
        "use std::marker::PhantomData;\n",
        "trait Marker<const B: bool> {}\n",
        "impl Marker<true> for () {}\n",
        "struct Foo<T>(PhantomData<T>);\n",
        "impl<T: Marker<{ 1 > 0 }>> Foo<T> {\n",
        "    fn value(&self) -> u8 { 1 }\n",
        "}\n",
    );
    let orig_cmp_rustc = compile_lib(&[("src/lib.rs", orig_4114339898)], "src/lib.rs", &[]);
    assert_eq!(orig_cmp_rustc["exit"], 0, "{orig_cmp_rustc}");

    let mut cmp_outcomes = Vec::new();
    for (apply, force) in [(false, false), (false, true), (true, false), (true, true)] {
        let files = [("src/lib.rs", orig_4114339898)];
        let ws = workspace(&files);
        let gw = rustc_diagnostic_gateway(&files, "src/lib.rs", json!([]), &[]).await;
        let res = run_public_extract(
            gw.addr(),
            &ws,
            "src/lib.rs",
            5,
            1,
            &["value"],
            "Value",
            false,
            apply,
            force,
        )
        .await;
        match res {
            Ok(plan) => {
                if !apply {
                    assert_untouched(&ws, &files);
                }
                let proposed = plan.rewritten[0].1.clone();
                let check = compile_lib(&[("src/lib.rs", &proposed)], "src/lib.rs", &[]);
                cmp_outcomes.push(json!({
                    "apply": apply,
                    "force": force,
                    "status": "ok",
                    "applied": plan.applied,
                    "type_name": plan.type_name,
                    "diagnostics": plan.diagnostics,
                    "proposed_source": proposed,
                    "rustc_check": check,
                }));
            }
            Err(err) => {
                assert_untouched(&ws, &files);
                cmp_outcomes.push(json!({
                    "apply": apply,
                    "force": force,
                    "status": "err",
                    "error": format!("{err:#}"),
                }));
            }
        }
    }

    // Exact original fixture for review-comment:4114339902.
    let orig_4114339902 = concat!(
        "use std::marker::PhantomData;\n",
        "trait Marker<const B: bool> {}\n",
        "impl Marker<true> for () {}\n",
        "struct Foo<T>(PhantomData<T>);\n",
        "impl<T: Marker<{ !false }>> Foo<T> {\n",
        "    fn value(&self) -> u8 { 1 }\n",
        "}\n",
    );
    let orig_neg_rustc = compile_lib(&[("src/lib.rs", orig_4114339902)], "src/lib.rs", &[]);
    assert_eq!(orig_neg_rustc["exit"], 0, "{orig_neg_rustc}");

    let mut neg_outcomes = Vec::new();
    for (apply, force) in [(false, false), (false, true), (true, false), (true, true)] {
        let files = [("src/lib.rs", orig_4114339902)];
        let ws = workspace(&files);
        let gw = rustc_diagnostic_gateway(&files, "src/lib.rs", json!([]), &[]).await;
        let res = run_public_extract(
            gw.addr(),
            &ws,
            "src/lib.rs",
            5,
            1,
            &["value"],
            "Value",
            false,
            apply,
            force,
        )
        .await;
        match res {
            Ok(plan) => {
                if !apply {
                    assert_untouched(&ws, &files);
                }
                let proposed = plan.rewritten[0].1.clone();
                let check = compile_lib(&[("src/lib.rs", &proposed)], "src/lib.rs", &[]);
                neg_outcomes.push(json!({
                    "apply": apply,
                    "force": force,
                    "status": "ok",
                    "applied": plan.applied,
                    "type_name": plan.type_name,
                    "diagnostics": plan.diagnostics,
                    "proposed_source": proposed,
                    "rustc_check": check,
                }));
            }
            Err(err) => {
                assert_untouched(&ws, &files);
                neg_outcomes.push(json!({
                    "apply": apply,
                    "force": force,
                    "status": "err",
                    "error": format!("{err:#}"),
                }));
            }
        }
    }

    record_evidence(json!({
        "finding_id": "review-comment:4114339898",
        "evidence_separation": "LSP references are scripted empty fixtures; diagnostic validation and rustc_check run real rustc.",
        "original_source": orig_4114339898,
        "original_rustc": orig_cmp_rustc,
        "public_extract_outcomes": cmp_outcomes,
    }));
    record_evidence(json!({
        "finding_id": "review-comment:4114339902",
        "evidence_separation": "LSP references are scripted empty fixtures; diagnostic validation and rustc_check run real rustc.",
        "original_source": orig_4114339902,
        "original_rustc": orig_neg_rustc,
        "public_extract_outcomes": neg_outcomes,
    }));
    if baseline_mode() {
        return;
    }

    for item in &cmp_outcomes {
        assert_eq!(item["status"], "ok", "{item}");
        assert_eq!(item["type_name"], "Foo<T>", "{item}");
        assert!(item["diagnostics"].as_array().unwrap().is_empty(), "{item}");
        assert_eq!(item["rustc_check"]["exit"], 0, "{item}");
        let proposed = item["proposed_source"].as_str().unwrap();
        assert!(proposed.contains("trait Value<T: Marker<{ 1 > 0 }>>"));
        assert!(proposed.contains("impl<T: Marker<{ 1 > 0 }>> Value<T> for Foo<T>"));
    }

    for item in &neg_outcomes {
        assert_eq!(item["status"], "ok", "{item}");
        assert_eq!(item["type_name"], "Foo<T>", "{item}");
        assert!(item["diagnostics"].as_array().unwrap().is_empty(), "{item}");
        assert_eq!(item["rustc_check"]["exit"], 0, "{item}");
        let proposed = item["proposed_source"].as_str().unwrap();
        assert!(proposed.contains("trait Value<T: Marker<{ !false }>>"));
        assert!(proposed.contains("impl<T: Marker<{ !false }>> Value<T> for Foo<T>"));
    }

    // Executable runtime checks for <, >, <=, >=, <<, >>, !, !=, !(...) in generic bounds, where clauses, and method generics.
    let gw = scripted_gateway(json!([]), answers::no_diagnostics()).await;
    for header in [
        "impl<T: Marker<{ 1 > 0 }>> Foo<T>",
        "impl<T: Marker<{ 1 < 2 }>> Foo<T>",
        "impl<T: Marker<{ 1 >= 0 && 0 <= 1 }>> Foo<T>",
        "impl<T: Marker<{ (1 << 2) > (2 >> 1) }>> Foo<T>",
        "impl<T: Marker<{ !false }>> Foo<T>",
        "impl<T: Marker<{ !(false) && !{ false } && (1 != 0) }>> Foo<T>",
        "impl<T> Foo<T> where T: Marker<{ 1 > 0 }>, (): Marker<{ 1 < 2 && !false }>",
    ] {
        let src = format!(
            "use std::marker::PhantomData;\ntrait Marker<const B: bool> {{}}\nimpl Marker<true> for () {{}}\nstruct Foo<T>(PhantomData<T>);\n{header} {{\n    fn value<U: Marker<{{ 1 > 0 && 1 < 2 && !false }}>>(&self) -> u8 where U: Marker<{{ 2 > 1 }}> {{ 7 }}\n    fn kept(&self) -> u8 {{ 3 }}\n}}\nfn main() {{\n    let f = Foo::<()>(PhantomData);\n    println!(\"{{}}:{{}}\", f.value::<()>(), f.kept());\n}}\n"
        );
        let before = compile_and_run(&[("src/main.rs", &src)], "src/main.rs", &[]);
        assert_eq!(before["stdout"], "7:3\n", "{header}: {before}");
        let ws = workspace(&[("src/main.rs", &src)]);
        let plan = run_public_extract(
            gw.addr(),
            &ws,
            "src/main.rs",
            5,
            1,
            &["value"],
            "Value",
            false,
            true,
            false,
        )
        .await
        .expect(header);
        assert!(plan.applied);
        let after_src = ws.read("src/main.rs");
        let after = compile_and_run(&[("src/main.rs", &after_src)], "src/main.rs", &[]);
        assert_eq!(after["stdout"], before["stdout"], "{header}:\n{after_src}");
    }

    // Actual unsupported macro invocations remain refused pre-write across apply and force.
    for (macro_src, line, expected) in [
        (
            "macro_rules! ty { ($t:ty) => { $t } }\nstruct Name<T>(T);\nimpl<T> Name<ty!{T}> {\n    fn value(&self) {}\n}\n",
            3,
            "macros in an impl header",
        ),
        (
            "macro_rules! bound { () => { Copy } }\nstruct Name<T>(T);\nimpl<T: bound!()> Name<T> {\n    fn value(&self) {}\n}\n",
            3,
            "macros in an impl header",
        ),
        (
            "trait Marker<const B: bool> {}\nstruct Name<T>(T);\nimpl<T: Marker<{ cfg!( unix ) }>> Name<T> {\n    fn value(&self) {}\n}\n",
            3,
            "macros in an impl header",
        ),
        (
            "struct Name;\nmacro_rules! make { ($i:item) => { $i } }\nmake! { impl Name { fn value(&self) {} } }\n",
            3,
            "generated inside macros",
        ),
    ] {
        for (apply, force) in [(false, false), (false, true), (true, false), (true, true)] {
            let files = [("src/lib.rs", macro_src)];
            let ws = workspace(&files);
            let err = run_public_extract(
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
            .await
            .expect_err(macro_src);
            assert!(
                format!("{err:#}").contains(expected),
                "{macro_src}: {err:#}"
            );
            assert_untouched(&ws, &files);
        }
    }
}
