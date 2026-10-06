/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Public MCP tests for conservative TypeScript function safe-delete.

use futures_util::{SinkExt, StreamExt};
use prod_code_mcp::protocol::{McpContentItem, McpToolCallResult};
use prod_code_protocol::{
    ProdCodeCodec, ShadowHypothesisResult, ShadowRunRequest, ShadowRunResponse, WireMessage,
};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tokio::net::TcpListener;
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::codec::Framed;
use tokio_util::sync::CancellationToken;

const CONFIG: &str = r#"{"compilerOptions":{"target":"ES2022","module":"ESNext","strict":true,"noEmit":true},"include":["src/**/*.ts"]}"#;
const SOURCE: &str = "export {};\r\n\r\nconst emoji = \"🙂\";\r\nfunction hidden(): number {\r\n  return 7;\r\n}\r\nvoid emoji;\r\n";

#[derive(Clone)]
enum ShadowReply {
    Pass,
    Fail,
    Error,
    Malformed,
}

struct CompilerProxy {
    addr: SocketAddr,
    cancel: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl CompilerProxy {
    async fn start(upstream: SocketAddr, reply: ShadowReply) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("proxy bind");
        let addr = listener.local_addr().expect("proxy address");
        let cancel = CancellationToken::new();
        let accepting = cancel.clone();
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                let accepted = tokio::select! {
                    () = accepting.cancelled() => break,
                    Some(_) = connections.join_next(), if !connections.is_empty() => continue,
                    accepted = listener.accept() => accepted,
                };
                let Ok((client, _)) = accepted else { break };
                let cancellation = accepting.clone();
                let reply = reply.clone();
                connections.spawn(async move {
                    let server = tokio::select! {
                        () = cancellation.cancelled() => return,
                        server = tokio::net::TcpStream::connect(upstream) => server,
                    };
                    let Ok(server) = server else {
                        return;
                    };
                    let mut client = Framed::new(client, ProdCodeCodec::new());
                    let mut server = Framed::new(server, ProdCodeCodec::new());
                    loop {
                        tokio::select! {
                            () = cancellation.cancelled() => return,
                            incoming = client.next() => match incoming {
                                Some(Ok(WireMessage::ShadowRunRequest(request))) => {
                                    let response = WireMessage::ShadowRunResponse(shadow_response(request, &reply));
                                    let sent = tokio::select! {
                                        () = cancellation.cancelled() => return,
                                        sent = client.send(response) => sent,
                                    };
                                    if sent.is_err() {
                                        return;
                                    }
                                }
                                Some(Ok(message)) => {
                                    let sent = tokio::select! {
                                        () = cancellation.cancelled() => return,
                                        sent = server.send(message) => sent,
                                    };
                                    if sent.is_err() { return; }
                                }
                                _ => return,
                            },
                            outgoing = server.next() => match outgoing {
                                Some(Ok(message)) => {
                                    let sent = tokio::select! {
                                        () = cancellation.cancelled() => return,
                                        sent = client.send(message) => sent,
                                    };
                                    if sent.is_err() { return; }
                                }
                                _ => return,
                            },
                        }
                    }
                });
            }
            connections.abort_all();
            while connections.join_next().await.is_some() {}
        });
        Self {
            addr,
            cancel,
            task: Some(task),
        }
    }

    async fn shutdown(mut self) {
        self.cancel.cancel();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.task.as_mut().expect("proxy task"),
        )
        .await
        .expect("proxy shutdown timed out")
        .expect("proxy task panicked");
        self.task.take();
    }
}

impl Drop for CompilerProxy {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

fn shadow_response(request: ShadowRunRequest, reply: &ShadowReply) -> ShadowRunResponse {
    let expected = [
        "tsc",
        "--noEmit",
        "--pretty",
        "false",
        "--incremental",
        "false",
        "--project",
        "tsconfig.json",
    ];
    let valid = request.command.iter().map(String::as_str).eq(expected)
        && request.env.is_empty()
        && request.timeout_secs == 120
        && request.parallel == 1
        && request.tail_bytes == 16 * 1024
        && request.hypotheses.len() == 1
        && request.hypotheses[0].name == "typescript-compiler-verification"
        && request.hypotheses[0].files.len() == 1
        && request.hypotheses[0].files[0].relative_path == "src/main.ts"
        && request.hypotheses[0].files[0].content.is_some();
    if !valid || matches!(reply, ShadowReply::Malformed) {
        return ShadowRunResponse {
            server_workspace_root: request.client_workspace_root,
            mode: "unknown".into(),
            results: Vec::new(),
            error: None,
        };
    }
    if matches!(reply, ShadowReply::Error) {
        return ShadowRunResponse {
            server_workspace_root: request.client_workspace_root,
            mode: "overlay".into(),
            results: Vec::new(),
            error: Some("compiler unavailable".into()),
        };
    }
    let (exit_code, output) = match reply {
        ShadowReply::Fail => (1, b"src/main.ts(1,1): error TS2304: missing".to_vec()),
        _ => (0, Vec::new()),
    };
    ShadowRunResponse {
        server_workspace_root: request.client_workspace_root,
        mode: "overlay".into(),
        results: vec![ShadowHypothesisResult {
            name: "typescript-compiler-verification".into(),
            exit_code: Some(exit_code),
            duration_ms: 1,
            timed_out: false,
            error: None,
            output_len: output.len() as u64,
            output_tail: (!output.is_empty()).then_some(output),
        }],
        error: None,
    }
}

fn fixture(source: &str) -> Workspace {
    Workspace::new(&[("tsconfig.json", CONFIG), ("src/main.ts", source)])
}

fn position(source: &str, needle: &str) -> (u32, u32) {
    let offset = source.find(needle).expect("needle");
    let before = &source[..offset];
    (
        before.matches('\n').count() as u32 + 1,
        before
            .rsplit('\n')
            .next()
            .unwrap_or_default()
            .encode_utf16()
            .count() as u32
            + 1,
    )
}

fn lsp_position(source: &str, offset: usize) -> Value {
    let before = &source[..offset];
    json!({
        "line": before.matches('\n').count(),
        "character": before.rsplit('\n').next().unwrap_or_default().encode_utf16().count()
    })
}

fn declaration_symbol(source: &str, name: &str) -> Value {
    let start = source.find("function").expect("function");
    let name_start = source.find(name).expect("name");
    let tail = &source[start..];
    let end = tail
        .rfind('}')
        .map(|offset| start + offset + 1)
        .or_else(|| tail.find('\n').map(|offset| start + offset))
        .unwrap_or(source.len());
    json!([{
        "name": name,
        "kind": 12,
        "range": { "start": lsp_position(source, start), "end": lsp_position(source, end) },
        "selectionRange": {
            "start": lsp_position(source, name_start),
            "end": lsp_position(source, name_start + name.len())
        }
    }])
}

fn declaration_reference(path: &Path, source: &str, name: &str) -> Value {
    let start = source.find(name).expect("name");
    json!([{
        "uri": url::Url::from_file_path(path).unwrap().to_string(),
        "range": {
            "start": lsp_position(source, start),
            "end": lsp_position(source, start + name.len())
        }
    }])
}

async fn gateway(source: &'static str, references: Value) -> ScriptedGateway {
    ScriptedGateway::start(move |method, _| match method {
        "textDocument/documentSymbol" => declaration_symbol(source, "hidden"),
        "textDocument/references" => references.clone(),
        _ => Value::Null,
    })
    .await
}

async fn call(
    remote: SocketAddr,
    fixture: &Workspace,
    source: &str,
    needle: &str,
    force: bool,
) -> McpToolCallResult {
    let (line, character) = position(source, needle);
    prod_code_mcp::tools::execute_tool(
        remote,
        &fixture.root(),
        "code_safe_delete",
        json!({
            "path": "src/main.ts",
            "line": line,
            "character": character,
            "force": force
        }),
    )
    .await
    .expect("MCP result")
}

fn text(result: &McpToolCallResult) -> String {
    result
        .content
        .iter()
        .map(|item| {
            let McpContentItem::Text { text } = item;
            text.as_str()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, dir: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).expect("read dir") {
            let path = entry.expect("entry").path();
            if path.file_name().is_some_and(|name| name == ".git") {
                continue;
            }
            if path.is_dir() {
                visit(root, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}

#[tokio::test]
async fn unused_private_function_is_deleted_with_utf16_and_crlf_evidence() {
    let fixture = fixture(SOURCE);
    let references = declaration_reference(&fixture.path("src/main.ts"), SOURCE, "hidden");
    let gateway = gateway(SOURCE, references).await;
    let proxy = CompilerProxy::start(gateway.addr(), ShadowReply::Pass).await;
    let result = call(proxy.addr, &fixture, SOURCE, "hidden", true).await;
    proxy.shutdown().await;
    let output = text(&result);
    assert!(
        !result.is_error && output.contains("compiler-verified"),
        "{output}"
    );
    assert_eq!(
        fixture.read("src/main.ts"),
        SOURCE.replace("function hidden(): number {\r\n  return 7;\r\n}", "")
    );
}

#[tokio::test]
async fn semantic_use_refuses_even_with_force_and_preserves_every_byte() {
    let source = "export {};\nfunction hidden(): number { return 7; }\nconst kept = hidden();\n";
    let fixture = fixture(source);
    let mut references = declaration_reference(&fixture.path("src/main.ts"), source, "hidden")
        .as_array()
        .unwrap()
        .clone();
    let use_at = source.rfind("hidden").unwrap();
    references.push(json!({
        "uri": url::Url::from_file_path(fixture.path("src/main.ts")).unwrap().to_string(),
        "range": {
            "start": lsp_position(source, use_at),
            "end": lsp_position(source, use_at + "hidden".len())
        }
    }));
    let symbols = declaration_symbol(source, "hidden");
    let gateway = ScriptedGateway::start(move |method, _| match method {
        "textDocument/documentSymbol" => symbols.clone(),
        "textDocument/references" => Value::Array(references.clone()),
        _ => Value::Null,
    })
    .await;
    let before = snapshot(&fixture.root());
    let result = call(gateway.addr(), &fixture, source, "hidden", true).await;
    assert!(result.is_error && text(&result).contains("still referenced"));
    assert_eq!(snapshot(&fixture.root()), before);
}

#[tokio::test]
async fn malformed_evidence_and_compiler_failures_are_transactional_refusals() {
    for (symbols, references, expected) in [
        (Value::Null, Value::Null, "usable list"),
        (
            declaration_symbol(SOURCE, "hidden"),
            json!([]),
            "listed no location",
        ),
        (
            declaration_symbol(SOURCE, "hidden"),
            Value::Null,
            "listed no location",
        ),
        (
            declaration_symbol(SOURCE, "hidden"),
            json!("malformed"),
            "listed no location",
        ),
    ] {
        let fixture = fixture(SOURCE);
        let gateway = ScriptedGateway::start(move |method, _| match method {
            "textDocument/documentSymbol" => symbols.clone(),
            "textDocument/references" => references.clone(),
            _ => Value::Null,
        })
        .await;
        let before = snapshot(&fixture.root());
        let result = call(gateway.addr(), &fixture, SOURCE, "hidden", true).await;
        assert!(
            result.is_error && text(&result).contains(expected),
            "{}",
            text(&result)
        );
        assert_eq!(snapshot(&fixture.root()), before);
    }

    for (reply, expected) in [
        (ShadowReply::Fail, "does not compile"),
        (ShadowReply::Error, "compiler evidence is unavailable"),
        (ShadowReply::Malformed, "compiler evidence is unavailable"),
    ] {
        let fixture = fixture(SOURCE);
        let references = declaration_reference(&fixture.path("src/main.ts"), SOURCE, "hidden");
        let gateway = gateway(SOURCE, references).await;
        let proxy = CompilerProxy::start(gateway.addr(), reply).await;
        let before = snapshot(&fixture.root());
        let result = call(proxy.addr, &fixture, SOURCE, "hidden", true).await;
        proxy.shutdown().await;
        assert!(
            result.is_error && text(&result).contains(expected),
            "{}",
            text(&result)
        );
        assert_eq!(snapshot(&fixture.root()), before);
    }
}

#[tokio::test]
async fn exports_unsupported_syntax_dynamic_sources_and_configs_refuse_unchanged() {
    for (source, expected) in [
        ("export function hidden(): void {}\n", "exported"),
        (
            "export {};\nasync function hidden(): Promise<void> {}\n",
            "async",
        ),
        (
            "export {};\nfunction* hidden(): Generator<number> {}\n",
            "generator",
        ),
        (
            "export {};\nfunction hidden<T>(): T { throw 1; }\n",
            "generic",
        ),
        ("export {};\nfunction hidden(): void;\n", "overload"),
        (
            "export {};\nfunction hidden(): void { eval(\"x\"); }\n",
            "dynamic",
        ),
        (
            "export {};\nfunction hidden(): void { return /x/; }\n",
            "regular-expression",
        ),
        (
            "// @generated\nexport {};\nfunction hidden(): void {}\n",
            "generated",
        ),
    ] {
        let fixture = fixture(source);
        let before = snapshot(&fixture.root());
        let symbols = declaration_symbol(source, "hidden");
        let gateway = ScriptedGateway::start(move |method, _| match method {
            "textDocument/documentSymbol" => symbols.clone(),
            _ => Value::Null,
        })
        .await;
        let result = call(gateway.addr(), &fixture, source, "hidden", true).await;
        let output = text(&result);
        assert!(output.contains(expected), "{expected}: {output}");
        assert_eq!(snapshot(&fixture.root()), before);
    }

    for config in [
        r#"{"extends":"../base.json","compilerOptions":{"module":"ESNext"},"include":["src"]}"#,
        r#"{"compilerOptions":{"module":"CommonJS"},"include":["src"]}"#,
        r#"{"compilerOptions":{"module":"ESNext","allowJs":true},"include":["src"]}"#,
        r#"{"compilerOptions":{"module":"ESNext"},"files":["src/main.ts"]}"#,
    ] {
        let fixture = Workspace::new(&[("tsconfig.json", config), ("src/main.ts", SOURCE)]);
        let before = snapshot(&fixture.root());
        let result = call(
            SocketAddr::from(([127, 0, 0, 1], 9)),
            &fixture,
            SOURCE,
            "hidden",
            true,
        )
        .await;
        let output = text(&result);
        assert!(
            output.contains("configuration")
                || output.contains("module")
                || output.contains("allowJs")
                || output.contains("files"),
            "{output}"
        );
        assert_eq!(snapshot(&fixture.root()), before);
    }
}

#[tokio::test]
async fn hidden_direct_eval_spellings_refuse_with_force_and_preserve_every_byte() {
    for (source, expected) in [
        (
            "export {};\nconst escaped = \"\\\\🙂\";\nfunction hidden(): number { return 7; }\nvoid escaped;\nvoid (eval)(\"hidden()\");\n",
            "dynamic",
        ),
        (
            "export {};\nfunction hidden(): number { return 7; }\nvoid eval!(\"hidden()\");\n",
            "dynamic",
        ),
        (
            "export {};\nfunction hidden(): number { return 7; }\nvoid \\u0065val(\"hidden()\");\n",
            "escaped identifiers",
        ),
    ] {
        let fixture = fixture(source);
        let references = declaration_reference(&fixture.path("src/main.ts"), source, "hidden");
        let gateway = gateway(source, references).await;
        let proxy = CompilerProxy::start(gateway.addr(), ShadowReply::Pass).await;
        let before = snapshot(&fixture.root());
        let result = call(proxy.addr, &fixture, source, "hidden", true).await;
        proxy.shutdown().await;
        let output = text(&result);
        assert!(
            result.is_error && output.contains(expected),
            "unsafe deletion was not refused: {output}"
        );
        assert_eq!(snapshot(&fixture.root()), before);
    }
}

#[tokio::test]
async fn compiler_proxy_cancellation_reaps_owned_connections() {
    let gateway = ScriptedGateway::start(|_, _| Value::Null).await;
    let proxy = CompilerProxy::start(gateway.addr(), ShadowReply::Pass).await;
    let mut connection = Framed::new(
        tokio::net::TcpStream::connect(proxy.addr)
            .await
            .expect("proxy connection"),
        ProdCodeCodec::new(),
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        connection
            .send(WireMessage::LspPayload(
                json!({"jsonrpc": "2.0", "id": 42, "method": "initialize", "params": {}})
                    .to_string(),
            ))
            .await
            .expect("proxy request");
        let Some(Ok(WireMessage::LspPayload(response))) = connection.next().await else {
            panic!("accepted proxy must forward a response");
        };
        let response: Value = serde_json::from_str(&response).expect("proxy response JSON");
        assert_eq!(response["id"], json!(42));
    })
    .await
    .expect("accepted proxy connection must answer before cancellation");
    proxy.shutdown().await;
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(5), connection.next())
            .await
            .expect("accepted connection must close before runtime shutdown")
            .is_none()
    );
}

#[tokio::test]
async fn analyzer_errors_refuse_without_writes() {
    let offline_fixture = fixture(SOURCE);
    let gateway = ScriptedGateway::start(|method, _| match method {
        "textDocument/documentSymbol" => answers::failure("offline"),
        _ => Value::Null,
    })
    .await;
    let before = snapshot(&offline_fixture.root());
    let result = call(gateway.addr(), &offline_fixture, SOURCE, "hidden", false).await;
    assert!(result.is_error && text(&result).contains("could not describe"));
    assert_eq!(snapshot(&offline_fixture.root()), before);
}

#[cfg(unix)]
#[tokio::test]
async fn linked_or_javascript_project_sources_refuse_before_analyzer_dispatch() {
    use std::os::unix::fs::symlink;
    let fixture = fixture(SOURCE);
    std::fs::write(fixture.path("src/other.js"), "export {};\n").unwrap();
    let result = call(
        SocketAddr::from(([127, 0, 0, 1], 9)),
        &fixture,
        SOURCE,
        "hidden",
        true,
    )
    .await;
    let output = text(&result);
    assert!(output.contains("JavaScript"), "{output}");

    std::fs::remove_file(fixture.path("src/other.js")).unwrap();
    symlink("main.ts", fixture.path("src/linked.ts")).unwrap();
    let result = call(
        SocketAddr::from(([127, 0, 0, 1], 9)),
        &fixture,
        SOURCE,
        "hidden",
        true,
    )
    .await;
    let output = text(&result);
    assert!(output.contains("linked path"), "{output}");
}

#[tokio::test]
async fn direct_entrypoint_refuses_wrong_files_modules_and_utf16_positions() {
    let wrong = Workspace::new(&[("main.js", "export {};\n")]);
    let error = prod_code_mcp::safe_delete_typescript::delete_function(
        SocketAddr::from(([127, 0, 0, 1], 9)),
        &wrong.root(),
        &wrong.path("main.js"),
        1,
        1,
    )
    .await
    .expect_err("JavaScript is outside the direct entrypoint");
    assert!(format!("{error:#}").contains("not a supported TypeScript source"));

    let script = "function hidden(): void {}\n";
    let no_module = fixture(script);
    let error = prod_code_mcp::safe_delete_typescript::delete_function(
        SocketAddr::from(([127, 0, 0, 1], 9)),
        &no_module.root(),
        &no_module.path("src/main.ts"),
        1,
        10,
    )
    .await
    .expect_err("global scripts are refused");
    assert!(format!("{error:#}").contains("not a contained TypeScript ES module"));

    let invalid = fixture(SOURCE);
    let error = prod_code_mcp::safe_delete_typescript::delete_function(
        SocketAddr::from(([127, 0, 0, 1], 9)),
        &invalid.root(),
        &invalid.path("src/main.ts"),
        999,
        999,
    )
    .await
    .expect_err("an absent UTF-16 position is refused");
    assert!(format!("{error:#}").contains("not a valid UTF-16 source position"));
}

#[tokio::test]
async fn type_only_marker_in_preserve_mode_refuses_global_function_deletion() {
    const SOURCE: &str = "export type Marker = number;\nfunction hidden(): number { return 7; }\n(globalThis as any)[\"output\"] = (globalThis as any)[\"hidden\"]();\n";
    let fixture = fixture(SOURCE);
    let config = fixture.path("tsconfig.json");
    let mut json: Value = serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
    json["compilerOptions"]["module"] = json!("preserve");
    std::fs::write(config, serde_json::to_string(&json).unwrap()).unwrap();
    let references = declaration_reference(&fixture.path("src/main.ts"), SOURCE, "hidden");
    let gateway = gateway(SOURCE, references).await;
    let proxy = CompilerProxy::start(gateway.addr(), ShadowReply::Pass).await;
    let before = snapshot(&fixture.root());
    let result = call(proxy.addr, &fixture, SOURCE, "hidden", true).await;
    proxy.shutdown().await;
    assert!(
        result.is_error,
        "a type-only marker cannot prove runtime module privacy: {}",
        text(&result)
    );
    assert_eq!(snapshot(&fixture.root()), before);
}
