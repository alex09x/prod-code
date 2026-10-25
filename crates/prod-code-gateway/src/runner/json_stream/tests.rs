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
use prod_code_protocol::RemoteExecDiagnostic;

/// A connected pair: the gateway side as the accumulator sees it and the client side.
async fn pair() -> (
    Framed<AnyStream, ProdCodeCodec>,
    Framed<tokio::net::TcpStream, ProdCodeCodec>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = tokio::net::TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (server, _) = listener.accept().await.unwrap();
    (
        Framed::new(AnyStream::from(server), ProdCodeCodec::new()),
        Framed::new(client, ProdCodeCodec::new()),
    )
}

/// Everything the gateway side sent, once it has been dropped.
async fn received(
    server: Framed<AnyStream, ProdCodeCodec>,
    mut client: Framed<tokio::net::TcpStream, ProdCodeCodec>,
) -> Vec<RemoteExecStream> {
    drop(server);
    let mut out = Vec::new();
    while let Some(Ok(msg)) = client.next().await {
        match msg {
            WireMessage::RemoteExecStream(ev) => out.push(ev),
            other => panic!("unexpected message {other:?}"),
        }
    }
    out
}

fn passed(name: &str, duration_ms: Option<u64>) -> RemoteExecStream {
    RemoteExecStream::TestEvent(RemoteExecTestEvent::Passed {
        name: name.to_string(),
        duration_ms,
    })
}

fn failed(name: &str, duration_ms: Option<u64>, output: Option<&str>) -> RemoteExecTestEvent {
    RemoteExecTestEvent::Failed {
        name: name.to_string(),
        duration_ms,
        message: None,
        assertion_diff: None,
        backtrace: None,
        output: output.map(str::to_string),
    }
}

#[tokio::test]
async fn rust_output_is_counted_and_forwarded_line_by_line() {
    let (mut server, client) = pair().await;
    let mut acc = JsonStreamAccumulator::new();
    let diagnostic = r#"{"reason":"compiler-message","message":{"level":"warning","message":"unused variable","code":{"code":"unused_variables"},"rendered":"warning: unused variable\n","spans":[{"file_name":"src/lib.rs","line_start":3,"line_end":3,"column_start":9,"column_end":10,"is_primary":true,"label":"here"}]}}"#;
    let text = format!(
        "{diagnostic}\r\n\r\n{{\"type\":\"test\",\"event\":\"started\",\"name\":\"a\"}}\n\
         {{\"type\":\"test\",\"event\":\"ok\",\"name\":\"a\",\"exec_time\":0.25}}\n\
         {{\"type\":\"test\",\"event\":\"failed\",\"name\":\"b\",\"stdout\":\"boom\"}}\n\
         {{\"type\":\"test\",\"event\":\"ignored\",\"name\":\"c\"}}\n\
         {{\"type\":\"test\",\"event\":\"bench\",\"name\":\"d\",\"median\":12.5}}\n\
         test e ... ok <0.5s>\ntest f ... FAILED\nnot a test line\n{{\"type\":\"suite\"}}\n"
    );
    assert!(
        acc.push_chunk(text.as_bytes(), RemoteExecLanguage::Rust, &mut server)
            .await
            .unwrap()
    );
    assert_eq!(
        (acc.tests_passed, acc.tests_failed, acc.tests_skipped),
        (2, 2, 1)
    );
    assert_eq!(acc.test_failures.len(), 2);
    assert_eq!(acc.benches.len(), 1);
    assert_eq!(
        acc.diagnostics,
        vec![RemoteExecDiagnostic {
            level: "warning".into(),
            code: Some("unused_variables".into()),
            message: "unused variable".into(),
            spans: vec![prod_code_protocol::RemoteExecSpan {
                file: "src/lib.rs".into(),
                line_start: 3,
                line_end: Some(3),
                col_start: 9,
                col_end: Some(10),
                is_primary: true,
                label: Some("here".into()),
            }],
            rendered: Some("warning: unused variable\n".into()),
            suggestion: None,
        }]
    );
    assert_eq!(acc.stdout_line_buf, "");
    let sent = received(server, client).await;
    // One message per recognised line, in order; the unrecognised lines are dropped.
    assert_eq!(sent.len(), 8);
    assert_eq!(
        sent[1],
        RemoteExecStream::TestEvent(RemoteExecTestEvent::Started { name: "a".into() })
    );
    assert_eq!(sent[2], passed("a", Some(250)));
    assert_eq!(
        sent[3],
        RemoteExecStream::TestEvent(failed("b", None, Some("boom")))
    );
    assert_eq!(acc.test_failures[1], failed("f", None, None));
}

#[tokio::test]
async fn go_output_uses_the_go_parser_and_other_languages_forward_nothing() {
    let go = "{\"Action\":\"run\",\"Package\":\"p\",\"Test\":\"T\"}\n\
              {\"Action\":\"pass\",\"Package\":\"p\",\"Test\":\"T\",\"Elapsed\":0.5}\n\
              {\"Action\":\"skip\",\"Test\":\"S\"}\n--- FAIL: TestX (0.05s)\n";
    for (language, counts, sent_len) in [
        (RemoteExecLanguage::Go, (1, 1, 1), 4),
        (RemoteExecLanguage::Python, (0, 0, 0), 0),
        (RemoteExecLanguage::Generic, (0, 0, 0), 0),
    ] {
        let (mut server, client) = pair().await;
        let mut acc = JsonStreamAccumulator::new();
        assert!(
            acc.push_chunk(go.as_bytes(), language, &mut server)
                .await
                .unwrap()
        );
        assert_eq!(
            (acc.tests_passed, acc.tests_failed, acc.tests_skipped),
            counts,
            "{language:?}"
        );
        let sent = received(server, client).await;
        assert_eq!(sent.len(), sent_len, "{language:?}");
        if sent_len > 0 {
            assert_eq!(sent[1], passed("p.T", Some(500)));
            assert_eq!(
                sent[3],
                RemoteExecStream::TestEvent(failed("TestX", Some(50), None))
            );
        }
    }
}

#[tokio::test]
async fn a_line_split_over_chunks_waits_and_flush_parses_the_unterminated_tail() {
    let (mut server, client) = pair().await;
    let mut acc = JsonStreamAccumulator::new();
    let go = RemoteExecLanguage::Rust;
    // Invalid UTF-8 is skipped without being buffered.
    assert!(
        acc.push_chunk(&[0xff, 0xfe], go, &mut server)
            .await
            .unwrap()
    );
    assert_eq!(acc.stdout_line_buf, "");
    assert!(
        acc.push_chunk(b"test one ..", go, &mut server)
            .await
            .unwrap()
    );
    assert_eq!(acc.stdout_line_buf, "test one ..");
    assert_eq!(acc.tests_passed, 0);
    assert!(
        acc.push_chunk(b". ok\ntest two ... ok  ", go, &mut server)
            .await
            .unwrap()
    );
    assert_eq!(acc.tests_passed, 1);
    assert_eq!(acc.stdout_line_buf, "test two ... ok  ");
    assert!(acc.flush(go, &mut server).await.unwrap());
    assert_eq!((acc.tests_passed, acc.stdout_line_buf.as_str()), (2, ""));
    // An empty buffer and a blank tail send nothing.
    assert!(acc.flush(go, &mut server).await.unwrap());
    acc.stdout_line_buf = "   ".into();
    assert!(acc.flush(go, &mut server).await.unwrap());
    assert_eq!(acc.stdout_line_buf, "");
    for (tail, language) in [
        ("test three ... FAILED", RemoteExecLanguage::Rust),
        ("test four ... ignored", RemoteExecLanguage::Rust),
        ("test five ... bench: 7 ns/iter", RemoteExecLanguage::Rust),
        ("--- PASS: TestSix (0.00s)", RemoteExecLanguage::Go),
        (
            "{\"reason\":\"compiler-message\",\"message\":{\"message\":\"m\"}}",
            RemoteExecLanguage::Rust,
        ),
        ("test seven ... ok", RemoteExecLanguage::Cpp),
    ] {
        acc.stdout_line_buf = tail.into();
        assert!(acc.flush(language, &mut server).await.unwrap());
    }
    assert_eq!(
        (
            acc.tests_passed,
            acc.tests_failed,
            acc.tests_skipped,
            acc.benches.len()
        ),
        (3, 1, 1, 1)
    );
    assert_eq!(acc.diagnostics.len(), 1);
    assert_eq!(acc.diagnostics[0].level, "error");
    assert_eq!(received(server, client).await.len(), 7);
}

#[tokio::test]
async fn oversized_output_is_dropped_instead_of_buffered() {
    let (mut server, client) = pair().await;
    let mut acc = JsonStreamAccumulator::new();
    let lang = RemoteExecLanguage::Rust;
    let big = "x".repeat(MAX_JSON_LINE_BUFFER_BYTES + 1);
    // A complete line over the limit is discarded, the line after it still parses.
    let chunk = format!("{big}\ntest ok_after ... ok\n");
    assert!(
        acc.push_chunk(chunk.as_bytes(), lang, &mut server)
            .await
            .unwrap()
    );
    assert_eq!((acc.tests_passed, acc.stdout_line_buf.as_str()), (1, ""));
    // Without a newline the buffer is cleared once it passes the limit.
    assert!(
        acc.push_chunk(big.as_bytes(), lang, &mut server)
            .await
            .unwrap()
    );
    assert_eq!(acc.stdout_line_buf, "");
    // A tail over the limit is not parsed by flush either.
    acc.stdout_line_buf = format!("test {big} ... ok");
    assert!(acc.flush(lang, &mut server).await.unwrap());
    assert_eq!((acc.tests_passed, acc.stdout_line_buf.as_str()), (1, ""));
    assert_eq!(
        received(server, client).await,
        vec![passed("ok_after", None)]
    );
}
