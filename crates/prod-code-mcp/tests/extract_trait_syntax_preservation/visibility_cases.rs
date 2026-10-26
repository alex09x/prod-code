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
use prod_code_mcp::extract_trait::{impl_block, rewrite};
use prod_code_testkit::answers;
use serde_json::json;

pub async fn widest_visibility_preservation() {
    // Exact original fixture 1 for review-comment:4081920876 (no external caller, order dependence).
    let inner_local_first = concat!(
        "pub struct S;\n",
        "impl S {\n",
        "    pub(super) fn local(&self) {}\n",
        "    pub(crate) fn wide(&self) {}\n",
        "}\n",
    );
    let inner_wide_first = concat!(
        "pub struct S;\n",
        "impl S {\n",
        "    pub(crate) fn wide(&self) {}\n",
        "    pub(super) fn local(&self) {}\n",
        "}\n",
    );
    let files_no_caller = [
        ("src/lib.rs", "pub mod outer;\n"),
        ("src/outer/mod.rs", "pub mod inner;\n"),
        ("src/outer/inner.rs", inner_local_first),
    ];
    let gw_empty = scripted_gateway(json!([]), answers::no_diagnostics()).await;
    let ws_a = workspace(&files_no_caller);
    let plan_a = run_public_extract(
        gw_empty.addr(),
        &ws_a,
        "src/outer/inner.rs",
        2,
        1,
        &["local", "wide"],
        "Api",
        false,
        false,
        false,
    )
    .await
    .unwrap();
    let ws_b = workspace(&[
        ("src/lib.rs", "pub mod outer;\n"),
        ("src/outer/mod.rs", "pub mod inner;\n"),
        ("src/outer/inner.rs", inner_wide_first),
    ]);
    let plan_b = run_public_extract(
        gw_empty.addr(),
        &ws_b,
        "src/outer/inner.rs",
        2,
        1,
        &["local", "wide"],
        "Api",
        false,
        false,
        false,
    )
    .await
    .unwrap();

    // Exact original fixture 2 for review-comment:4081920876 (with sibling caller in src/client.rs).
    let files_with_caller = [
        ("src/lib.rs", "pub mod outer;\nmod client;\n"),
        ("src/outer/mod.rs", "pub mod inner;\n"),
        ("src/outer/inner.rs", inner_local_first),
        (
            "src/client.rs",
            "use crate::outer::inner::S;\npub fn run() { S.wide(); }\n",
        ),
    ];
    let orig_caller_check = compile_lib(&files_with_caller, "src/lib.rs", &[]);
    assert_eq!(orig_caller_check["exit"], 0, "{orig_caller_check}");

    let mut caller_outcomes = Vec::new();
    for (apply, force) in [(false, false), (false, true), (true, false), (true, true)] {
        let ws = workspace(&files_with_caller);
        let refs = answers::locations(&ws.path("src/client.rs"), &[(2, 18)]);
        let gw = rustc_diagnostic_gateway(&files_with_caller, "src/lib.rs", refs, &[]).await;
        let res = run_public_extract(
            gw.addr(),
            &ws,
            "src/outer/inner.rs",
            2,
            1,
            &["local", "wide"],
            "Api",
            false,
            apply,
            force,
        )
        .await;
        match res {
            Ok(plan) => {
                if !apply {
                    assert_untouched(&ws, &files_with_caller);
                }
                let inner_after = plan
                    .rewritten
                    .iter()
                    .find(|(p, _)| p.ends_with("src/outer/inner.rs"))
                    .map(|(_, t)| t.clone())
                    .unwrap_or_default();
                let client_after = plan
                    .rewritten
                    .iter()
                    .find(|(p, _)| p.ends_with("src/client.rs"))
                    .map(|(_, t)| t.clone())
                    .unwrap_or_default();
                let check = compile_lib(
                    &[
                        ("src/lib.rs", "pub mod outer;\nmod client;\n"),
                        ("src/outer/mod.rs", "pub mod inner;\n"),
                        ("src/outer/inner.rs", &inner_after),
                        ("src/client.rs", &client_after),
                    ],
                    "src/lib.rs",
                    &[],
                );
                caller_outcomes.push(json!({
                    "apply": apply,
                    "force": force,
                    "status": "ok",
                    "applied": plan.applied,
                    "diagnostics": plan.diagnostics,
                    "inner_proposed": inner_after,
                    "client_proposed": client_after,
                    "rustc_check": check,
                }));
            }
            Err(err) => {
                assert_untouched(&ws, &files_with_caller);
                caller_outcomes.push(json!({
                    "apply": apply,
                    "force": force,
                    "status": "err",
                    "error": format!("{err:#}"),
                }));
            }
        }
    }

    record_evidence(json!({
        "finding_id": "review-comment:4081920876",
        "evidence_separation": "textDocument/references for src/client.rs is a scripted fixture; diagnostic validation and rustc_check run real rustc on the multi-file crate.",
        "local_first_proposed": plan_a.rewritten[0].1,
        "wide_first_proposed": plan_b.rewritten[0].1,
        "original_caller_rustc": orig_caller_check,
        "caller_outcomes": caller_outcomes,
    }));
    if baseline_mode() {
        return;
    }

    assert!(
        plan_a.rewritten[0].1.contains("pub(crate) trait Api"),
        "{}",
        plan_a.rewritten[0].1
    );
    assert!(
        plan_b.rewritten[0].1.contains("pub(crate) trait Api"),
        "{}",
        plan_b.rewritten[0].1
    );

    for item in &caller_outcomes {
        assert_eq!(item["status"], "ok", "{item}");
        assert!(item["diagnostics"].as_array().unwrap().is_empty(), "{item}");
        assert_eq!(item["rustc_check"]["exit"], 0, "{item}");
    }

    // Complete order-independence matrix across private, pub(self), pub(super), pub(in ...), pub(crate), pub.
    for (vis_a, vis_b, expected_prefix) in [
        ("", "", "trait Api"),
        ("", "pub(self)", "trait Api"),
        ("", "pub(super)", "pub(super) trait Api"),
        ("pub(self)", "pub(super)", "pub(super) trait Api"),
        ("pub(super)", "pub(in super)", "pub(super) trait Api"),
        (
            "pub(super)",
            "pub(in super::super)",
            "pub(in super::super) trait Api",
        ),
        (
            "pub(in crate::outer::inner)",
            "pub(in crate::outer)",
            "pub(in crate::outer) trait Api",
        ),
        ("pub(in crate::outer)", "pub(crate)", "pub(crate) trait Api"),
        ("pub(super)", "pub(crate)", "pub(crate) trait Api"),
        ("pub(in super::super)", "pub(crate)", "pub(crate) trait Api"),
        ("pub(crate)", "pub", "pub trait Api"),
        ("pub(in crate::outer)", "pub", "pub trait Api"),
    ] {
        let fmt_vis = |v: &str| {
            if v.is_empty() {
                String::new()
            } else {
                format!("{v} ")
            }
        };
        let src_ab = format!(
            "pub struct S;\nimpl S {{\n    {}fn a(&self) -> u32 {{ 1 }}\n    {}fn b(&self) -> u32 {{ 2 }}\n}}\n",
            fmt_vis(vis_a),
            fmt_vis(vis_b)
        );
        let src_ba = format!(
            "pub struct S;\nimpl S {{\n    {}fn b(&self) -> u32 {{ 2 }}\n    {}fn a(&self) -> u32 {{ 1 }}\n}}\n",
            fmt_vis(vis_b),
            fmt_vis(vis_a)
        );
        for src in [&src_ab, &src_ba] {
            for methods in [["a".into(), "b".into()], ["b".into(), "a".into()]] {
                let imp = impl_block(src, src.find("impl S").unwrap()).unwrap();
                let (rewritten, _) = rewrite(src, &imp, &methods, "Api").unwrap();
                assert!(
                    rewritten.contains(&format!("\n{expected_prefix} {{"))
                        || rewritten.starts_with(&format!("{expected_prefix} {{")),
                    "for ({vis_a:?}, {vis_b:?}) expected `{expected_prefix}`, got:\n{rewritten}"
                );
            }
        }
    }

    // Executable multi-module runtime test with pub(in crate::outer) vs pub(in crate::outer::inner).
    let exec_vis = concat!(
        "pub mod outer {\n",
        "    pub mod inner {\n",
        "        pub struct S;\n",
        "        impl S {\n",
        "            pub(in crate::outer::inner) fn narrow(&self) -> u32 { 10 }\n",
        "            pub(in crate::outer) fn wider(&self) -> u32 { 20 }\n",
        "        }\n",
        "    }\n",
        "    pub fn call_from_outer() -> u32 {\n",
        "        use inner::*;\n",
        "        inner::S.wider()\n",
        "    }\n",
        "}\n",
        "fn main() {\n",
        "    println!(\"{}\", outer::call_from_outer());\n",
        "}\n",
    );
    let before_run = compile_and_run(&[("src/main.rs", exec_vis)], "src/main.rs", &[]);
    assert_eq!(before_run["stdout"], "20\n");
    let imp = impl_block(exec_vis, exec_vis.find("impl S").unwrap()).unwrap();
    let (after_vis, _) =
        rewrite(exec_vis, &imp, &["narrow".into(), "wider".into()], "Api").unwrap();
    assert!(
        after_vis.contains("pub(in crate::outer) trait Api"),
        "{after_vis}"
    );
    let after_run = compile_and_run(&[("src/main.rs", &after_vis)], "src/main.rs", &[]);
    assert_eq!(after_run["stdout"], "20\n");
}
