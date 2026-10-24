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
fn approved_roots_include_only_existing_package_stores_and_preserve_missing_roots() {
    crate::test_cache_support::isolated!();
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    let pnpm = home.join("pnpm");
    fs::create_dir(&pnpm).unwrap();
    unsafe {
        std::env::set_var("PNPM_HOME", &pnpm);
    }
    for name in [".local/share/pnpm", ".pnpm-store", "Library/pnpm"] {
        fs::create_dir_all(home.join(name)).unwrap();
    }
    let missing = home.join("missing");
    assert_eq!(
        build_approved_roots(&[&home, &missing]),
        vec![
            home.canonicalize().unwrap(),
            missing,
            pnpm.canonicalize().unwrap(),
            home.join(".local/share/pnpm").canonicalize().unwrap(),
            home.join(".pnpm-store").canonicalize().unwrap(),
            home.join("Library/pnpm").canonicalize().unwrap()
        ]
    );
    let mut visited = VisitedDirs::default();
    assert!(visited.insert(&home));
    assert!(!visited.insert(&home.join(".")));
    fs::write(home.join("file"), "value").unwrap();
    assert!(!visited.insert(&home.join("file")));
    assert!(!visited.insert(&home.join("absent")));
}
