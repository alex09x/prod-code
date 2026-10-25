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
use serde_json::json;

fn request() -> RemoteExecRequest {
    RemoteExecRequest {
        client_workspace_root: "/w".into(),
        base_workspace_name: Some("base".into()),
        language: RemoteExecLanguage::Rust,
        command: RemoteExecCommand::Test,
        args: vec!["-p".into(), "foo".into()],
        env: vec![("K".into(), "V".into())],
        format: RemoteExecFormat::Json,
        timeout_secs: 30,
        pull_changes: true,
        subdir: Some("sub".into()),
        client_agent: Some("agent".into()),
        client_host: Some("host".into()),
    }
}

#[test]
fn names_and_display_are_stable_wire_tokens() {
    use RemoteExecLanguage::*;
    let langs = [
        (Rust, "rust"),
        (Go, "go"),
        (Cpp, "cpp"),
        (TypeScript, "typescript"),
        (Python, "python"),
        (Swift, "swift"),
        (Generic, "generic"),
    ];
    for (l, s) in langs {
        assert_eq!(l.as_str(), s);
        assert_eq!(l.to_string(), s);
        assert_eq!(serde_json::to_value(l).unwrap(), json!(s));
    }
    let cmds = [
        (RemoteExecCommand::Check, "check"),
        (RemoteExecCommand::Test, "test"),
        (RemoteExecCommand::Lint, "lint"),
        (RemoteExecCommand::Bench, "bench"),
        (RemoteExecCommand::Custom("make all".into()), "make all"),
    ];
    for (c, s) in cmds {
        assert_eq!(c.as_str(), s);
        assert_eq!(c.to_string(), s);
        assert_eq!(
            serde_json::from_value::<RemoteExecCommand>(json!(s)).unwrap(),
            c
        );
    }
    assert_eq!(RemoteExecFormat::default(), RemoteExecFormat::Raw);
    assert_eq!(RemoteExecFormat::Raw.as_str(), "raw");
    assert_eq!(RemoteExecFormat::Json.as_str(), "json");
}

#[test]
fn request_defaults_fill_missing_optional_fields() {
    let r: RemoteExecRequest = serde_json::from_value(json!({
        "client_workspace_root": "/w",
        "language": "go",
        "command": "bench"
    }))
    .unwrap();
    assert_eq!(r.language, RemoteExecLanguage::Go);
    assert_eq!(r.command, RemoteExecCommand::Bench);
    assert_eq!(r.format, RemoteExecFormat::Raw);
    assert!(r.args.is_empty() && r.env.is_empty());
    assert_eq!((r.timeout_secs, r.pull_changes), (0, false));
    assert!(r.base_workspace_name.is_none() && r.subdir.is_none());
    assert!(r.client_agent.is_none() && r.client_host.is_none());
}

#[test]
fn request_converts_into_exec_request_with_built_argv() {
    let r = request();
    assert_eq!(
        r.to_argv(),
        [
            "cargo",
            "test",
            "--workspace",
            "--message-format=json",
            "-p",
            "foo"
        ]
    );
    let e = r.into_exec_request();
    assert_eq!(e.client_workspace_root, "/w");
    assert_eq!(e.base_workspace_name.as_deref(), Some("base"));
    assert_eq!(
        e.command,
        [
            "cargo",
            "test",
            "--workspace",
            "--message-format=json",
            "-p",
            "foo"
        ]
    );
    assert_eq!(e.env, vec![("K".to_string(), "V".to_string())]);
    assert_eq!(e.timeout_secs, 30);
    assert!(e.pull_changes);
    assert_eq!(e.subdir.as_deref(), Some("sub"));
    assert_eq!(e.client_agent.as_deref(), Some("agent"));
    assert_eq!(e.client_host.as_deref(), Some("host"));
}

#[test]
fn test_event_name_and_wire_shape() {
    let events = [
        (
            RemoteExecTestEvent::Started { name: "a".into() },
            json!({"action": "started", "name": "a"}),
        ),
        (
            RemoteExecTestEvent::Passed {
                name: "b".into(),
                duration_ms: Some(4),
            },
            json!({"action": "passed", "name": "b", "duration_ms": 4}),
        ),
        (
            RemoteExecTestEvent::Failed {
                name: "c".into(),
                duration_ms: None,
                message: Some("boom".into()),
                assertion_diff: None,
                backtrace: None,
                output: None,
            },
            json!({"action": "failed", "name": "c", "message": "boom"}),
        ),
        (
            RemoteExecTestEvent::Skipped {
                name: "d".into(),
                reason: None,
            },
            json!({"action": "skipped", "name": "d"}),
        ),
        (
            RemoteExecTestEvent::Bench {
                name: "e".into(),
                estimate: "1 ns".into(),
                range: Some("0..2".into()),
            },
            json!({"action": "bench", "name": "e", "estimate": "1 ns", "range": "0..2"}),
        ),
    ];
    for (event, wire) in events {
        assert_eq!(serde_json::to_value(&event).unwrap(), wire);
        assert_eq!(
            serde_json::from_value::<RemoteExecTestEvent>(wire).unwrap(),
            event
        );
    }
    let names: Vec<_> = ["a", "b", "c", "d", "e"].into();
    let got = [
        RemoteExecTestEvent::Started { name: "a".into() },
        RemoteExecTestEvent::Passed {
            name: "b".into(),
            duration_ms: None,
        },
        RemoteExecTestEvent::Failed {
            name: "c".into(),
            duration_ms: None,
            message: None,
            assertion_diff: None,
            backtrace: None,
            output: None,
        },
        RemoteExecTestEvent::Skipped {
            name: "d".into(),
            reason: Some("r".into()),
        },
        RemoteExecTestEvent::Bench {
            name: "e".into(),
            estimate: "x".into(),
            range: None,
        },
    ];
    assert_eq!(got.iter().map(|e| e.name()).collect::<Vec<_>>(), names);
}

#[test]
fn stream_messages_carry_a_kind_tag() {
    let diag = RemoteExecDiagnostic {
        level: "error".into(),
        code: Some("E0001".into()),
        message: "bad".into(),
        spans: vec![RemoteExecSpan {
            file: "a.rs".into(),
            line_start: 1,
            line_end: None,
            col_start: 2,
            col_end: Some(3),
            is_primary: true,
            label: None,
        }],
        rendered: None,
        suggestion: None,
    };
    let wire = serde_json::to_value(RemoteExecStream::Diagnostic(diag.clone())).unwrap();
    assert_eq!(
        wire,
        json!({"kind": "diagnostic", "level": "error", "code": "E0001", "message": "bad",
               "spans": [{"file": "a.rs", "line_start": 1, "col_start": 2, "col_end": 3, "is_primary": true}]})
    );
    assert_eq!(
        serde_json::from_value::<RemoteExecStream>(wire).unwrap(),
        RemoteExecStream::Diagnostic(diag)
    );
    let ev = RemoteExecStream::TestEvent(RemoteExecTestEvent::Started { name: "t".into() });
    let wire = serde_json::to_value(&ev).unwrap();
    assert_eq!(wire["kind"], "test_event");
    assert_eq!(
        serde_json::from_value::<RemoteExecStream>(wire).unwrap(),
        ev
    );
}

#[test]
fn result_ok_requires_zero_exit_no_timeout_no_error() {
    let base: RemoteExecResult = serde_json::from_value(json!({
        "exit_code": 0, "duration_ms": 5, "server_workspace_root": "/s"
    }))
    .unwrap();
    assert!(base.ok());
    assert_eq!(
        (base.tests_passed, base.tests_failed, base.tests_skipped),
        (0, 0, 0)
    );
    assert!(
        base.diagnostics.is_empty() && base.test_failures.is_empty() && base.benches.is_empty()
    );
    let wire = serde_json::to_value(&base).unwrap();
    assert_eq!(wire["timed_out"], false);
    assert!(wire.get("error").is_none() && wire.get("diagnostics").is_none());

    let mut r = base.clone();
    r.exit_code = Some(1);
    assert!(!r.ok());
    r.exit_code = None;
    assert!(!r.ok());
    let mut r = base.clone();
    r.timed_out = true;
    assert!(!r.ok());
    let mut r = base;
    r.error = Some("spawn failed".into());
    assert!(!r.ok());
}
