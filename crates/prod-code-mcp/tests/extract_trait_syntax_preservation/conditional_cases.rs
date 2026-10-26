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
    run_mcp_tool_extract, run_public_extract, scripted_gateway, workspace,
};
use prod_code_testkit::answers;
use serde_json::json;

pub async fn method_cfg_and_cfg_attr_preservation() {
    // Exact original fixture for review-comment:4081920846.
    let orig_4081920846 = concat!(
        "pub struct Rect;\n",
        "impl Rect {\n",
        "    #[cfg(feature = \"enabled\")]\n",
        "    pub fn area(&self) -> u32 { 1 }\n",
        "}\n",
    );
    let orig_enabled = compile_lib(
        &[("src/lib.rs", orig_4081920846)],
        "src/lib.rs",
        &["feature=\"enabled\""],
    );
    let orig_disabled = compile_lib(&[("src/lib.rs", orig_4081920846)], "src/lib.rs", &[]);
    assert_eq!(orig_enabled["exit"], 0, "{orig_enabled}");
    assert_eq!(orig_disabled["exit"], 0, "{orig_disabled}");

    // Negative compiler control: omitting #[cfg] on the trait method declaration fails with E0046 when disabled.
    let unpreserved_control = concat!(
        "pub struct Rect;\n",
        "pub trait Measure {\n    fn area(&self) -> u32;\n}\n",
        "impl Measure for Rect {\n    #[cfg(feature = \"enabled\")]\n    fn area(&self) -> u32 { 1 }\n}\n",
    );
    let control_disabled = compile_lib(&[("src/lib.rs", unpreserved_control)], "src/lib.rs", &[]);
    assert_ne!(control_disabled["exit"], 0, "{control_disabled}");
    assert!(
        control_disabled["stderr"]
            .as_str()
            .unwrap_or("")
            .contains("E0046"),
        "{control_disabled}"
    );

    let gw = scripted_gateway(json!([]), answers::no_diagnostics()).await;
    let mut outcomes = Vec::new();
    for (apply, force) in [(false, false), (false, true), (true, false), (true, true)] {
        let ws = workspace(&[("src/lib.rs", orig_4081920846)]);
        let res = run_public_extract(
            gw.addr(),
            &ws,
            "src/lib.rs",
            2,
            1,
            &["area"],
            "Measure",
            false,
            apply,
            force,
        )
        .await;
        match res {
            Ok(plan) => {
                if !apply {
                    assert_untouched(&ws, &[("src/lib.rs", orig_4081920846)]);
                }
                let proposed = plan.rewritten[0].1.clone();
                let enabled_after = compile_lib(
                    &[("src/lib.rs", &proposed)],
                    "src/lib.rs",
                    &["feature=\"enabled\""],
                );
                let disabled_after = compile_lib(&[("src/lib.rs", &proposed)], "src/lib.rs", &[]);
                outcomes.push(json!({
                    "apply": apply,
                    "force": force,
                    "status": "ok",
                    "applied": plan.applied,
                    "proposed_source": proposed,
                    "rustc_enabled": enabled_after,
                    "rustc_disabled": disabled_after,
                }));
            }
            Err(err) => {
                assert_untouched(&ws, &[("src/lib.rs", orig_4081920846)]);
                outcomes.push(json!({
                    "apply": apply,
                    "force": force,
                    "status": "err",
                    "error": format!("{err:#}"),
                }));
            }
        }
    }
    record_evidence(json!({
        "finding_id": "review-comment:4081920846",
        "evidence_separation": "LSP references/diagnostics are scripted fixtures; rustc_enabled/rustc_disabled are real compiler executions on the build node.",
        "original_source": orig_4081920846,
        "original_rustc_enabled": orig_enabled,
        "original_rustc_disabled": orig_disabled,
        "e0046_negative_control_disabled": control_disabled,
        "public_extract_outcomes": outcomes,
    }));
    if baseline_mode() {
        return;
    }

    for item in &outcomes {
        assert_eq!(item["status"], "ok", "{item}");
        let proposed = item["proposed_source"].as_str().unwrap();
        assert!(
            proposed.contains(
                "pub trait Measure {\n    #[cfg(feature = \"enabled\")]\n    fn area(&self) -> u32;\n}"
            ),
            "{proposed}"
        );
        assert!(
            proposed.contains(
                "impl Measure for Rect {\n    #[cfg(feature = \"enabled\")]\n    fn area(&self) -> u32 { 1 }\n}"
            ),
            "{proposed}"
        );
        assert_eq!(item["rustc_enabled"]["exit"], 0, "{item}");
        assert_eq!(item["rustc_disabled"]["exit"], 0, "{item}");
    }

    // Executable runtime + multiline cfg_attr + doc comment + #[inline] control.
    let exec_source = concat!(
        "pub struct Rect { pub w: u32 }\n",
        "impl Rect {\n",
        "    /// Compute area when enabled.\n",
        "    #[cfg(feature = \"enabled\")]\n",
        "    #[cfg_attr(\n        feature = \"enabled\",\n        must_use\n    )]\n",
        "    #[inline]\n",
        "    pub fn area(&self) -> u32 { self.w * self.w }\n",
        "    pub fn width(&self) -> u32 { self.w }\n",
        "}\n",
        "fn main() {\n",
        "    let r = Rect { w: 4 };\n",
        "    #[cfg(feature = \"enabled\")]\n",
        "    println!(\"enabled:{}:{}\", r.area(), r.width());\n",
        "    #[cfg(not(feature = \"enabled\"))]\n",
        "    println!(\"disabled:{}\", r.width());\n",
        "}\n",
    );
    let before_on = compile_and_run(
        &[("src/main.rs", exec_source)],
        "src/main.rs",
        &["feature=\"enabled\""],
    );
    let before_off = compile_and_run(&[("src/main.rs", exec_source)], "src/main.rs", &[]);
    assert_eq!(before_on["stdout"], "enabled:16:4\n");
    assert_eq!(before_off["stdout"], "disabled:4\n");

    let ws = workspace(&[("src/main.rs", exec_source)]);
    let tool_out = run_mcp_tool_extract(
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
    assert!(tool_out.contains("[applied]"), "{tool_out}");
    let after_source = ws.read("src/main.rs");
    assert!(
        after_source.contains("pub trait Measure {\n    /// Compute area when enabled.\n    #[cfg(feature = \"enabled\")]\n    #[cfg_attr(\n        feature = \"enabled\",\n        must_use\n    )]\n    fn area(&self) -> u32;\n}"),
        "{after_source}"
    );
    assert!(
        !after_source
            .split("impl Measure")
            .next()
            .unwrap()
            .contains("#[inline]"),
        "#[inline] must stay on the impl method, not the trait declaration:\n{after_source}"
    );
    let after_on = compile_and_run(
        &[("src/main.rs", &after_source)],
        "src/main.rs",
        &["feature=\"enabled\""],
    );
    let after_off = compile_and_run(&[("src/main.rs", &after_source)], "src/main.rs", &[]);
    assert_eq!(after_on["stdout"], before_on["stdout"]);
    assert_eq!(after_off["stdout"], before_off["stdout"]);
}

pub async fn impl_cfg_and_cfg_attr_preservation() {
    // Exact original fixture for review-comment:4081920853.
    let orig_4081920853 = concat!(
        "pub struct Rect;\n",
        "#[cfg(any())]\n",
        "impl Rect {\n",
        "    pub fn area(&self) -> u32 { 1 }\n",
        "    pub fn kept(&self) {}\n",
        "}\n",
    );
    let gw = scripted_gateway(json!([]), answers::no_diagnostics()).await;
    let mut outcomes = Vec::new();
    for (methods, label) in [
        (&["area"][..], "with_retained"),
        (&["area", "kept"][..], "all_extracted"),
    ] {
        for (apply, force) in [(false, false), (false, true), (true, false), (true, true)] {
            let ws = workspace(&[("src/lib.rs", orig_4081920853)]);
            let res = run_public_extract(
                gw.addr(),
                &ws,
                "src/lib.rs",
                3,
                1,
                methods,
                "Measure",
                false,
                apply,
                force,
            )
            .await;
            match res {
                Ok(plan) => {
                    if !apply {
                        assert_untouched(&ws, &[("src/lib.rs", orig_4081920853)]);
                    }
                    let proposed = plan.rewritten[0].1.clone();
                    let check = compile_lib(&[("src/lib.rs", &proposed)], "src/lib.rs", &[]);
                    outcomes.push(json!({
                        "mode": label,
                        "apply": apply,
                        "force": force,
                        "status": "ok",
                        "kept": plan.kept,
                        "proposed_source": proposed,
                        "rustc_check": check,
                    }));
                }
                Err(err) => {
                    assert_untouched(&ws, &[("src/lib.rs", orig_4081920853)]);
                    outcomes.push(json!({
                        "mode": label,
                        "apply": apply,
                        "force": force,
                        "status": "err",
                        "error": format!("{err:#}"),
                    }));
                }
            }
        }
    }
    record_evidence(json!({
        "finding_id": "review-comment:4081920853",
        "evidence_separation": "LSP references/diagnostics are scripted fixtures; rustc_check is a real compiler execution on the build node.",
        "original_source": orig_4081920853,
        "public_extract_outcomes": outcomes,
    }));
    if baseline_mode() {
        return;
    }

    for item in &outcomes {
        assert_eq!(item["status"], "ok", "{item}");
        assert_eq!(item["rustc_check"]["exit"], 0, "{item}");
        let proposed = item["proposed_source"].as_str().unwrap();
        if item["mode"] == "with_retained" {
            assert_eq!(proposed.matches("#[cfg(any())]").count(), 3, "{proposed}");
            assert!(proposed.contains("#[cfg(any())]\nimpl Rect {\n    pub fn kept(&self) {}\n}"));
        } else {
            assert_eq!(proposed.matches("#[cfg(any())]").count(), 2, "{proposed}");
            assert!(!proposed.contains("impl Rect {"));
        }
        assert!(proposed.contains("#[cfg(any())]\npub trait Measure {"));
        assert!(proposed.contains("#[cfg(any())]\nimpl Measure for Rect {"));
    }

    // Feature-gated type + impl: when feature="enabled" is disabled, Rect does not exist!
    // Unconditional trait/impl blocks would fail to compile when disabled.
    let gated_exec = concat!(
        "#[cfg(feature = \"enabled\")]\n",
        "pub struct Rect(pub u32);\n",
        "#[cfg(feature = \"enabled\")]\n",
        "#[cfg_attr(feature = \"enabled\", allow(dead_code))]\n",
        "impl Rect {\n",
        "    pub fn area(&self) -> u32 { self.0 * self.0 }\n",
        "    pub fn kept(&self) -> u32 { self.0 }\n",
        "}\n",
        "fn main() {\n",
        "    #[cfg(feature = \"enabled\")]\n",
        "    {\n",
        "        let r = Rect(5);\n",
        "        println!(\"on:{}:{}\", r.area(), r.kept());\n",
        "    }\n",
        "    #[cfg(not(feature = \"enabled\"))]\n",
        "    println!(\"off\");\n",
        "}\n",
    );
    let before_on = compile_and_run(
        &[("src/main.rs", gated_exec)],
        "src/main.rs",
        &["feature=\"enabled\""],
    );
    let before_off = compile_and_run(&[("src/main.rs", gated_exec)], "src/main.rs", &[]);
    assert_eq!(before_on["stdout"], "on:25:5\n");
    assert_eq!(before_off["stdout"], "off\n");

    let ws = workspace(&[("src/main.rs", gated_exec)]);
    let plan = run_public_extract(
        gw.addr(),
        &ws,
        "src/main.rs",
        5,
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
    let after_source = ws.read("src/main.rs");
    assert_eq!(
        after_source
            .matches("#[cfg_attr(feature = \"enabled\", allow(dead_code))]")
            .count(),
        3,
        "{after_source}"
    );
    let after_on = compile_and_run(
        &[("src/main.rs", &after_source)],
        "src/main.rs",
        &["feature=\"enabled\""],
    );
    let after_off = compile_and_run(&[("src/main.rs", &after_source)], "src/main.rs", &[]);
    assert_eq!(after_on["stdout"], before_on["stdout"]);
    assert_eq!(after_off["stdout"], before_off["stdout"]);
}
