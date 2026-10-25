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
fn custom_and_home_cache_paths_export_the_same_directory() {
    crate::test_cache_support::isolated!();
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    let custom = home.join("custom-types");
    unsafe {
        std::env::set_var(TS_TYPES_CACHE_ENV, &custom);
    }
    assert_eq!(ts_types_cache_dir(), custom);
    assert_eq!(
        ts_types_cache_env(),
        vec![
            (
                TS_TYPES_CACHE_ENV.into(),
                custom.to_string_lossy().into_owned()
            ),
            (
                "TS_TYPES_CACHE".into(),
                custom.to_string_lossy().into_owned()
            )
        ]
    );
    unsafe {
        std::env::remove_var(TS_TYPES_CACHE_ENV);
    }
    let expected = home.join(".cache/prod-code/typescript-types");
    assert_eq!(ts_types_cache_dir(), expected);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&expected, fs::Permissions::from_mode(0o777)).unwrap();
        ensure_cache_dir(&expected).unwrap();
        assert_eq!(
            fs::metadata(&expected).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    let file = home.join("file");
    fs::write(&file, "keep").unwrap();
    assert_eq!(
        ensure_cache_dir(&file).unwrap_err().kind(),
        io::ErrorKind::AlreadyExists
    );
    assert_eq!(fs::read_to_string(file).unwrap(), "keep");
}

#[test]
fn unwritable_home_and_shared_cache_use_the_next_private_fallback() {
    crate::test_cache_support::isolated!();
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    unsafe {
        std::env::remove_var(TS_TYPES_CACHE_ENV);
    }
    fs::write(home.join(".cache"), "block home cache").unwrap();
    let shared = home.join("shared");
    let temporary = home.join("temporary");
    assert_eq!(
        cache_dir_with_fallbacks(shared.clone(), temporary.clone()),
        shared
    );
    fs::remove_dir(&shared).unwrap();
    fs::write(&shared, "block shared cache").unwrap();
    assert_eq!(
        cache_dir_with_fallbacks(shared.clone(), temporary.clone()),
        temporary
    );
    assert_eq!(fs::read_to_string(shared).unwrap(), "block shared cache");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(temporary).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}
