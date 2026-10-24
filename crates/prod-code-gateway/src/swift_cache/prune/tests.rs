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

#[cfg(unix)]
#[test]
fn pruning_is_oldest_first_and_does_not_follow_symlinks_or_hidden_files() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("cache");
    fs::create_dir_all(cache.join("nested")).unwrap();
    for (name, modified) in [("old.pcm", 1), ("nested/new.swiftmodule", 2)] {
        let path = cache.join(name);
        fs::write(&path, "data").unwrap();
        fs::File::open(path)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(modified))
            .unwrap();
    }
    fs::write(cache.join(".hidden.pcm"), "hidden").unwrap();
    let outside = temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("keep.pcm"), "outside").unwrap();
    symlink(&outside, cache.join("linked-directory")).unwrap();
    symlink(outside.join("keep.pcm"), cache.join("linked.pcm")).unwrap();
    assert_eq!(
        prune_stale_module_cache_in(&cache, Duration::MAX, 4).unwrap(),
        1
    );
    assert!(!cache.join("old.pcm").exists());
    assert_eq!(
        fs::read_to_string(cache.join("nested/new.swiftmodule")).unwrap(),
        "data"
    );
    assert_eq!(
        prune_stale_module_cache_in(&cache, Duration::ZERO, 0).unwrap(),
        1
    );
    assert_eq!(
        fs::read_to_string(outside.join("keep.pcm")).unwrap(),
        "outside"
    );
    assert_eq!(
        fs::read_to_string(cache.join(".hidden.pcm")).unwrap(),
        "hidden"
    );
    let alias = temp.path().join("alias");
    symlink(&cache, &alias).unwrap();
    assert_eq!(
        prune_stale_module_cache_in(&alias, Duration::ZERO, 0).unwrap(),
        0
    );
    assert_eq!(
        prune_stale_module_cache_in(&temp.path().join("missing"), Duration::ZERO, 0).unwrap(),
        0
    );
}
