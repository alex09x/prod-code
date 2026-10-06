/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::exec::ExecUsage;
use super::super::remote_exec::{
    RemoteExecCommand, RemoteExecDiagnostic, RemoteExecFormat, RemoteExecLanguage,
    RemoteExecRequest, RemoteExecResult, RemoteExecSpan, RemoteExecStream, RemoteExecTestEvent,
    parse_cargo_json_event, parse_go_test_json_event,
};
use super::super::wire::WireMessage;

#[test]
fn remote_exec_request_to_argv_and_round_trip() {
    let req = RemoteExecRequest {
        client_workspace_root: "/path/to/project".into(),
        base_workspace_name: Some("project".into()),
        language: RemoteExecLanguage::Rust,
        command: RemoteExecCommand::Check,
        args: vec!["--lib".into()],
        env: vec![("RUST_BACKTRACE".into(), "1".into())],
        format: RemoteExecFormat::Json,
        timeout_secs: 60,
        pull_changes: true,
        subdir: Some("subcrate".into()),
        client_agent: Some("agent-cli".into()),
        client_host: Some("host.lan".into()),
    };

    // 1. Verify toolchain argv generation
    let argv = req.to_argv();
    assert_eq!(
        argv,
        vec![
            "cargo",
            "check",
            "--workspace",
            "--all-targets",
            "--message-format=json",
            "--lib"
        ]
    );

    // 2. Verify conversion into ExecRequest
    let exec_req = req.clone().into_exec_request();
    assert_eq!(exec_req.command, argv);
    assert_eq!(exec_req.timeout_secs, 60);
    assert!(exec_req.pull_changes);

    // 3. Verify wire round-trip as WireMessage::RemoteExecRequest
    let wire = WireMessage::RemoteExecRequest(req.clone());
    let json = serde_json::to_string(&wire).unwrap();
    let decoded: WireMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(wire, decoded);

    // 4. Test other languages argv generation
    let go_test = RemoteExecRequest {
        language: RemoteExecLanguage::Go,
        command: RemoteExecCommand::Test,
        format: RemoteExecFormat::Json,
        args: vec!["-run".into(), "TestOrder".into()],
        ..req.clone()
    };
    assert_eq!(
        go_test.to_argv(),
        vec!["go", "test", "-json", "./...", "-run", "TestOrder"]
    );

    let go_test_raw = RemoteExecRequest {
        language: RemoteExecLanguage::Go,
        command: RemoteExecCommand::Test,
        format: RemoteExecFormat::Raw,
        args: vec![],
        ..req.clone()
    };
    assert_eq!(go_test_raw.to_argv(), vec!["go", "test", "-v", "./..."]);

    let ts_lint = RemoteExecRequest {
        language: RemoteExecLanguage::TypeScript,
        command: RemoteExecCommand::Lint,
        format: RemoteExecFormat::Json,
        args: vec![],
        ..req.clone()
    };
    assert_eq!(
        ts_lint.to_argv(),
        vec!["npx", "--no-install", "eslint", ".", "--format=json"]
    );

    let py_test = RemoteExecRequest {
        language: RemoteExecLanguage::Python,
        command: RemoteExecCommand::Test,
        format: RemoteExecFormat::Json,
        args: vec!["-k".into(), "test_auth".into()],
        ..req.clone()
    };
    assert_eq!(
        py_test.to_argv(),
        vec!["pytest", "--json-report", "-k", "test_auth"]
    );
}

#[test]
fn remote_exec_stream_and_result_round_trip() {
    let diag = RemoteExecDiagnostic {
        level: "error".into(),
        code: Some("E0308".into()),
        message: "mismatched types".into(),
        spans: vec![RemoteExecSpan {
            file: "src/lib.rs".into(),
            line_start: 12,
            line_end: Some(12),
            col_start: 5,
            col_end: Some(15),
            is_primary: true,
            label: Some("expected u32, found &str".into()),
        }],
        rendered: Some("error[E0308]: mismatched types".into()),
        suggestion: None,
    };

    let stream_msg = WireMessage::RemoteExecStream(RemoteExecStream::Diagnostic(diag.clone()));
    let json = serde_json::to_string(&stream_msg).unwrap();
    let decoded: WireMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(stream_msg, decoded);

    let test_ev = RemoteExecTestEvent::Failed {
        name: "test_login".into(),
        duration_ms: Some(145),
        message: Some("assertion failed: `left == right`".into()),
        assertion_diff: Some("- expected 200\n+ got 403".into()),
        backtrace: Some("at test_login (tests/auth.rs:42)".into()),
        output: Some("panicked at tests/auth.rs:42".into()),
    };

    let stream_ev = WireMessage::RemoteExecStream(RemoteExecStream::TestEvent(test_ev.clone()));
    let json_ev = serde_json::to_string(&stream_ev).unwrap();
    let decoded_ev: WireMessage = serde_json::from_str(&json_ev).unwrap();
    assert_eq!(stream_ev, decoded_ev);

    let result = RemoteExecResult {
        exit_code: Some(1),
        duration_ms: 1250,
        server_workspace_root: "/home/alex/storage/ws".into(),
        timed_out: false,
        error: None,
        usage: Some(ExecUsage {
            cpu_user_ms: 850,
            cpu_sys_ms: 120,
            max_rss_kb: 45000,
        }),
        platform: Some("linux x86_64".into()),
        diagnostics: vec![diag],
        tests_passed: 10,
        tests_failed: 1,
        tests_skipped: 2,
        test_failures: vec![test_ev],
        benches: vec![RemoteExecTestEvent::Bench {
            name: "bench_throughput".into(),
            estimate: "254.5 ns/iter".into(),
            range: Some("+/- 12".into()),
        }],
    };

    let result_msg = WireMessage::RemoteExecResult(result.clone());
    let json_res = serde_json::to_string(&result_msg).unwrap();
    let decoded_res: WireMessage = serde_json::from_str(&json_res).unwrap();
    assert_eq!(result_msg, decoded_res);
    assert!(!result.ok());
}

#[test]
fn parse_cargo_and_go_test_json_events_correctly() {
    // Cargo compiler message
    let cargo_diag_json = r#"{"reason":"compiler-message","package_id":"foo","message":{"level":"error","code":{"code":"E0425"},"message":"cannot find value `x` in this scope","spans":[{"file_name":"src/lib.rs","line_start":3,"column_start":9,"is_primary":true,"label":"not found in this scope"}],"rendered":"error[E0425]: cannot find value `x` in this scope\n"}}"#;
    let event = parse_cargo_json_event(cargo_diag_json).unwrap();
    match event {
        RemoteExecStream::Diagnostic(d) => {
            assert_eq!(d.level, "error");
            assert_eq!(d.code.as_deref(), Some("E0425"));
            assert_eq!(d.spans.len(), 1);
            assert_eq!(d.spans[0].file, "src/lib.rs");
            assert_eq!(d.spans[0].line_start, 3);
        }
        other => panic!("expected Diagnostic, got {other:?}"),
    }

    // Cargo test passed
    let cargo_test_ok =
        r#"{"type":"test","event":"ok","name":"tests::it_works","exec_time":0.005}"#;
    let event = parse_cargo_json_event(cargo_test_ok).unwrap();
    match event {
        RemoteExecStream::TestEvent(RemoteExecTestEvent::Passed { name, duration_ms }) => {
            assert_eq!(name, "tests::it_works");
            assert_eq!(duration_ms, Some(5));
        }
        other => panic!("expected TestEvent::Passed, got {other:?}"),
    }

    // Go test fail event
    let go_fail = r#"{"Time":"2026-10-02T12:00:00Z","Action":"fail","Package":"pkg/orders","Test":"TestCalculateTotal","Elapsed":0.042}"#;
    let event = parse_go_test_json_event(go_fail).unwrap();
    match event {
        RemoteExecStream::TestEvent(RemoteExecTestEvent::Failed {
            name, duration_ms, ..
        }) => {
            assert_eq!(name, "pkg/orders.TestCalculateTotal");
            assert_eq!(duration_ms, Some(42));
        }
        other => panic!("expected TestEvent::Failed, got {other:?}"),
    }

    // Standard Cargo/libtest text output parsing
    let pass_line = "test tests::my_test_pass ... ok";
    let pass_ev = parse_cargo_json_event(pass_line).unwrap();
    match pass_ev {
        RemoteExecStream::TestEvent(RemoteExecTestEvent::Passed { name, duration_ms }) => {
            assert_eq!(name, "tests::my_test_pass");
            assert_eq!(duration_ms, None);
        }
        other => panic!("expected TestEvent::Passed, got {other:?}"),
    }

    let pass_with_dur = "test tests::my_test_fast ... ok (0.012s)";
    let pass_dur_ev = parse_cargo_json_event(pass_with_dur).unwrap();
    match pass_dur_ev {
        RemoteExecStream::TestEvent(RemoteExecTestEvent::Passed { name, duration_ms }) => {
            assert_eq!(name, "tests::my_test_fast");
            assert_eq!(duration_ms, Some(12));
        }
        other => panic!("expected TestEvent::Passed with duration, got {other:?}"),
    }

    let fail_line = "test tests::my_test_fail ... FAILED";
    let fail_ev = parse_cargo_json_event(fail_line).unwrap();
    match fail_ev {
        RemoteExecStream::TestEvent(RemoteExecTestEvent::Failed { name, .. }) => {
            assert_eq!(name, "tests::my_test_fail");
        }
        other => panic!("expected TestEvent::Failed, got {other:?}"),
    }

    let skip_line = "test tests::my_test_skip ... ignored";
    let skip_ev = parse_cargo_json_event(skip_line).unwrap();
    match skip_ev {
        RemoteExecStream::TestEvent(RemoteExecTestEvent::Skipped { name, .. }) => {
            assert_eq!(name, "tests::my_test_skip");
        }
        other => panic!("expected TestEvent::Skipped, got {other:?}"),
    }

    let bench_line = "test tests::bench_compute ... bench: 45.20 ns/iter (+/- 2)";
    let bench_ev = parse_cargo_json_event(bench_line).unwrap();
    match bench_ev {
        RemoteExecStream::TestEvent(RemoteExecTestEvent::Bench { name, estimate, .. }) => {
            assert_eq!(name, "tests::bench_compute");
            assert_eq!(estimate, "45.20 ns/iter (+/- 2)");
        }
        other => panic!("expected TestEvent::Bench, got {other:?}"),
    }

    // Summary line should be ignored
    let summary_line = "test result: FAILED. 1 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.05s";
    assert!(parse_cargo_json_event(summary_line).is_none());

    // Standard Go human-readable text output parsing
    let go_run_line = "=== RUN   TestLoginHandler";
    let go_run_ev = parse_go_test_json_event(go_run_line).unwrap();
    match go_run_ev {
        RemoteExecStream::TestEvent(RemoteExecTestEvent::Started { name }) => {
            assert_eq!(name, "TestLoginHandler");
        }
        other => panic!("expected TestEvent::Started, got {other:?}"),
    }

    let go_pass_line = "--- PASS: TestLoginHandler (0.015s)";
    let go_pass_ev = parse_go_test_json_event(go_pass_line).unwrap();
    match go_pass_ev {
        RemoteExecStream::TestEvent(RemoteExecTestEvent::Passed { name, duration_ms }) => {
            assert_eq!(name, "TestLoginHandler");
            assert_eq!(duration_ms, Some(15));
        }
        other => panic!("expected TestEvent::Passed, got {other:?}"),
    }

    let go_subtest_pass = "    --- PASS: TestLoginHandler/Valid_Credentials (0.002s)";
    let go_sub_ev = parse_go_test_json_event(go_subtest_pass).unwrap();
    match go_sub_ev {
        RemoteExecStream::TestEvent(RemoteExecTestEvent::Passed { name, duration_ms }) => {
            assert_eq!(name, "TestLoginHandler/Valid_Credentials");
            assert_eq!(duration_ms, Some(2));
        }
        other => panic!("expected TestEvent::Passed, got {other:?}"),
    }

    let go_fail_line = "--- FAIL: TestRefreshToken (0.034s)";
    let go_fail_ev = parse_go_test_json_event(go_fail_line).unwrap();
    match go_fail_ev {
        RemoteExecStream::TestEvent(RemoteExecTestEvent::Failed {
            name, duration_ms, ..
        }) => {
            assert_eq!(name, "TestRefreshToken");
            assert_eq!(duration_ms, Some(34));
        }
        other => panic!("expected TestEvent::Failed, got {other:?}"),
    }

    let go_skip_line = "--- SKIP: TestIntegrationDisabled (0.00s)";
    let go_skip_ev = parse_go_test_json_event(go_skip_line).unwrap();
    match go_skip_ev {
        RemoteExecStream::TestEvent(RemoteExecTestEvent::Skipped { name, .. }) => {
            assert_eq!(name, "TestIntegrationDisabled");
        }
        other => panic!("expected TestEvent::Skipped, got {other:?}"),
    }

    // Go summaries should be ignored
    assert!(parse_go_test_json_event("PASS").is_none());
    assert!(parse_go_test_json_event("FAIL").is_none());
    assert!(parse_go_test_json_event("ok  \tpkg/auth\t0.021s").is_none());
}

#[test]
fn python_check_default_and_custom_targets() {
    let default_check = RemoteExecRequest {
        client_workspace_root: "/test".into(),
        base_workspace_name: None,
        language: RemoteExecLanguage::Python,
        command: RemoteExecCommand::Check,
        args: vec![],
        env: vec![],
        format: RemoteExecFormat::Raw,
        timeout_secs: 10,
        pull_changes: false,
        subdir: None,
        client_agent: None,
        client_host: None,
    };
    assert_eq!(
        default_check.to_argv(),
        vec!["python3", "-m", "compileall", "-q", "."]
    );

    let custom_check = RemoteExecRequest {
        args: vec!["src/mypackage".into(), "tests/".into()],
        ..default_check
    };
    assert_eq!(
        custom_check.to_argv(),
        vec![
            "python3",
            "-m",
            "compileall",
            "-q",
            "src/mypackage",
            "tests/"
        ]
    );
}
