/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use futures_util::{SinkExt, StreamExt};
use prod_code_mcp::move_item::move_item;
use prod_code_protocol::{ProdCodeCodec, WireMessage};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use tokio_util::codec::Framed;

const MANIFEST: &str = "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
const PARALLEL: &str = "fn parallel_map_chunks<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {\n    std::thread::scope(|scope| {\n        let f = &f;\n        let jobs: Vec<_> = items.chunks(2).map(|chunk| {\n            scope.spawn(move || chunk.iter().map(f).collect::<Vec<_>>())\n        }).collect();\n        jobs.into_iter().flat_map(|job| job.join().unwrap()).collect()\n    })\n}\n";

fn compile_test(root: &Path, file: &Path) {
    let binary = root.join("fixture-test");
    let compiled = Command::new("rustc")
        .args(["--edition=2021", "--test"])
        .arg(file)
        .arg("-o")
        .arg(&binary)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let ran = Command::new(binary).output().unwrap();
    assert!(
        ran.status.success(),
        "{}",
        String::from_utf8_lossy(&ran.stdout)
    );
    assert!(String::from_utf8_lossy(&ran.stdout).contains("1 passed"));
}

async fn gateway(
    file: &Path,
    name: &str,
    start: u32,
    end: u32,
    caller: Option<(u32, u32)>,
) -> ScriptedGateway {
    let file = file.to_path_buf();
    let name = name.to_string();
    ScriptedGateway::start(move |method, _| match method {
        "textDocument/documentSymbol" => {
            serde_json::json!([answers::document_symbol(&name, 12, start, end, 4)])
        }
        "textDocument/references" => {
            answers::locations(&file, &caller.into_iter().collect::<Vec<_>>())
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await
}

#[tokio::test]
async fn move_generic_scoped_helper_has_a_bounded_actionable_failure() {
    let ws = Workspace::new(&[("Cargo.toml", MANIFEST), ("src/lib.rs", PARALLEL)]);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let remote = gateway(&ws.path("src/lib.rs"), "parallel_map_chunks", 1, 9, None).await;
    // Complete sync/symbol/reference queries, then leave validation's diagnostic unanswered.
    let peer = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let upstream = tokio::net::TcpStream::connect(remote.addr()).await.unwrap();
        let mut client = Framed::new(socket, ProdCodeCodec::new());
        let mut server = Framed::new(upstream, ProdCodeCodec::new());
        loop {
            tokio::select! {
                message = client.next() => {
                    let Some(Ok(message)) = message else { break };
                    if let WireMessage::LspPayload(ref payload) = message {
                        let value: serde_json::Value = serde_json::from_str(payload).unwrap();
                        if value["method"] == "textDocument/diagnostic" {
                            continue;
                        }
                    }
                    if server.send(message).await.is_err() { break; }
                }
                message = server.next() => {
                    let Some(Ok(message)) = message else { break };
                    if client.send(message).await.is_err() { break; }
                }
            }
        }
    });
    let target = ws.path("src/parallel.rs");
    let result = tokio::time::timeout(
        Duration::from_secs(65),
        move_item(
            addr,
            &ws.root(),
            &ws.path("src/lib.rs"),
            1,
            4,
            &target,
            true,
            false,
        ),
    )
    .await;
    peer.abort();
    let _ = peer.await;
    let error = result
        .expect("move must finish within 65 seconds")
        .unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("validating proposed files"), "{message}");
    assert!(message.contains("prod-code cluster"), "{message}");
    assert!(message.contains("nothing was written"), "{message}");
    assert!(!target.exists());
    assert_eq!(ws.read("src/lib.rs"), PARALLEL);
}

#[tokio::test]
async fn move_generic_scoped_helper_to_a_child_still_runs() {
    let source = format!(
        "{PARALLEL}\n#[test]\nfn example() {{ assert_eq!(parallel_map_chunks(&[1, 2, 3], |x| x * 2), [2, 4, 6]); }}\n"
    );
    let ws = Workspace::new(&[("Cargo.toml", MANIFEST), ("src/lib.rs", &source)]);
    let file = ws.path("src/lib.rs");
    let caller = source
        .lines()
        .nth(11)
        .unwrap()
        .find("parallel_map_chunks")
        .unwrap() as u32
        + 1;
    let remote = gateway(&file, "parallel_map_chunks", 1, 9, Some((12, caller))).await;
    move_item(
        remote.addr(),
        &ws.root(),
        &file,
        1,
        4,
        &ws.path("src/parallel.rs"),
        true,
        false,
    )
    .await
    .unwrap();
    compile_test(&ws.root(), &file);
}

#[tokio::test]
async fn move_integration_test_preserves_attributes_and_private_helpers() {
    for (source_path, target_path, declared_path) in [
        (
            "tests/cache_hit_lazy_staging.rs",
            "tests/cache_hit_lazy_staging/read_only_hit.rs",
            "cache_hit_lazy_staging/read_only_hit.rs",
        ),
        (
            "tests/cache_hit_lazy_staging/main.rs",
            "tests/cache_hit_lazy_staging/cases/hit.rs",
            "cases/hit.rs",
        ),
    ] {
        let source = format!(
            "fn private_helper() -> u32 {{ 7 }}\n#[path = \"{declared_path}\"]\nmod read_only_hit;\n\n#[test]\nfn test_example() {{\n    assert_eq!(private_helper(), 7);\n}}\n"
        );
        let ws = Workspace::new(&[
            ("Cargo.toml", MANIFEST),
            ("src/lib.rs", ""),
            (source_path, &source),
        ]);
        let file = ws.path(source_path);
        let target = ws.path(target_path);
        let remote = gateway(&file, "test_example", 6, 8, None).await;
        let moved = move_item(remote.addr(), &ws.root(), &file, 6, 4, &target, true, false)
            .await
            .unwrap();
        assert!(moved.applied);
        assert!(ws.read(target_path).contains("#[test]"));
        assert_eq!(
            ws.read(source_path).matches("mod read_only_hit;").count(),
            1
        );
        assert!(!ws.read(source_path).contains("#[test]"));
        compile_test(&ws.root(), &file);
    }
}

#[tokio::test]
async fn move_to_child_rebases_paths_widens_visibility_and_preserves_preamble() {
    let source = "/* Fixture license header. */\n// A file header.\n#![allow(\n    dead_code\n)]\n\nuse super::compiler::RustcInvocation;\n\nfn append_remap_path_prefix_args(_: &RustcInvocation) -> u32 {\n    // super::inputs is a comment; the string must stay unchanged too.\n    let _literal = r#\"super::inputs\"#;\n    super::inputs::value()\n}\n\npub fn caller() -> u32 { append_remap_path_prefix_args(&RustcInvocation) }\n";
    let lib = "mod compiler { pub struct RustcInvocation; }\nmod inputs { pub fn value() -> u32 { 42 } }\nmod compiler_args;\n#[test]\nfn moved_caller() { assert_eq!(compiler_args::caller(), 42); }\n";
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/lib.rs", lib),
        ("src/compiler_args.rs", source),
    ]);
    let file = ws.path("src/compiler_args.rs");
    let target = ws.path("src/compiler_args/remap.rs");
    let column = source
        .lines()
        .nth(14)
        .unwrap()
        .find("append_remap")
        .unwrap() as u32
        + 1;
    let remote = gateway(
        &file,
        "append_remap_path_prefix_args",
        9,
        13,
        Some((15, column)),
    )
    .await;
    move_item(remote.addr(), &ws.root(), &file, 9, 4, &target, true, false)
        .await
        .unwrap();
    let parent = ws.read("src/compiler_args.rs");
    assert!(
        parent.starts_with(
            "/* Fixture license header. */\n// A file header.\n#![allow(\n    dead_code\n)]\n"
        ),
        "{parent}"
    );
    let child = ws.read("src/compiler_args/remap.rs");
    assert!(
        child.contains("pub(super) fn append_remap_path_prefix_args"),
        "{child}"
    );
    assert!(child.contains("r#\"super::inputs\"#"), "{child}");
    compile_test(&ws.root(), &ws.path("src/lib.rs"));
}
