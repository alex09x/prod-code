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
fn runtime_paths_2_fallback_walk_filters_binary_hidden_and_globs() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for (name, bytes) in [
        ("a.rs", b"fn example() {\r\n  Needle needle\r\n  other\r\n}\r\n".as_slice()),
        ("b.rs", b"needle\nNEEDLE\n"),
        ("skip.rs", b"needle\n"),
        ("notes.txt", b"needle\n"),
        (".hidden.rs", b"needle\n"),
        ("binary.rs", b"needle\0\n"),
        ("invalid.rs", b"needle\xff\n"),
    ] {
        std::fs::write(root.join(name), bytes).unwrap();
    }
    std::fs::create_dir(root.join("target")).unwrap();
    std::fs::write(root.join("target/generated.rs"), "needle").unwrap();
    let opts = GrepOptions {
        pattern: "needle".into(),
        fixed_strings: true,
        include: vec!["  ".into(), " *.rs ".into()],
        exclude: vec![" ".into(), "!skip.rs".into(), ".hidden.rs".into()],
        ..Default::default()
    };
    // Smart case: a lowercase pattern matches `NEEDLE` too. Binary, invalid UTF-8, hidden and
    // excluded files are skipped; without a .gitignore `target/` is an ordinary directory.
    let all = run_in_process_grep(root, root, &opts, 100, 0).unwrap();
    let mut found: Vec<(String, u32)> = all.hits.iter().map(|h| (h.path.to_string(), h.line)).collect();
    found.sort();
    assert_eq!(
        found,
        [("a.rs", 2), ("b.rs", 1), ("b.rs", 2), ("target/generated.rs", 1)]
            .map(|(p, l)| (p.to_string(), l))
    );
    let report = run_in_process_grep(root, root, &opts, 2, 1).unwrap();
    assert_eq!(report.engine, "ignore-walk");
    assert_eq!(report.pattern, "needle");
    assert_eq!((report.total_matches, report.truncated), (4, true));
    assert_eq!(report.hits.len(), 2);
    let first = &report.hits[0];
    assert_eq!((&*first.path, first.line, first.column, &*first.text), ("a.rs", 2, 3, "  Needle needle"));
    assert_eq!(first.enclosing_symbol.as_deref(), Some("example"));
    assert_eq!(first.context_before, vec![(1, "fn example() {".into())]);
    assert_eq!(first.context_after, vec![(3, "  other".into())]);
    assert_eq!((&*report.hits[1].path, report.hits[1].line), ("b.rs", 1));
    let zero = run_in_process_grep(root, root, &opts, 0, 0).unwrap();
    assert_eq!((zero.hits.len(), zero.total_matches, zero.truncated), (0, 4, true));
}

#[test]
fn runtime_paths_2_fallback_matching_and_invalid_globs() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("input.txt"), "é BETA alpha\nAlpha\na|b\n").unwrap();
    for (pattern, fixed, ignore, expected) in [
        ("BETA|Alpha", false, false, vec![(1, 4), (2, 1)]),
        ("beta|absent", false, false, vec![(1, 4)]),
        ("BETA", true, true, vec![(1, 4)]),
        ("a|b", true, false, vec![(3, 1)]),
        ("|||", false, false, vec![]),
        ("", true, false, vec![(1, 1), (2, 1), (3, 1)]),
    ] {
        let opts = GrepOptions { pattern: pattern.into(), fixed_strings: fixed, ignore_case: ignore, ..Default::default() };
        let result = run_in_process_grep(dir.path(), dir.path(), &opts, 100, 0).unwrap();
        assert_eq!(result.hits.iter().map(|h| (h.line, h.column)).collect::<Vec<_>>(), expected, "{pattern}");
        assert!(!result.truncated);
    }
    for include in [true, false] {
        let mut opts = GrepOptions::default();
        if include { opts.include.push("[".into()); } else { opts.exclude.push("[".into()); }
        let err = run_in_process_grep(dir.path(), dir.path(), &opts, 10, 0).unwrap_err();
        assert_eq!(err.to_string(), if include { "invalid include glob `[`" } else { "invalid exclude glob `[`" });
    }
}

#[test]
fn runtime_paths_2_enrichment_handles_missing_files_and_cached_lines() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("example.rs");
    std::fs::write(&path, "fn example() {\r\n  café();\r\n}\r\n").unwrap();
    let raw = |file: PathBuf, line, text| RawMatch { full_path: file, rel_path: "example.rs".into(), line, column: 3, line_text: text };
    let hits = enrich_matches(&[
        raw(path.clone(), 2, None), raw(path, 3, Some("override".into())),
        raw(dir.path().join("absent"), 0, None),
    ], 20);
    assert_eq!(hits[0].text, "  café();");
    assert_eq!(hits[0].context_before, vec![(1, "fn example() {".into())]);
    assert_eq!(hits[0].context_after, vec![(3, "}".into())]);
    assert_eq!(hits[1].text, "override");
    assert_eq!(hits[1].context_before.len(), 2);
    assert_eq!(hits[2].text, "");
    assert_eq!(hits[2].enclosing_symbol, None);
    assert_eq!(hits[2].context_before, vec![]);
    assert_eq!(hits[2].context_after, vec![]);
}
