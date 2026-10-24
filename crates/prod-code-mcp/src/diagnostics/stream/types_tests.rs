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
fn runtime_paths_2_stream_keys_canonicalize_existing_and_preserve_missing_paths() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    std::fs::write(root.join("a.json"), "{}").unwrap();
    let remote = "127.0.0.1:1".parse().unwrap();
    for file in [PathBuf::from("a.json"), root.join("a.json")] {
        let key = StreamSessionKey::new(remote, &root.join("."), &file, "scope");
        assert_eq!(key, StreamSessionKey { remote, root: root.clone(), file: root.join("a.json"), session_id: "scope".into() });
    }
    for file in [PathBuf::from("new.json"), root.join("new.json")] {
        let key = StreamSessionKey::new(remote, &root, &file, "new");
        assert_eq!(key.file, root.join("new.json"));
    }
    let absent_root = root.join("absent");
    let key = StreamSessionKey::new(remote, &absent_root, Path::new("next.json"), "missing");
    assert_eq!(key.root, absent_root);
    assert_eq!(key.file, root.join("absent/next.json"));
    assert_ne!(key, StreamSessionKey::new(remote, &root, Path::new("next.json"), "missing"));
}
