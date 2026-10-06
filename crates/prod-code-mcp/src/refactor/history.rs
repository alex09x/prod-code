/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// What each file held before the last edit applied to it, and what the edit left there.
pub type Applied = HashMap<PathBuf, (String, Vec<u8>)>;

fn applied() -> &'static Mutex<Applied> {
    static APPLIED: OnceLock<Mutex<Applied>> = OnceLock::new();
    APPLIED.get_or_init(Default::default)
}

/// Records, for every file an edit rewrote across repository checkouts, the text it had before
/// and the bytes it has now, so a report rendered after the write can still show what changed (#122).
pub fn remember_applied_multi(before: &[(PathBuf, Option<Vec<u8>>)]) {
    let Ok(mut map) = applied().lock() else {
        return;
    };
    for (abs, old) in before {
        let Ok(now) = std::fs::read(abs) else {
            continue;
        };
        let old = old
            .as_deref()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default();
        map.insert(abs.clone(), (old, now));
    }
}

#[allow(dead_code)]
pub fn remember_applied(root: &Path, before: &[(String, Option<Vec<u8>>)]) {
    let converted: Vec<(PathBuf, Option<Vec<u8>>)> = before
        .iter()
        .map(|(rel, old)| (root.join(rel), old.clone()))
        .collect();
    remember_applied_multi(&converted);
}

/// The text a file had before the edit that produced what is on disk now: what a report shows as
/// the old side of its diff. When no edit wrote the file, or the file has changed since, that is
/// simply what is on disk.
pub fn text_before_apply(path: &Path) -> String {
    let current = std::fs::read(path).unwrap_or_default();
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if let Ok(map) = applied().lock()
        && let Some((old, written)) = map.get(&canonical).or_else(|| map.get(path))
        && *written == current
    {
        return old.clone();
    }
    String::from_utf8_lossy(&current).into_owned()
}
