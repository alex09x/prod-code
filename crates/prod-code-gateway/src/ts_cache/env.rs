/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The environment variable name used to explicitly configure the shared TypeScript `@types` cache directory.
pub const TS_TYPES_CACHE_ENV: &str = "PROD_CODE_TS_TYPES_CACHE";

/// Returns the path to the shared TypeScript `@types` cache directory.
///
/// Precedence:
/// 1. `PROD_CODE_TS_TYPES_CACHE` environment variable if set.
/// 2. `$HOME/.cache/prod-code/typescript-types`.
/// 3. `/var/tmp/prod-code/typescript-types` (or `/tmp/prod-code/typescript-types`).
/// 4. Temporary directory fallback (`std::env::temp_dir().join("prod-code-typescript-types")`).
///
/// Ensures the directory exists with mode `0700` on Unix systems.
pub fn ts_types_cache_dir() -> PathBuf {
    if let Some(custom) = std::env::var_os(TS_TYPES_CACHE_ENV) {
        let p = PathBuf::from(custom);
        let _ = ensure_cache_dir(&p);
        return p;
    }

    if let Some(home) = std::env::var_os("HOME") {
        let p = PathBuf::from(home)
            .join(".cache")
            .join("prod-code")
            .join("typescript-types");
        if ensure_cache_dir(&p).is_ok() {
            return p;
        }
    }

    let var_tmp = PathBuf::from("/var/tmp/prod-code/typescript-types");
    if ensure_cache_dir(&var_tmp).is_ok() {
        return var_tmp;
    }

    let temp = std::env::temp_dir().join("prod-code-typescript-types");
    let _ = ensure_cache_dir(&temp);
    temp
}

/// Ensures `dir` exists and has secure permissions (`0700` on Unix).
pub fn ensure_cache_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

/// Returns the environment variables that configure the shared TypeScript types cache
/// across vtsls, tsserver, tsc, and TypeScript/Node tools (Roadmap 3.5).
pub fn ts_types_cache_env() -> Vec<(String, String)> {
    let dir = ts_types_cache_dir();
    let dir_str = dir.to_string_lossy().into_owned();
    vec![
        (TS_TYPES_CACHE_ENV.to_string(), dir_str.clone()),
        ("TS_TYPES_CACHE".to_string(), dir_str),
    ]
}
