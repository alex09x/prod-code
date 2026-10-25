/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::net::Ipv6Addr;
use std::path::Path;

use super::tests::fake_gh;
use super::{Outcome, ReportRequest, draft, environment, issue_labels, report, scrub};

// Invented addresses, generated numerically; these identify no actual machine.
fn synthetic_addresses() -> Vec<String> {
    let unique = Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 0x1234);
    let link = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0x5678);
    vec![
        format!("[{unique}]:9400"),
        format!("[{link}%synthetic0]:9400"),
        unique.to_string(),
        format!("{link}%synthetic0"),
        format!("[{link}%25synthetic0]:9400"),
        format!(
            "{:x}:{:x}:{:x}:{:x}:{:x}:{:x}:{:x}:{:x}",
            0xfd00, 0, 0, 0, 0, 0, 0, 0x1234
        ),
        Ipv6Addr::new(0x2606, 0xffff, 0, 0, 0, 0, 0, 0x1234).to_string(),
        Ipv6Addr::new(0xfd00, 0x1234, 0, 0, 0, 0, 0, 0).to_string(),
        format!("::ffff:{}", std::net::Ipv4Addr::new(10, 255, 0, 123)),
    ]
}

#[test]
fn ipv6_scrubbing_preserves_syntax_and_safe_controls() {
    for address in synthetic_addresses() {
        let expected = if address.starts_with('[') {
            "[<node>]:9400"
        } else {
            "<node>"
        };
        let input = format!("é `http://{address}/status`, ({address}); {address}. end");
        let output = format!("é `http://{expected}/status`, ({expected}); {expected}. end");
        assert_eq!(scrub(&input, None, None), output, "IPv6 must be removed");
    }
    let safe = "[::1]:9400 ::1 :: [::]:0 2001:db8::1234 [2001:db8::1]:9400 \
                127.0.0.1 0.0.0.0 192.0.2.1 198.51.100.1 203.0.113.1 \
                Store::limit std::net::Ipv6Addr crate::dead::beef \
                std::abc::def label:value 12:34:56 a:b:c invalid::address";
    assert_eq!(scrub(safe, None, None), safe);
    let mapped_safe = "::ffff:127.0.0.1 ::ffff:0.0.0.0 ::ffff:192.0.2.1";
    assert_eq!(scrub(mapped_safe, None, None), mapped_safe);
}

#[test]
fn ipv6_compact_error_suffixes_are_scrubbed_in_drafts() {
    for address in [
        Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 0x1234),
        Ipv6Addr::new(0xfd00, 0x1234, 0, 0, 0, 0, 0, 0),
    ] {
        for suffix in ["timeout", "error", "closed", "500ms", "_failed"] {
            let endpoint = format!("peer:{address}:{suffix}");
            let expected = format!("peer:<node>:{suffix}");
            assert_eq!(scrub(&endpoint, None, None), expected);
            let title = format!("Synthetic lookup failure at {endpoint}");
            let body = format!("Synthetic lookup to {endpoint} returned no declaration.");
            let preview = draft(&title, &body, None).unwrap();
            assert!(preview.title.contains(&expected));
            assert!(preview.body.contains(&expected));
            assert!(!preview.body.contains(&address.to_string()));
        }
    }
    let paths = "crate::dead::beef::Error dead::beef::Timeout std::net::Ipv6Addr";
    assert_eq!(scrub(paths, None, None), paths);
}

#[tokio::test]
async fn ipv6_is_scrubbed_at_draft_and_fake_gh_boundaries() {
    for (literal, address) in synthetic_addresses().into_iter().flat_map(|address| {
        let punctuated = format!("{address}: connection refused");
        let labeled = format!("peer:{address}");
        [
            (address.clone(), address.clone()),
            (address.clone(), punctuated),
            (address, labeled),
        ]
    }) {
        let dir = tempfile::tempdir().unwrap();
        let gh = fake_gh(dir.path(), "[]");
        let title = format!("Synthetic report failure at {address}");
        let node = prod_code_protocol::StatusResponse {
            platform: Some(format!("synthetic platform {address}")),
            version: Some(format!("synthetic version {address}")),
            git_commit: Some(format!("synthetic commit {address}")),
            detected_engines: vec![format!("synthetic engine {address}")],
            ..Default::default()
        };
        let body =
            format!("Synthetic request at {address} returned no results instead of a declaration.");
        let preview = draft(&title, &body, Some(&node)).unwrap();
        assert!(!preview.title.contains(&literal), "draft title leaked IPv6");
        assert!(
            !preview.body.contains(&literal),
            "draft environment or body leaked IPv6"
        );
        // Carry the actual environment formatter's output through both public boundaries
        // without contacting a real node or altering process-wide environment variables.
        let body = format!("{body}\n\n{}", environment(Some(&node)));
        let reference = format!("synthetic-record-{address}");
        for dry_run in [true, false] {
            let outcome = report(
                None,
                ReportRequest {
                    root: Path::new(env!("CARGO_MANIFEST_DIR")),
                    title: &title,
                    body: &body,
                    force: false,
                    dry_run,
                    allow_private_names: true,
                    private_ref: Some(&reference),
                    labels: &[
                        " Enhancement ".into(),
                        "enhancement".into(),
                        "mcp".into(),
                        "roadmap".into(),
                    ],
                },
                &gh,
            )
            .await
            .unwrap();
            if dry_run {
                let Outcome::DryRun(draft) = outcome else {
                    panic!("expected draft")
                };
                assert!(!draft.title.contains(&literal), "public title leaked IPv6");
                assert!(
                    !draft.body.contains(&literal),
                    "public body or private reference leaked IPv6"
                );
                assert!(
                    draft
                        .body
                        .contains("Private details: report `synthetic-record-")
                );
                assert!(!dir.path().join("calls.log").exists());
            } else {
                assert!(matches!(outcome, Outcome::Filed(_)));
                let calls = std::fs::read_to_string(dir.path().join("calls.log")).unwrap();
                let sent = std::fs::read_to_string(dir.path().join("body.md")).unwrap();
                assert!(!calls.contains(&literal), "gh search/title leaked IPv6");
                assert!(
                    calls.contains("--label enhancement --label mcp --label roadmap --body-file -")
                );
                assert!(
                    !sent.contains(&literal),
                    "gh body or private reference leaked IPv6"
                );
                assert!(
                    sent.contains("synthetic platform <node>")
                        || sent.contains("synthetic platform [<node>]")
                        || sent.contains("synthetic platform peer:<node>")
                        || sent.contains("synthetic platform peer:[<node>]")
                );
                assert!(sent.contains("Private details: report `synthetic-record-"));
            }
        }
    }
}

#[test]
fn ipv6_followed_by_punctuation_colon_preserves_valid_compression() {
    let unique = Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 0x1234);
    let compressed = Ipv6Addr::new(0xfd00, 0x1234, 0, 0, 0, 0, 0, 0);
    let mapped = format!("::ffff:{}", std::net::Ipv4Addr::new(10, 255, 0, 123));
    for address in [unique.to_string(), compressed.to_string(), mapped] {
        for suffix in [
            "",
            ": connection refused",
            ":. end",
            ":: connection refused",
        ] {
            let input = format!("Request to {address}{suffix}");
            assert_eq!(
                scrub(&input, None, None),
                format!("Request to <node>{suffix}"),
                "IPv6 and punctuation must be parsed separately"
            );
        }
    }
    let safe = "Request to ::1: refused; ::: refused; 2001:db8::: refused; \
                Store::limit: refused; std::net::Ipv6Addr: refused; label: value; \
                abc::def::Type dead::beef::Result abc::def::_value";
    assert_eq!(scrub(safe, None, None), safe);
}

#[test]
fn ipv6_after_label_separator_preserves_rust_paths_and_safe_controls() {
    for address in synthetic_addresses() {
        let replacement = if address.starts_with('[') {
            "[<node>]:9400"
        } else {
            "<node>"
        };
        let input = format!("peer:{address}; endpoint:{address}: refused");
        assert_eq!(
            scrub(&input, None, None),
            format!("peer:{replacement}; endpoint:{replacement}: refused"),
            "label separators must not hide IPv6"
        );
    }
    let safe = "peer:::1 peer::: peer:2001:db8::1234 \
                peer:::ffff:127.0.0.1 peer:::ffff:192.0.2.1 \
                Store::limit std::net::Ipv6Addr crate::dead::beef \
                std::abc::def abc::def::Type dead::beef::Result \
                module::fd00::1234 module::fe80::5678 \
                abc::def::_value label:value time:12:34:56";
    assert_eq!(scrub(safe, None, None), safe);
}

#[test]
fn distinct_normalized_types_are_refused_but_duplicates_and_markers_work() {
    let labels = |values: &[&str]| values.iter().map(|v| v.to_string()).collect::<Vec<_>>();
    for first in super::TYPE_LABELS {
        for second in super::TYPE_LABELS {
            if first != second {
                assert!(
                    issue_labels(&labels(&[first, second])).is_err(),
                    "multiple types accepted"
                );
            }
        }
    }
    assert_eq!(
        issue_labels(&labels(&[" BUG ", "bug", "Bug", "mcp", " Roadmap "])).unwrap(),
        ["bug", "mcp", "roadmap"]
    );
    assert_eq!(
        issue_labels(&labels(&["mcp", "roadmap"])).unwrap(),
        ["bug", "mcp", "roadmap"]
    );
}

#[tokio::test]
async fn multiple_types_fail_before_any_gh_invocation() {
    let dir = tempfile::tempdir().unwrap();
    let gh = fake_gh(dir.path(), "[]");
    for dry_run in [false, true] {
        for force in [false, true] {
            let result = report(
                None,
                ReportRequest {
                    root: dir.path(),
                    title: "Synthetic report with conflicting labels",
                    body: "A synthetic request returned no results instead of a declaration.",
                    dry_run,
                    force,
                    allow_private_names: true,
                    private_ref: None,
                    labels: &[
                        " Bug ".into(),
                        "PERF".into(),
                        "mcp".into(),
                        "roadmap".into(),
                    ],
                },
                &gh,
            )
            .await;
            assert!(result.is_err(), "report accepted multiple types");
            assert!(result.unwrap_err().to_string().contains("one type"));
            assert!(!dir.path().join("calls.log").exists(), "gh was invoked");
        }
    }
}

#[test]
fn report_issue_help_schema_and_agent_guidance_advertise_roadmap() {
    let tool = crate::tools::list_tools()
        .into_iter()
        .find(|tool| tool.name == "code_report_issue")
        .unwrap();
    let labels = tool.input_schema["properties"]["labels"]["description"]
        .as_str()
        .unwrap();
    for guidance in [
        tool.description.as_str(),
        labels,
        crate::protocol::AGENT_INSTRUCTIONS,
    ] {
        assert!(
            guidance.contains("roadmap"),
            "report guidance omits roadmap"
        );
        assert!(guidance.contains("one type") || guidance.contains("One type"));
    }
}

#[tokio::test]
async fn mcp_request_uses_shared_type_validation_and_scrubbing() {
    let address = synthetic_addresses().remove(0);
    let remote = (std::net::Ipv4Addr::LOCALHOST, 0).into();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let args = serde_json::json!({
        "title": format!("Synthetic lookup failure at {address}"),
        "body": format!("A synthetic request at {address} returned no result instead of a declaration."),
        "private_ref": format!("synthetic-record-{address}"),
        "dry_run": true,
        "allow_private_names": true,
        "labels": [" Enhancement ", "enhancement", "mcp", "roadmap"]
    });
    let result = crate::tools::dispatch_tool(remote, root, "code_report_issue", args.clone())
        .await
        .unwrap();
    assert!(!result.is_error);
    let crate::protocol::McpContentItem::Text { text } = &result.content[0];
    assert!(!text.contains(&address), "MCP draft leaked IPv6");
    assert!(text.contains("Labels: enhancement, mcp, roadmap"));
    let mut conflicting = args;
    conflicting["labels"] = serde_json::json!(["bug", "PERF"]);
    let result = crate::tools::dispatch_tool(remote, root, "code_report_issue", conflicting)
        .await
        .unwrap();
    assert!(result.is_error);
    let crate::protocol::McpContentItem::Text { text } = &result.content[0];
    assert!(text.contains("one type"));
}
