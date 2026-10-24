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
