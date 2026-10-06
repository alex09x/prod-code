/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::{Path, PathBuf};

use super::super::root::{ShadowRootOwner, acquire_ram_shadow_root, default_root, ram_shadow_root};

#[test]
fn ram_shadow_root_returns_shm_path_when_available() {
    let storage = Path::new("/var/lib/prod-code/storage/my-repo");
    let result = ram_shadow_root(storage);
    if Path::new("/dev/shm").is_dir() {
        assert!(result.is_some());
        let path = result.unwrap();
        assert!(path.starts_with("/dev/shm"));
        assert!(path.to_string_lossy().contains(".prod-code-shadow-ram-"));
    } else {
        assert_eq!(result, None);
    }
}

#[test]
fn ram_shadow_root_keeps_its_owner_lock_after_acquisition_returns() {
    let storage = tempfile::tempdir().unwrap();
    let Some(root) = ram_shadow_root(storage.path()) else {
        return;
    };

    assert_eq!(
        acquire_ram_shadow_root(storage.path()).unwrap(),
        Some(root.clone())
    );
    assert!(
        ShadowRootOwner::acquire(&root).is_err(),
        "another owner must not claim the RAM namespace during this gateway process"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn test_shadow_default_root_configuration() {
    let storage = Path::new("/srv/workspaces/storage");
    let default = default_root(storage);
    assert!(
        default
            .to_string_lossy()
            .contains(".prod-code-shadow-storage-")
    );

    // Custom shadow root override via PROD_CODE_SHADOW_ROOT
    unsafe {
        std::env::set_var("PROD_CODE_SHADOW_ROOT", "/tmp/custom_shadow");
    }
    let custom = default_root(storage);
    assert_eq!(custom, PathBuf::from("/tmp/custom_shadow"));
    unsafe {
        std::env::remove_var("PROD_CODE_SHADOW_ROOT");
    }
}
