/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::diagnostics::stream::types::StreamSessionKey;
use std::path::{Path, PathBuf};

pub fn stream_session_dir() -> PathBuf {
    let dir = if let Some(cache_home) = std::env::var_os("XDG_CACHE_HOME") {
        PathBuf::from(cache_home).join("prod-code/stream-sessions")
    } else if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        if cfg!(target_os = "macos") {
            home.join("Library/Caches/prod-code/stream-sessions")
        } else {
            home.join(".cache/prod-code/stream-sessions")
        }
    } else {
        let user = std::env::var("USER").unwrap_or_else(|_| "default".to_string());
        std::env::temp_dir().join(format!("prod-code-{user}-stream-sessions"))
    };

    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        builder.mode(0o700);
        let _ = builder.create(&dir);
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(&dir) {
            let mut perms = metadata.permissions();
            perms.set_mode(0o700);
            let _ = std::fs::set_permissions(&dir, perms);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = std::fs::create_dir_all(&dir);
    }

    dir
}

pub fn write_private_session_file(path: &Path, content: &str) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(content.as_bytes())?;
        file.flush()?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, content)?;
    }
    Ok(())
}

pub fn stream_session_disk_path(key: &StreamSessionKey) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.remote.hash(&mut hasher);
    key.root.hash(&mut hasher);
    key.file.hash(&mut hasher);
    key.session_id.hash(&mut hasher);
    let hash = hasher.finish();
    stream_session_dir().join(format!("{:016x}.session", hash))
}
