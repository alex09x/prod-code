/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! #1813 and #1836: upload participation of written-back files, pre-flight sync omission
//! warnings, and `[sync] include` opt-in across Git and non-Git workspaces.

use super::writeback::Writeback;
use crate::sync::{
    UploadExclusion, UploadExclusionReason, classify_upload_exclusions, clear_sync_cache_for,
    commit_workspace_sync, prepare_workspace_sync_for, upload_exclusion_warning,
};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    ExecChunk, ExecExit, FileDelta, ProdCodeCodec, SyncProbeResponse, SyncResponse, WireMessage,
};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio_util::codec::Framed;

const NODE_A: &str = "node-a.test:9400";
const NODE_B: &str = "node-b.test:9400";

fn git(root: &Path, args: &[&str]) -> bool {
    std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .status()
        .is_ok_and(|s| s.success())
}

fn git_commit_base(root: &Path) {
    assert!(git(root, &["add", "."]));
    assert!(git(
        root,
        &[
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@e",
            "commit",
            "-qm",
            "base"
        ]
    ));
}

fn delta(path: &str, content: &[u8]) -> FileDelta {
    FileDelta {
        relative_path: path.into(),
        content: Some(content.to_vec()),
        is_executable: false,
    }
}

#[test]
fn issue_1813_written_back_fixtures_reach_node_b_and_results_warn() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    if !git(&root, &["init", "-q"]) {
        return; // no git here
    }
    std::fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
    git_commit_base(&root);
    let baseline_a = prepare_workspace_sync_for(&root, NODE_A, None).unwrap();
    commit_workspace_sync(&root, &baseline_a);
    let baseline = prepare_workspace_sync_for(&root, NODE_B, None).unwrap();
    commit_workspace_sync(&root, &baseline);

    crate::sync::apply_pulled_files_for(&root, NODE_A, &[delta("artifacts/out.bin", &[9, 9])])
        .unwrap();
    let mut writeback = Writeback::capture(&root, true, false).unwrap();
    writeback
        .apply(
            &root,
            NODE_A,
            vec![
                delta("results/out.bin", &[0, 1, 2]),
                delta("ignored/gen.rs", b"fn g() {}\n"),
                delta("fixture/out.bin", &[0, 159, 146]),
                delta("tests/fixtures/meta.json", b"{\"n\":1}\n"),
            ],
        )
        .unwrap();
    assert_eq!(writeback.pulled.len(), 4, "{:?}", writeback.kept);
    // Same-node sync on NODE_A must not emit deletion deltas for writeback-only artifacts.
    let plan_a = prepare_workspace_sync_for(&root, NODE_A, None).unwrap();
    assert!(
        !plan_a.files.iter().any(|f| f.content.is_none()),
        "unexpected deletion on pinned node: {:?}",
        plan_a.files
    );
    let excluded = classify_upload_exclusions(&root, &writeback.pulled);
    let mut named: Vec<_> = excluded
        .iter()
        .map(|e| (e.path.as_str(), e.reason))
        .collect();
    named.sort_by(|a, b| a.0.cmp(b.0));
    assert_eq!(
        named,
        [
            ("ignored/gen.rs", UploadExclusionReason::GitIgnored),
            (
                "results/out.bin",
                UploadExclusionReason::OutputOrDataDirectory
            ),
        ]
    );
    let warning = upload_exclusion_warning(&excluded).unwrap();
    assert!(warning.contains("results/out.bin (output/data directory)"));
    assert!(warning.contains("not uploaded to other nodes"));

    let plan = prepare_workspace_sync_for(&root, NODE_B, None).unwrap();
    let planned: Vec<&str> = plan
        .files
        .iter()
        .map(|f| f.relative_path.as_str())
        .collect();
    for path in ["fixture/out.bin", "tests/fixtures/meta.json"] {
        let file = plan.files.iter().find(|f| f.relative_path == path);
        assert!(
            file.is_some_and(|f| f.content.is_some()),
            "{path} missing from {planned:?}"
        );
    }
    for path in ["results/out.bin", "ignored/gen.rs"] {
        assert!(!planned.contains(&path), "{path} uploaded: {planned:?}");
    }
    assert_eq!(
        plan.omitted,
        vec![
            UploadExclusion {
                path: "artifacts/out.bin".into(),
                reason: UploadExclusionReason::OutputOrDataDirectory,
            },
            UploadExclusion {
                path: "results/out.bin".into(),
                reason: UploadExclusionReason::OutputOrDataDirectory,
            },
        ]
    );
    clear_sync_cache_for(&root, NODE_A);
    clear_sync_cache_for(&root, NODE_B);
}

async fn spawn_mirror_exec_gateway(
    mirror: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    required_path: &'static str,
) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut framed = Framed::new(socket, ProdCodeCodec::new());
        while let Some(Ok(msg)) = framed.next().await {
            match msg {
                WireMessage::SyncProbeRequest(req) => {
                    let missing = {
                        let guard = mirror.lock().unwrap();
                        req.files
                            .iter()
                            .filter(|s| !guard.contains_key(&s.relative_path))
                            .map(|s| s.relative_path.clone())
                            .collect()
                    };
                    framed
                        .send(WireMessage::SyncProbeResponse(SyncProbeResponse {
                            server_workspace_root: "/mirror".into(),
                            seeded: false,
                            files_deleted: 0,
                            missing,
                        }))
                        .await
                        .unwrap();
                }
                WireMessage::SyncRequest(req) => {
                    let (mut updated, mut deleted, mut bytes) = (0, 0, 0);
                    {
                        let mut guard = mirror.lock().unwrap();
                        for f in req.files {
                            if let Some(data) = f.content {
                                bytes += data.len();
                                updated += 1;
                                guard.insert(f.relative_path, data);
                            } else if guard.remove(&f.relative_path).is_some() {
                                deleted += 1;
                            }
                        }
                    }
                    let resp = SyncResponse {
                        server_workspace_root: "/mirror".into(),
                        files_updated: updated,
                        files_deleted: deleted,
                        bytes_transferred: bytes,
                        duration_ms: 1,
                        workspace_was_fresh: false,
                        stale_paths: vec![],
                    };
                    framed.send(WireMessage::SyncResponse(resp)).await.unwrap();
                }
                WireMessage::ExecRequest(_) => {
                    let found_len = mirror.lock().unwrap().get(required_path).map(Vec::len);
                    let (code, out) = match found_len {
                        Some(len) => (0, format!("loaded {len} bytes\n")),
                        None => (1, format!("FileNotFoundError: '{required_path}'\n")),
                    };
                    let chunk = ExecChunk {
                        stderr: code != 0,
                        data: Some(out.into_bytes()),
                    };
                    framed.send(WireMessage::ExecChunk(chunk)).await.unwrap();
                    let exit = ExecExit {
                        exit_code: Some(code),
                        duration_ms: 2,
                        server_workspace_root: "/mirror".into(),
                        timed_out: false,
                        error: None,
                        usage: None,
                        platform: None,
                    };
                    framed.send(WireMessage::ExecExit(exit)).await.unwrap();
                }
                WireMessage::Disconnect { .. } => break,
                other => panic!("unexpected frame: {other:?}"),
            }
        }
    });
    (addr, handle)
}

#[tokio::test]
async fn issue_1836_untracked_site_data_json_warns_then_uploads_with_sync_include() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    if !git(&root, &["init", "-q"]) {
        return;
    }
    std::fs::create_dir_all(root.join("scripts/site")).unwrap();
    std::fs::write(root.join("scripts/site/build.py"), "print('ok')\n").unwrap();
    git_commit_base(&root);

    // Exact #1836 shape: 539 KiB untracked, unignored JSON under `site/data/`.
    let payload = vec![b'x'; 539 * 1024];
    std::fs::create_dir_all(root.join("site/data")).unwrap();
    std::fs::write(root.join("site/data/benchmarks-v0.3.17.json"), &payload).unwrap();

    let mirror = Arc::new(Mutex::new(HashMap::new()));
    // CLI streaming path (`run_remote`) emits `[prod-code exec] sync warning: ...` before command stderr.
    let (addr0, g0) =
        spawn_mirror_exec_gateway(Arc::clone(&mirror), "site/data/benchmarks-v0.3.17.json").await;
    let mut streamed = Vec::new();
    super::run_remote(
        addr0,
        &root,
        None,
        vec!["python3".into(), "scripts/site/build.py".into()],
        vec![],
        30,
        true,
        |is_stderr, data| streamed.push((is_stderr, String::from_utf8_lossy(data).into_owned())),
    )
    .await
    .unwrap();
    g0.await.unwrap();
    assert!(
        streamed.len() >= 2
            && streamed[0].0
            && streamed[0].1.starts_with(
                "[prod-code exec] sync warning: 1 unignored local file(s) omitted from the remote mirror: site/data/benchmarks-v0.3.17.json"
            )
            && streamed[1].1.contains("FileNotFoundError"),
        "{streamed:?}"
    );
    clear_sync_cache_for(&root, &addr0.to_string());

    // MCP path (`handle_exec`) includes `[sync warning: ...]` and `structuredContent.sync_omitted` in the final tool result.
    let (addr1, g1) =
        spawn_mirror_exec_gateway(Arc::clone(&mirror), "site/data/benchmarks-v0.3.17.json").await;
    let res1 = crate::tools::exec_handler::handle_exec(
        &crate::cluster::PlacementStore::default(),
        addr1,
        &root,
        &serde_json::json!({ "argv": ["python3", "scripts/site/build.py"] }),
    )
    .await
    .unwrap();
    g1.await.unwrap();

    let crate::protocol::McpContentItem::Text { text: text1 } = &res1.content[0];
    assert!(
        text1.contains(
            "sync warning: 1 unignored local file(s) omitted from the remote mirror: site/data/benchmarks-v0.3.17.json (output/data directory)"
        ),
        "{text1}"
    );
    assert!(text1.contains("[sync] include"), "{text1}");
    let structured = res1
        .structured_content
        .as_ref()
        .expect("structured content");
    assert_eq!(
        structured["sync_omitted"],
        serde_json::json!([{
            "path": "site/data/benchmarks-v0.3.17.json",
            "reason": "output_or_data_directory"
        }])
    );
    // A host that shows only the structured channel still gets the command's result (#2009).
    assert!(structured["exit_code"].is_number(), "{structured}");
    assert_eq!(structured["timed_out"], false, "{structured}");
    let output = structured["output"].as_str().expect("output");
    assert!(
        !output.is_empty() && text1.ends_with(output),
        "{structured}"
    );
    assert_eq!(
        structured["output_bytes"].as_u64(),
        Some(output.len() as u64)
    );

    // Opting `site/data/**` into `prod-code.toml` uploads all 539 KiB and clears the warning.
    std::fs::write(
        root.join("prod-code.toml"),
        "[sync]\ninclude = [\"site/data/**\"]\n",
    )
    .unwrap();
    let (addr2, g2) =
        spawn_mirror_exec_gateway(Arc::clone(&mirror), "site/data/benchmarks-v0.3.17.json").await;
    let res2 = crate::tools::exec_handler::handle_exec(
        &crate::cluster::PlacementStore::default(),
        addr2,
        &root,
        &serde_json::json!({ "argv": ["python3", "scripts/site/build.py"] }),
    )
    .await
    .unwrap();
    g2.await.unwrap();

    let crate::protocol::McpContentItem::Text { text: text2 } = &res2.content[0];
    assert!(
        text2.contains(&format!("loaded {} bytes", payload.len())),
        "{text2}"
    );
    assert!(!text2.contains("sync warning:"), "{text2}");
    assert_eq!(res2.structured_content, None);

    // Removing `[sync] include` emits an explicit deletion delta so the mirror drops it.
    std::fs::write(root.join("prod-code.toml"), "").unwrap();
    let plan = prepare_workspace_sync_for(&root, &addr2.to_string(), None).unwrap();
    assert!(
        plan.files
            .iter()
            .any(|f| f.relative_path == "site/data/benchmarks-v0.3.17.json" && f.content.is_none()),
        "{:?}",
        plan.files
    );
    assert_eq!(
        plan.omitted,
        vec![UploadExclusion {
            path: "site/data/benchmarks-v0.3.17.json".into(),
            reason: UploadExclusionReason::OutputOrDataDirectory,
        }]
    );
    clear_sync_cache_for(&root, &addr1.to_string());
    clear_sync_cache_for(&root, &addr2.to_string());
}
