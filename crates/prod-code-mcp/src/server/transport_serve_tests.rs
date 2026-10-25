/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The stdio loop: oversized frames, cancelled calls, a transport that breaks, the shape of
//! every handler outcome on the wire, and the response bound when nothing can be cut.

use super::transport::{
    MAX_JSONRPC_FRAME_BYTES, bound_serialized_response, format_handled_response, serve_mcp_requests,
};
use futures_util::StreamExt;
use prod_code_protocol::{ProdCodeCodec, WireMessage};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, ReadBuf};
use tokio::net::TcpListener;
use tokio_util::codec::Framed;

const MARKER: &str = "\n[... output truncated to avoid exceeding MCP frame line limits]\n";

/// The client end of an in-memory transport to a server pointed at `remote`.
fn serve(
    remote: SocketAddr,
) -> (
    BufReader<tokio::io::DuplexStream>,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    let (client, server) = tokio::io::duplex(64 * 1024);
    let (read, write) = tokio::io::split(server);
    let handle = tokio::spawn(serve_mcp_requests(
        remote,
        std::path::PathBuf::from("/tmp"),
        None,
        false,
        BufReader::new(read),
        write,
    ));
    (BufReader::new(client), handle)
}

pub(super) fn nowhere() -> SocketAddr {
    "127.0.0.1:1".parse().unwrap()
}

pub(super) async fn send(client: &mut BufReader<tokio::io::DuplexStream>, line: &str) {
    client.get_mut().write_all(line.as_bytes()).await.unwrap();
    client.get_mut().write_all(b"\n").await.unwrap();
}

pub(super) async fn next_response(client: &mut BufReader<tokio::io::DuplexStream>) -> Value {
    let mut line = String::new();
    client.read_line(&mut line).await.unwrap();
    serde_json::from_str(line.trim()).unwrap_or_else(|e| panic!("{e}: {line:?}"))
}

/// A node that takes a status request and never answers it.
async fn silent_node() -> (SocketAddr, TcpListener) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    (listener.local_addr().unwrap(), listener)
}

async fn await_status_request(
    listener: &TcpListener,
) -> Framed<tokio::net::TcpStream, ProdCodeCodec> {
    let (socket, _) = listener.accept().await.unwrap();
    let mut framed = Framed::new(socket, ProdCodeCodec::new());
    assert!(matches!(
        framed.next().await.unwrap().unwrap(),
        WireMessage::StatusRequest
    ));
    framed
}

#[tokio::test]
async fn a_line_over_the_frame_limit_is_refused_and_the_next_request_still_served() {
    let (mut client, handle) = serve(nowhere());
    let huge = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"{}\"}}",
        "x".repeat(MAX_JSONRPC_FRAME_BYTES)
    );
    send(&mut client, &huge).await;
    assert_eq!(
        next_response(&mut client).await,
        json!({
            "jsonrpc": "2.0", "id": null,
            "error": { "code": -32600, "message": "Request exceeds maximum JSON-RPC frame size" }
        })
    );
    send(&mut client, r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#).await;
    assert_eq!(
        next_response(&mut client).await,
        json!({"jsonrpc":"2.0","id":2,"result":{}})
    );
    drop(client);
    assert!(handle.await.unwrap().is_ok());
}

#[tokio::test]
async fn a_cancelled_call_is_dropped_without_an_answer_and_an_unknown_one_is_ignored() {
    let (addr, listener) = silent_node().await;
    let (mut client, handle) = serve(addr);
    let call = |id: &str| {
        format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"code_status"}}}}"#
        )
    };

    send(&mut client, &call("5")).await;
    let _stuck = await_status_request(&listener).await;
    send(
        &mut client,
        r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":5}}"#,
    )
    .await;

    send(&mut client, &call("\"abc\"")).await;
    let _stuck_too = await_status_request(&listener).await;
    send(
        &mut client,
        r#"{"jsonrpc":"2.0","method":"$/cancelRequest","params":{"id":"abc"}}"#,
    )
    .await;

    // Nothing to cancel under this id, and a cancellation naming no id at all.
    send(
        &mut client,
        r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":99}}"#,
    )
    .await;
    send(
        &mut client,
        r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{}}"#,
    )
    .await;

    // The next line on the wire is the ping's answer: neither cancelled call produced one.
    send(&mut client, r#"{"jsonrpc":"2.0","id":6,"method":"ping"}"#).await;
    assert_eq!(
        next_response(&mut client).await,
        json!({"jsonrpc":"2.0","id":6,"result":{}})
    );
    drop(client);
    assert!(handle.await.unwrap().is_ok());
}

/// A transport whose far end is gone.
struct Broken;

impl AsyncWrite for Broken {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl AsyncRead for Broken {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Err(std::io::Error::other("stdin went away")))
    }
}

async fn serve_over<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    resumed: bool,
    reader: R,
    writer: W,
) -> anyhow::Result<()> {
    serve_mcp_requests(
        nowhere(),
        "/tmp".into(),
        None,
        resumed,
        BufReader::new(reader),
        writer,
    )
    .await
}

#[tokio::test]
async fn a_transport_that_breaks_ends_the_loop_the_way_each_side_of_it_deserves() {
    // The writer is gone while a plain request is answered: the client left, a clean stop.
    let (mut client, server) = tokio::io::duplex(1024);
    client
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n")
        .await
        .unwrap();
    assert!(serve_over(false, server, Broken).await.is_ok());

    // The writer is gone when a tool call finishes: that is an error of the transport.
    let (mut client, server) = tokio::io::duplex(1024);
    client
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"no_such_tool\"}}\n")
        .await
        .unwrap();
    let err = serve_over(false, server, Broken).await.unwrap_err();
    assert_eq!(err.to_string(), "broken pipe");

    // ... or when the resumed session announces its tools.
    let (_client, server) = tokio::io::duplex(1024);
    let err = serve_over(true, server, Broken).await.unwrap_err();
    assert_eq!(err.to_string(), "broken pipe");

    // The reader fails: the loop says what it was reading.
    let err = serve_over(false, Broken, Vec::new()).await.unwrap_err();
    assert_eq!(
        format!("{err:#}"),
        "Failed reading from stdin: stdin went away"
    );
}

#[test]
fn every_handler_outcome_has_its_own_line_on_the_wire() {
    let parse = |line: Option<String>| -> Value {
        let line = line.expect("a line");
        assert!(
            line.ends_with('\n') && !line[..line.len() - 1].contains('\n'),
            "{line:?}"
        );
        serde_json::from_str(&line).unwrap()
    };

    assert_eq!(format_handled_response(Some(json!(1)), Ok(Ok(None))), None);
    let ok = json!({"jsonrpc":"2.0","id":1,"result":{"ok":true}});
    assert_eq!(
        parse(format_handled_response(
            Some(json!(1)),
            Ok(Ok(Some(ok.clone())))
        )),
        ok
    );

    let internal = |id: Option<Value>| {
        parse(format_handled_response(
            id,
            Ok(Err(anyhow::anyhow!("boom"))),
        ))
    };
    assert_eq!(
        internal(Some(json!(7))),
        json!({"jsonrpc":"2.0","id":7,"error":{"code":-32603,"message":"Internal error: boom"}})
    );
    assert_eq!(internal(None)["id"], Value::Null);

    let panicked = |id: Option<Value>, payload: Box<dyn std::any::Any + Send>| {
        parse(format_handled_response(id, Err(payload)))
    };
    assert_eq!(
        panicked(Some(json!("k")), Box::new("static message")),
        json!({"jsonrpc":"2.0","id":"k","error":{"code":-32603,"message":"Internal error (panic): static message"}})
    );
    assert_eq!(
        panicked(None, Box::new(String::from("owned message")))["error"]["message"],
        "Internal error (panic): owned message"
    );
    let unknown = panicked(Some(json!(8)), Box::new(42_u8));
    assert_eq!(
        unknown["error"]["message"],
        "Internal error (panic): unknown panic"
    );
    assert_eq!(unknown["id"], 8);
}

fn tool_result(blocks: Vec<Value>, id: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":{"content":blocks,"isError":false}})
}

#[test]
fn a_response_that_cannot_be_cut_to_fit_is_replaced_by_the_smallest_valid_answer() {
    let text = |s: String| json!({"type":"text","text":s});
    let image = json!({"type":"image","data":"d".repeat(MAX_JSONRPC_FRAME_BYTES + 1)});
    let marker_only = json!({"type":"text","text":MARKER.trim_start()});

    // The text is cut to nothing and the rest still does not fit: a marker under the same id.
    let out = bound_serialized_response(
        tool_result(vec![text("t".repeat(100)), image.clone()], json!(3)),
        MAX_JSONRPC_FRAME_BYTES,
    );
    assert_eq!(
        serde_json::from_str::<Value>(&out).unwrap(),
        tool_result(vec![marker_only.clone()], json!(3))
    );

    // An id too long to repeat is dropped.
    let out = bound_serialized_response(
        tool_result(
            vec![text("t".repeat(100)), image],
            json!("i".repeat(MAX_JSONRPC_FRAME_BYTES)),
        ),
        MAX_JSONRPC_FRAME_BYTES,
    );
    assert_eq!(
        serde_json::from_str::<Value>(&out).unwrap(),
        tool_result(vec![marker_only], Value::Null)
    );

    // A frame limit smaller than any answer: the fixed one.
    let out = bound_serialized_response(tool_result(vec![text("t".repeat(100))], json!(1)), 50);
    assert_eq!(
        out,
        r#"{"jsonrpc":"2.0","id":null,"result":{"content":[{"type":"text","text":"[... output truncated to avoid exceeding MCP frame line limits]\n"}],"isError":false}}"#
    );

    // Two texts too big together: the first is cut as far as it goes, then the second to fit.
    let big = "a".repeat(600_000);
    let out = bound_serialized_response(
        tool_result(
            vec![text(big.clone()), json!({"type":"text"}), text(big)],
            json!(4),
        ),
        MAX_JSONRPC_FRAME_BYTES,
    );
    assert!(out.len() <= MAX_JSONRPC_FRAME_BYTES);
    let bounded: Value = serde_json::from_str(&out).unwrap();
    let blocks = bounded["result"]["content"].as_array().unwrap();
    assert_eq!(blocks.len(), 3);
    assert_eq!(blocks[0]["text"], format!("{}{MARKER}", "a".repeat(4096)));
    assert_eq!(blocks[1], json!({"type":"text"}));
    assert_eq!(
        blocks[2]["text"],
        format!("{}{MARKER}", "a".repeat(262_144))
    );
}
