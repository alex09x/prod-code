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
fn portable_mypy_view_copies_nested_packages_and_top_level_stubs() {
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("cache");
    let view = temp.path().join("view");
    fs::create_dir_all(cache.join("sample-stubs/nested")).unwrap();
    fs::create_dir(&view).unwrap();
    fs::write(cache.join("sample-stubs/nested/api.pyi"), "value: int\n").unwrap();
    fs::write(cache.join("direct.pyi"), "direct: str\n").unwrap();
    let imports = [("sample".into(), "sample-stubs".into())]
        .into_iter()
        .collect();
    populate_mypypath_view_copy(&cache, &view, &imports, &["direct.pyi".into()]).unwrap();
    assert_eq!(
        fs::read_to_string(view.join("sample/nested/api.pyi")).unwrap(),
        "value: int\n"
    );
    assert_eq!(
        fs::read_to_string(view.join("direct.pyi")).unwrap(),
        "direct: str\n"
    );
    fs::write(cache.join("sample-stubs/nested/api.pyi"), "changed: str\n").unwrap();
    populate_mypypath_view_copy(&cache, &view, &imports, &[]).unwrap();
    assert_eq!(
        fs::read_to_string(view.join("sample/nested/api.pyi")).unwrap(),
        "changed: str\n"
    );
    assert_eq!(
        copy_dir_recursive(&cache.join("missing"), &view.join("missing"))
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );
}

#[test]
fn private_cache_resolution_rejects_invalid_paths_and_falls_back_to_private_home() {
    crate::test_cache_support::isolated!();
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    for path in [
        PathBuf::new(),
        PathBuf::from("/"),
        std::env::current_dir().unwrap(),
    ] {
        assert_eq!(resolve_python_cache_dir(path), None);
    }
    let file = home.join("file");
    fs::write(&file, "keep").unwrap();
    assert_eq!(resolve_python_cache_dir(file.clone()), None);
    unsafe {
        std::env::set_var(PYTHON_STUB_CACHE_ENV, &file);
    }
    assert_eq!(
        python_stub_cache_dir(),
        home.join(".cache/prod-code/python-stubs")
    );
    assert_eq!(
        ensure_cache_dir(Path::new("")).unwrap_err().to_string(),
        "invalid cache directory path"
    );
    assert_eq!(fs::read_to_string(file).unwrap(), "keep");
}

fn names(dir: &Path) -> Vec<String> {
    let mut found: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    found.sort();
    found
}

/// A cache directory with the given entries: a name ending in `/` is a directory.
fn cache_with(entries: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for entry in entries {
        if entry.ends_with('/') {
            fs::create_dir_all(dir.path().join(entry)).unwrap();
        } else {
            fs::write(dir.path().join(entry), "x: int\n").unwrap();
        }
    }
    dir
}

#[cfg(unix)]
#[test]
fn the_mypy_view_links_stub_packages_under_their_import_names_and_relinks_stale_entries() {
    use std::os::unix::fs::symlink;
    let cache = cache_with(&[
        "sample-stubs/",
        "plain/",
        "pkg/",
        "pkg-stubs/",
        "direct.pyi",
        "notes.txt",
        ".hidden/",
    ]);
    symlink("plain", cache.path().join("linked-stubs")).unwrap();
    let view = ensure_mypypath_view(cache.path()).unwrap();
    assert_eq!(view, cache.path().join(".mypypath"));
    assert_eq!(
        names(&view),
        [".ready", "direct.pyi", "linked", "pkg", "plain", "sample"]
    );
    for (name, target) in [
        ("sample", "../sample-stubs"),
        ("plain", "../plain"),
        // A package next to its stubs keeps its own name.
        ("pkg", "../pkg"),
        ("linked", "../linked-stubs"),
        ("direct.pyi", "../direct.pyi"),
    ] {
        assert_eq!(fs::read_link(view.join(name)).unwrap(), Path::new(target));
    }
    assert_eq!(fs::read_to_string(view.join(".ready")).unwrap(), "ready");

    // A second pass repairs what changed: a real directory and a wrong link are replaced, a new
    // package is added, and the marker is back.
    fs::remove_file(view.join("sample")).unwrap();
    fs::create_dir(view.join("sample")).unwrap();
    fs::write(view.join("sample/leftover"), "").unwrap();
    fs::remove_file(view.join("direct.pyi")).unwrap();
    symlink("../elsewhere.pyi", view.join("direct.pyi")).unwrap();
    fs::create_dir(cache.path().join("newer-stubs")).unwrap();
    assert_eq!(ensure_mypypath_view(cache.path()).unwrap(), view);
    assert_eq!(
        fs::read_link(view.join("sample")).unwrap(),
        Path::new("../sample-stubs")
    );
    assert_eq!(
        fs::read_link(view.join("direct.pyi")).unwrap(),
        Path::new("../direct.pyi")
    );
    assert_eq!(
        fs::read_link(view.join("newer")).unwrap(),
        Path::new("../newer-stubs")
    );
    assert_eq!(fs::read_to_string(view.join(".ready")).unwrap(), "ready");

    // A cache that is no directory has no view.
    let file = cache.path().join("notes.txt");
    assert!(ensure_mypypath_view(&file).is_err());
}

#[cfg(unix)]
#[test]
fn the_type_checker_environment_points_mypy_at_the_best_view_there_is() {
    let render = |dir: &Path, mypypath: &Path| {
        let dir = dir.to_string_lossy().into_owned();
        vec![
            (PYTHON_STUB_CACHE_ENV.to_string(), dir.clone()),
            (
                "MYPYPATH".to_string(),
                mypypath.to_string_lossy().into_owned(),
            ),
            ("TYPINGS_PATH".to_string(), dir),
        ]
    };

    // Stub packages get a view, which is built once and then reused as it is.
    let stubs = cache_with(&["sample-stubs/"]);
    let expected = render(stubs.path(), &stubs.path().join(".mypypath"));
    assert_eq!(python_stub_cache_env_for_dir(stubs.path()), expected);
    assert!(stubs.path().join(".mypypath/.ready").is_file());
    assert_eq!(python_stub_cache_env_for_dir(stubs.path()), expected);

    // A view that is ready is used without looking at the cache.
    let ready = cache_with(&[".mypypath/"]);
    fs::write(ready.path().join(".mypypath/.ready"), "ready").unwrap();
    assert_eq!(
        python_stub_cache_env_for_dir(ready.path()),
        render(ready.path(), &ready.path().join(".mypypath"))
    );
    assert_eq!(names(&ready.path().join(".mypypath")), [".ready"]);

    // Without stub packages, or when the view cannot be made, MYPYPATH is the cache itself.
    let plain = cache_with(&["pkg/", "typed.pyi", "notes-stubs.txt"]);
    let blocked = cache_with(&["sample-stubs/", ".mypypath"]);
    let missing = plain.path().join("absent");
    for dir in [plain.path(), blocked.path(), missing.as_path()] {
        assert_eq!(
            python_stub_cache_env_for_dir(dir),
            render(dir, dir),
            "{}",
            dir.display()
        );
    }
    assert!(!plain.path().join(".mypypath").exists());

    // A workspace keeps its own typings view.
    let workspace = cache_with(&["typings/other-stubs/"]);
    let typings = workspace.path().join("typings");
    assert_eq!(
        python_stub_cache_env_for_workspace(workspace.path()),
        render(&typings, &typings.join(".mypypath"))
    );
}

#[cfg(unix)]
#[test]
fn only_a_link_into_the_cache_counts_as_a_shared_stub_link() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("cache");
    let outside = temp.path().join("outside");
    fs::create_dir_all(cache.join("pkg")).unwrap();
    fs::create_dir(&outside).unwrap();
    symlink(cache.join("pkg"), temp.path().join("into")).unwrap();
    symlink(&outside, temp.path().join("away")).unwrap();
    symlink(temp.path().join("gone"), temp.path().join("dangling")).unwrap();
    for (name, expected) in [
        ("into", true),
        ("away", false),
        ("dangling", false),
        ("absent", false),
        // A directory in the cache is not a link to it.
        ("cache", false),
    ] {
        assert_eq!(
            is_shared_stub_cache_link(&temp.path().join(name), &cache),
            expected,
            "{name}"
        );
    }
    assert!(!is_shared_stub_cache_link(
        &temp.path().join("into"),
        &temp.path().join("no-cache")
    ));
}

#[cfg(unix)]
#[test]
fn cache_directories_are_made_private_and_links_are_not_followed() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temp = tempfile::tempdir().unwrap();
    for (name, mode) in [("open", 0o777), ("group", 0o750), ("private", 0o700)] {
        let dir = temp.path().join(name);
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(mode)).unwrap();
        ensure_cache_dir(&dir).unwrap();
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700,
            "{name}"
        );
    }
    let nested = temp.path().join("a/b/c");
    ensure_cache_dir(&nested).unwrap();
    assert_eq!(
        fs::metadata(&nested).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        ensure_cache_dir(Path::new("/")).unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
    let target = temp.path().join("open");
    fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
    let link = temp.path().join("link");
    symlink(&target, &link).unwrap();
    assert!(
        ensure_cache_dir(&link)
            .unwrap_err()
            .raw_os_error()
            .is_some()
    );
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o755
    );
    let file = temp.path().join("file");
    fs::write(&file, "keep").unwrap();
    assert!(ensure_cache_dir(&file).is_err());
    assert_eq!(fs::read_to_string(&file).unwrap(), "keep");
}

#[cfg(unix)]
#[test]
fn the_cache_directory_comes_from_the_override_then_home_then_the_shared_temporary_areas() {
    crate::test_cache_support::isolated!();
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());

    // The override wins, is made private, and a relative one is taken from the working directory.
    let custom = home.join("custom");
    fs::create_dir(&custom).unwrap();
    unsafe { std::env::set_var(PYTHON_STUB_CACHE_ENV, &custom) };
    assert_eq!(python_stub_cache_dir(), custom);
    assert_eq!(
        std::os::unix::fs::MetadataExt::mode(&fs::metadata(&custom).unwrap()) & 0o777,
        0o700
    );
    assert_eq!(
        python_stub_cache_env(),
        python_stub_cache_env_for_dir(&custom)
    );
    std::env::set_current_dir(&home).unwrap();
    unsafe { std::env::set_var(PYTHON_STUB_CACHE_ENV, "relative/stubs") };
    let relative = std::env::current_dir().unwrap().join("relative/stubs");
    assert_eq!(python_stub_cache_dir(), relative);
    assert!(relative.is_dir());

    // Without a usable override or home, the per-user directory of the shared temporary area
    // is used, or the per-process one when that cannot be made private.
    let user = unsafe { libc::geteuid() };
    assert_eq!(cache_user_suffix(), user.to_string());
    let var_tmp = PathBuf::from("/var/tmp").join(format!("prod-code-python-stubs-{user}"));
    let per_process = std::env::temp_dir().join(format!(
        "prod-code-python-stubs-{user}-{}",
        std::process::id()
    ));
    let existed = var_tmp.exists();
    unsafe { std::env::remove_var(PYTHON_STUB_CACHE_ENV) };
    let file = home.join("not-a-directory");
    fs::write(&file, "keep").unwrap();
    for home_value in [Some(file.clone()), None] {
        match &home_value {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let expected = if resolve_python_cache_dir(var_tmp.clone()).is_some() {
            var_tmp.clone()
        } else {
            per_process.clone()
        };
        assert_eq!(python_stub_cache_dir(), expected, "{home_value:?}");
    }
    if !existed {
        let _ = fs::remove_dir(&var_tmp);
    }
    let _ = fs::remove_dir(&per_process);
    assert_eq!(fs::read_to_string(file).unwrap(), "keep");
}
