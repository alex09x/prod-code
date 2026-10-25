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
fn cache_resolution_rejects_unsafe_paths_and_obeys_override_precedence() {
    crate::test_cache_support::isolated!();
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    for path in [
        PathBuf::new(),
        PathBuf::from("/"),
        std::env::current_dir().unwrap(),
    ] {
        assert_eq!(resolve_cache_dir(path), None);
    }
    let file = home.join("file");
    fs::write(&file, "keep").unwrap();
    assert_eq!(resolve_cache_dir(file.clone()), None);
    assert_eq!(
        ensure_cache_dir(Path::new("")).unwrap_err().to_string(),
        "invalid cache directory path"
    );
    let override_path = home.join("override");
    unsafe {
        std::env::set_var(SWIFT_MODULE_CACHE_ENV, &file);
        std::env::set_var("SWIFTPM_MODULECACHE_OVERRIDE", &override_path);
    }
    assert_eq!(swift_module_cache_dir(), override_path);
    let value = override_path.to_string_lossy().into_owned();
    assert_eq!(
        swift_module_cache_env(),
        vec![
            ("SWIFTPM_MODULECACHE_OVERRIDE".into(), value.clone()),
            ("SWIFT_MODULE_CACHE_PATH".into(), value.clone()),
            ("CLANG_MODULE_CACHE_PATH".into(), value)
        ]
    );
    unsafe {
        std::env::remove_var(SWIFT_MODULE_CACHE_ENV);
        std::env::remove_var("SWIFTPM_MODULECACHE_OVERRIDE");
    }
    assert_eq!(
        swift_module_cache_dir(),
        home.join(".cache/prod-code/swift-module-cache")
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = home.join("real");
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o777)).unwrap();
        ensure_cache_dir(&dir).unwrap();
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let link = home.join("link");
        symlink(&dir, &link).unwrap();
        assert_eq!(resolve_cache_dir(link), None);
        assert_eq!(fs::read_to_string(&file).unwrap(), "keep");
    }
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
    for invalid in [Path::new("/"), Path::new("a/..")] {
        let error = ensure_cache_dir(invalid).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{invalid:?}");
        assert_eq!(error.to_string(), "invalid cache directory path");
    }
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
fn the_cache_directory_is_taken_from_the_first_usable_source() {
    use std::os::unix::fs::MetadataExt;
    crate::test_cache_support::isolated!();
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    let user = unsafe { libc::geteuid() };
    assert_eq!(cache_user_suffix(), user.to_string());

    // Both overrides usable: the dedicated one wins, and the directory is made private.
    let (custom, swiftpm) = (home.join("custom"), home.join("swiftpm"));
    fs::create_dir(&custom).unwrap();
    unsafe {
        std::env::set_var(SWIFT_MODULE_CACHE_ENV, &custom);
        std::env::set_var("SWIFTPM_MODULECACHE_OVERRIDE", &swiftpm);
    }
    assert_eq!(swift_module_cache_dir(), custom);
    assert_eq!(fs::metadata(&custom).unwrap().mode() & 0o777, 0o700);
    assert!(!swiftpm.exists());
    let value = custom.to_string_lossy().into_owned();
    assert_eq!(
        swift_module_cache_env(),
        vec![
            ("SWIFTPM_MODULECACHE_OVERRIDE".into(), value.clone()),
            ("SWIFT_MODULE_CACHE_PATH".into(), value.clone()),
            ("CLANG_MODULE_CACHE_PATH".into(), value)
        ]
    );

    // A relative override is taken from the working directory; one that ends in `..` is refused.
    std::env::set_current_dir(&home).unwrap();
    unsafe { std::env::set_var(SWIFT_MODULE_CACHE_ENV, "relative/modules") };
    let relative = std::env::current_dir().unwrap().join("relative/modules");
    assert_eq!(swift_module_cache_dir(), relative);
    assert!(relative.is_dir());
    assert_eq!(resolve_cache_dir(PathBuf::from("relative/..")), None);

    // Unusable overrides fall through to the directory under home.
    let file = home.join("not-a-directory");
    fs::write(&file, "keep").unwrap();
    unsafe {
        std::env::set_var(SWIFT_MODULE_CACHE_ENV, &file);
        std::env::set_var("SWIFTPM_MODULECACHE_OVERRIDE", &file);
    }
    assert_eq!(
        swift_module_cache_dir(),
        home.join(".cache/prod-code/swift-module-cache")
    );

    // With no usable home either, the per-user directory of the shared temporary area is used,
    // or the per-process one when that cannot be made private.
    let var_tmp = PathBuf::from("/var/tmp").join(format!("prod-code-swift-module-cache-{user}"));
    let per_process = std::env::temp_dir().join(format!(
        "prod-code-swift-module-cache-{user}-{}",
        std::process::id()
    ));
    let existed = var_tmp.exists();
    for home_value in [Some(file.clone()), None] {
        match &home_value {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let expected = if resolve_cache_dir(var_tmp.clone()).is_some() {
            var_tmp.clone()
        } else {
            per_process.clone()
        };
        assert_eq!(swift_module_cache_dir(), expected, "{home_value:?}");
    }
    if !existed {
        let _ = fs::remove_dir(&var_tmp);
    }
    let _ = fs::remove_dir(&per_process);
    assert_eq!(fs::read_to_string(file).unwrap(), "keep");
}
