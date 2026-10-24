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
fn runtime_paths_2_placement_updates_preserve_other_workspaces() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("nested/placement.json");
    let a = "127.0.0.1:9001".parse().unwrap();
    let b = "127.0.0.1:9002".parse().unwrap();
    remember_placement_at(None, "a", a);
    forget_placement_at(None, "a");
    assert!(!path.exists());
    remember_placement_at(Some(&path), "a", a);
    remember_placement_at(Some(&path), "b", b);
    remember_placement_at(Some(&path), "a", b);
    assert_eq!(
        load_placement(&path).workspaces,
        BTreeMap::from([("a".into(), b), ("b".into(), b)])
    );
    let bytes = std::fs::read(&path).unwrap();
    forget_placement_at(Some(&path), "missing");
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    forget_placement_at(Some(&path), "a");
    assert_eq!(
        load_placement(&path).workspaces,
        BTreeMap::from([("b".into(), b)])
    );
    forget_placement_at(Some(&path), "b");
    assert_eq!(load_placement(&path).workspaces, BTreeMap::new());
    std::fs::write(&path, "bad JSON").unwrap();
    assert_eq!(load_placement(&path).workspaces, BTreeMap::new());
    // A non-directory parent and a directory destination exercise best-effort persistence.
    save_placement(&path.join("child"), &Placement::default());
    let directory = temp.path().join("directory");
    std::fs::create_dir(&directory).unwrap();
    save_placement(&directory, &Placement::default());
    assert!(directory.is_dir());
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 0);
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 2);
}
