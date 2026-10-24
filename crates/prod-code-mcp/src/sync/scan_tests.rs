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
fn coverage_binary_extension_classification() {
    for ext in [
        "zip", "tar", "gz", "bin", "png", "jpg", "jpeg", "pdf", "wasm", "so", "dylib", "a", "o",
        "exe", "hprof", "mp4", "mov", "pyc", "db", "sqlite",
    ] {
        assert!(
            is_binary_or_media_file(&format!("sample.{}", ext.to_uppercase())),
            "{ext}"
        );
    }
    for name in ["", "sample.rs", "png", "sample.png.rs", "sample.json"] {
        assert!(!is_binary_or_media_file(name));
    }
}

#[test]
fn coverage_scan_single_files_and_filtered_directories() {
    let dir = tempfile::tempdir().unwrap();
    for rel in [
        "src/data/kept.rs",
        "data/skip.rs",
        "vendor/skip.rs",
        "samples/skip.rs",
        "src/main.rs",
        "note.txt",
    ] {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, rel).unwrap();
    }
    let mut files = scan_workspace_files(dir.path(), None).unwrap();
    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    assert_eq!(
        files
            .iter()
            .map(|f| (f.relative_path.as_str(), f.content.as_deref()))
            .collect::<Vec<_>>(),
        [
            ("src/data/kept.rs", Some(b"src/data/kept.rs".as_slice())),
            ("src/main.rs", Some(b"src/main.rs".as_slice()))
        ]
    );
    for sub in [
        dir.path().join("src/main.rs"),
        Path::new("src/main.rs").to_path_buf(),
    ] {
        let files = scan_workspace_files(dir.path(), Some(&sub)).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].relative_path, "src/main.rs");
        assert_eq!(files[0].content.as_deref(), Some(b"src/main.rs".as_slice()));
    }
    assert!(
        scan_workspace_files(dir.path(), Some(Path::new("note.txt")))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        scan_workspace_files(dir.path(), Some(Path::new("missing")))
            .unwrap_err()
            .to_string(),
        format!("Path {:?} does not exist", dir.path().join("missing"))
    );
    assert_eq!(
        scan_workspace_files(Path::new("/"), None)
            .unwrap_err()
            .to_string(),
        "refusing to scan filesystem root: /"
    );
    assert_eq!(
        walk_dir(dir.path(), Path::new("/"), &mut vec![])
            .unwrap_err()
            .to_string(),
        "refusing to scan filesystem root: /"
    );
}
