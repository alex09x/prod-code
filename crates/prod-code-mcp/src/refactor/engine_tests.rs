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
fn coverage_w2_engine_create_overwrite_and_text_creation_roll_back() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(root.join("existing"), "original").unwrap();
    let mut run = MultiRun::default();
    run.apply(&[MultiOp::Create {
        root: root.clone(),
        rel: "existing".into(),
        overwrite: true,
        ignore_if_exists: false,
    }])
    .unwrap();
    let insertion = serde_json::json!({"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"newText":"new contents"});
    run.apply(&[MultiOp::Text {
        root: root.clone(),
        rel: "nested/new.rs".into(),
        edits: vec![insertion],
    }])
    .unwrap();
    assert_eq!(std::fs::read(root.join("existing")).unwrap(), b"");
    assert_eq!(
        std::fs::read(root.join("nested/new.rs")).unwrap(),
        b"new contents"
    );
    assert_eq!(
        run.originals,
        [
            (root.join("existing"), Some(b"original".to_vec())),
            (root.join("nested/new.rs"), None)
        ]
    );
    assert_eq!(
        run.touched,
        [root.join("existing"), root.join("nested/new.rs")]
    );
    assert_eq!(run.also_forget, [(root.clone(), "existing".into())]);
    let (_, errors) = run.journal.roll_back();
    assert!(errors.is_empty());
    assert_eq!(std::fs::read(root.join("existing")).unwrap(), b"original");
    assert!(!root.join("nested").exists());
}

#[test]
fn coverage_b2_engine_ordering_and_rollback() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let mut run = MultiRun::default();
    let create = |rel: &str, overwrite, ignore_if_exists| MultiOp::Create {
        root: root.clone(),
        rel: rel.into(),
        overwrite,
        ignore_if_exists,
    };
    let rename = |from: &str, to: &str, overwrite, ignore_if_exists| MultiOp::Rename {
        from_root: root.clone(),
        from_rel: from.into(),
        to_root: root.clone(),
        to_rel: to.into(),
        overwrite,
        ignore_if_exists,
    };
    run.apply(&[create("new/file", false, false)]).unwrap();
    run.text(&root, "new/file", &[serde_json::json!({"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"newText":"hello"})]).unwrap();
    run.apply(&[
        create("new/file", false, true),
        rename("new", "moved", false, false),
    ])
    .unwrap();
    assert_eq!(std::fs::read(root.join("moved/file")).unwrap(), b"hello");
    assert_eq!(
        run.originals,
        vec![
            (root.join("moved/file"), None),
            (root.join("moved/"), None),
            (root.join("moved"), None)
        ]
    );
    assert_eq!(
        run.text(&root, "new/file", &[]).unwrap_err().to_string(),
        "edit new/file: an earlier step of the edit moved or deleted new; the changes of a workspace edit apply in order, so an edit after a move names the new path"
    );
    assert_eq!(
        run.apply(&[create("moved/file", false, false)])
            .unwrap_err()
            .to_string(),
        "cannot create moved/file: it already exists, and the edit neither overwrites nor ignores it"
    );
    run.apply(&[
        create("target", false, false),
        rename("moved/file", "target", false, true),
    ])
    .unwrap();
    assert_eq!(std::fs::read(root.join("target")).unwrap(), b"");
    assert_eq!(
        run.apply(&[rename("moved/file", "target", false, false)])
            .unwrap_err()
            .to_string(),
        "rename moved/file -> target: target already exists, and the edit does not overwrite it"
    );
    run.apply(&[
        rename("moved/file", "target", true, false),
        rename("target", "target", false, false),
    ])
    .unwrap();
    assert_eq!(std::fs::read(root.join("target")).unwrap(), b"hello");
    assert_eq!(
        run.apply(&[rename("absent", "target", true, false)])
            .unwrap_err()
            .to_string(),
        "rename absent -> target: absent does not exist"
    );
    let (_, errors) = run.journal.roll_back();
    assert_eq!(errors, Vec::<String>::new());
    assert_eq!(files_under(&root), Vec::<String>::new());
}

#[test]
fn coverage_b2_engine_delete_and_deduplication() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("tree/nested")).unwrap();
    std::fs::write(root.join("tree/nested/file"), "old").unwrap();
    let mut run = MultiRun::default();
    let delete = |rel: &str, recursive| MultiOp::Delete {
        root: root.clone(),
        rel: rel.into(),
        recursive,
    };
    assert_eq!(
        run.apply(&[delete("tree", false)]).unwrap_err().to_string(),
        "deleting the directory tree needs `recursive: true`, it is not empty"
    );
    assert_eq!(files_under(&root.join("absent")), Vec::<String>::new());
    run.touch(root.join("tree"));
    run.touch(root.join("tree"));
    assert_eq!(run.touched, vec![root.join("tree")]);
    run.original(&root.join("tree/nested/file"), Some(b"first".to_vec()));
    run.original(&root.join("tree/nested/file"), Some(b"second".to_vec()));
    assert_eq!(run.originals[0].1, Some(b"first".to_vec()));
    run.apply(&[delete("tree", true), delete("missing", false)])
        .unwrap();
    assert_eq!(
        run.also_forget,
        vec![(root.clone(), "tree/nested/file".into())]
    );
    assert!(!root.join("tree").exists());
    assert_eq!(run.journal.roll_back(), (1, vec![]));
    assert_eq!(
        std::fs::read(root.join("tree/nested/file")).unwrap(),
        b"old"
    );
    assert!(is_abs_at_or_under(&root.join("tree/a"), &root.join("tree")));
    assert!(!is_abs_at_or_under(
        &root.join("treehouse"),
        &root.join("tree")
    ));
    forget_synced_across_roots(
        &[root.join("tree"), root.clone()],
        &[
            root.join("tree/nested/file"),
            root.join("outside"),
            PathBuf::from("unrelated"),
        ],
        &[(root.clone(), "new".into())],
    );
}
