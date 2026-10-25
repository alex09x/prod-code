/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::RustEngine;
use crate::tests::create_test_fixture;

#[test]
fn coverage_w2_module_paths_use_semantics_for_empty_and_redirected_modules() {
    let (dir, lib) = create_test_fixture();
    std::fs::write(
        &lib,
        "#[path = \"storage.rs\"]\npub mod renamed;\npub mod empty;\npub mod outer { pub mod inner; }\n",
    )
    .unwrap();
    let src = lib.parent().unwrap();
    std::fs::write(
        src.join("storage.rs"),
        "pub struct Record;\nimpl Record { pub fn read(&self) {} }\n",
    )
    .unwrap();
    std::fs::write(src.join("empty.rs"), "// intentionally empty\n").unwrap();
    std::fs::create_dir(src.join("outer")).unwrap();
    std::fs::write(src.join("outer/inner.rs"), "").unwrap();
    let engine = RustEngine::load(dir.path()).unwrap();
    let snapshot = engine.snapshot();
    for (file, expected) in [
        (lib.clone(), "fixture"),
        (src.join("storage.rs"), "fixture::renamed"),
        (src.join("empty.rs"), "fixture::empty"),
        (src.join("outer/inner.rs"), "fixture::outer::inner"),
    ] {
        assert_eq!(
            snapshot.module_path_for_file(&file).unwrap().as_deref(),
            Some(expected)
        );
    }
    let file_id = snapshot.require_file_id(&src.join("storage.rs")).unwrap();
    assert_eq!(
        snapshot.moniker_container(ra_ap_ide::FilePosition {
            file_id,
            offset: 0.into()
        }),
        None
    );
}

#[test]
fn coverage_w2_empty_crate_has_no_module_moniker() {
    let (dir, lib) = create_test_fixture();
    std::fs::write(&lib, "// empty crate\n").unwrap();
    let engine = RustEngine::load(dir.path()).unwrap();
    assert_eq!(engine.snapshot().module_path_for_file(&lib).unwrap(), None);
}
