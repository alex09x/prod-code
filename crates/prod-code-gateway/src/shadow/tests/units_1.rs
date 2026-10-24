/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use prod_code_protocol::{
    AnyStream, FileDelta, ProdCodeCodec, ShadowHypothesis, ShadowRunRequest, ShadowRunResponse,
    WireMessage,
};
use tokio_util::codec::Framed;

use super::super::root::{ShadowRootOwner, remove_shadow_dir, storage_identity};
use super::super::runner::run_shadow;
use super::super::types::{HYPOTHESIS_DIR_PREFIX, OWNERSHIP_LOCK_FILE};

#[test]
fn a_storage_identity_resolves_what_exists_and_keeps_the_rest_lexical() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    std::fs::create_dir_all(dir.path().join("sub")).unwrap();
    for (given, expected) in [
        ("sub", "sub"),
        ("missing/inner", "missing/inner"),
        ("missing/inner/../other", "missing/other"),
        ("sub/../fresh", "fresh"),
        ("./sub/./deeper", "sub/deeper"),
        ("missing/../sub", "sub"),
    ] {
        assert_eq!(
            storage_identity(&dir.path().join(given)),
            base.join(expected),
            "{given}"
        );
    }
    // A relative path is taken from the working directory.
    let cwd = std::env::current_dir().unwrap().canonicalize().unwrap();
    assert_eq!(
        storage_identity(Path::new("no-such-relative-dir-for-unit-tests/inner")),
        cwd.join("no-such-relative-dir-for-unit-tests/inner")
    );
}

#[test]
fn one_gateway_owns_a_shadow_root_and_sweeps_only_its_hypothesis_directories() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("shadow");
    let owner = ShadowRootOwner::acquire(&root).unwrap();
    let error = ShadowRootOwner::acquire(&root)
        .err()
        .expect("a second owner");
    assert!(
        error
            .to_string()
            .contains("is already owned by another gateway"),
        "{error}"
    );
    let canonical = root.canonicalize().unwrap();
    for name in ["one", "two"] {
        let hypothesis = canonical.join(format!("{HYPOTHESIS_DIR_PREFIX}{name}"));
        std::fs::create_dir_all(hypothesis.join("upper/deep")).unwrap();
        std::fs::write(hypothesis.join("upper/deep/f.txt"), "x").unwrap();
    }
    std::fs::create_dir_all(canonical.join("unrelated")).unwrap();
    std::fs::write(
        canonical.join(format!("{HYPOTHESIS_DIR_PREFIX}file")),
        "not a dir",
    )
    .unwrap();
    assert_eq!(owner.sweep(), 2);
    let mut left: Vec<String> = std::fs::read_dir(&canonical)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(
        left,
        [
            format!("{HYPOTHESIS_DIR_PREFIX}file"),
            OWNERSHIP_LOCK_FILE.to_string(),
            "unrelated".to_string()
        ]
    );
    // The namespace is free again once its owner is gone, and sweeping a vanished root is a no-op.
    std::fs::remove_dir_all(&canonical).unwrap();
    assert_eq!(owner.sweep(), 0);
    drop(owner);
    std::fs::create_dir_all(&root).unwrap();
    drop(ShadowRootOwner::acquire(&root).unwrap());
}

#[cfg(unix)]
#[test]
fn a_shadow_root_that_cannot_be_created_or_locked_is_refused_with_the_path() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("plain-file");
    std::fs::write(&file, "x").unwrap();
    let error = ShadowRootOwner::acquire(&file.join("root"))
        .err()
        .expect("under a file");
    assert!(
        error.to_string().starts_with("cannot create shadow root "),
        "{error}"
    );

    // The ownership lock is never followed through a link planted in its place.
    let root = dir.path().join("linked-root");
    std::fs::create_dir_all(&root).unwrap();
    std::os::unix::fs::symlink(&file, root.join(OWNERSHIP_LOCK_FILE)).unwrap();
    let error = ShadowRootOwner::acquire(&root)
        .err()
        .expect("a linked lock");
    assert!(
        error
            .to_string()
            .starts_with("cannot open shadow ownership lock "),
        "{error}"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "x");
}

#[test]
fn removing_a_shadow_directory_ignores_a_missing_one_and_deletes_a_tree() {
    let dir = tempfile::tempdir().unwrap();
    remove_shadow_dir(&dir.path().join("never-created"));
    let tree = dir.path().join("tree");
    std::fs::create_dir_all(tree.join("a/b")).unwrap();
    std::fs::write(tree.join("a/b/f"), "x").unwrap();
    std::fs::write(dir.path().join("keep"), "k").unwrap();
    remove_shadow_dir(&tree);
    assert!(!tree.exists());
    assert!(dir.path().join("keep").exists());
}

struct Storage {
    dir: tempfile::TempDir,
}

impl Storage {
    fn new(with_workspace: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("storage")).unwrap();
        if with_workspace {
            std::fs::create_dir_all(dir.path().join("storage/ws")).unwrap();
            std::fs::write(dir.path().join("storage/ws/a.txt"), "base\n").unwrap();
        }
        Self { dir }
    }

    fn workspace(&self) -> PathBuf {
        self.dir.path().join("storage/ws")
    }

    async fn run(&self, req: ShadowRunRequest) -> ShadowRunResponse {
        let state = crate::ServerState::new(self.dir.path().join("storage"));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = tokio::net::TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (server, _) = listener.accept().await.unwrap();
        let mut framed = Framed::new(AnyStream::from(server), ProdCodeCodec::new());
        run_shadow(&state, &mut framed, req).await.unwrap();
        drop(framed);
        match Framed::new(client, ProdCodeCodec::new()).next().await {
            Some(Ok(WireMessage::ShadowRunResponse(resp))) => resp,
            other => panic!("no shadow response: {other:?}"),
        }
    }
}

fn hypothesis(name: &str, content: Option<&str>) -> ShadowHypothesis {
    ShadowHypothesis {
        name: name.to_string(),
        files: vec![FileDelta {
            relative_path: "a.txt".to_string(),
            content: content.map(|c| c.as_bytes().to_vec()),
            is_executable: false,
        }],
    }
}

fn request(command: &[&str], hypotheses: Vec<ShadowHypothesis>) -> ShadowRunRequest {
    ShadowRunRequest {
        client_workspace_root: "/client/ws".to_string(),
        base_workspace_name: Some("ws".to_string()),
        hypotheses,
        command: command.iter().map(ToString::to_string).collect(),
        env: Vec::new(),
        timeout_secs: 0,
        subdir: None,
        parallel: 0,
        tail_bytes: 0,
        in_memory: false,
        client_agent: None,
        client_host: None,
    }
}

#[tokio::test]
async fn a_shadow_run_that_cannot_start_says_why_and_runs_nothing() {
    let one = || vec![hypothesis("h", Some("x"))];
    for (name, with_workspace, req, expected) in [
        (
            "not synced",
            false,
            request(&["true"], one()),
            "is not synced to this gateway",
        ),
        ("no command", true, request(&[], one()), "empty command"),
        (
            "no hypotheses",
            true,
            request(&["true"], Vec::new()),
            "no hypotheses",
        ),
        (
            "empty name",
            true,
            request(&["true"], vec![hypothesis(" ", Some("x"))]),
            "hypothesis name \" \" is empty or repeated",
        ),
        (
            "repeated name",
            true,
            request(
                &["true"],
                vec![hypothesis("h", Some("1")), hypothesis("h", Some("2"))],
            ),
            "hypothesis name \"h\" is empty or repeated",
        ),
    ] {
        let storage = Storage::new(with_workspace);
        let resp = storage.run(req).await;
        let error = resp.error.unwrap_or_default();
        assert!(error.contains(expected), "{name}: {error}");
        assert!(resp.results.is_empty(), "{name}");
        assert_eq!(resp.mode, "", "{name}");
    }
}

#[tokio::test]
async fn each_hypothesis_sees_its_own_text_and_the_workspace_is_left_as_it_was() {
    let storage = Storage::new(true);
    let mut req = request(
        &["sh", "-c", "cat a.txt"],
        vec![
            hypothesis("first", Some("proposed one\n")),
            hypothesis("second", Some("proposed two\n")),
        ],
    );
    // A subdirectory that escapes the workspace is ignored, not followed.
    req.subdir = Some("..".to_string());
    let resp = storage.run(req).await;
    assert_eq!(resp.error, None);
    assert!(
        ["in-place", "overlay", "overlay-ram"].contains(&resp.mode.as_str()),
        "{}",
        resp.mode
    );
    let names: Vec<&str> = resp.results.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["first", "second"]);
    for (result, text) in resp
        .results
        .iter()
        .zip(["proposed one\n", "proposed two\n"])
    {
        assert_eq!(
            (result.exit_code, result.error.as_deref(), result.timed_out),
            (Some(0), None, false)
        );
        let tail = String::from_utf8(result.output_tail.clone().unwrap()).unwrap();
        assert_eq!(tail, text);
        assert_eq!(result.output_len, text.len() as u64);
    }
    assert_eq!(
        std::fs::read_to_string(storage.workspace().join("a.txt")).unwrap(),
        "base\n"
    );
}
