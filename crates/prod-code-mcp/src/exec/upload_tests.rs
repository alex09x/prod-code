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
    assert!(git(&root, &["add", "."]));
    assert!(git(
        &root,
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
    let baseline = prepare_workspace_sync_for(&root, NODE_B, None).unwrap();
    commit_workspace_sync(&root, &baseline);

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
        vec![UploadExclusion {
            path: "results/out.bin".into(),
            reason: UploadExclusionReason::OutputOrDataDirectory,
        }]
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
                    let mut updated = 0;
                    let mut deleted = 0;
                    let mut bytes = 0;
                    {
                        let mut guard = mirror.lock().unwrap();
                        for f in req.files {
                            match f.content {
                                Some(data) => {
                                    bytes += data.len();
                                    updated += 1;
                                    guard.insert(f.relative_path, data);
                                }
                                None => {
                                    if guard.remove(&f.relative_path).is_some() {
                                        deleted += 1;
                                    }
                                }
                            }
                        }
                    }
                    framed
                        .send(WireMessage::SyncResponse(SyncResponse {
                            server_workspace_root: "/mirror".into(),
                            files_updated: updated,
                            files_deleted: deleted,
                            bytes_transferred: bytes,
                            duration_ms: 1,
                            workspace_was_fresh: false,
                            stale_paths: vec![],
                        }))
                        .await
                        .unwrap();
                }
                WireMessage::ExecRequest(_) => {
                    let found_len = mirror.lock().unwrap().get(required_path).map(Vec::len);
                    let (code, out) = match found_len {
                        Some(len) => (0, format!("loaded {len} bytes\n")),
                        None => (1, format!("FileNotFoundError: '{required_path}'\n")),
                    };
                    framed
                        .send(WireMessage::ExecChunk(ExecChunk {
                            stderr: code != 0,
                            data: Some(out.into_bytes()),
                        }))
                        .await
                        .unwrap();
                    framed
                        .send(WireMessage::ExecExit(ExecExit {
                            exit_code: Some(code),
                            duration_ms: 2,
                            server_workspace_root: "/mirror".into(),
                            timed_out: false,
                            error: None,
                            usage: None,
                            platform: None,
                        }))
                        .await
                        .unwrap();
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
    assert!(git(&root, &["add", "."]));
    assert!(git(
        &root,
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

    // Exact #1836 shape: 539 KiB untracked, unignored JSON under `site/data/`.
    let payload = vec![b'x'; 539 * 1024];
    std::fs::create_dir_all(root.join("site/data")).unwrap();
    std::fs::write(root.join("site/data/benchmarks-v0.3.17.json"), &payload).unwrap();

    let mirror = Arc::new(Mutex::new(HashMap::new()));
    let (addr1, g1) =
        spawn_mirror_exec_gateway(Arc::clone(&mirror), "site/data/benchmarks-v0.3.17.json").await;
    let res1 = crate::tools::exec_handler::handle_exec(
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
    assert!(
        text1.find("sync warning:").unwrap() < text1.find("FileNotFoundError").unwrap(),
        "{text1}"
    );
    assert_eq!(
        res1.structured_content,
        Some(serde_json::json!({
            "sync_omitted": [{
                "path": "site/data/benchmarks-v0.3.17.json",
                "reason": "output_or_data_directory"
            }]
        }))
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

#[test]
fn issue_1836_non_git_sync_include_uploads_and_deletes_on_removal() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.py"), "print('hi')\n").unwrap();
    std::fs::create_dir_all(root.join("site/data")).unwrap();
    std::fs::write(root.join("site/data/benchmarks.json"), b"{\"ok\":true}\n").unwrap();
    std::fs::create_dir_all(root.join("fixtures")).unwrap();
    std::fs::write(root.join("fixtures/rows.csv"), b"a,b\n1,2\n").unwrap();

    let plan1 = prepare_workspace_sync_for(&root, NODE_A, None).unwrap();
    assert!(
        plan1
            .omitted
            .iter()
            .any(|e| e.path == "site/data/"
                && e.reason == UploadExclusionReason::OutputOrDataDirectory),
        "{:?}",
        plan1.omitted
    );
    commit_workspace_sync(&root, &plan1);

    std::fs::write(
        root.join("prod-code.toml"),
        "[sync]\ninclude = [\"site/data/**\", \"fixtures/*.csv\"]\n",
    )
    .unwrap();
    let plan2 = prepare_workspace_sync_for(&root, NODE_A, None).unwrap();
    assert!(plan2.omitted.is_empty(), "{:?}", plan2.omitted);
    for expected in ["site/data/benchmarks.json", "fixtures/rows.csv"] {
        assert!(
            plan2
                .files
                .iter()
                .any(|f| f.relative_path == expected && f.content.is_some()),
            "missing {expected} in {:?}",
            plan2.files
        );
    }
    commit_workspace_sync(&root, &plan2);

    std::fs::remove_file(root.join("prod-code.toml")).unwrap();
    let plan3 = prepare_workspace_sync_for(&root, NODE_A, None).unwrap();
    for removed in ["site/data/benchmarks.json", "fixtures/rows.csv"] {
        assert!(
            plan3
                .files
                .iter()
                .any(|f| f.relative_path == removed && f.content.is_none()),
            "missing deletion for {removed} in {:?}",
            plan3.files
        );
    }
    clear_sync_cache_for(&root, NODE_A);
}
