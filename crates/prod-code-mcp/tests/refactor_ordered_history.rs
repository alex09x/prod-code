/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

use prod_code_mcp::refactor::{
    apply_multi_repository_workspace_edit, apply_workspace_edit, text_before_apply,
};
use serde_json::{Value, json};

#[path = "refactor_ordered_history/safety.rs"]
mod safety;

fn uri(path: &Path) -> String {
    url::Url::from_file_path(path).unwrap().to_string()
}

fn insert(path: &Path, text: &str) -> Value {
    json!({"textDocument": {"uri": uri(path)}, "edits": [{
        "range": {"start": {"line": 0, "character": 0},
                  "end": {"line": 0, "character": 0}}, "newText": text
    }]})
}

fn create(path: &Path) -> Value {
    json!({"kind": "create", "uri": uri(path)})
}

fn delete(path: &Path) -> Value {
    json!({"kind": "delete", "uri": uri(path)})
}

fn rename(from: &Path, to: &Path, overwrite: bool) -> Value {
    json!({"kind": "rename", "oldUri": uri(from), "newUri": uri(to),
           "options": {"overwrite": overwrite}})
}

fn fail_later(root: &Path, changes: &mut Vec<Value>) {
    changes.push(rename(&root.join("missing"), &root.join("never"), false));
}

#[test]
fn binary_slot_can_be_deleted_recreated_and_edited() {
    for fail in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let slot = root.join("slot");
        std::fs::write(&slot, [0xff, 0xfe, 0x00]).unwrap();
        let mut changes = vec![delete(&slot), create(&slot), insert(&slot, "valid UTF-8\n")];
        if fail {
            fail_later(&root, &mut changes);
        }
        let result = apply_workspace_edit(&root, &json!({"documentChanges": changes}));
        if fail {
            assert!(format!("{:#}", result.unwrap_err()).contains("was undone"));
            assert_eq!(std::fs::read(&slot).unwrap(), [0xff, 0xfe, 0x00]);
        } else {
            result.unwrap();
            assert_eq!(std::fs::read(&slot).unwrap(), b"valid UTF-8\n");
        }
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
    }
}

#[test]
fn directory_slot_can_be_moved_recreated_and_edited() {
    for fail in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let slot = root.join("slot");
        let moved = root.join("moved");
        std::fs::create_dir(&slot).unwrap();
        std::fs::write(slot.join("child"), [0xff, 0x00]).unwrap();
        let mut changes = vec![
            rename(&slot, &moved, false),
            create(&slot),
            insert(&slot, "replacement\n"),
        ];
        if fail {
            fail_later(&root, &mut changes);
        }
        let result = apply_workspace_edit(&root, &json!({"documentChanges": changes}));
        if fail {
            assert!(format!("{:#}", result.unwrap_err()).contains("was undone"));
            assert_eq!(std::fs::read(slot.join("child")).unwrap(), [0xff, 0x00]);
            assert!(!moved.exists());
            assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        } else {
            result.unwrap();
            assert_eq!(std::fs::read(&slot).unwrap(), b"replacement\n");
            assert_eq!(std::fs::read(moved.join("child")).unwrap(), [0xff, 0x00]);
            assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
        }
    }
}

#[test]
fn file_history_follows_moves_overwrites_and_repository_boundaries() {
    for overwrite in [false, true] {
        for sequential in [false, true] {
            for cross_root in [false, true] {
                for fail in [false, true] {
                    let dir = tempfile::tempdir().unwrap();
                    let root = dir.path().canonicalize().unwrap();
                    let other = root.join("other");
                    std::fs::create_dir(&other).unwrap();
                    let source = root.join("source");
                    let middle = root.join("middle");
                    let dest = if cross_root { &other } else { &root }.join("dest");
                    std::fs::write(&source, b"source\n").unwrap();
                    safety::set_mode(&source, 0o640);
                    if overwrite {
                        std::fs::write(&dest, b"destination\n").unwrap();
                        safety::set_mode(&dest, 0o600);
                    }
                    let mut changes = vec![insert(&source, "changed ")];
                    if sequential {
                        changes.push(rename(&source, &middle, false));
                        changes.push(rename(&middle, &dest, overwrite));
                    } else {
                        changes.push(rename(&source, &dest, overwrite));
                    }
                    changes.push(insert(&dest, "after "));
                    if fail {
                        fail_later(&root, &mut changes);
                    }
                    let roots = if cross_root {
                        vec![root.as_path(), other.as_path()]
                    } else {
                        vec![root.as_path()]
                    };
                    let result = apply_multi_repository_workspace_edit(
                        &roots,
                        &json!({"documentChanges": changes}),
                    );
                    assert!(!middle.exists());
                    if fail {
                        assert!(format!("{:#}", result.unwrap_err()).contains("was undone"));
                        assert_eq!(std::fs::read(&source).unwrap(), b"source\n");
                        safety::assert_mode(&source, 0o640);
                        if overwrite {
                            assert_eq!(std::fs::read(&dest).unwrap(), b"destination\n");
                            safety::assert_mode(&dest, 0o600);
                        } else {
                            assert!(!dest.exists());
                        }
                    } else {
                        result.unwrap();
                        assert!(!source.exists());
                        assert_eq!(std::fs::read(&dest).unwrap(), b"after changed source\n");
                        safety::assert_mode(&dest, 0o640);
                        assert_eq!(text_before_apply(&dest), "source\n");
                    }
                    let expected = 1 + usize::from(overwrite || !fail) + usize::from(fail);
                    assert_eq!(safety::entry_count(&root), expected);
                }
            }
        }
    }
}
