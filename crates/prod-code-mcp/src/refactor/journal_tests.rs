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
fn coverage_b2_copy_and_remove_tree() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    std::fs::create_dir_all(source.join("nested")).unwrap();
    std::fs::write(source.join("nested/data"), b"\0payload").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("missing", source.join("link")).unwrap();
    let dest = dir.path().join("dest");
    copy_path_without_following_links(&source, &dest).unwrap();
    assert_eq!(
        std::fs::read(dest.join("nested/data")).unwrap(),
        b"\0payload"
    );
    #[cfg(unix)]
    {
        assert_eq!(
            std::fs::read_link(dest.join("link")).unwrap(),
            Path::new("missing")
        );
        remove_path(&dest.join("link")).unwrap();
        assert!(std::fs::symlink_metadata(dest.join("link")).is_err());
    }
    let error = copy_path_without_following_links(&source, &dest).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("cannot create directory {}", dest.display())
    );
    let missing = dir.path().join("absent");
    assert_eq!(
        copy_path_without_following_links(&missing, &dest)
            .unwrap_err()
            .to_string(),
        format!(
            "cannot inspect {} for cross-volume rename",
            missing.display()
        )
    );
    assert_eq!(
        copy_path_without_following_links(&source.join("nested/data"), &missing.join("data"))
            .unwrap_err()
            .to_string(),
        format!(
            "cannot copy {} to {}",
            source.join("nested/data").display(),
            missing.join("data").display()
        )
    );
    remove_path(&dest).unwrap();
    remove_path(&dest).unwrap();
    assert!(!dest.exists());
    let transfer = unique_transfer_path(&source);
    assert_eq!(transfer.parent(), source.parent());
    assert!(
        transfer
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with(".source.prod-code-transfer-")
    );
    assert!(!transfer.exists());
}

#[test]
fn coverage_b2_rollback_all_step_kinds_and_failures() {
    let dir = tempfile::tempdir().unwrap();
    let p = |name| dir.path().join(name);
    std::fs::write(p("written"), b"new").unwrap();
    std::fs::write(p("same"), b"old").unwrap();
    std::fs::write(p("created"), b"created").unwrap();
    std::fs::create_dir_all(p("tree/child")).unwrap();
    std::fs::write(p("moved"), b"moved").unwrap();
    let mut journal = Journal {
        undo: vec![
            Undo::Write(p("written"), b"old".to_vec()),
            Undo::Write(p("same"), b"old".to_vec()),
            Undo::RemoveFile(p("created")),
            Undo::RemoveFile(p("absent")),
            Undo::RemovePath(p("tree")),
            Undo::Move(p("moved"), p("original")),
        ],
        ..Default::default()
    };
    assert_eq!(journal.roll_back(), (4, vec![]));
    assert_eq!(std::fs::read(p("written")).unwrap(), b"old");
    assert_eq!(std::fs::read(p("original")).unwrap(), b"moved");
    assert!(!p("created").exists());
    assert!(!p("tree").exists());
    assert!(journal.undo.is_empty());
    journal.create_parents(&p("a/b/file")).unwrap();
    assert!(p("a/b").is_dir());
    assert_eq!(journal.roll_back(), (2, vec![]));
    assert!(!p("a").exists());
    journal.undo.push(Undo::Write(p("absent/file"), vec![0]));
    let (restored, errors) = journal.roll_back();
    assert_eq!(restored, 0);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].starts_with(&format!("{}: ", p("absent/file").display())));
}

#[test]
fn coverage_b2_set_aside_commit_and_move_paths() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("file");
    std::fs::write(&file, b"original").unwrap();
    let mut journal = Journal::default();
    journal.set_aside(&file).unwrap();
    let aside = journal.set_aside[0].clone();
    assert!(!file.exists());
    assert_eq!(std::fs::read(&aside).unwrap(), b"original");
    assert_eq!(journal.roll_back(), (1, vec![]));
    assert_eq!(std::fs::read(&file).unwrap(), b"original");
    journal.set_aside(&file).unwrap();
    let aside = journal.set_aside[0].clone();
    journal.commit();
    assert!(!aside.exists());
    assert!(journal.undo.is_empty());
    assert!(journal.set_aside.is_empty());
    let tree = dir.path().join("tree");
    std::fs::create_dir(&tree).unwrap();
    std::fs::write(tree.join("file"), b"data").unwrap();
    journal.set_aside(&tree).unwrap();
    let aside = journal.set_aside[0].clone();
    journal.commit();
    assert!(!aside.exists());
    journal.set_aside = vec![dir.path().join("old/a"), dir.path().join("other")];
    journal.moved(&dir.path().join("old"), &dir.path().join("new"));
    assert_eq!(
        journal.set_aside,
        vec![dir.path().join("new/a"), dir.path().join("other")]
    );
    journal.commit(); // Missing aside paths are harmless and are still drained.
    assert!(journal.set_aside.is_empty());
}
