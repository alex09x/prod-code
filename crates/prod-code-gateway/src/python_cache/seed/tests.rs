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
fn copies_publish_atomically_and_preserve_existing_destination() {
    let temp = tempfile::tempdir().unwrap();
    let src = temp.path().join("source");
    let dst = temp.path().join("nested/destination");
    fs::create_dir_all(src.join("package")).unwrap();
    fs::write(src.join("package/api.pyi"), "answer: int\n").unwrap();
    assert_eq!(copy_dir_preserving(&src, &dst).unwrap(), 12);
    assert_eq!(
        fs::read_to_string(dst.join("package/api.pyi")).unwrap(),
        "answer: int\n"
    );
    fs::write(src.join("package/api.pyi"), "changed").unwrap();
    assert_eq!(copy_dir_preserving(&src, &dst).unwrap(), 12);
    assert_eq!(
        fs::read_to_string(dst.join("package/api.pyi")).unwrap(),
        "answer: int\n"
    );
    assert_eq!(fs::read_dir(dst.parent().unwrap()).unwrap().count(), 1);
    let missing = temp.path().join("missing");
    let failed_dst = temp.path().join("failed");
    assert_eq!(
        copy_dir_preserving(&missing, &failed_dst)
            .unwrap_err()
            .kind(),
        io::ErrorKind::Other
    );
    assert!(!failed_dst.exists());
    assert!(!fs::read_dir(temp.path()).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".staging")
    }));
}

#[test]
fn seeds_configs_and_repairs_links_without_replacing_local_typings() {
    crate::test_cache_support::isolated!();
    let temp = tempfile::tempdir().unwrap();
    let from = temp.path().join("source");
    fs::create_dir_all(from.join("typings/pkg")).unwrap();
    fs::write(from.join("pyproject.toml"), "[project]\nname='sample'\n").unwrap();
    fs::write(from.join("typings/pkg/__init__.pyi"), "value: int\n").unwrap();
    let absent = temp.path().join("absent");
    assert_eq!(seed_python_worktree(&absent, &absent).unwrap(), None);
    assert_eq!(
        seed_python_worktree_within(&absent, &absent, None).unwrap(),
        None
    );
    for (index, config, expected) in [
        (0, "{}", serde_json::json!({"stubPath":"typings"})),
        (
            1,
            "{\"stubPath\":\"custom\",\"strict\":[]}",
            serde_json::json!({"stubPath":"custom","strict":[]}),
        ),
        (2, "[]", serde_json::json!([])),
        (3, "null", serde_json::Value::Null),
    ] {
        let to = temp.path().join(format!("worktree-{index}"));
        fs::create_dir_all(&to).unwrap();
        fs::write(to.join("pyrightconfig.json"), config).unwrap();
        let seeded =
            seed_python_worktree_within(&from, &to, crate::test_cache_support::ample_disk_space())
                .unwrap()
                .unwrap();
        assert_eq!(seeded, if index == 0 { 12 } else { 1 });
        assert_eq!(
            fs::read_to_string(to.join("typings/pkg/__init__.pyi")).unwrap(),
            "value: int\n"
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(
                &fs::read_to_string(to.join("pyrightconfig.json")).unwrap()
            )
            .unwrap(),
            expected
        );
        assert_eq!(
            seed_python_worktree_within(&from, &to, crate::test_cache_support::ample_disk_space())
                .unwrap(),
            Some(0)
        );
        #[cfg(unix)]
        {
            fs::remove_file(to.join("typings")).unwrap();
            std::os::unix::fs::symlink(temp.path().join("nonexistent"), to.join("typings"))
                .unwrap();
            assert_eq!(
                seed_python_worktree_within(
                    &from,
                    &to,
                    crate::test_cache_support::ample_disk_space()
                )
                .unwrap(),
                Some(1)
            );
            assert_eq!(
                fs::read_to_string(to.join("typings/pkg/__init__.pyi")).unwrap(),
                "value: int\n"
            );
        }
    }
    let to = temp.path().join("local");
    fs::create_dir_all(to.join("typings/local")).unwrap();
    fs::write(to.join("typings/local/api.pyi"), "local: str\n").unwrap();
    fs::write(to.join("pyrightconfig.json"), "malformed").unwrap();
    assert_eq!(
        seed_python_worktree_within(&from, &to, crate::test_cache_support::ample_disk_space())
            .unwrap(),
        Some(22)
    );
    assert!(
        !fs::symlink_metadata(to.join("typings"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read_to_string(to.join("typings/local/api.pyi")).unwrap(),
        "local: str\n"
    );
    assert_eq!(
        fs::read_to_string(to.join("pyrightconfig.json")).unwrap(),
        "malformed"
    );
}

#[test]
fn incompatible_worktrees_seed_their_own_virtualenv_and_respect_the_aggregate_budget() {
    crate::test_cache_support::isolated!();
    let temp = tempfile::tempdir().unwrap();
    let from = temp.path().join("source");
    let to = temp.path().join("destination");
    let package = ".venv/lib/python3.12/site-packages/sample-stubs";
    for (root, version) in [(&from, "old"), (&to, "new")] {
        fs::create_dir_all(root.join(package)).unwrap();
        fs::write(root.join("requirements.txt"), version).unwrap();
        fs::write(root.join(package).join("__init__.pyi"), version).unwrap();
    }
    fs::create_dir_all(from.join("typings")).unwrap();
    fs::write(from.join("typings/source_only.pyi"), "source").unwrap();
    assert_eq!(
        seed_python_worktree_within(&from, &to, crate::test_cache_support::ample_disk_space())
            .unwrap(),
        Some(4)
    );
    assert_eq!(
        fs::read_to_string(to.join("typings/sample-stubs/__init__.pyi")).unwrap(),
        "new"
    );
    assert!(!to.join("typings/source_only.pyi").exists());
    let denied = temp.path().join("denied");
    fs::create_dir(&denied).unwrap();
    fs::write(denied.join("requirements.txt"), "separate").unwrap();
    fs::create_dir_all(denied.join(package)).unwrap();
    fs::write(denied.join(package).join("__init__.pyi"), "denied").unwrap();
    assert_eq!(
        seed_python_worktree_within(
            &from,
            &denied,
            Some(DiskSpace {
                free: 0,
                total: 100
            })
        )
        .unwrap(),
        Some(1)
    );
    assert!(!denied.join("typings/sample-stubs").exists());
}
