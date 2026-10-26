/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Boundaries of a Go result replacement (#1411), through a real gopls: a result written in
//! parentheses is the one unnamed result, a caller in another module of a `go.work` refuses
//! because only the declaring module is compiled, and an exported method that reflection can
//! call by name refuses. The compiler shadow here really compiles: it copies the module, puts
//! the proposal in place and runs the requested `go` command. `go` and `gopls` are required.

use futures_util::{SinkExt, StreamExt};
use prod_code_mcp::signature::{Modifiers, Param, SignatureChange};
use prod_code_protocol::{
    ProdCodeCodec, ShadowHypothesisResult, ShadowRunRequest, ShadowRunResponse, WireMessage,
};
use prod_code_testkit::gopls::{GoModule, GoplsBridge, require_go_toolchain};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tokio::net::TcpListener;
use tokio_util::codec::Framed;

/// Runs the shadow's `go` command in a copy of `root` with the proposed files in place.
fn compile(root: &Path, req: &ShadowRunRequest) -> ShadowRunResponse {
    let copy = tempfile::Builder::new()
        .prefix("goshadow")
        .tempdir()
        .unwrap();
    let status = std::process::Command::new("cp")
        .arg("-R")
        .arg(format!("{}/.", root.display()))
        .arg(copy.path())
        .status()
        .unwrap();
    assert!(status.success());
    for file in &req.hypotheses[0].files {
        std::fs::write(
            copy.path().join(&file.relative_path),
            file.content.as_deref().unwrap_or_default(),
        )
        .unwrap();
    }
    let dir = req
        .subdir
        .as_deref()
        .map_or(copy.path().to_path_buf(), |s| copy.path().join(s));
    let out = std::process::Command::new(&req.command[0])
        .args(&req.command[1..])
        .current_dir(dir)
        .envs(req.env.iter().cloned())
        .output()
        .unwrap();
    let mut output = out.stdout;
    output.extend(out.stderr);
    eprintln!(
        "shadow {:?}: {}",
        req.command,
        String::from_utf8_lossy(&output)
    );
    ShadowRunResponse {
        server_workspace_root: req.client_workspace_root.clone(),
        mode: "overlay".into(),
        results: vec![ShadowHypothesisResult {
            name: req.hypotheses[0].name.clone(),
            exit_code: out.status.code(),
            duration_ms: 1,
            timed_out: false,
            error: None,
            output_len: output.len() as u64,
            output_tail: Some(output),
        }],
        error: None,
    }
}

/// The scripted gateway with a compiling shadow in front of it.
async fn with_compiler(upstream: SocketAddr, root: PathBuf) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((client, _)) = listener.accept().await {
            let root = root.clone();
            tokio::spawn(async move {
                let server = tokio::net::TcpStream::connect(upstream).await.unwrap();
                let mut client = Framed::new(client, ProdCodeCodec::new());
                let mut server = Framed::new(server, ProdCodeCodec::new());
                loop {
                    tokio::select! {
                        incoming = client.next() => match incoming {
                            Some(Ok(WireMessage::ShadowRunRequest(req))) => {
                                let response = compile(&root, &req);
                                client.send(WireMessage::ShadowRunResponse(response)).await.unwrap();
                            }
                            Some(Ok(message)) => server.send(message).await.unwrap(),
                            _ => return,
                        },
                        outgoing = server.next() => match outgoing {
                            Some(Ok(message)) => client.send(message).await.unwrap(),
                            _ => return,
                        },
                    }
                }
            });
        }
    });
    addr
}

async fn change_result(
    remote: SocketAddr,
    fixture: &GoModule,
    rel: &str,
    needle: &str,
    params: &[&str],
    returns: &str,
    apply: bool,
) -> anyhow::Result<SignatureChange> {
    let text = fixture.read(rel);
    let before = &text[..text.find(needle).expect("needle")];
    let line = before.matches('\n').count() as u32 + 1;
    let col = before.rsplit('\n').next().unwrap().chars().count() as u32 + 1;
    let request: Vec<Param> = params.iter().map(|n| Param::Keep(n.to_string())).collect();
    prod_code_mcp::signature_go::change_with(
        remote,
        fixture.root(),
        &fixture.path(rel),
        line,
        col,
        &request,
        &Modifiers {
            returns: Some(returns.into()),
            ..Default::default()
        },
        apply,
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_parenthesized_result_is_the_one_unnamed_result() {
    require_go_toolchain();
    let fixture = GoModule::new(&[
        ("go.mod", "module example.com/paren\n\ngo 1.22\n"),
        (
            "lib.go",
            "package main\n\nfunc Count(n int) (int) { return 7 }\n",
        ),
        (
            "main.go",
            "package main\n\nimport \"fmt\"\n\nfunc main() { fmt.Println(Count(1)) }\n",
        ),
    ]);
    assert_eq!(fixture.run(), "7\n");
    let bridge = GoplsBridge::start(&fixture).await;
    let remote = with_compiler(bridge.addr(), fixture.root().to_path_buf()).await;
    let changed = change_result(remote, &fixture, "lib.go", "Count(n", &["n"], "int64", true)
        .await
        .expect("the parenthesized result is replaced");
    assert!(changed.applied);
    assert_eq!(changed.returns, Some(("int".into(), "int64".into())));
    assert_eq!(
        fixture.read("lib.go"),
        "package main\n\nfunc Count(n int) (int64) { return 7 }\n"
    );
    assert_eq!(fixture.run(), "7\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_caller_in_a_sibling_workspace_module_refuses() {
    require_go_toolchain();
    let fixture = GoModule::new(&[
        ("go.work", "go 1.22\n\nuse (\n\t./a\n\t./b\n)\n"),
        ("a/go.mod", "module example.com/a\n\ngo 1.22\n"),
        ("a/a.go", "package a\n\nfunc Value() int { return 1 }\n"),
        ("b/go.mod", "module example.com/b\n\ngo 1.22\n"),
        (
            "b/b.go",
            "package b\n\nimport \"example.com/a\"\n\nfunc Use() int { return a.Value() }\n",
        ),
    ]);
    let build = |dir: &str| {
        std::process::Command::new("go")
            .args(["build", "./..."])
            .current_dir(fixture.root().join(dir))
            .env("GOTOOLCHAIN", "local")
            .output()
            .unwrap()
    };
    let b = build("b");
    assert!(b.status.success(), "{}", String::from_utf8_lossy(&b.stderr));
    let bridge = GoplsBridge::start(&fixture).await;
    let remote = with_compiler(bridge.addr(), fixture.root().to_path_buf()).await;
    let untouched = fixture.snapshot();
    let refused = change_result(remote, &fixture, "a/a.go", "Value()", &[], "uint", true)
        .await
        .expect_err("only module a would be compiled");
    eprintln!("sibling module: {refused:#}");
    assert!(
        format!("{refused:#}").contains("another Go module"),
        "{refused:#}"
    );
    assert_eq!(fixture.snapshot(), untouched, "nothing is written");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_method_reflection_can_call_by_name_refuses() {
    require_go_toolchain();
    let fixture = GoModule::new(&[
        ("go.mod", "module example.com/reflected\n\ngo 1.22\n"),
        (
            "lib.go",
            "package main\n\ntype V struct{}\n\nfunc (v V) ValueResult() int { return 1 }\n",
        ),
        (
            "main.go",
            "package main\n\nimport (\n\t\"fmt\"\n\t\"reflect\"\n)\n\nfunc main() {\n\tfmt.Println(reflect.ValueOf(V{}).MethodByName(\"ValueResult\").Call(nil)[0].Int())\n}\n",
        ),
    ]);
    assert_eq!(fixture.run(), "1\n");
    let bridge = GoplsBridge::start(&fixture).await;
    let remote = with_compiler(bridge.addr(), fixture.root().to_path_buf()).await;
    let untouched = fixture.snapshot();
    for apply in [false, true] {
        let refused = change_result(
            remote,
            &fixture,
            "lib.go",
            "ValueResult()",
            &[],
            "uint",
            apply,
        )
        .await
        .expect_err("a reflective caller has no reference");
        eprintln!("reflection apply={apply}: {refused:#}");
        assert!(
            format!("{refused:#}").contains("\"reflect\""),
            "{refused:#}"
        );
        assert_eq!(fixture.snapshot(), untouched, "nothing is written");
    }
}
