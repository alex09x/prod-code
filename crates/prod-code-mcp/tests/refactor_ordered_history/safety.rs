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

pub fn set_mode(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
}

pub fn assert_mode(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            mode
        );
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
}

pub fn entry_count(root: &Path) -> usize {
    std::fs::read_dir(root)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            1 + if path.is_dir() { entry_count(&path) } else { 0 }
        })
        .sum()
}

#[test]
fn directory_history_survives_success_and_rolls_back_on_failure() {
    for fail in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let old = root.join("old");
        let new = root.join("new");
        std::fs::create_dir(&old).unwrap();
        std::fs::write(old.join("text"), b"original\n").unwrap();
        std::fs::write(old.join("remove"), [0xff]).unwrap();
        let mut changes = vec![
            insert(&old.join("text"), "changed "),
            rename(&old, &new, false),
            delete(&new.join("remove")),
        ];
        if fail {
            fail_later(&root, &mut changes);
        }
        let result = apply_workspace_edit(&root, &json!({"documentChanges": changes}));
        if fail {
            assert!(format!("{:#}", result.unwrap_err()).contains("was undone"));
            assert_eq!(std::fs::read(old.join("text")).unwrap(), b"original\n");
            assert_eq!(std::fs::read(old.join("remove")).unwrap(), [0xff]);
            assert!(!new.exists());
        } else {
            result.unwrap();
            assert_eq!(text_before_apply(&new.join("text")), "original\n");
            assert_eq!(
                std::fs::read(new.join("text")).unwrap(),
                b"changed original\n"
            );
            assert!(!old.exists());
            assert!(!new.join("remove").exists());
        }
    }
}

#[test]
fn invalid_ordered_sequences_restore_every_byte_and_path() {
    for invalid in ["binary", "nonempty", "vacated", "duplicate", "ignore"] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let text = root.join("text");
        let binary = root.join("binary");
        let tree = root.join("tree");
        std::fs::write(&text, b"original\n").unwrap();
        set_mode(&text, 0o640);
        std::fs::write(&binary, [0xff, 0x00]).unwrap();
        std::fs::create_dir(&tree).unwrap();
        std::fs::write(tree.join("child"), b"child").unwrap();
        let mut changes = vec![insert(&text, "changed ")];
        match invalid {
            "binary" => {
                changes.push(rename(&binary, &root.join("moved"), false));
                changes.push(insert(&root.join("moved"), "bad"));
            }
            "nonempty" => {
                changes.push(create(&tree.join("new")));
                changes.push(delete(&tree));
            }
            "vacated" => {
                changes.push(delete(&text));
                changes.push(insert(&text, "bad"));
            }
            "duplicate" => {
                changes.push(create(&root.join("new")));
                changes.push(create(&root.join("new")));
            }
            "ignore" => {
                let mut ignored = create(&binary);
                ignored["options"] = json!({"ignoreIfExists": true});
                changes.push(ignored);
                changes.push(insert(&binary, "bad"));
            }
            _ => unreachable!(),
        }
        let error = apply_workspace_edit(&root, &json!({"documentChanges": changes})).unwrap_err();
        assert!(format!("{error:#}").contains("was undone"), "{error:#}");
        assert_eq!(std::fs::read(&text).unwrap(), b"original\n");
        assert_mode(&text, 0o640);
        assert_eq!(std::fs::read(&binary).unwrap(), [0xff, 0x00]);
        assert_eq!(std::fs::read(tree.join("child")).unwrap(), b"child");
        assert_eq!(entry_count(&root), 4);
    }
}

#[test]
fn unaffected_invalid_text_is_still_rejected_before_any_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let binary = root.join("binary");
    std::fs::write(&binary, [0xff]).unwrap();
    let error = apply_workspace_edit(
        &root,
        &json!({"documentChanges": [
            create(&root.join("unrelated")), insert(&binary, "bad")
        ]}),
    )
    .unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("nothing was written"), "{message}");
    assert!(message.contains("not UTF-8"), "{message}");
    assert_eq!(std::fs::read(&binary).unwrap(), [0xff]);
    assert_eq!(entry_count(&root), 1);
}

#[cfg(unix)]
#[test]
fn moved_symlinks_remain_confined_and_failed_writes_restore_the_tree() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let nested = root.join("nested");
    let old = nested.join("old");
    let moved = root.join("moved");
    std::fs::create_dir(&nested).unwrap();
    std::fs::create_dir(&old).unwrap();
    // Valid initially, escaping only once the directory moves one level up.
    symlink("../../outside", old.join("link")).unwrap();
    let edit = json!({"documentChanges": [
        rename(&old, &moved, false), insert(&moved.join("link"), "bad")
    ]});
    assert!(apply_workspace_edit(&root, &edit).is_err());
    assert!(old.is_dir());
    assert!(!moved.exists());
    assert_eq!(
        std::fs::read_link(old.join("link")).unwrap(),
        Path::new("../../outside")
    );
    assert_eq!(std::fs::read_dir(&nested).unwrap().count(), 1);
}
