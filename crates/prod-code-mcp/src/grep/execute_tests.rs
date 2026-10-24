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

#[test]
fn runtime_paths_2_rg_json_ignores_noise_and_recovers_missing_line_text() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("a.rs");
    std::fs::write(&path, "fn a() {\r\n  café();\r\n}\r\n").unwrap();
    let match_event = |path: &str, text: Option<&str>, submatches: serde_json::Value| serde_json::json!({"type":"match","data":{"path":{"text":path},"lines":{"text":text},"line_number":2,"submatches":submatches}}).to_string();
    let output = [
        "invalid JSON".into(), "{\"type\":\"begin\",\"data\":null}".into(), "{\"type\":\"match\",\"data\":null}".into(),
        match_event(path.to_str().unwrap(), None, serde_json::json!([])),
        match_event("./a.rs", Some("supplied\r\n"), serde_json::json!([{ "start": 4 }])),
        match_event("./a.rs", Some("omitted"), serde_json::json!([])),
    ].join("\n");
    let opts = GrepOptions { pattern: "café".into(), ..Default::default() };
    let report = parse_rg_json_output(temp.path(), output.as_bytes(), &opts, 2, 1, "scripted-rg");
    assert_eq!((report.total_matches, report.truncated, report.engine.as_str()), (3, true, "scripted-rg"));
    assert_eq!(report.hits.iter().map(|h| (h.path.as_str(), h.line, h.column, h.text.as_str())).collect::<Vec<_>>(), [("a.rs", 2, 1, "  café();"), ("a.rs", 2, 5, "supplied")]);
    assert_eq!(report.hits[0].context_before, [(1, "fn a() {".into())]);
    assert_eq!(report.hits[1].context_after, [(3, "}".into())]);
}

#[test]
fn runtime_paths_2_rg_args_trim_globs_and_keep_pattern_literal() {
    let opts = GrepOptions { pattern: "-x|é".into(), fixed_strings: true, ignore_case: true, include: vec![" ".into(), " *.rs ".into()], exclude: vec!["!vendor/**".into(), " ".into()], ..Default::default() };
    let args = build_rg_args("src", &opts);
    assert_eq!(&args[..8], ["--json", "--line-number", "--column", "--no-heading", "--color=never", "--fixed-strings", "--ignore-case", "-g"]);
    assert_eq!(&args[args.len()-7..], ["-g", "*.rs", "-g", "!vendor/**", "-e", "-x|é", "src"]);
    assert_eq!(args.iter().filter(|a| a.as_str() == "-g").count(), DEFAULT_EXCLUDES.len() + 2);
}
