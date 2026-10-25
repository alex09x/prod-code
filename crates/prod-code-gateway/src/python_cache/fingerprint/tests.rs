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
fn installed_distribution_metadata_is_sorted_deduplicated_and_delimited() {
    let temp = tempfile::tempdir().unwrap();
    let site = temp.path();
    for (name, content) in [
        ("z.dist-info", "Version: 2\n"),
        ("a.dist-info", "Version: 1\n"),
    ] {
        fs::create_dir(site.join(name)).unwrap();
        fs::write(site.join(name).join("METADATA"), content).unwrap();
    }
    fs::create_dir(site.join("missing.dist-info")).unwrap();
    fs::write(site.join("file.dist-info"), "not a directory").unwrap();
    fs::create_dir(site.join("ignored")).unwrap();
    fs::write(site.join("ignored/METADATA"), "ignored").unwrap();
    let material = b"a.dist-info\0Version: 1\n\xffz.dist-info\0Version: 2\n\xff";
    let expected = Some(format!("{:016x}", xxhash_rust::xxh3::xxh3_64(material)));
    assert_eq!(
        python_installed_stub_fingerprint(&[
            site.join("a-stubs"),
            site.join("z-stubs"),
            site.join("absent/other")
        ]),
        expected
    );
    assert_eq!(python_installed_stub_fingerprint(&[]), None);
    assert_eq!(
        python_installed_stub_fingerprint(&[site.join("absent/other")]),
        None
    );
}

#[test]
fn namespace_compatibility_tracks_each_manifest_and_installed_metadata_direction() {
    let temp = tempfile::tempdir().unwrap();
    let from = temp.path().join("from");
    let to = temp.path().join("to");
    fs::create_dir(&from).unwrap();
    fs::create_dir(&to).unwrap();
    for (source, destination, compatible) in [
        (None, None, true),
        (Some("a"), None, true),
        (None, Some("a"), false),
        (Some("a"), Some("a"), true),
        (Some("a"), Some("b"), false),
    ] {
        for (dir, value) in [(&from, source), (&to, destination)] {
            let path = dir.join("requirements.txt");
            let _ = fs::remove_file(&path);
            if let Some(value) = value {
                fs::write(&path, value).unwrap();
            }
        }
        let (namespace, actual) = python_stub_cache_namespace(
            &from,
            &to,
            &[],
            &[],
            &from.join("typings"),
            &to.join("typings"),
        );
        assert_eq!(actual, compatible);
        assert_eq!(namespace.len(), 16);
    }
    fs::remove_file(from.join("requirements.txt")).unwrap();
    fs::remove_file(to.join("requirements.txt")).unwrap();
    let (_, compatible) = python_stub_cache_namespace(
        &from,
        &to,
        &[],
        &[],
        &from.join("typings"),
        &to.join("typings"),
    );
    assert!(compatible);
    fs::create_dir_all(to.join("site/pkg.dist-info")).unwrap();
    fs::write(to.join("site/pkg.dist-info/METADATA"), "Version: 1").unwrap();
    let (_, compatible) = python_stub_cache_namespace(
        &from,
        &to,
        &[],
        &[to.join("site/pkg-stubs")],
        &from.join("typings"),
        &to.join("typings"),
    );
    assert!(!compatible);
}
