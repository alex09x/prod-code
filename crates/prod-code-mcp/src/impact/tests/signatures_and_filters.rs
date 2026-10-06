/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use tempfile;

use crate::impact::call_sites::*;
use crate::impact::diff::*;
use crate::impact::signatures::*;
use crate::impact::symbols::*;
use crate::impact::test_cmd::*;
use crate::impact::*;

#[test]
fn test_polyglot_signature_extraction_and_normalization() {
    let rust_src = "pub fn add_item(\n    id: u64,\n    name: &str,\n) -> Result<(), Error> {\n    Ok(())\n}\n";
    let lines: Vec<&str> = rust_src.lines().collect();
    let (start, end, sig) = extract_signature_span(&lines, 1, "add_item", "rust").unwrap();
    assert_eq!(start, 1);
    assert_eq!(end, 4);
    assert_eq!(
        sig,
        "pub fn add_item(id: u64, name: &str,) -> Result<(), Error>"
    );

    let go_src = "func (s *Store) Save(\n    ctx context.Context,\n    data []byte,\n) error {\n    return nil\n}\n";
    let lines: Vec<&str> = go_src.lines().collect();
    let (start, end, sig) = extract_signature_span(&lines, 1, "Save", "go").unwrap();
    assert_eq!(start, 1);
    assert_eq!(end, 4);
    assert_eq!(
        sig,
        "func (s *Store) Save(ctx context.Context, data []byte,) error"
    );

    let py_src = "def calculate_price(\n    base: float,\n    tax_rate: float = 0.05,\n) -> float:\n    return base * (1 + tax_rate)\n";
    let lines: Vec<&str> = py_src.lines().collect();
    let (start, end, sig) = extract_signature_span(&lines, 1, "calculate_price", "python").unwrap();
    assert_eq!(start, 1);
    assert_eq!(end, 4);
    assert_eq!(
        sig,
        "def calculate_price(base: float, tax_rate: float = 0.05,) -> float"
    );

    let ts_src = "export async function fetchUser(\n    userId: string,\n    timeoutMs: number = 5000\n): Promise<User> {\n    return null;\n}\n";
    let lines: Vec<&str> = ts_src.lines().collect();
    let (start, end, sig) = extract_signature_span(&lines, 1, "fetchUser", "typescript").unwrap();
    assert_eq!(start, 1);
    assert_eq!(end, 4);
    assert_eq!(
        sig,
        "export async function fetchUser(userId: string, timeoutMs: number = 5000): Promise<User>"
    );

    // One-line function bodies must not leak into signature (Issue #785)
    let one_line_rust =
        "fn action_payload(message: &[u8]) -> Vec<u8> { codec_payload(0, message) }\n";
    let lines: Vec<&str> = one_line_rust.lines().collect();
    let (start, end, sig) = extract_signature_span(&lines, 1, "action_payload", "rust").unwrap();
    assert_eq!(start, 1);
    assert_eq!(end, 1);
    assert_eq!(sig, "fn action_payload(message: &[u8]) -> Vec<u8>");

    let multiline_rust = "fn action_payload(\n    message: &[u8]\n) -> Vec<u8> {\n    codec_payload(0, message)\n}\n";
    let lines: Vec<&str> = multiline_rust.lines().collect();
    let (m_start, m_end, m_sig) =
        extract_signature_span(&lines, 1, "action_payload", "rust").unwrap();
    assert_eq!(m_start, 1);
    assert_eq!(m_end, 3);
    assert_eq!(m_sig, "fn action_payload(message: &[u8]) -> Vec<u8>");
    assert_eq!(sig, m_sig);

    let one_line_py = "def square(x: int) -> int: return x * x\n";
    let lines: Vec<&str> = one_line_py.lines().collect();
    let (start, end, sig) = extract_signature_span(&lines, 1, "square", "python").unwrap();
    assert_eq!(start, 1);
    assert_eq!(end, 1);
    assert_eq!(sig, "def square(x: int) -> int");
}

#[test]
fn test_signature_warning_rendering() {
    let report = ImpactReport {
        language: "rust".to_string(),
        base: "HEAD".to_string(),
        changed_files: vec!["src/order.rs".to_string()],
        changed: vec![Symbol {
            name: "process_order".to_string(),
            file: "src/order.rs".to_string(),
            line: 12,
            col: 8,
        }],
        callers: vec![],
        tests: vec![],
        test_command: None,
        unattributed_files: vec![],
        index: None,
        reaches: vec![],
        incomplete: vec![],
        signature_warnings: vec![SignatureWarning {
            symbol: Symbol {
                name: "process_order".to_string(),
                file: "src/order.rs".to_string(),
                line: 12,
                col: 8,
            },
            old_signature: "pub fn process_order(id: u64) -> bool".to_string(),
            new_signature: "pub fn process_order(id: u64, priority: bool) -> bool".to_string(),
            unadjusted_call_sites: vec![
                CallSite {
                    file: "src/worker.rs".to_string(),
                    line: 45,
                    col: 10,
                    caller: Some("run_worker".to_string()),
                    is_sibling: true,
                },
                CallSite {
                    file: "src/order.rs".to_string(),
                    line: 99,
                    col: 5,
                    caller: Some("retry_order".to_string()),
                    is_sibling: false,
                },
            ],
        }],
    };

    let rendered = report.render();
    assert!(
        rendered.contains("signature warnings (unadjusted call sites before full compilation):")
    );
    assert!(rendered.contains("`process_order` signature changed in src/order.rs:12:8"));
    assert!(rendered.contains("old: pub fn process_order(id: u64) -> bool"));
    assert!(rendered.contains("new: pub fn process_order(id: u64, priority: bool) -> bool"));
    assert!(rendered.contains("unadjusted sibling call sites (1):"));
    assert!(rendered.contains("• [sibling] src/worker.rs:45:10 in `run_worker`"));
    assert!(rendered.contains("• src/order.rs:99:5 in `retry_order`"));

    let ci = report.ci_summary(None, "no tests affected");
    assert!(
        ci.contains("⚠️ **Signature Warnings**: updated signatures left unadjusted call sites:")
    );
    assert!(ci.contains("`process_order` (`src/order.rs:12`)"));
    assert!(ci.contains("[sibling] `src/worker.rs:45:10` in `run_worker`"));
}

#[test]
fn untracked_scratch_files_are_excluded_from_diff_hunks() {
    assert!(is_scratch_path(".prod/tmp/resident-repair/a.go"));
    assert!(is_scratch_path(".scratch/draft.go"));
    assert!(is_scratch_path("internal/.scratch/draft.go"));
    assert!(is_scratch_path(".tmp/copy.go"));
    assert!(is_scratch_path(".cache/gen.go"));
    // Legitimate non-standard source paths must NOT be excluded (#760)
    assert!(!is_scratch_path(".support/helper.rs"));
    assert!(!is_scratch_path(".scratch-support/helper.rs"));
    assert!(!is_scratch_path(".tmpfiles/helper.rs"));
    assert!(!is_scratch_path("internal/push/a.go"));
    assert!(!is_scratch_path("main.go"));
}

#[test]
fn diff_hunks_filters_untracked_scratch_paths_in_git_repo() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path();
    std::process::Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(path)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(path)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(path)
        .output()
        .unwrap();
    std::fs::write(path.join("tracked.txt"), "hello\n").unwrap();
    std::process::Command::new("git")
        .args(["add", "tracked.txt"])
        .current_dir(path)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "initial"])
        .current_dir(path)
        .output()
        .unwrap();

    // Isolate from global Git excludes so .prod or .scratch are not hidden by global gitignore (#760)
    let empty_excludes = path.join(".empty_excludes");
    std::fs::write(&empty_excludes, "").unwrap();
    std::process::Command::new("git")
        .args([
            "config",
            "core.excludesFile",
            empty_excludes.to_str().unwrap(),
        ])
        .current_dir(path)
        .output()
        .unwrap();

    std::fs::create_dir_all(path.join(".prod/tmp/resident-repair")).unwrap();
    std::fs::write(
        path.join(".prod/tmp/resident-repair/scratch.go"),
        "package scratch\n",
    )
    .unwrap();

    std::fs::create_dir_all(path.join(".scratch")).unwrap();
    std::fs::write(path.join(".scratch/draft.go"), "package draft\n").unwrap();

    std::fs::create_dir_all(path.join(".support")).unwrap();
    std::fs::write(path.join(".support/helper.rs"), "pub fn helper() {}\n").unwrap();

    std::fs::write(path.join("untracked.rs"), "pub fn untracked() {}\n").unwrap();

    // Ensure git status reports the scratch paths as untracked before diff_hunks filters them
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain=v1", "-uall"])
        .current_dir(path)
        .output()
        .unwrap();
    let status_str = String::from_utf8_lossy(&status.stdout);
    assert!(
        status_str.contains("?? .prod/tmp/resident-repair/scratch.go"),
        "{status_str}"
    );
    assert!(status_str.contains("?? .scratch/draft.go"), "{status_str}");
    assert!(status_str.contains("?? .support/helper.rs"), "{status_str}");
    assert!(status_str.contains("?? untracked.rs"), "{status_str}");

    let hunks = diff_hunks(path, None).expect("diff_hunks succeeds");

    assert!(
        !hunks.contains_key(".prod/tmp/resident-repair/scratch.go"),
        "expected .prod scratch file to be excluded, but got {:?}",
        hunks.keys().collect::<Vec<_>>()
    );
    assert!(
        !hunks.contains_key(".scratch/draft.go"),
        "expected .scratch file to be excluded, but got {:?}",
        hunks.keys().collect::<Vec<_>>()
    );

    assert!(
        hunks.contains_key(".support/helper.rs"),
        "expected .support/helper.rs to be included in diff_hunks"
    );
    assert!(
        hunks.contains_key("untracked.rs"),
        "expected untracked.rs to be included in diff_hunks"
    );
}

#[test]
fn test_javascript_functions_and_markers() {
    let js_code = r#"export async function handleTabCreated(tab) {
    function addChildTabHandoff(child) {
        return child.id;
    }
    return addChildTabHandoff(tab);
}

export const routeMethod = (req) => {
    return req.method;
};
"#;
    let symbols = serde_json::json!([
        {
            "name": "handleTabCreated",
            "kind": 12,
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 5, "character": 1 } },
            "selectionRange": { "start": { "line": 0, "character": 22 }, "end": { "line": 0, "character": 38 } },
            "children": [
                {
                    "name": "addChildTabHandoff",
                    "kind": 12,
                    "range": { "start": { "line": 1, "character": 4 }, "end": { "line": 3, "character": 5 } },
                    "selectionRange": { "start": { "line": 1, "character": 13 }, "end": { "line": 1, "character": 31 } }
                }
            ]
        },
        {
            "name": "routeMethod",
            "kind": 14,
            "detail": "(req: any) => any",
            "range": { "start": { "line": 7, "character": 0 }, "end": { "line": 9, "character": 2 } },
            "selectionRange": { "start": { "line": 7, "character": 13 }, "end": { "line": 7, "character": 24 } }
        }
    ]);
    let mut out = Vec::new();
    collect_functions(symbols.as_array().unwrap(), Some(js_code), &mut out).unwrap();
    let names: Vec<&str> = out.iter().map(|(n, _, _, _, _)| n.as_str()).collect();
    assert_eq!(
        names,
        vec!["handleTabCreated", "addChildTabHandoff", "routeMethod"]
    );

    // Test marker recognition
    let test_src =
        "it('should create tab', async () => {});\ntest(\"route valid request\", () => {});\n";
    assert_eq!(
        test_marker("typescript", test_src, 1, "anonymous").as_deref(),
        Some("should create tab")
    );
    assert_eq!(
        test_marker("typescript", test_src, 2, "anonymous").as_deref(),
        Some("route valid request")
    );

    // Test file convention
    assert!(!looks_like_test(
        "typescript",
        "anyFunc",
        "extension/background.test.js"
    ));
    assert!(!looks_like_test(
        "typescript",
        "anyFunc",
        "test/unit.spec.ts"
    ));
    assert!(!looks_like_test(
        "typescript",
        "anyFunc",
        "src/__tests__/app.js"
    ));
    assert!(!looks_like_test(
        "typescript",
        "anyFunc",
        "extension/background.js"
    ));
}

#[test]
fn base_signature_lookup_tracks_the_changed_duplicate_declaration() {
    let base_lines = [
        "class First {",
        "  run(value: number) {}",
        "}",
        "class Target {",
        "  run(value: string) {}",
        "}",
    ];
    let hunk = Hunk {
        start: 4,
        added: 2,
        removed: 0,
    };
    let base_line = base_line_for_new_line(&[hunk], 7).unwrap();
    assert_eq!(base_line, 5);
    assert_eq!(
        find_function_in_text(&base_lines, "run", "typescript", base_line),
        Some(5)
    );
}

#[test]
fn call_site_range_includes_multiline_arguments_for_diff_matching() {
    let text = "fn caller() {\n    helper(\n        value,\n        true,\n    );\n}\n";
    let end_line = call_expression_end_line(text, 2, 5, "helper", "rust").unwrap();
    assert_eq!(end_line, 5);
    let hunk = Hunk {
        start: 4,
        added: 1,
        removed: 1,
    };
    assert!(!hunk.touches(2, 2));
    assert!(hunk.touches(2, end_line));
}
