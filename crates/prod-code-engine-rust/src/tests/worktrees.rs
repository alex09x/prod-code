/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Tests for worktree attachment refcounting, isolation, and path translation.

use crate::engine::RustEngine;

#[test]
fn test_worktree_refactor_paths_resolve_to_worktree() {
    let temp = tempfile::tempdir().unwrap();
    let base_dir = temp.path().join("base_repo");
    std::fs::create_dir_all(base_dir.join("src")).unwrap();
    std::fs::write(
        base_dir.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let base_lib = base_dir.join("src/lib.rs");
    std::fs::write(
        &base_lib,
        "pub fn rename_me() -> u32 {\n    42\n}\n\npub fn caller() -> u32 {\n    rename_me()\n}\n",
    )
    .unwrap();

    let mut engine = RustEngine::load(&base_dir).expect("Must load base fixture");

    let wt_dir = temp.path().join("wt_repo");
    std::fs::create_dir_all(wt_dir.join("src")).unwrap();
    std::fs::write(
        wt_dir.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let wt_lib = wt_dir.join("src/lib.rs");
    std::fs::write(
        &wt_lib,
        "pub fn rename_me() -> u32 {\n    42\n}\n\npub fn caller() -> u32 {\n    rename_me()\n}\n",
    )
    .unwrap();

    engine.attach_worktree(&wt_dir).expect("attach worktree");

    // 1. Rename symbol from the worktree: all edited files must be inside worktree, NOT base
    let rename_res = engine
        .rename(&wt_lib, 1, 8, "renamed_fn")
        .expect("rename query")
        .expect("rename outcome");

    assert!(!rename_res.files.is_empty(), "rename produced files");
    for file in &rename_res.files {
        assert!(
            file.path.starts_with(&wt_dir),
            "Renamed file path {:?} must be in worktree {:?}, not base {:?}",
            file.path,
            wt_dir,
            base_dir
        );
    }

    // 2. Safe delete from the worktree: rewritten path must be inside worktree, NOT base
    let wt_unused = wt_dir.join("src/unused.rs");
    let base_unused = base_dir.join("src/unused.rs");
    std::fs::write(&base_unused, "pub fn dead_code() -> u8 { 0 }\n").unwrap();
    std::fs::write(&wt_unused, "pub fn dead_code() -> u8 { 0 }\n").unwrap();
    engine.reload_file(&wt_unused).unwrap();

    let delete_res = engine
        .safe_delete(&wt_unused, 1, 8)
        .expect("delete query")
        .expect("delete outcome");

    assert!(!delete_res.files.is_empty(), "delete produced files");
    for file in &delete_res.files {
        assert!(
            file.path.starts_with(&wt_dir),
            "Deleted file path {:?} must be in worktree {:?}, not base {:?}",
            file.path,
            wt_dir,
            base_dir
        );
    }
}

#[test]
fn test_worktree_attachment_refcounting() {
    let temp = tempfile::tempdir().unwrap();
    let base_dir = temp.path().join("base");
    std::fs::create_dir_all(base_dir.join("src")).unwrap();
    std::fs::write(
        base_dir.join("Cargo.toml"),
        "[package]\nname = \"base-pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(base_dir.join("src/lib.rs"), "pub fn hello() {}\n").unwrap();

    let mut engine = RustEngine::load(&base_dir).unwrap();

    let wt_dir = temp.path().join("base--wt-test");
    std::fs::create_dir_all(wt_dir.join("src")).unwrap();
    std::fs::write(
        wt_dir.join("Cargo.toml"),
        "[package]\nname = \"base-pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(wt_dir.join("src/lib.rs"), "pub fn hello() {}\n").unwrap();

    assert!(!engine.has_worktree(&wt_dir));

    // First attach: refcount 1 -> attached
    engine.attach_worktree(&wt_dir).expect("first attach");
    assert!(engine.has_worktree(&wt_dir));

    // Second attach: refcount 2 -> still attached
    engine.attach_worktree(&wt_dir).expect("second attach");
    assert!(engine.has_worktree(&wt_dir));

    // First detach: refcount 2 -> 1, returns false, still attached
    let detached_first = engine.detach_worktree(&wt_dir);
    assert!(
        !detached_first,
        "must not detach while second reference exists"
    );
    assert!(engine.has_worktree(&wt_dir));

    // Second detach: refcount 1 -> 0, returns true, now detached
    let detached_second = engine.detach_worktree(&wt_dir);
    assert!(detached_second, "must detach when last reference drops");
    assert!(!engine.has_worktree(&wt_dir));
}

#[test]
fn test_failed_worktree_attach_does_not_poison_refcount() {
    let temp = tempfile::tempdir().unwrap();
    let base_dir = temp.path().join("base");
    std::fs::create_dir_all(base_dir.join("src")).unwrap();
    std::fs::write(
        base_dir.join("Cargo.toml"),
        "[package]\nname = \"base-pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(base_dir.join("src/lib.rs"), "pub fn hello() {}\n").unwrap();

    let mut engine = RustEngine::load(&base_dir).unwrap();

    let invalid_wt = temp.path().join("invalid-wt");
    std::fs::create_dir_all(&invalid_wt).unwrap();
    // No Cargo.toml: discovery must fail
    let fail_res = engine.attach_worktree(&invalid_wt);
    assert!(fail_res.is_err(), "must fail when manifest is missing");
    assert!(!engine.has_worktree(&invalid_wt));
    assert_eq!(engine.worktree_attachment_count(&invalid_wt), 0);

    // Now repair the worktree with a valid manifest
    std::fs::create_dir_all(invalid_wt.join("src")).unwrap();
    std::fs::write(
        invalid_wt.join("Cargo.toml"),
        "[package]\nname = \"base-pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(invalid_wt.join("src/lib.rs"), "pub fn hello() {}\n").unwrap();

    // Retry must successfully mount the overlay, NOT falsely return Ok without mounting
    engine
        .attach_worktree(&invalid_wt)
        .expect("retry must succeed after repair");
    assert!(engine.has_worktree(&invalid_wt));
    assert_eq!(engine.worktree_attachment_count(&invalid_wt), 1);

    // Detaching once must cleanly remove the overlay
    assert!(engine.detach_worktree(&invalid_wt));
    assert!(!engine.has_worktree(&invalid_wt));
    assert_eq!(engine.worktree_attachment_count(&invalid_wt), 0);
}

#[test]
fn test_worktree_file_id_for_path_isolation() {
    let temp = tempfile::tempdir().unwrap();
    let base_dir = temp.path().join("base");
    std::fs::create_dir_all(base_dir.join("src")).unwrap();
    std::fs::write(
        base_dir.join("Cargo.toml"),
        "[package]\nname = \"base-pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(base_dir.join("src/lib.rs"), "pub fn base_fn() {}\n").unwrap();

    let mut engine = RustEngine::load(&base_dir).unwrap();

    let wt1_dir = temp.path().join("wt1");
    std::fs::create_dir_all(wt1_dir.join("src")).unwrap();
    std::fs::write(
        wt1_dir.join("Cargo.toml"),
        "[package]\nname = \"base-pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(wt1_dir.join("src/lib.rs"), "pub fn base_fn() {}\n").unwrap();

    let wt2_dir = temp.path().join("wt2");
    std::fs::create_dir_all(wt2_dir.join("src")).unwrap();
    std::fs::write(
        wt2_dir.join("Cargo.toml"),
        "[package]\nname = \"base-pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(wt2_dir.join("src/lib.rs"), "pub fn base_fn() {}\n").unwrap();

    engine.attach_worktree(&wt1_dir).unwrap();
    engine.attach_worktree(&wt2_dir).unwrap();

    let wt1_private_file = wt1_dir.join("src/private_a.rs");
    let wt2_private_file = wt2_dir.join("src/private_b.rs");

    engine
        .update_base(
            &wt1_private_file,
            Some("pub fn private_a() {}\n".to_string()),
        )
        .unwrap();
    engine
        .update_base(
            &wt2_private_file,
            Some("pub fn private_b() {}\n".to_string()),
        )
        .unwrap();

    let snap_base = engine.snapshot();
    let snap_wt1 = engine.snapshot_for(&wt1_dir);
    let snap_wt2 = engine.snapshot_for(&wt2_dir);

    // wt1 snapshot can see wt1's private file, but NOT wt2's private file
    assert!(snap_wt1.file_id_for_path(&wt1_private_file).is_some());
    assert!(
        snap_wt1.file_id_for_path(&wt2_private_file).is_none(),
        "WT1 snapshot must not leak file_id for WT2's private file"
    );

    // wt2 snapshot can see wt2's private file, but NOT wt1's private file
    assert!(snap_wt2.file_id_for_path(&wt2_private_file).is_some());
    assert!(
        snap_wt2.file_id_for_path(&wt1_private_file).is_none(),
        "WT2 snapshot must not leak file_id for WT1's private file"
    );

    // Base snapshot cannot see either worktree's private files
    assert!(snap_base.file_id_for_path(&wt1_private_file).is_none());
    assert!(snap_base.file_id_for_path(&wt2_private_file).is_none());
}

#[test]
fn test_worktree_reloads_when_cargo_manifest_changes() {
    let temp = tempfile::tempdir().unwrap();
    let base_dir = temp.path().join("base");
    std::fs::create_dir_all(base_dir.join("src")).unwrap();
    std::fs::write(
        base_dir.join("Cargo.toml"),
        "[package]\nname = \"base-pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(base_dir.join("src/lib.rs"), "pub fn base_fn() {}\n").unwrap();

    let mut engine = RustEngine::load(&base_dir).unwrap();

    let wt_dir = temp.path().join("base--wt-test");
    std::fs::create_dir_all(wt_dir.join("src")).unwrap();
    std::fs::write(
        wt_dir.join("Cargo.toml"),
        "[package]\nname = \"base-pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let wt_lib = wt_dir.join("src/lib.rs");
    std::fs::write(&wt_lib, "pub fn wt_fn() {}\n").unwrap();

    engine.attach_worktree(&wt_dir).expect("first attach");
    assert!(!engine.is_worktree_stale(&wt_dir));

    let dep_dir = temp.path().join("dep_crate");
    std::fs::create_dir_all(dep_dir.join("src")).unwrap();
    std::fs::write(
        dep_dir.join("Cargo.toml"),
        "[package]\nname = \"dep-crate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        dep_dir.join("src/lib.rs"),
        "pub fn dep_fn() -> u32 { 100 }\n",
    )
    .unwrap();

    std::fs::write(
        &wt_lib,
        "use dep_crate::dep_fn;\npub fn wt_fn() -> u32 { dep_fn() }\n",
    )
    .unwrap();
    engine.reload_file(&wt_lib).unwrap();
    let diags_before = {
        let snap_before = engine.snapshot_for(&wt_dir);
        snap_before.diagnostics(&wt_lib).unwrap()
    };
    assert!(
        diags_before.iter().any(|d| d.code.contains("unresolved")
            || d.message.contains("dep_crate")
            || d.code == "E0432"),
        "Diagnostics must report unresolved import before dependency is added: {:?}",
        diags_before
    );

    let dep_path = dep_dir.to_str().unwrap().replace('\\', "/");
    std::fs::write(
        wt_dir.join("Cargo.toml"),
        format!(
            "[package]\nname = \"base-pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ndep-crate = {{ path = \"{dep_path}\" }}\n"
        ),
    )
    .unwrap();

    assert!(
        engine.is_worktree_stale(&wt_dir),
        "Worktree must be detected as stale after Cargo.toml edit"
    );

    let reloaded = engine.ensure_fresh_for_path(&wt_lib).unwrap();
    assert!(
        reloaded,
        "ensure_fresh_for_path must report that reload occurred"
    );
    assert!(
        !engine.is_worktree_stale(&wt_dir),
        "Worktree must not be stale after reload"
    );

    let snap_after = engine.snapshot_for(&wt_dir);
    let diags_after = snap_after.diagnostics(&wt_lib).unwrap();
    assert!(
        !diags_after.iter().any(|d| d.code == "E0432"
            || d.message.contains("dep_crate")
            || d.code.contains("unresolved")),
        "Diagnostics must resolve dep_crate after manifest reload: {:?}",
        diags_after
    );
}
