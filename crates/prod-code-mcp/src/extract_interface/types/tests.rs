/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

fn result() -> ExtractInterfaceResult {
    ExtractInterfaceResult {
        type_name: "Store".into(),
        interface_name: "Reader".into(),
        methods: vec![],
        files_modified: vec![],
        overlays: vec![],
        diff: String::new(),
        applied: false,
        verified: false,
        diagnostics: vec![],
    }
}

#[test]
fn render_empty_and_diagnostic_results() {
    let mut r = result();
    assert_eq!(
        r.render(0),
        "`interface Reader` for `Store`\n- extracted methods: none\n- applied: false\n"
    );
    r.methods = vec!["read".into(), "close".into()];
    r.applied = true;
    r.diagnostics = vec!["missing type".into(), "unknown name".into()];
    assert_eq!(
        r.render(10),
        "`interface Reader` for `Store`\n- extracted methods: read, close\n- applied: true\n- diagnostics (2):\n  • missing type\n  • unknown name\n"
    );
}

#[test]
fn render_diff_limits_are_utf8_safe() {
    let mut r = result();
    r.diff = "+é\n".into();
    let prefix = "`interface Reader` for `Store`\n- extracted methods: none\n- applied: false\n\nDiff:\n```diff\n";
    for (limit, suffix) in [
        (0, "\n... [truncated]\n```\n"),
        (1, "+\n... [truncated]\n```\n"),
        (2, "+\n... [truncated]\n```\n"),
        (3, "+é\n... [truncated]\n```\n"),
        (4, "+é\n```\n"),
        (usize::MAX, "+é\n```\n"),
    ] {
        assert_eq!(
            r.render(limit),
            format!("{prefix}{suffix}"),
            "limit {limit}"
        );
    }
}

#[test]
fn result_round_trip_skips_overlay_contents() {
    let mut r = result();
    r.overlays
        .push((PathBuf::from("store.ts"), "private buffer".into()));
    let json = serde_json::to_value(&r).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "type_name":"Store", "interface_name":"Reader", "methods":[],
            "files_modified":[], "diff":"", "applied":false, "verified":false,
            "diagnostics":[]
        })
    );
    let decoded: ExtractInterfaceResult = serde_json::from_value(json).unwrap();
    assert_eq!(decoded.overlays, vec![]);
    assert_eq!(decoded.render(10), r.render(10));
}
