/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// Run environment-sensitive tests alone, with private cache directories. A mutex in one
/// module cannot protect readers in other modules, or restore the environment on panic.
pub(crate) fn run_in_child() -> bool {
    const CHILD: &str = "PROD_CODE_CACHE_TEST_CHILD";
    let thread = std::thread::current();
    let name = thread.name().expect("test harness thread name");
    if std::env::var(CHILD).as_deref() == Ok(name) {
        return false;
    }

    let home = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture", "--test-threads=1"])
        .env(CHILD, name)
        .env("HOME", home.path())
        .env("PROD_CODE_PYTHON_STUB_CACHE", home.path().join("python"))
        .env("PROD_CODE_TS_TYPES_CACHE", home.path().join("typescript"))
        .env("PROD_CODE_SWIFT_MODULE_CACHE", home.path().join("swift"))
        .env_remove("SWIFTPM_MODULECACHE_OVERRIDE")
        .env_remove("PROD_CODE_BUILD_RAM")
        .output()
        .expect("run isolated cache test");
    assert!(
        output.status.success(),
        "isolated test {name} failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

macro_rules! isolated {
    () => {
        if $crate::test_cache_support::run_in_child() {
            return;
        }
    };
}
pub(crate) use isolated;

pub(crate) fn ample_disk_space() -> Option<crate::DiskSpace> {
    Some(crate::DiskSpace {
        free: 1 << 30,
        total: 2 << 30,
    })
}
