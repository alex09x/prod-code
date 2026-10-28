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

fn request(path: &str) -> PullFileRequest {
    PullFileRequest {
        client_workspace_root: "/checkout/project".into(),
        base_workspace_name: Some("project--wt-owned".into()),
        path: path.into(),
        offset: 0,
        list_directory: false,
    }
}

#[test]
fn pull_build_output_is_chunked_and_keeps_executable_mode() {
    let storage = tempfile::tempdir().unwrap();
    let mut req = request("target/release/program");
    let copy = crate::workspace::server_workspace_path(
        storage.path(),
        &req.client_workspace_root,
        req.base_workspace_name.as_deref(),
    );
    let artifact = copy.join(&req.path);
    std::fs::create_dir_all(artifact.parent().unwrap()).unwrap();
    // Exceeds the former 64 MiB whole-file ceiling; each wire frame stays small.
    let file = std::fs::File::create(&artifact).unwrap();
    let size = 65 * 1024 * 1024 + 17;
    file.set_len(size).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&artifact, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    loop {
        let response = pull_workspace_file(storage.path(), &req);
        assert!(response.file.error.is_none(), "{:?}", response.file.error);
        assert_eq!(response.total_bytes, size);
        assert_eq!(response.offset, req.offset);
        let bytes = response.file.content.unwrap();
        assert!(!bytes.is_empty());
        assert!(bytes.len() as u64 <= PULL_CHUNK_BYTES);
        assert!(bytes.iter().all(|b| *b == 0));
        #[cfg(unix)]
        assert_eq!(response.file.is_executable, Some(true));
        req.offset += bytes.len() as u64;
        if !response.file.truncated {
            break;
        }
    }
    assert_eq!(req.offset, size);
}

#[test]
fn pull_missing_file_reports_copy_without_creating_it() {
    let storage = tempfile::tempdir().unwrap();
    let req = request("target/release/missing");
    let response = pull_workspace_file(storage.path(), &req);
    assert!(response.file.content.is_none());
    let error = response.file.error.unwrap();
    assert!(error.contains("not found"), "{error}");
    assert!(error.contains(&response.server_workspace_root), "{error}");
    assert!(!Path::new(&response.server_workspace_root).exists());
}

#[test]
fn lists_bounded_directory_without_writing_or_following_entries() {
    let storage = tempfile::tempdir().unwrap();
    let mut req = request("evidence/run");
    req.list_directory = true;
    let copy = crate::workspace::server_workspace_path(
        storage.path(),
        &req.client_workspace_root,
        req.base_workspace_name.as_deref(),
    );
    let directory = copy.join(&req.path);
    std::fs::create_dir_all(directory.join("nested")).unwrap();
    std::fs::write(directory.join("result.txt"), "original").unwrap();
    let old_request = request("evidence/run");
    assert!(
        pull_workspace_file(storage.path(), &old_request)
            .file
            .error
            .unwrap()
            .contains("not a regular file")
    );
    #[cfg(unix)]
    std::os::unix::fs::symlink("/etc", directory.join("outside")).unwrap();
    let result = pull_workspace_file(storage.path(), &req);
    assert!(result.file.error.is_none(), "{:?}", result.file.error);
    assert_eq!(result.file.is_executable, None);
    let listing: prod_code_protocol::ArtifactListing =
        serde_json::from_slice(&result.file.content.unwrap()).unwrap();
    assert!(!listing.truncated);
    assert!(
        listing
            .entries
            .iter()
            .any(|e| e.name == "result.txt" && e.kind == "file")
    );
    assert!(
        listing
            .entries
            .iter()
            .any(|e| e.name == "nested" && e.kind == "directory")
    );
    #[cfg(unix)]
    {
        assert!(
            listing
                .entries
                .iter()
                .any(|e| e.name == "outside" && e.kind == "symlink")
        );
        let mut escaped = request("evidence/run/outside");
        escaped.list_directory = true;
        assert!(
            pull_workspace_file(storage.path(), &escaped)
                .file
                .error
                .unwrap()
                .contains("refused")
        );
    }
    assert_eq!(
        std::fs::read_to_string(directory.join("result.txt")).unwrap(),
        "original"
    );
    req.offset = 1;
    assert!(
        pull_workspace_file(storage.path(), &req)
            .file
            .error
            .unwrap()
            .contains("offset")
    );
    req.path = "../outside".into();
    req.offset = 0;
    assert!(
        pull_workspace_file(storage.path(), &req)
            .file
            .error
            .unwrap()
            .contains("refused")
    );
}

#[test]
fn listing_reports_its_entry_limit() {
    let storage = tempfile::tempdir().unwrap();
    let mut req = request("evidence/run");
    req.list_directory = true;
    let copy = crate::workspace::server_workspace_path(
        storage.path(),
        &req.client_workspace_root,
        req.base_workspace_name.as_deref(),
    );
    let directory = copy.join(&req.path);
    std::fs::create_dir_all(&directory).unwrap();
    for n in 0..=prod_code_protocol::ARTIFACT_LIST_LIMIT {
        std::fs::write(directory.join(format!("{n:04}")), "").unwrap();
    }
    let result = pull_workspace_file(storage.path(), &req);
    let listing: prod_code_protocol::ArtifactListing =
        serde_json::from_slice(&result.file.content.unwrap()).unwrap();
    assert!(listing.truncated);
    assert_eq!(
        listing.entries.len(),
        prod_code_protocol::ARTIFACT_LIST_LIMIT
    );
}

#[test]
fn listing_a_missing_copy_does_not_create_it() {
    let storage = tempfile::tempdir().unwrap();
    let mut req = request("evidence/run");
    req.list_directory = true;
    let response = pull_workspace_file(storage.path(), &req);
    assert!(response.file.error.is_some());
    assert!(!Path::new(&response.server_workspace_root).exists());
}

#[test]
fn pull_resolves_the_exec_copies_own_ram_target_without_using_stale_disk_output() {
    let storage = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let copy = crate::workspace::server_workspace_path(
        storage.path(),
        "/checkout/project",
        Some("project--wt-owned"),
    );
    std::fs::create_dir_all(copy.join("target/release")).unwrap();
    std::fs::write(copy.join("target/release/program"), b"stale disk output").unwrap();
    // Recreate exec's isolated target namespace without invoking the allocator: its
    // headroom policy can legitimately decline a cache on a busy build node.
    let namespace = crate::runner::ram_cache::ram_cache_namespace(&copy).unwrap();
    let target = cache.path().join(namespace).join("target");
    std::fs::create_dir_all(target.join("release")).unwrap();
    std::fs::write(target.join("release/program"), b"new build output").unwrap();
    crate::sync::artifact_target::record(&copy, Some(&target), &[]);
    let response = pull_workspace_file(storage.path(), &request("target/release/program"));
    assert_eq!(response.file.content.unwrap(), b"new build output");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            copy.join("target/release/program"),
            target.join("release/escape"),
        )
        .unwrap();
        let response = pull_workspace_file(storage.path(), &request("target/release/escape"));
        assert!(response.file.error.unwrap().contains("refused"));
    }
    crate::sync::artifact_target::record(&copy, None, &[]);
    let response = pull_workspace_file(storage.path(), &request("target/release/program"));
    assert_eq!(response.file.content.unwrap(), b"stale disk output");
}

#[cfg(unix)]
#[test]
fn pull_refuses_other_copies_host_files_and_symlink_escapes() {
    use std::os::unix::fs::symlink;
    let storage = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let copy = storage.path().join("project--wt-owned");
    let other = storage.path().join("project--wt-other");
    std::fs::create_dir_all(&copy).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("artifact"), b"other copy").unwrap();
    std::fs::write(outside.path().join("private"), b"host file").unwrap();
    symlink(&other, copy.join("other")).unwrap();
    symlink(outside.path(), copy.join("outside")).unwrap();
    for path in [
        "../project--wt-other/artifact",
        "other/artifact",
        "other/missing",
        "outside/private",
        "/etc/passwd",
    ] {
        let response = pull_workspace_file(storage.path(), &request(path));
        assert!(response.file.content.is_none(), "{path}");
        assert!(response.file.error.unwrap().contains("refused"), "{path}");
    }
    let alias_req = PullFileRequest {
        base_workspace_name: Some("alias".into()),
        ..request("artifact")
    };
    symlink(&other, storage.path().join("alias")).unwrap();
    assert!(
        pull_workspace_file(storage.path(), &alias_req)
            .file
            .error
            .unwrap()
            .contains("refused")
    );
}
