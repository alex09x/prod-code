/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Cache completion timestamps and `$PATH` executable resolution (#1794).

use super::*;
use std::ffi::OsString;
use std::time::{Duration, Instant};

/// The cache entry is stamped when the probe finished, not when the call began.
#[cfg(unix)]
#[test]
fn cache_timestamp_is_taken_after_probe_completion() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("slow_modules");
    std::fs::create_dir_all(&root).unwrap();
    let npm = temp.path().join("slow_npm");
    make_executable_script(
        &npm,
        &format!("sleep 0.4\necho \"{}\"", root.to_string_lossy()),
    );

    let cache = NpmRootCache::new();
    let before = Instant::now();
    let found = npm_global_root_with(
        &cache,
        || Some(npm.clone()),
        |_| None,
        Duration::from_secs(5),
    );
    assert_eq!(found, Some(root));
    let entry = cache.cached_entry().expect("probe result is cached");
    let stamped_after = entry.probed_at.saturating_duration_since(before);
    assert!(
        stamped_after >= Duration::from_millis(350),
        "probed_at must follow the 400 ms probe, was {stamped_after:?} after the call began"
    );
}

/// A caller that waited for another caller's lock stamps its own probe after that wait
/// and after its own probe, not at its pre-wait start.
#[cfg(unix)]
#[test]
fn cache_timestamp_is_taken_after_lock_wait() {
    let temp = tempfile::tempdir().unwrap();
    let root_a = temp.path().join("modules_a");
    let root_b = temp.path().join("modules_b");
    std::fs::create_dir_all(&root_a).unwrap();
    std::fs::create_dir_all(&root_b).unwrap();
    let started = temp.path().join("first_probe_started");
    let npm = temp.path().join("prefix_npm");
    make_executable_script(
        &npm,
        &format!(
            "touch \"{started}\"\nsleep 0.4\n\
             if [ \"$PREFIX\" = b ]; then echo \"{b}\"; else echo \"{a}\"; fi",
            started = started.to_string_lossy(),
            a = root_a.to_string_lossy(),
            b = root_b.to_string_lossy(),
        ),
    );

    let cache = NpmRootCache::new();
    let prefix_env =
        |value: &'static str| move |key: &str| (key == "PREFIX").then(|| OsString::from(value));
    let (second_start, second) = std::thread::scope(|s| {
        let first = s.spawn(|| {
            npm_global_root_with(
                &cache,
                || Some(npm.clone()),
                prefix_env("a"),
                Duration::from_secs(5),
            )
        });
        let wait_until = Instant::now() + Duration::from_secs(5);
        while !started.exists() && Instant::now() < wait_until {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(started.exists(), "first probe must start");
        let second_start = Instant::now();
        let second = npm_global_root_with(
            &cache,
            || Some(npm.clone()),
            prefix_env("b"),
            Duration::from_secs(5),
        );
        assert_eq!(first.join().unwrap(), Some(root_a.clone()));
        (second_start, second)
    });
    assert_eq!(second, Some(root_b.clone()));

    let entry = cache.cached_entry().expect("second probe is cached");
    assert_eq!(entry.result, Some(root_b));
    let stamped_after = entry.probed_at.saturating_duration_since(second_start);
    assert!(
        stamped_after >= Duration::from_millis(600),
        "probed_at must follow the lock wait and the second probe, was {stamped_after:?}"
    );
}

/// A non-executable `npm` earlier on `$PATH` does not shadow a real one later on it.
#[cfg(unix)]
#[test]
fn non_executable_path_entry_does_not_shadow_a_later_npm() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let shadow_dir = temp.path().join("shadow");
    let real_dir = temp.path().join("real");
    std::fs::create_dir_all(&shadow_dir).unwrap();
    std::fs::create_dir_all(&real_dir).unwrap();
    let shadow = shadow_dir.join("npm");
    std::fs::write(&shadow, "#!/bin/sh\necho shadow\n").unwrap();
    std::fs::set_permissions(&shadow, std::fs::Permissions::from_mode(0o644)).unwrap();
    let real = real_dir.join("npm");
    make_executable_script(&real, "echo real");

    let path = std::env::join_paths([&shadow_dir, &real_dir]).unwrap();
    let resolved = resolve_npm_binary_with(|key| (key == "PATH").then(|| path.clone()));
    assert_eq!(resolved, Some(real));
}
