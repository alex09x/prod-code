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
fn coverage_b2_uri_nested_roots_and_errors() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let nested = root.join("nested");
    std::fs::create_dir(&nested).unwrap();
    let path = nested.join("é space.rs");
    let uri = Url::from_file_path(&path).unwrap().to_string();
    for roots in [vec![root.clone(), nested.clone()], vec![nested.clone(), root.clone()]] {
        assert_eq!(uri_to_root_and_relative(&roots, &uri).unwrap(), (nested.as_path(), "é space.rs".into()));
    }
    assert_eq!(uri_to_relative(&root, &uri).unwrap(), "nested/é space.rs");
    assert_eq!(resolve(&path).unwrap(), path);
    for uri in ["not a URI", "https://example.test/file"] {
        assert_eq!(uri_to_relative(&root, uri).unwrap_err().to_string(), format!("not a file URI: {uri}"));
    }
    let root_uri = Url::from_file_path(&root).unwrap().to_string();
    assert_eq!(uri_to_relative(&root, &root_uri).unwrap_err().to_string(), format!("{root_uri} names the checkout itself, not a file in it"));
    let outside = root.join("other/file");
    let uri = Url::from_file_path(&outside).unwrap().to_string();
    assert_eq!(uri_to_relative(&nested, &uri).unwrap_err().to_string(), format!("{} is outside the checkout {}", outside.display(), nested.display()));
    assert_eq!(uri_to_root_and_relative(&[], &uri).unwrap_err().to_string(), format!("{} is outside any of the specified repository roots ()", outside.display()));
    assert_eq!(resolve(Path::new("")).unwrap_err().to_string(), " has no existing directory above it");
}

#[test]
#[cfg(unix)]
fn coverage_b2_uri_symlink_containment() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let inside = root.join("inside");
    let outside = root.join("outside");
    std::fs::create_dir(&inside).unwrap();
    std::fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, inside.join("escape")).unwrap();
    assert_eq!(contained(&inside, "new/file").unwrap(), inside.join("new/file"));
    assert_eq!(contained(&inside, "escape/file").unwrap_err().to_string(), format!("escape/file leads outside the checkout, to {}, after the earlier steps of the edit", outside.join("file").display()));
    std::os::unix::fs::symlink("missing", inside.join("broken")).unwrap();
    assert!(resolve(&inside.join("broken/file")).unwrap_err().to_string().starts_with(&format!("{} cannot be resolved: ", inside.join("broken").display())));
}
