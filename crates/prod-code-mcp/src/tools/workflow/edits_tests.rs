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
use std::fs;

fn block(path: &str, old: &str, new: &str) -> BlockEdit {
    BlockEdit {
        path: path.into(),
        old: old.into(),
        new: new.into(),
    }
}

fn prep(path: &str, base_sha256: &str, old: &str, new: &str) -> PreparedFile {
    PreparedFile {
        path: path.into(),
        base_sha256: base_sha256.into(),
        old: old.into(),
        new: new.into(),
    }
}

fn write_tree(root: &Path, files: &[(&str, &[u8])]) {
    for (rel, bytes) in files {
        let full = root.join(rel);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(full, bytes).unwrap();
    }
}

#[test]
fn preview_is_non_mutating_and_serde_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let orig_a = b"fn alpha() -> i32 {\n    1\n}\n";
    let orig_b = b"pub const LABEL: &str = \"before\";\n";
    write_tree(root, &[("src/a.rs", orig_a), ("src/nested/b.rs", orig_b)]);

    let edits = vec![
        block("src/a.rs", "    1\n", "    42\n"),
        block("src/nested/b.rs", "\"before\"", "\"after\""),
    ];
    let decoded: Vec<BlockEdit> =
        serde_json::from_str(&serde_json::to_string(&edits).unwrap()).unwrap();
    assert_eq!(decoded, edits);
    assert!(
        serde_json::from_str::<BlockEdit>(r#"{"path":"a","old":"b","new":"c","x":1}"#).is_err()
    );

    let out = preview(root, &decoded).unwrap();
    let round_trip: Preview = serde_json::from_str(&serde_json::to_string(&out).unwrap()).unwrap();
    assert_eq!(round_trip, out);
    assert!(out.diff.contains("--- a/src/a.rs") && out.diff.contains("+++ b/src/a.rs"));
    assert!(out.diff.contains("-    1") && out.diff.contains("+    42"));
    assert!(out.diff.contains("-pub const LABEL: &str = \"before\";"));
    assert!(out.diff.contains("+pub const LABEL: &str = \"after\";"));
    assert_eq!(out.files[0].base_sha256, sha256_hex(orig_a));
    assert_eq!(out.files[1].base_sha256, sha256_hex(orig_b));

    assert_eq!(fs::read(root.join("src/a.rs")).unwrap(), orig_a);
    assert_eq!(fs::read(root.join("src/nested/b.rs")).unwrap(), orig_b);
}

#[test]
fn multi_file_apply_updates_all_targets_and_rejects_replay() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_tree(
        root,
        &[
            ("a.rs", b"fn a() { old_a(); }\n"),
            ("sub/b.rs", b"fn b() { old_b(); }\n"),
            ("sub/c.rs", b"fn c() { keep_c(); }\n"),
        ],
    );

    let prepared = preview(
        root,
        &[
            block("a.rs", "old_a()", "new_a()"),
            block("sub/b.rs", "old_b()", "new_b()"),
        ],
    )
    .unwrap();

    let touched = apply(root, &prepared.files).unwrap();
    assert_eq!(
        touched,
        vec![PathBuf::from("a.rs"), PathBuf::from("sub/b.rs")]
    );
    assert_eq!(
        fs::read_to_string(root.join("a.rs")).unwrap(),
        "fn a() { new_a(); }\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("sub/b.rs")).unwrap(),
        "fn b() { new_b(); }\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("sub/c.rs")).unwrap(),
        "fn c() { keep_c(); }\n"
    );
    assert!(apply(root, &prepared.files).is_err());
    crate::sync::clear_sync_cache(&root.canonicalize().unwrap());
}

#[test]
fn stale_hash_in_second_file_refuses_all_writes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let orig_first = "fn first() -> u32 { 10 }\n";
    write_tree(
        root,
        &[
            ("first.rs", orig_first.as_bytes()),
            ("second.rs", b"fn second() -> u32 { 20 }\n"),
        ],
    );

    let prepared = preview(
        root,
        &[
            block("first.rs", "10", "11"),
            block("second.rs", "20", "22"),
        ],
    )
    .unwrap();

    let mutated_second = "// concurrent edit\nfn second() -> u32 { 20 }\n";
    fs::write(root.join("second.rs"), mutated_second).unwrap();

    let err = apply(root, &prepared.files).unwrap_err();
    assert!(format!("{err:#}").contains("SHA-256"), "{err:#}");
    assert_eq!(
        fs::read_to_string(root.join("first.rs")).unwrap(),
        orig_first
    );
    assert_eq!(
        fs::read_to_string(root.join("second.rs")).unwrap(),
        mutated_second
    );
}

#[test]
fn refuses_empty_noop_missing_duplicate_overlapping_and_duplicate_paths() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_tree(
        root,
        &[
            ("dup.rs", b"let x = 1;\nlet y = 2;\nlet x = 1;\n"),
            ("overlap.rs", b"prefix ababa suffix\n"),
            ("single.rs", b"let only = true;\n"),
        ],
    );

    for invalid in [
        vec![block("single.rs", "", "x")],
        vec![block("single.rs", "true", "true")],
        vec![block("single.rs", "absent", "false")],
        vec![block("dup.rs", "let x = 1;", "let x = 9;")],
        vec![block("overlap.rs", "aba", "xyz")],
        vec![
            block("single.rs", "only", "first"),
            block("./single.rs", "true", "false"),
        ],
    ] {
        assert!(
            preview(root, &invalid).is_err(),
            "expected failure for {invalid:?}"
        );
    }

    let dup_sha = sha256_hex(&fs::read(root.join("dup.rs")).unwrap());
    assert!(
        apply(
            root,
            &[prep("dup.rs", &dup_sha, "let x = 1;", "let x = 3;")]
        )
        .is_err()
    );
    let single_sha = sha256_hex(&fs::read(root.join("single.rs")).unwrap());
    assert!(apply(root, &[prep("single.rs", &single_sha, "missing", "x")]).is_err());
}

#[test]
fn unicode_replacements_use_exact_zero_based_utf16_end_positions() {
    // Non-BMP astral characters (🦀, 𝄞 = 2 UTF-16 code units each) on the final line without '\n'.
    let no_nl = "const BANNER: &str = \"café 漢字\"\nlet tail = \"🦀𝄞\"; // tail";
    assert_eq!(utf16_end_position(no_nl), (1, 26));
    assert_eq!(utf16_end_position("line0\nline1\r\n"), (2, 0));

    let edit_json = build_workspace_edit(&[(PathBuf::from("/tmp/u.rs"), (1, 26), "ok".into())]);
    assert_eq!(
        edit_json["documentChanges"][0]["edits"][0]["range"],
        json!({ "start": { "line": 0, "character": 0 }, "end": { "line": 1, "character": 26 } })
    );

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_tree(
        root,
        &[
            ("unicode.rs", no_nl.as_bytes()),
            ("crlf.rs", "αβγ\r\nδεζ 🦀\r\n".as_bytes()),
        ],
    );

    let prepared = preview(
        root,
        &[
            block("unicode.rs", "café 漢字", "naïve 🎉🚀"),
            block("crlf.rs", "δεζ 🦀", "ηθι 𝄞"),
        ],
    )
    .unwrap();
    apply(root, &prepared.files).unwrap();
    assert_eq!(
        fs::read_to_string(root.join("unicode.rs")).unwrap(),
        "const BANNER: &str = \"naïve 🎉🚀\"\nlet tail = \"🦀𝄞\"; // tail"
    );
    assert_eq!(
        fs::read_to_string(root.join("crlf.rs")).unwrap(),
        "αβγ\r\nηθι 𝄞\r\n"
    );
    crate::sync::clear_sync_cache(&root.canonicalize().unwrap());
}

#[test]
fn refuses_path_escapes_and_every_symlink_component_including_in_root_aliases() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    let outside = temp.path().join("outside");
    fs::create_dir_all(root.join("dir")).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::write(root.join("dir/real.rs"), "fn real() { 1 }\n").unwrap();
    fs::write(outside.join("secret.rs"), "fn secret() { 1 }\n").unwrap();

    let abs = root.join("dir/real.rs").display().to_string();
    for bad in [
        "",
        ".",
        "dir",
        "dir/",
        "../outside/secret.rs",
        "dir/../../outside/secret.rs",
        "dir/../dir/real.rs",
        abs.as_str(),
    ] {
        assert!(
            preview(&root, &[block(bad, "1", "2")]).is_err(),
            "accepted {bad:?}"
        );
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        symlink(outside.join("secret.rs"), root.join("out_file.rs")).unwrap();
        symlink(&outside, root.join("out_dir")).unwrap();
        symlink(root.join("dir/real.rs"), root.join("in_root_alias.rs")).unwrap();
        symlink(root.join("dir"), root.join("in_root_dir_alias")).unwrap();

        let real_hash = sha256_hex(b"fn real() { 1 }\n");
        for link in [
            "out_file.rs",
            "out_dir/secret.rs",
            "in_root_alias.rs",
            "in_root_dir_alias/real.rs",
        ] {
            assert!(preview(&root, &[block(link, "1", "2")]).is_err(), "{link}");
            assert!(
                apply(&root, &[prep(link, &real_hash, "1", "2")]).is_err(),
                "{link}"
            );
        }
    }
}

#[test]
fn enforces_file_count_file_size_snippet_budget_utf8_and_rollback_errors() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    assert!(preview(root, &[]).is_err() && apply(root, &[]).is_err());
    let nine: Vec<BlockEdit> = (0..9)
        .map(|i| {
            let name = format!("f{i}.rs");
            fs::write(root.join(&name), format!("v{i}\n")).unwrap();
            block(&name, &format!("v{i}"), &format!("w{i}"))
        })
        .collect();
    assert!(preview(root, &nine).is_err());
    assert!(preview(root, &nine[..8]).is_ok());

    let mut exact = vec![b'a'; MAX_FILE_BYTES - 4];
    exact.extend_from_slice(b"TAIL");
    fs::write(root.join("exact.txt"), &exact).unwrap();
    assert!(preview(root, &[block("exact.txt", "TAIL", "DONE")]).is_ok());
    exact.push(b'\n');
    fs::write(root.join("big.txt"), &exact).unwrap();
    assert!(preview(root, &[block("big.txt", "TAIL", "DONE")]).is_err());
    let big_sha = sha256_hex(&exact);
    assert!(apply(root, &[prep("big.txt", &big_sha, "TAIL", "DONE")]).is_err());

    let (big_old, big_new) = ("x".repeat(32 * 1024), "y".repeat(32 * 1024));
    fs::write(root.join("budget.txt"), &big_old).unwrap();
    assert!(preview(root, &[block("budget.txt", &big_old, &big_new)]).is_ok());
    assert!(
        preview(
            root,
            &[block("budget.txt", &big_old, &format!("{big_new}z"))]
        )
        .is_err()
    );

    fs::write(root.join("bin.dat"), [0xff, 0xfe, b'a']).unwrap();
    assert!(preview(root, &[block("bin.dat", "a", "b")]).is_err());

    #[cfg(unix)]
    if unsafe { libc::geteuid() } != 0 {
        use std::os::unix::fs::PermissionsExt as _;
        let p = preview(root, &nine[..2]).unwrap();
        let second = root.join("f1.rs");
        fs::set_permissions(&second, fs::Permissions::from_mode(0o444)).unwrap();
        let err = apply(root, &p.files).unwrap_err();
        fs::set_permissions(&second, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(format!("{err:#}").contains("put back"), "{err:#}");
        assert_eq!(fs::read_to_string(root.join("f0.rs")).unwrap(), "v0\n");
        crate::sync::clear_sync_cache(&root.canonicalize().unwrap());
    }
}
