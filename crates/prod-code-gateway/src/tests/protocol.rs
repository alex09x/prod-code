/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::*;
use std::path::PathBuf;
use std::time::Duration;

/// Only a server's requests are held back from the client, not its notifications (#391).
#[test]
fn a_server_request_is_told_from_a_notification() {
    assert!(super::super::is_server_request(
        r#"{"jsonrpc":"2.0","id":2,"method":"workspace/configuration","params":{}}"#
    ));
    assert!(!super::super::is_server_request(
        r#"{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":"file:///a","diagnostics":[]}}"#
    ));
    assert!(!super::super::is_server_request(
        r#"{"jsonrpc":"2.0","id":2,"result":null}"#
    ));
}

#[tokio::test]
async fn shared_output_queue_deadline_closes_every_generation_sender() {
    let (raw_tx, _rx) = rapidfire::mpsc::bounded(1);
    let output = SharedOutputSender::new(raw_tx, Duration::from_millis(10));
    output.send(WireMessage::Ping).await.unwrap();

    assert!(matches!(
        output.send(WireMessage::Ping).await,
        Err(SharedOutputSendError::Deadline)
    ));
    assert!(matches!(
        output.send(WireMessage::Ping).await,
        Err(SharedOutputSendError::Closed)
    ));
}

#[tokio::test]
async fn owned_join_aborts_and_observes_the_exact_writer_task() {
    let task = tokio::spawn(async {
        std::future::pending::<()>().await;
        Ok(())
    });
    let mut owned = OwnedJoin::new(task);
    owned.abort();
    let result = owned.task_mut().await;
    assert!(
        result
            .as_ref()
            .is_err_and(tokio::task::JoinError::is_cancelled)
    );
    assert!(flatten_writer_result(result).is_err());
    owned.clear_finished();
}

#[test]
fn read_server_file_caps_external_source_to_2mib_even_with_explicit_large_limit() {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return;
    };
    let external_base = home.join(".cargo/registry");
    if std::fs::create_dir_all(&external_base).is_err() {
        return;
    }
    let test_file = external_base.join(format!("test_cap_{}.txt", std::process::id()));
    let data = vec![b'x'; 3 * 1024 * 1024]; // 3 MiB
    if std::fs::write(&test_file, &data).is_err() {
        return;
    }
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _guard = Cleanup(test_file.clone());

    let temp_storage = tempfile::tempdir().unwrap();

    let req = prod_code_protocol::ReadFileRequest {
        path: test_file.to_string_lossy().into_owned(),
        max_bytes: 64 * 1024 * 1024, // Explicit 64 MiB requested
    };
    let resp = read_server_file(temp_storage.path(), &req);

    assert!(
        resp.error.is_none(),
        "read_server_file failed: {:?}",
        resp.error
    );
    assert!(resp.truncated, "external source must be truncated to 2 MiB");
    let content = resp.content.expect("content present");
    assert_eq!(
        content.len(),
        2 * 1024 * 1024,
        "external source capped at 2 MiB"
    );
}

#[test]
fn read_server_file_allows_workspace_artifact_up_to_64mib() {
    let temp_storage = tempfile::tempdir().unwrap();
    let artifact = temp_storage.path().join("target/release/large_bin");
    std::fs::create_dir_all(artifact.parent().unwrap()).unwrap();
    let data = vec![b'y'; 3 * 1024 * 1024]; // 3 MiB
    std::fs::write(&artifact, &data).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&artifact, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let req = prod_code_protocol::ReadFileRequest {
        path: artifact.to_string_lossy().into_owned(),
        max_bytes: 0, // Default in workspace
    };
    let resp = read_server_file(temp_storage.path(), &req);
    assert!(
        resp.error.is_none(),
        "read_server_file failed: {:?}",
        resp.error
    );
    assert!(
        !resp.truncated,
        "workspace artifact must not be truncated under 64 MiB"
    );
    #[cfg(unix)]
    assert_eq!(
        resp.is_executable,
        Some(true),
        "gateway response must retain executable mode"
    );
    assert_eq!(resp.content.expect("content").len(), 3 * 1024 * 1024);
}

#[test]
fn is_readable_source_path_allows_polyglot_dependencies_and_rejects_arbitrary_files() {
    let storage = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();

    // 1. Rust cargo registry
    let cargo_file = home.path().join(".cargo/registry/src/github.com/lib.rs");
    std::fs::create_dir_all(cargo_file.parent().unwrap()).unwrap();
    std::fs::write(&cargo_file, "pub fn foo() {}").unwrap();
    assert!(is_readable_source_path_with_home(
        storage.path(),
        &cargo_file,
        Some(home.path())
    ));

    // 2. Python virtualenv / uv cache
    let py_file = home.path().join(".cache/uv/wheels/pkg/module.py");
    std::fs::create_dir_all(py_file.parent().unwrap()).unwrap();
    std::fs::write(&py_file, "def bar(): pass").unwrap();
    assert!(is_readable_source_path_with_home(
        storage.path(),
        &py_file,
        Some(home.path())
    ));

    // 3. Node pnpm store
    let pnpm_file = home.path().join(".local/share/pnpm/store/pkg/index.d.ts");
    std::fs::create_dir_all(pnpm_file.parent().unwrap()).unwrap();
    std::fs::write(&pnpm_file, "export declare const x: number;").unwrap();
    assert!(is_readable_source_path_with_home(
        storage.path(),
        &pnpm_file,
        Some(home.path())
    ));

    // 4. Do not expose unrelated checkouts just because they contain node_modules.
    let nm_file = home.path().join("projects/foo/node_modules/bar/index.js");
    std::fs::create_dir_all(nm_file.parent().unwrap()).unwrap();
    std::fs::write(&nm_file, "module.exports = {};").unwrap();
    assert!(!is_readable_source_path_with_home(
        storage.path(),
        &nm_file,
        Some(home.path())
    ));

    let workspace_nm_file = storage.path().join("workspace/node_modules/bar/index.js");
    std::fs::create_dir_all(workspace_nm_file.parent().unwrap()).unwrap();
    std::fs::write(&workspace_nm_file, "module.exports = {};").unwrap();
    assert!(is_readable_source_path(storage.path(), &workspace_nm_file));

    // 5. Arbitrary sensitive files rejected
    let ssh_key = home.path().join(".ssh/id_rsa");
    std::fs::create_dir_all(ssh_key.parent().unwrap()).unwrap();
    std::fs::write(&ssh_key, "private-key-material").unwrap();
    assert!(!is_readable_source_path_with_home(
        storage.path(),
        &ssh_key,
        Some(home.path())
    ));

    let bashrc = home.path().join(".bashrc");
    std::fs::write(&bashrc, "export SECRET=1").unwrap();
    assert!(!is_readable_source_path_with_home(
        storage.path(),
        &bashrc,
        Some(home.path())
    ));
}

#[test]
fn gopath_source_policy_checks_each_configured_root() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("first");
    let second = temp.path().join("second");
    let source = second.join("pkg/mod/example.test/module@v1/source.go");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    std::fs::write(&source, "package module\n").unwrap();
    let gopath = std::env::join_paths([first.as_os_str(), second.as_os_str()]).unwrap();

    assert!(is_gopath_source_path(
        &source.canonicalize().unwrap(),
        Some(&gopath)
    ));
    assert!(!is_gopath_source_path(&source, Some(first.as_os_str())));
}
