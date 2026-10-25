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
use crate::fixit::{Edit, Fix};

fn multipart(file: &str) -> Fix {
    let mut fix = parse_fixes(UNUSED).remove(0);
    fix.edits.push(Edit {
        file: file.into(),
        ..fix.edits[0].clone()
    });
    fix
}

fn output(fixes: &[Fix]) -> String {
    fixes
        .iter()
        .map(|fix| {
            let spans: Vec<_> = fix.edits.iter().map(|e| serde_json::json!({
            "file_name": e.file, "byte_start": e.start, "byte_end": e.end,
            "line_start": e.line, "text": [{"text": e.line_text}],
            "suggested_replacement": e.replacement, "suggestion_applicability": "MachineApplicable"
        })).collect();
            serde_json::json!({"reason":"compiler-message", "message":{
                "level":"warning", "message":fix.message, "spans":spans, "children":[]
            }})
            .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn rejected_multipart_allows_independent_fix(symlink: bool) {
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/lib.rs", SOURCE),
        ("valid.rs", SOURCE),
    ]);
    let outside = Workspace::new(&[("outside.rs", SOURCE)]);
    let escape = if symlink {
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path("outside.rs"), ws.path("src/link.rs")).unwrap();
        "src/link.rs".to_string()
    } else {
        // Both temporary repositories share a parent. The leading component is innocuous.
        let name = outside
            .root()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        format!("src/../../{name}/outside.rs")
    };
    let bad = multipart(&escape);
    let mut good = parse_fixes(UNUSED).remove(0);
    good.edits[0].file = "valid.rs".into();
    let fixes = vec![bad, good];
    let (files, outcomes) = plan(&ws.root(), &fixes);
    println!(
        "boundary planner: symlink={symlink}, files={}, skipped={:?}",
        files.len(),
        outcomes.iter().map(|o| &o.skipped).collect::<Vec<_>>()
    );
    // Run orchestration even when the original planner accepts the bad fix.
    let gateway = Gateway::start(vec![Step::rust(&output(&fixes)), Step::green()]).await;
    let result = check_and_fix(gateway.addr, &ws.root(), None, VerifyKind::Lint, 10).await;
    let requests = gateway.finish().await;
    println!(
        "boundary protocol: symlink={symlink}, result={:?}, valid={:?}",
        result.as_ref().map(|r| r.ok()).map_err(|e| e.to_string()),
        ws.read("valid.rs")
    );
    let result = result.expect("an unsafe suggestion must be skipped before global application");
    assert_eq!(requests.len(), 2);
    assert_eq!(files.len(), 1);
    assert_eq!(files[&ws.path("valid.rs")].as_bytes(), FIXED.as_bytes());
    assert!(
        outcomes[0]
            .skipped
            .as_deref()
            .unwrap()
            .contains("not a file of this workspace")
    );
    assert!(
        outcomes[1].skipped.is_none(),
        "rejected multipart must not reserve its first span"
    );
    assert!(result.ok());
    assert!(result.outcomes[0].skipped.is_some());
    assert!(result.outcomes[1].skipped.is_none());
    assert_eq!(
        ws.read("src/lib.rs").as_bytes(),
        SOURCE.as_bytes(),
        "no partial multipart edit"
    );
    assert_eq!(
        outside.read("outside.rs").as_bytes(),
        SOURCE.as_bytes(),
        "outside file unchanged"
    );
    assert_eq!(ws.read("valid.rs").as_bytes(), FIXED.as_bytes());
}

#[tokio::test]
async fn issue1349_traversal_skips_whole_suggestion_and_applies_valid_fix() {
    rejected_multipart_allows_independent_fix(false).await;
}

#[cfg(unix)]
#[tokio::test]
async fn issue1349_symlink_skips_whole_suggestion_and_applies_valid_fix() {
    rejected_multipart_allows_independent_fix(true).await;
}

#[test]
fn issue1349_inside_parent_components_and_alias_overlap_remain_valid() {
    let ws = Workspace::new(&[("src/lib.rs", SOURCE)]);
    let mut first = parse_fixes(UNUSED).remove(0);
    first.edits[0].file = "src/../src/lib.rs".into();
    let second = parse_fixes(UNUSED).remove(0);
    let (files, outcomes) = plan(&ws.root(), &[first, second]);
    assert_eq!(
        files.len(),
        1,
        "aliases must share text and overlap tracking"
    );
    assert_eq!(files[&ws.path("src/lib.rs")].as_bytes(), FIXED.as_bytes());
    assert!(outcomes[0].skipped.is_none());
    assert_eq!(
        outcomes[1].skipped.as_deref(),
        Some("it overlaps a fix already taken")
    );
}

#[test]
fn issue1349_rejected_multipart_does_not_reserve_edits_or_split_utf8() {
    let ws = Workspace::new(&[("src/lib.rs", SOURCE), ("utf8.rs", "é\n")]);
    let good = parse_fixes(UNUSED).remove(0);
    for invalid in [
        Edit {
            file: "utf8.rs".into(),
            start: 1,
            end: 2,
            line_text: Some("é".into()),
            ..good.edits[0].clone()
        },
        Edit {
            file: "src/lib.rs".into(),
            line_text: Some("stale compiler line".into()),
            ..good.edits[0].clone()
        },
        Edit {
            file: "src/lib.rs".into(),
            end: SOURCE.len() + 1,
            ..good.edits[0].clone()
        },
        good.edits[0].clone(), // overlapping parts of one suggestion are skipped whole
    ] {
        let bad = Fix {
            edits: vec![good.edits[0].clone(), invalid],
            ..good.clone()
        };
        let (files, outcomes) = plan(&ws.root(), &[bad, good.clone()]);
        assert!(outcomes[0].skipped.is_some());
        assert!(
            outcomes[1].skipped.is_none(),
            "rejected multipart cannot reserve a valid span"
        );
        assert_eq!(files.len(), 1);
        assert_eq!(files[&ws.path("src/lib.rs")].as_bytes(), FIXED.as_bytes());
        assert_eq!(ws.read("src/lib.rs").as_bytes(), SOURCE.as_bytes());
        assert_eq!(ws.read("utf8.rs").as_bytes(), "é\n".as_bytes());
    }
    let valid_utf8 = Fix {
        edits: vec![Edit {
            file: "utf8.rs".into(),
            start: 0,
            end: 2,
            line_text: Some("é".into()),
            replacement: "e".into(),
            ..good.edits[0].clone()
        }],
        ..good
    };
    let (files, outcomes) = plan(&ws.root(), &[valid_utf8]);
    assert_eq!(files[&ws.path("utf8.rs")].as_bytes(), b"e\n");
    assert!(outcomes[0].skipped.is_none());
}

#[cfg(unix)]
#[test]
fn issue1349_canonical_root_and_inside_symlink_share_overlap_tracking() {
    let ws = Workspace::new(&[("src/lib.rs", SOURCE)]);
    std::os::unix::fs::symlink(ws.path("src/lib.rs"), ws.path("alias.rs")).unwrap();
    let root_alias = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(ws.root(), root_alias.path().join("checkout")).unwrap();
    let mut alias = parse_fixes(UNUSED).remove(0);
    alias.edits[0].file = "alias.rs".into();
    let original = parse_fixes(UNUSED).remove(0);
    let (files, outcomes) = plan(&root_alias.path().join("checkout"), &[alias, original]);
    assert_eq!(files.len(), 1);
    assert_eq!(files[&ws.path("src/lib.rs")].as_bytes(), FIXED.as_bytes());
    assert!(outcomes[0].skipped.is_none());
    assert_eq!(
        outcomes[1].skipped.as_deref(),
        Some("it overlaps a fix already taken")
    );
}
