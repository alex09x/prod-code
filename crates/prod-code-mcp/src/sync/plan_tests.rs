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

#[test]
fn coverage_manifest_resends_deletions_and_partial_scope() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(dir.path().join("other.rs"), "// other\n").unwrap();
    let initial = prepare_non_git_workspace_sync(dir.path(), "fixture", None, SyncCache::default()).unwrap();
    assert!(initial.initial);
    assert_eq!(initial.node, "fixture");
    assert_eq!(initial.files.len(), 2);
    assert_eq!(initial.state.filter_version, RELEVANCE_VERSION);
    let unchanged = prepare_non_git_workspace_sync(dir.path(), "fixture", None, initial.state).unwrap();
    assert!(unchanged.files.is_empty());
    let mut state = unchanged.state;
    state.resend.extend(["src/main.rs".into(), "other.rs".into(), "missing.rs".into()]);
    state.dirty_paths.insert("old.rs".into());
    let partial = prepare_non_git_workspace_sync(dir.path(), "fixture", Some(Path::new("src")), state).unwrap();
    assert!(!partial.initial);
    assert_eq!(partial.files.len(), 1);
    assert_eq!(partial.files[0].relative_path, "src/main.rs");
    assert_eq!(partial.files[0].content.as_deref(), Some(b"fn main() {}\n".as_slice()));
    assert_eq!(partial.state.resend, ["missing.rs".into(), "other.rs".into()].into());
    assert!(partial.state.dirty_paths.is_empty());
    std::fs::remove_file(dir.path().join("other.rs")).unwrap();
    let mut full = prepare_non_git_workspace_sync(dir.path(), "fixture", None, partial.state).unwrap();
    full.files.sort_by(|a,b| a.relative_path.cmp(&b.relative_path));
    assert_eq!(full.files.iter().map(|f| (f.relative_path.as_str(), f.content.as_deref())).collect::<Vec<_>>(), [("missing.rs", None), ("other.rs", None)]);
    assert_eq!(full.state.files.keys().map(String::as_str).collect::<Vec<_>>(), ["src/main.rs"]);
    assert_eq!(full.state.base_commit_sha, None);
    assert!(full.state.resend.is_empty());
    assert_eq!(prepare_workspace_sync(Path::new("/"), None).unwrap_err().to_string(), "refusing to sync filesystem root: /");
    assert_eq!(prepare_non_git_workspace_sync(Path::new("/"), "", None, SyncCache::default()).unwrap_err().to_string(), "refusing to sync filesystem root: /");
}
