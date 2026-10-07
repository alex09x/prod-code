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
use std::time::{Duration, SystemTime};

static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn test_ts_types_cache_dir_and_env() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let custom = temp.path().join("custom-ts-types");

    unsafe {
        std::env::set_var(TS_TYPES_CACHE_ENV, &custom);
    }

    let dir = ts_types_cache_dir();
    assert_eq!(dir, custom);
    assert!(dir.is_dir());

    let envs = ts_types_cache_env();
    let custom_str = custom.to_str().unwrap();
    assert!(
        envs.iter()
            .any(|(k, v)| k == TS_TYPES_CACHE_ENV && v == custom_str)
    );
    assert!(
        envs.iter()
            .any(|(k, v)| k == "TS_TYPES_CACHE" && v == custom_str)
    );

    unsafe {
        std::env::remove_var(TS_TYPES_CACHE_ENV);
    }
}

#[test]
fn test_is_typescript_project() {
    let temp = tempfile::tempdir().unwrap();
    assert!(!is_typescript_project(temp.path()));

    fs::write(temp.path().join("package.json"), "{}").unwrap();
    assert!(is_typescript_project(temp.path()));

    let temp2 = tempfile::tempdir().unwrap();
    fs::write(temp2.path().join("tsconfig.json"), "{}").unwrap();
    assert!(is_typescript_project(temp2.path()));

    let temp3 = tempfile::tempdir().unwrap();
    fs::write(temp3.path().join("index.ts"), "export const x = 1;").unwrap();
    assert!(is_typescript_project(temp3.path()));
}

#[test]
fn test_parse_tmp_ts_timestamp() {
    let now_nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let name = format!(".tmp-ts-12345-1-{now_nanos:x}");
    let parsed = parse_tmp_ts_timestamp(&name);
    assert!(parsed.is_some());

    // Invalid timestamp: prior to 2020
    let old_name = ".tmp-ts-12345-1-100";
    assert!(parse_tmp_ts_timestamp(old_name).is_none());

    // Malformed name
    assert!(parse_tmp_ts_timestamp("other.tmp").is_none());
    assert!(parse_tmp_ts_timestamp(".tmp-ts-notapid-1-123456").is_none());
}

#[test]
fn test_coordinate_tsconfig() {
    let temp = tempfile::tempdir().unwrap();
    let tsconfig = temp.path().join("tsconfig.json");

    // Custom typeRoots without node_modules/@types
    let content = serde_json::json!({
        "compilerOptions": {
            "typeRoots": ["custom_types"]
        }
    });
    fs::write(&tsconfig, serde_json::to_string(&content).unwrap()).unwrap();

    coordinate_tsconfig(&tsconfig);

    let updated: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&tsconfig).unwrap()).unwrap();
    let roots = updated["compilerOptions"]["typeRoots"].as_array().unwrap();
    assert!(
        roots
            .iter()
            .any(|r| r.as_str() == Some("node_modules/@types"))
    );
}

#[cfg(unix)]
#[test]
fn test_new_cache_entry_survives_pruning_when_source_mtime_is_old() {
    use std::os::unix::ffi::OsStrExt;

    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.d.ts");
    let cache = temp.path().join("cache");
    let cached = cache.join("@types/node/index.d.ts");
    fs::write(&source, "export declare const value: string;\n").unwrap();

    let old = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .saturating_sub(Duration::from_secs(8 * 24 * 60 * 60).as_secs());
    let times = [
        libc::timespec {
            tv_sec: old as libc::time_t,
            tv_nsec: 0,
        },
        libc::timespec {
            tv_sec: old as libc::time_t,
            tv_nsec: 0,
        },
    ];
    let source_c = std::ffi::CString::new(source.as_os_str().as_bytes()).unwrap();
    assert_eq!(
        unsafe { libc::utimensat(libc::AT_FDCWD, source_c.as_ptr(), times.as_ptr(), 0) },
        0
    );

    super::fs_ops::copy_and_publish_type_file(&source, &cached).unwrap();
    super::prune_stale_types_cache_with_grace(
        &cache,
        Duration::from_secs(7 * 24 * 60 * 60),
        u64::MAX,
        Duration::from_secs(60),
    )
    .unwrap();

    assert!(cached.exists(), "freshly published cache entry was pruned");
}
