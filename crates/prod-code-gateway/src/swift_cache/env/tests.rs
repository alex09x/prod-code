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
