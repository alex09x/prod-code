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
    assert_untouched, baseline_mode, compile_and_run, record_evidence, run_public_extract,
    scripted_gateway, workspace,
};
use prod_code_testkit::answers;
use serde_json::json;

pub async fn positive_controls_and_uncertainty_guards() {
    // 1. Nested block comment positive control (review-comment:4081920888).
    let nested_comment_src = concat!(
        "pub struct Rect;\n",
        "impl Rect {\n",
        "    pub fn area(&self) -> u32 { /* unmatched } /* nested ] */ in this explanation */ 1 }\n",
        "}\n",
        "fn main() {\n",
        "    println!(\"{}\", Rect.area());\n",
        "}\n",
    );
    let before_comment =
        compile_and_run(&[("src/main.rs", nested_comment_src)], "src/main.rs", &[]);
    assert_eq!(before_comment["stdout"], "1\n");
    let gw = scripted_gateway(json!([]), answers::no_diagnostics()).await;
    let ws = workspace(&[("src/main.rs", nested_comment_src)]);
    let plan = run_public_extract(
        gw.addr(),
        &ws,
        "src/main.rs",
        2,
        1,
        &["area"],
        "Measure",
        false,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(plan.applied);
    let after_comment_src = ws.read("src/main.rs");
    let after_comment = compile_and_run(&[("src/main.rs", &after_comment_src)], "src/main.rs", &[]);
    assert_eq!(after_comment["stdout"], before_comment["stdout"]);

    // 2. Ordinary lifetime/type/const generics + where clause + retained and full extraction control.
    let window_src = concat!(
        "pub struct Window<'a, T, const N: usize> {\n",
        "    pub values: &'a [T; N],\n",
        "}\n",
        "impl<'a, T: Copy + Into<i64>, const N: usize> Window<'a, T, N>\n",
        "where\n",
        "    T: std::fmt::Display,\n",
        "{\n",
        "    pub fn first(&self) -> T {\n",
        "        self.values[0]\n",
        "    }\n",
        "    pub fn total<I>(&self, extra: I) -> i64\n",
        "    where\n",
        "        I: IntoIterator<Item = T>,\n",
        "    {\n",
        "        self.values[0].into() + extra.into_iter().map(Into::into).sum::<i64>()\n",
        "    }\n",
        "    pub fn copied(&self) -> Self {\n",
        "        Self { values: self.values }\n",
        "    }\n",
        "}\n",
        "fn main() {\n",
        "    let values = [10_i32, 20, 30];\n",
        "    let w = Window { values: &values };\n",
        "    println!(\"{}\", w.total([4_i32]) + i64::from(w.copied().first()));\n",
        "}\n",
    );
    let before_window = compile_and_run(&[("src/main.rs", window_src)], "src/main.rs", &[]);
    assert_eq!(before_window["stdout"], "24\n");
    let ws_win = workspace(&[("src/main.rs", window_src)]);
    let plan_win = run_public_extract(
        gw.addr(),
        &ws_win,
        "src/main.rs",
        4,
        1,
        &["total", "copied"],
        "WindowOps",
        false,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(plan_win.applied);
    assert_eq!(plan_win.kept, vec!["first"]);
    let after_win_src = ws_win.read("src/main.rs");
    let after_window = compile_and_run(&[("src/main.rs", &after_win_src)], "src/main.rs", &[]);
    assert_eq!(after_window["stdout"], before_window["stdout"]);

    record_evidence(json!({
        "finding_id": "review-comment:4081920888-and-controls",
        "evidence_separation": "LSP references/diagnostics are scripted fixtures; compile_and_run invokes real rustc and executes the compiled binary.",
        "nested_comment_before": before_comment,
        "nested_comment_after": after_comment,
        "generic_window_before": before_window,
        "generic_window_after": after_window,
    }));
    if baseline_mode() {
        return;
    }

    // 3. Scope-specific uncertainty guards remain pre-write refusals (even with force=true).
    for (guard_src, line, expected_err) in [
        (
            "pub struct Example<T>(T);\nimpl<T: From<Self>> Example<T> {\n    pub fn selected(self) -> T { T::from(self) }\n}\n",
            2,
            "depend on `Self`",
        ),
        (
            "pub struct Example<T>(T);\nimpl<T> Example<T> where T: From<Self> {\n    pub fn selected(self) -> T { T::from(self) }\n}\n",
            2,
            "depend on `Self`",
        ),
        (
            "pub struct Example;\nimpl Example {\n    pub fn selected(&self) -> impl Copy { 7_u8 }\n}\n",
            2,
            "opaque return",
        ),
        (
            "pub struct Example;\n#[allow(dead_code)]\nimpl Example {\n    pub fn selected(&self) {}\n}\n",
            3,
            "attributes on impl blocks",
        ),
    ] {
        for (apply, force) in [(false, false), (false, true), (true, false), (true, true)] {
            let files = [("src/lib.rs", guard_src)];
            let ws = workspace(&files);
            let err = run_public_extract(
                gw.addr(),
                &ws,
                "src/lib.rs",
                line,
                1,
                &["selected"],
                "Selected",
                false,
                apply,
                force,
            )
            .await
            .expect_err(guard_src);
            assert!(
                format!("{err:#}").contains(expected_err),
                "{guard_src}: {err:#}"
            );
            assert_untouched(&ws, &files);
        }
    }
}
