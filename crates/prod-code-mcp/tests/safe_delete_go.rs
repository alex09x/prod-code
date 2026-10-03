//! Go function safe-delete through the public MCP boundary.
//!
//! Success and semantic-use refusals use real gopls. Scripted answers are reserved for malformed
//! evidence and remote-compiler failure modes that a healthy server cannot produce on demand.

use futures_util::{SinkExt, StreamExt};
use prod_code_mcp::protocol::{McpContentItem, McpToolCallResult};
use prod_code_protocol::{
    ProdCodeCodec, ShadowHypothesisResult, ShadowRunRequest, ShadowRunResponse, WireMessage,
};
use prod_code_testkit::gopls::{GoModule, GoplsBridge, require_go_toolchain};
use prod_code_testkit::{ScriptedGateway, answers};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_util::codec::Framed;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
enum ShadowReply {
    Pass,
    Fail(&'static str),
    Error,
    Malformed,
    Mutate(PathBuf, &'static str),
}

/// An owned proxy that forwards language requests to a gateway and supplies controlled shadow
/// responses. Cancellation is scoped to this listener and every connection it accepted.
struct CompilerProxy {
    addr: SocketAddr,
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

impl CompilerProxy {
    async fn start(upstream: SocketAddr, reply: ShadowReply) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind proxy");
        let addr = listener.local_addr().expect("proxy address");
        let cancel = CancellationToken::new();
        let accept_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    () = accept_cancel.cancelled() => return,
                    accepted = listener.accept() => accepted,
                };
                let Ok((client, _)) = accepted else {
                    return;
                };
                let connection_cancel = accept_cancel.clone();
                let reply = reply.clone();
                tokio::spawn(async move {
                    let Ok(server) = tokio::net::TcpStream::connect(upstream).await else {
                        return;
                    };
                    let mut client = Framed::new(client, ProdCodeCodec::new());
                    let mut server = Framed::new(server, ProdCodeCodec::new());
                    loop {
                        tokio::select! {
                            () = connection_cancel.cancelled() => return,
                            incoming = client.next() => match incoming {
                                Some(Ok(WireMessage::ShadowRunRequest(request))) => {
                                    let response = shadow_response(request, &reply);
                                    if client.send(WireMessage::ShadowRunResponse(response)).await.is_err() {
                                        return;
                                    }
                                }
                                Some(Ok(message)) => {
                                    if server.send(message).await.is_err() {
                                        return;
                                    }
                                }
                                _ => return,
                            },
                            outgoing = server.next() => match outgoing {
                                Some(Ok(message)) => {
                                    if client.send(message).await.is_err() {
                                        return;
                                    }
                                }
                                _ => return,
                            },
                        }
                    }
                });
            }
        });
        Self { addr, cancel, task }
    }

    fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for CompilerProxy {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.task.abort();
    }
}

fn shadow_response(request: ShadowRunRequest, reply: &ShadowReply) -> ShadowRunResponse {
    let expected = [
        "go",
        "test",
        "-exec=true",
        "-run=^$",
        "-mod=readonly",
        "./...",
    ];
    let valid = request.command.iter().map(String::as_str).eq(expected)
        && request.env == [("GOTOOLCHAIN".into(), "local".into())]
        && request.timeout_secs == 120
        && request.parallel == 1
        && request.tail_bytes == 16 * 1024
        && request.hypotheses.len() == 1
        && request.hypotheses[0].name == "go-compiler-verification"
        && !request.hypotheses[0].files.is_empty()
        && request.hypotheses[0]
            .files
            .iter()
            .all(|file| file.relative_path.ends_with(".go") && file.content.is_some());
    if !valid {
        return ShadowRunResponse {
            server_workspace_root: request.client_workspace_root,
            mode: String::new(),
            results: Vec::new(),
            error: Some("malformed compiler request".into()),
        };
    }
    if let ShadowReply::Mutate(path, text) = reply {
        std::fs::write(path, text).expect("concurrent writer");
    }
    if matches!(reply, ShadowReply::Error) {
        return ShadowRunResponse {
            server_workspace_root: request.client_workspace_root,
            mode: "overlay".into(),
            results: Vec::new(),
            error: Some("compiler service unavailable".into()),
        };
    }
    if matches!(reply, ShadowReply::Malformed) {
        return ShadowRunResponse {
            server_workspace_root: request.client_workspace_root,
            mode: "mystery".into(),
            results: Vec::new(),
            error: None,
        };
    }
    let (exit_code, output) = match reply {
        ShadowReply::Fail(output) => (1, output.as_bytes().to_vec()),
        _ => (0, Vec::new()),
    };
    ShadowRunResponse {
        server_workspace_root: request.client_workspace_root,
        mode: "overlay".into(),
        results: vec![ShadowHypothesisResult {
            name: "go-compiler-verification".into(),
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

fn text_of(result: &McpToolCallResult) -> String {
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

async fn call(
    remote: SocketAddr,
    fixture: &GoModule,
    rel: &str,
    needle: &str,
    force: bool,
) -> McpToolCallResult {
    let text = fixture.read(rel);
    let offset = text
        .find(needle)
        .unwrap_or_else(|| panic!("{needle} missing from {rel}"));
    let before = &text[..offset];
    let line = before.matches('\n').count() as u32 + 1;
    let col = before
        .rsplit('\n')
        .next()
        .unwrap_or_default()
        .encode_utf16()
        .count() as u32
        + 1;
    prod_code_mcp::tools::execute_tool(
        remote,
        fixture.root(),
        "code_safe_delete",
        json!({
            "path": rel,
            "line": line,
            "character": col,
            "force": force
        }),
    )
    .await
    .expect("safe delete returns an MCP result")
}

async fn refused(
    remote: SocketAddr,
    fixture: &GoModule,
    rel: &str,
    needle: &str,
    force: bool,
) -> String {
    let before = fixture.snapshot();
    let result = call(remote, fixture, rel, needle, force).await;
    let text = text_of(&result);
    assert!(result.is_error, "{needle}: {text}");
    assert_eq!(
        fixture.snapshot(),
        before,
        "{needle}: refusal changed the fixture"
    );
    text
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unreferenced_go_function_is_deleted_through_public_mcp() {
    require_go_toolchain();
    let source = "package main\n\ntype Mystruct struct{ value string }\nvar neighbor = \"🙂\"\nvar π = 1\nvar generatedText = \"// Code generated by fixture. DO NOT EDIT.\"\n\n// target documentation stays byte-for-byte.\n/* 🙂 } */ func unused() Mystruct {\n\treturn Mystruct{value: `raw } { string`} // } is not syntax\n}\n\nfunc keep() { // Code generated by body. DO NOT EDIT.\n}\n\n// main documentation belongs to the neighbor.\nfunc main() { _, _, _ = neighbor, π, generatedText; keep() }\n";
    let fixture = GoModule::new(&[
        ("go.mod", "module example.com/delete\n\ngo 1.22\n"),
        ("main.go", source),
    ]);
    let before = fixture.snapshot();
    let bridge = GoplsBridge::start(&fixture).await;
    let proxy = CompilerProxy::start(bridge.addr(), ShadowReply::Pass).await;

    let result = call(proxy.addr(), &fixture, "main.go", "unused", false).await;
    let output = text_of(&result);
    assert!(!result.is_error, "{output}");
    assert!(output.contains("compiler-verified"), "{output}");
    let expected = source.replace(
        "func unused() Mystruct {\n\treturn Mystruct{value: `raw } { string`} // } is not syntax\n}",
        "",
    );
    assert_eq!(fixture.read("main.go"), expected);
    let after = fixture.snapshot();
    assert_eq!(after.len(), before.len());
    assert_eq!(after.get("go.mod"), before.get("go.mod"));
    assert!(
        fixture
            .read("main.go")
            .contains("// target documentation stays byte-for-byte.")
    );
    assert!(
        fixture
            .read("main.go")
            .contains("// main documentation belongs to the neighbor.")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unreferenced_value_and_pointer_receiver_methods_are_deleted() {
    require_go_toolchain();
    for (declaration, receiver, method, retained) in [
        (
            "type item struct{ value string }",
            "item",
            "unusedValue",
            "item struct{ value string }",
        ),
        (
            "type item struct{ value string }",
            "*item",
            "unusedPointer",
            "item struct{ value string }",
        ),
        (
            "type (\n\titem struct{ value string }\n)",
            "item",
            "unusedGrouped",
            "item struct{ value string }",
        ),
        ("type item []int", "item", "unusedSliceValue", "item []int"),
        (
            "type item [2]int",
            "*item",
            "unusedArrayPointer",
            "item [2]int",
        ),
        (
            "type (\n\titem [2]int\n)",
            "item",
            "unusedGroupedArrayValue",
            "item [2]int",
        ),
        (
            "type (\n\titem []int\n)",
            "*item",
            "unusedGroupedSlicePointer",
            "item []int",
        ),
        (
            "type item struct{}\ntype outer struct { item item }",
            "item",
            "unusedNamedField",
            "outer struct { item item }",
        ),
    ] {
        let source = format!(
            "package main\n\n{declaration}\n\nfunc (value {receiver}) {method}() int {{ return 0 }}\n\nfunc main() {{}}\n"
        );
        let fixture = GoModule::new(&[
            ("go.mod", "module example.com/methoddelete\n\ngo 1.22\n"),
            ("main.go", &source),
        ]);
        std::fs::create_dir(fixture.path("sub")).expect("subpackage directory");
        std::fs::write(
            fixture.path("sub/other.go"),
            "package sub\n\ntype unrelated interface { unusedValue(); unusedPointer() }\n",
        )
        .expect("subpackage source");
        let bridge = GoplsBridge::start(&fixture).await;
        let proxy = CompilerProxy::start(bridge.addr(), ShadowReply::Pass).await;

        let result = call(proxy.addr(), &fixture, "main.go", method, false).await;
        let output = text_of(&result);
        assert!(!result.is_error, "{receiver}: {output}");
        assert!(output.contains("compiler-verified"), "{output}");
        let written = fixture.read("main.go");
        assert!(!written.contains(method), "{written}");
        assert!(written.contains(retained));
        assert!(written.contains("func main() {}"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn constant_array_receiver_deletes_and_parenthesized_alias_refuses() {
    require_go_toolchain();
    let source = "package main\n\nconst Count = 2\ntype item [Count*2]int\n\nfunc (value item) unusedConstantArray() int { return len(value) }\n\nfunc main() {}\n";
    let fixture = GoModule::new(&[
        ("go.mod", "module example.com/constantarray\n\ngo 1.22\n"),
        ("main.go", source),
    ]);
    let (compiled, compile_output) = fixture.go(&["test", "./..."]);
    assert!(compiled, "constant array fixture: {compile_output}");
    let before = fixture.snapshot();
    let mut expected = before.clone();
    expected.insert(
        "main.go".into(),
        source
            .replace(
                "func (value item) unusedConstantArray() int { return len(value) }",
                "",
            )
            .into_bytes(),
    );
    let bridge = GoplsBridge::start(&fixture).await;
    let proxy = CompilerProxy::start(bridge.addr(), ShadowReply::Pass).await;
    let result = call(
        proxy.addr(),
        &fixture,
        "main.go",
        "unusedConstantArray",
        false,
    )
    .await;
    let output = text_of(&result);
    assert!(
        !result.is_error && output.contains("compiler-verified"),
        "{output}"
    );
    assert_eq!(
        fixture.snapshot(),
        expected,
        "constant array deletion snapshot"
    );
    let (compiled, compile_output) = fixture.go(&["test", "./..."]);
    assert!(compiled, "deleted constant array fixture: {compile_output}");

    let source = "package main\n\ntype item struct{}\ntype (\n\tdirect = (((item)))\n\ttransitive = ((direct))\n\t別名 = (transitive)\n)\ntype outer struct { 別名 }\n\nfunc (value item) hidden() {}\nfunc main() {}\n";
    let fixture = GoModule::new(&[
        (
            "go.mod",
            "module example.com/parenthesizedalias\n\ngo 1.22\n",
        ),
        ("main.go", source),
    ]);
    let (compiled, compile_output) = fixture.go(&["test", "./..."]);
    assert!(compiled, "parenthesized alias fixture: {compile_output}");
    let bridge = GoplsBridge::start(&fixture).await;
    let proxy = CompilerProxy::start(bridge.addr(), ShadowReply::Pass).await;
    let output = refused(proxy.addr(), &fixture, "main.go", "hidden", false).await;
    assert!(output.contains("embedded or promoted"), "{output}");
    assert!(output.contains("main.go"), "{output}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn instantiated_alias_embedding_refuses_without_writing() {
    require_go_toolchain();
    let source = "package main\n\ntype item struct{}\ntype alias[T any] = item\ntype outer struct { alias[int] }\n\nfunc (value item) hidden() {}\nfunc main() {}\n";
    for force in [false, true] {
        let fixture = GoModule::new(&[
            (
                "go.mod",
                "module example.com/instantiatedalias\n\ngo 1.24\n",
            ),
            ("main.go", source),
        ]);
        let (compiled, compile_output) = fixture.go(&["test", "./..."]);
        assert!(compiled, "generic alias fixture: {compile_output}");
        let bridge = GoplsBridge::start(&fixture).await;
        let proxy = CompilerProxy::start(bridge.addr(), ShadowReply::Pass).await;
        let output = refused(proxy.addr(), &fixture, "main.go", "hidden", force).await;
        assert!(output.contains("embedded or promoted"), "{output}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn named_alias_array_field_is_not_embedding() {
    require_go_toolchain();
    let source = "package main\n\ntype item struct{}\ntype alias[T any] = item\ntype outer struct { alias [2]int }\n\nfunc (value item) hidden() {}\nfunc main() {}\n";
    let fixture = GoModule::new(&[
        ("go.mod", "module example.com/namedaliasarray\n\ngo 1.24\n"),
        ("main.go", source),
    ]);
    let (compiled, compile_output) = fixture.go(&["test", "./..."]);
    assert!(compiled, "named alias array fixture: {compile_output}");
    let bridge = GoplsBridge::start(&fixture).await;
    let proxy = CompilerProxy::start(bridge.addr(), ShadowReply::Pass).await;
    let result = call(proxy.addr(), &fixture, "main.go", "hidden", false).await;
    let output = text_of(&result);
    assert!(
        !result.is_error && output.contains("compiler-verified"),
        "{output}"
    );
    assert_eq!(
        fixture.read("main.go"),
        source.replace("func (value item) hidden() {}", "")
    );
    let (compiled, compile_output) = fixture.go(&["test", "./..."]);
    assert!(compiled, "deleted named alias fixture: {compile_output}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unsupported_shapes_and_non_name_positions_are_refused_without_writes() {
    require_go_toolchain();
    let fixture = GoModule::new(&[
        ("go.mod", "module example.com/shapes\n\ngo 1.22\n"),
        (
            "main.go",
            "package main\n\ntype item struct{}\n\nfunc Exported() {}\nfunc main() {}\nfunc init() {}\nfunc (item) method() {}\nfunc generic[T any](value T) {}\nfunc π() {}\nfunc target(param int) { _ = param }\nfunc caller() { target(1) }\n",
        ),
    ]);
    let bridge = GoplsBridge::start(&fixture).await;
    let proxy = CompilerProxy::start(bridge.addr(), ShadowReply::Pass).await;

    for (needle, expected, force) in [
        ("Exported", "unexported ASCII", true),
        ("main() {}", "entry point", false),
        ("init() {}", "entry point", false),
        ("method", "named receiver", false),
        ("generic", "generic", false),
        ("π() {}", "ordinary ASCII", false),
        ("param int", "declaration names", false),
        ("_ = param", "declaration names", false),
        ("target(1)", "declaration names", false),
    ] {
        let output = refused(proxy.addr(), &fixture, "main.go", needle, force).await;
        assert!(output.contains(expected), "{needle}: {output}");
    }

    for (source, expected) in [
        (
            "package main\n\n//go:noinline\nfunc hidden() {}\n",
            "directive",
        ),
        (
            "package main\n\n//export hidden\nfunc hidden() {}\n",
            "directive",
        ),
        (
            "package main\n\n//line generated.go:1\nfunc hidden() {}\n",
            "line directives",
        ),
        (
            "// Code generated by fixture. DO NOT EDIT.\npackage main\n\nfunc hidden() {}\n",
            "generated Go source",
        ),
        (
            "/* Copyright fixture */\n// Code generated by fixture. DO NOT EDIT.\npackage main\n\nfunc hidden() {}\n",
            "generated Go source",
        ),
        (
            "// Code generated incorrectly\n// Code generated by fixture. DO NOT EDIT.\npackage main\n\nfunc hidden() {}\n",
            "generated Go source",
        ),
        (
            "// Code generated by fixture. DO NOT EDIT.\r\npackage main\r\n\r\nfunc hidden() {}\r\n",
            "generated Go source",
        ),
        (
            "\u{feff}// Code generated by fixture. DO NOT EDIT.\npackage main\n\nfunc hidden() {}\n",
            "generated Go source",
        ),
    ] {
        let fixture = GoModule::new(&[
            ("go.mod", "module example.com/refuse\n\ngo 1.22\n"),
            ("main.go", source),
        ]);
        let bridge = GoplsBridge::start(&fixture).await;
        let proxy = CompilerProxy::start(bridge.addr(), ShadowReply::Pass).await;
        let output = refused(proxy.addr(), &fixture, "main.go", "hidden() {}", true).await;
        assert!(output.contains(expected), "{output}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_real_reference_shape_blocks_deletion() {
    require_go_toolchain();
    let fixture = GoModule::new(&[
        ("go.mod", "module example.com/uses\n\ngo 1.22\n"),
        (
            "main.go",
            "package main\n\nfunc recursive() { recursive() }\nfunc testOnly() {}\nfunc valueUse() {}\nvar saved = valueUse\nfunc main() { _ = saved }\n",
        ),
        (
            "main_test.go",
            "package main\n\nimport \"testing\"\nfunc TestOnly(t *testing.T) { testOnly() }\n",
        ),
    ]);
    let bridge = GoplsBridge::start(&fixture).await;
    let proxy = CompilerProxy::start(bridge.addr(), ShadowReply::Pass).await;
    for name in ["recursive", "testOnly", "valueUse"] {
        let output = refused(proxy.addr(), &fixture, "main.go", name, true).await;
        assert!(output.contains("still referenced"), "{name}: {output}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn receiver_calls_recursion_values_and_expressions_refuse_unchanged() {
    require_go_toolchain();
    let fixture = GoModule::new(&[
        ("go.mod", "module example.com/methoduses\n\ngo 1.22\n"),
        (
            "main.go",
            "package main\n\ntype item struct{}\n\nfunc (value item) direct() {}\nfunc (value item) recursive() { value.recursive() }\nfunc (value item) methodValue() {}\nfunc (value *item) methodExpression() {}\n\nfunc uses(value item) {\n\tvalue.direct()\n\t_ = value.methodValue\n\t_ = (*item).methodExpression\n}\nfunc main() {}\n",
        ),
    ]);
    let bridge = GoplsBridge::start(&fixture).await;
    let proxy = CompilerProxy::start(bridge.addr(), ShadowReply::Pass).await;
    for method in ["direct", "recursive", "methodValue", "methodExpression"] {
        let output = refused(proxy.addr(), &fixture, "main.go", method, true).await;
        assert!(output.contains("still referenced"), "{method}: {output}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_interface_and_runtime_assertion_obligations_refuse_unchanged() {
    require_go_toolchain();
    let fixture = GoModule::new(&[
        ("go.mod", "module example.com/methodinterfaces\n\ngo 1.22\n"),
        (
            "main.go",
            "package main\n\ntype item struct{}\n\nfunc (value item) localContract() {}\nfunc (value item) asserted() {}\n\ntype local interface { localContract() }\nvar _ local = item{}\nfunc runtime(value any) { _, _ = value.(interface { asserted() }) }\nfunc main() {}\n",
        ),
    ]);
    let bridge = GoplsBridge::start(&fixture).await;
    let proxy = CompilerProxy::start(bridge.addr(), ShadowReply::Pass).await;
    for method in ["localContract", "asserted"] {
        let output = refused(proxy.addr(), &fixture, "main.go", method, true).await;
        assert!(
            output.contains("implementation evidence")
                || output.contains("local interface obligation"),
            "{method}: {output}"
        );
    }
}

fn one_function() -> GoModule {
    GoModule::new(&[
        ("go.mod", "module example.com/evidence\n\ngo 1.22\n"),
        ("main.go", "package main\n\nfunc hidden() {}\n"),
    ])
}

fn declaration_symbol() -> Value {
    json!([{
        "name": "hidden",
        "kind": 12,
        "range": {
            "start": { "line": 2, "character": 0 },
            "end": { "line": 2, "character": 16 }
        },
        "selectionRange": {
            "start": { "line": 2, "character": 5 },
            "end": { "line": 2, "character": 11 }
        }
    }])
}

fn declaration_reference(path: &std::path::Path) -> Value {
    json!([{
        "uri": url::Url::from_file_path(path).unwrap().to_string(),
        "range": {
            "start": { "line": 2, "character": 5 },
            "end": { "line": 2, "character": 11 }
        }
    }])
}

fn one_method() -> GoModule {
    GoModule::new(&[
        ("go.mod", "module example.com/methodevidence\n\ngo 1.22\n"),
        (
            "main.go",
            "package main\n\ntype item struct{}\n\nfunc (value item) hidden() {}\n",
        ),
    ])
}

fn method_symbol() -> Value {
    json!([{
        "name": "item.hidden",
        "kind": 6,
        "range": {
            "start": { "line": 4, "character": 0 },
            "end": { "line": 4, "character": 29 }
        },
        "selectionRange": {
            "start": { "line": 4, "character": 18 },
            "end": { "line": 4, "character": 24 }
        }
    }])
}

fn method_declaration_reference(path: &std::path::Path) -> Value {
    json!([{
        "uri": url::Url::from_file_path(path).unwrap().to_string(),
        "range": {
            "start": { "line": 4, "character": 18 },
            "end": { "line": 4, "character": 24 }
        }
    }])
}

fn method_symbol_for(source: &str, method: &str) -> Value {
    let start = source.find("func (").expect("method declaration");
    let name = source[start..]
        .find(method)
        .map(|offset| start + offset)
        .expect("method name");
    let end = source[start..]
        .find('\n')
        .map_or(source.len(), |offset| start + offset);
    let position = |offset: usize| {
        let before = &source[..offset];
        json!({
            "line": before.matches('\n').count(),
            "character": before
                .rsplit('\n')
                .next()
                .unwrap_or_default()
                .encode_utf16()
                .count()
        })
    };
    json!([{
        "name": method,
        "kind": 6,
        "range": { "start": position(start), "end": position(end) },
        "selectionRange": {
            "start": position(name),
            "end": position(name + method.len())
        }
    }])
}

#[tokio::test]
async fn alias_generic_and_embedded_receivers_refuse_unchanged() {
    for (source, expected) in [
        (
            "package main\n\ntype original struct{}\ntype item = original\n\nfunc (value item) hidden() {}\n",
            "alias",
        ),
        (
            "package main\n\ntype item[T any] struct{}\n\nfunc (value item[T]) hidden() {}\n",
            "generic",
        ),
        (
            "package main\n\ntype item struct{}\ntype outer struct{ item }\n\nfunc (value item) hidden() {}\n",
            "embedded or promoted",
        ),
        (
            "package main\n\ntype item struct{}\ntype outer struct { field struct { item } }\n\nfunc (value item) hidden() {}\n",
            "embedded or promoted",
        ),
        (
            "package main\n\ntype item struct{}\ntype alias = item\ntype outer struct { alias }\n\nfunc (value item) hidden() {}\n",
            "embedded or promoted",
        ),
        (
            "package main\n\ntype item struct{}\ntype pointer = ((*item))\ntype outer struct { pointer }\n\nfunc (value item) hidden() {}\n",
            "embedded or promoted",
        ),
        (
            "package main\n\ntype item struct{}\ntype outer struct { named int /* field boundary\n*/ item }\n\nfunc (value item) hidden() {}\n",
            "embedded or promoted",
        ),
    ] {
        let fixture = GoModule::new(&[
            ("go.mod", "module example.com/methodshape\n\ngo 1.22\n"),
            ("main.go", source),
        ]);
        let symbols = method_symbol_for(source, "hidden");
        let gateway = ScriptedGateway::start(move |method, _params| match method {
            "textDocument/documentSymbol" => symbols.clone(),
            "textDocument/implementation" => json!([]),
            _ => Value::Null,
        })
        .await;
        let output = refused(gateway.addr(), &fixture, "main.go", "hidden", true).await;
        assert!(output.contains(expected), "{output}");
    }
}

async fn evidence_gateway(references: Value) -> ScriptedGateway {
    ScriptedGateway::start(move |method, _params| match method {
        "textDocument/documentSymbol" => declaration_symbol(),
        "textDocument/references" => references.clone(),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await
}

#[tokio::test]
async fn failed_malformed_and_nonempty_implementation_evidence_refuses_unchanged() {
    for (implementation, expected) in [
        (
            answers::failure("implementation unavailable"),
            "could not list",
        ),
        (json!("malformed"), "not a location"),
        (
            json!([{
                "uri": "file:///interface.go",
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": 6 }
                }
            }]),
            "implementation evidence",
        ),
    ] {
        let fixture = one_method();
        let path = fixture.path("main.go");
        let references = method_declaration_reference(&path);
        let gateway = ScriptedGateway::start(move |method, _params| match method {
            "textDocument/documentSymbol" => method_symbol(),
            "textDocument/implementation" => implementation.clone(),
            "textDocument/references" => references.clone(),
            _ => Value::Null,
        })
        .await;
        let output = refused(gateway.addr(), &fixture, "main.go", "hidden", true).await;
        assert!(output.contains(expected), "{output}");
    }
}

#[tokio::test]
async fn missing_malformed_stale_and_outside_reference_evidence_refuses_unchanged() {
    let outside = tempfile::tempdir().expect("outside dir");
    let outside_file = outside.path().join("outside.go");
    std::fs::write(&outside_file, "package outside\n\nfunc hidden() {}\n").expect("outside source");
    let outside_refs = declaration_reference(&outside_file);
    let cases = [
        Value::Null,
        json!([]),
        json!([{}]),
        json!([{
            "uri": "file:///definitely/missing.go",
            "range": {
                "start": { "line": 0, "character": 0 },
                "end": { "line": 0, "character": 6 }
            }
        }]),
        outside_refs,
    ];
    for references in cases {
        let fixture = one_function();
        let gateway = evidence_gateway(references).await;
        let output = refused(gateway.addr(), &fixture, "main.go", "hidden", true).await;
        assert!(
            output.contains("location")
                || output.contains("reference")
                || output.contains("outside")
                || output.contains("cannot inspect")
                || output.contains("listed no"),
            "{output}"
        );
    }

    let fixture = one_function();
    let path = fixture.path("main.go");
    let stale = json!([{
        "uri": url::Url::from_file_path(&path).unwrap().to_string(),
        "range": {
            "start": { "line": 2, "character": 0 },
            "end": { "line": 2, "character": 6 }
        }
    }]);
    let gateway = evidence_gateway(stale).await;
    let output = refused(gateway.addr(), &fixture, "main.go", "hidden", false).await;
    assert!(output.contains("stale or malformed"), "{output}");
}

#[tokio::test]
async fn bodyless_and_malformed_symbol_evidence_are_public_refusals() {
    let bodyless = GoModule::new(&[
        ("go.mod", "module example.com/bodyless\n\ngo 1.22\n"),
        ("main.go", "package main\n\nfunc hidden()\n"),
    ]);
    let bodyless_path = bodyless.path("main.go");
    let bodyless_gateway = ScriptedGateway::start(move |method, _params| match method {
        "textDocument/documentSymbol" => json!([{
            "name": "hidden",
            "kind": 12,
            "range": {
                "start": { "line": 2, "character": 0 },
                "end": { "line": 2, "character": 13 }
            },
            "selectionRange": {
                "start": { "line": 2, "character": 5 },
                "end": { "line": 2, "character": 11 }
            }
        }]),
        "textDocument/references" => declaration_reference(&bodyless_path),
        _ => Value::Null,
    })
    .await;
    let output = refused(
        bodyless_gateway.addr(),
        &bodyless,
        "main.go",
        "hidden",
        false,
    )
    .await;
    assert!(output.contains("complete body"), "{output}");

    let fixture = one_function();
    let path = fixture.path("main.go");
    for symbols in [
        Value::Null,
        json!([{}]),
        json!([{
            "name": "hidden",
            "kind": 12,
            "range": {
                "start": { "line": 2, "character": 0 },
                "end": { "line": 2, "character": 13 }
            },
            "selectionRange": {
                "start": { "line": 2, "character": 5 },
                "end": { "line": 2, "character": 11 }
            }
        }]),
    ] {
        let symbols = Arc::new(symbols);
        let reference_path = path.clone();
        let gateway = ScriptedGateway::start({
            let symbols = Arc::clone(&symbols);
            move |method, _params| match method {
                "textDocument/documentSymbol" => (*symbols).clone(),
                "textDocument/references" => declaration_reference(&reference_path),
                _ => Value::Null,
            }
        })
        .await;
        let output = refused(gateway.addr(), &fixture, "main.go", "hidden", false).await;
        assert!(
            output.contains("document symbol")
                || output.contains("usable list")
                || output.contains("complete body")
                || output.contains("declaration range"),
            "{output}"
        );
    }
}

#[tokio::test]
async fn compiler_failure_unavailable_and_malformed_evidence_refuse_unchanged() {
    for (reply, expected) in [
        (
            ShadowReply::Fail("./main.go:3: imported and not used: fmt"),
            "does not compile",
        ),
        (ShadowReply::Error, "compiler evidence is unavailable"),
        (ShadowReply::Malformed, "compiler evidence is unavailable"),
    ] {
        let fixture = one_function();
        let gateway = evidence_gateway(declaration_reference(&fixture.path("main.go"))).await;
        let proxy = CompilerProxy::start(gateway.addr(), reply).await;
        let output = refused(proxy.addr(), &fixture, "main.go", "hidden", true).await;
        assert!(output.contains(expected), "{output}");
    }
}

#[tokio::test]
async fn concurrent_change_is_not_overwritten_by_the_verified_proposal() {
    let fixture = one_function();
    let changed = "package main\n\nfunc hidden() { println(\"concurrent\") }\n";
    let gateway = evidence_gateway(declaration_reference(&fixture.path("main.go"))).await;
    let proxy = CompilerProxy::start(
        gateway.addr(),
        ShadowReply::Mutate(fixture.path("main.go"), changed),
    )
    .await;
    let result = call(proxy.addr(), &fixture, "main.go", "hidden", true).await;
    let output = text_of(&result);
    assert!(result.is_error, "{output}");
    assert!(output.contains("changed while"), "{output}");
    assert_eq!(fixture.read("main.go"), changed);
}

#[cfg(unix)]
#[tokio::test]
async fn malformed_go_positions_refuse_before_dispatch_without_writes() {
    let fixture = one_function();
    for (line, character) in [
        (0_u64, 6_u64),
        (3_u64, 0_u64),
        (4_294_967_299_u64, 4_294_967_302_u64),
    ] {
        let before = fixture.snapshot();
        let error = prod_code_mcp::tools::execute_tool(
            SocketAddr::from(([127, 0, 0, 1], 9)),
            fixture.root(),
            "code_safe_delete",
            json!({
                "path": "main.go",
                "line": line,
                "character": character,
                "force": true
            }),
        )
        .await
        .expect_err("malformed coordinates are rejected before dispatch");
        assert!(
            format!("{error:#}").contains("one-based coordinate"),
            "{error:#}"
        );
        assert_eq!(
            fixture.snapshot(),
            before,
            "position refusal changed the tree"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_linked_go_source_anywhere_in_the_module_refuses_before_evidence() {
    use std::os::unix::fs::symlink;

    let fixture = one_function();
    std::fs::write(
        fixture.path("linked.txt"),
        "package main\n\nfunc linkedSource() {}\n",
    )
    .expect("link target");
    symlink("linked.txt", fixture.path("linked.go")).expect("source symlink");
    let before = fixture.snapshot();
    let result = prod_code_mcp::tools::execute_tool(
        SocketAddr::from(([127, 0, 0, 1], 9)),
        fixture.root(),
        "code_safe_delete",
        json!({ "path": "main.go", "line": 3, "character": 6, "force": true }),
    )
    .await
    .expect("preflight refusal is an MCP result");
    let output = text_of(&result);
    assert!(result.is_error, "{output}");
    assert!(output.contains("linked source path"), "{output}");
    assert_eq!(fixture.snapshot(), before);
}

#[cfg(unix)]
#[tokio::test]
async fn a_linked_source_ancestor_refuses_before_evidence() {
    use std::os::unix::fs::symlink;

    let fixture = one_function();
    std::fs::create_dir(fixture.path("real")).expect("real module");
    std::fs::write(
        fixture.path("real/go.mod"),
        "module example.com/linked\n\ngo 1.22\n",
    )
    .expect("linked module");
    std::fs::write(
        fixture.path("real/main.go"),
        "package main\n\nfunc hidden() {}\n",
    )
    .expect("linked source");
    symlink("real", fixture.path("linked")).expect("module directory symlink");
    let before = fixture.snapshot();
    let result = prod_code_mcp::tools::execute_tool(
        SocketAddr::from(([127, 0, 0, 1], 9)),
        fixture.root(),
        "code_safe_delete",
        json!({ "path": "linked/main.go", "line": 3, "character": 6, "force": true }),
    )
    .await
    .expect("preflight refusal is an MCP result");
    let output = text_of(&result);
    assert!(result.is_error, "{output}");
    assert!(output.contains("linked source path"), "{output}");
    assert_eq!(fixture.snapshot(), before);
}
