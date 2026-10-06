/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::*;

/// Puts the user's toolchain directories (`~/.cargo/bin`, `~/go/bin`) first on PATH so remote
/// commands, rust-analyzer's `cargo metadata` and the gopls engine use the toolchains the
/// workspaces were built with, not a distro/snap binary a systemd user session resolves first.
/// The engines this host can actually serve: Rust is in-process, the others need their
/// language server on PATH. Clients place a workspace only on a node that lists its engine.
/// How long a probe for installed engines is reused. Engines appear when somebody installs
/// one, which is rare; the probe costs a `npm root -g` and half a dozen `which` calls, which
/// is 200 ms and more, and it used to run on every status, gossip and placement request.
pub(crate) const ENGINE_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(300);

/// The probe's answer and the moment it stops being used. An expiry rather than the time of
/// the probe, so that a test can seed an entry that is already stale without subtracting from
/// a monotonic clock.
type EngineCache = std::sync::RwLock<Option<(Instant, Vec<String>)>>;

fn engine_cache() -> &'static EngineCache {
    static CACHE: std::sync::OnceLock<EngineCache> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::RwLock::new(None))
}

/// [`available_engines`], answered from the last probe until it expires. Everything that
/// serves a request asks through here; the probe itself runs at startup and on the janitor's
/// tick, off the request path.
pub fn cached_available_engines() -> Vec<String> {
    if let Ok(cache) = engine_cache().read()
        && let Some((expires_at, engines)) = cache.as_ref()
        && Instant::now() < *expires_at
    {
        return engines.clone();
    }
    refresh_available_engines()
}

/// Probes for installed engines and stores the answer. Blocking: call it from a place that
/// is allowed to block, never from a request handler.
pub fn refresh_available_engines() -> Vec<String> {
    let engines = available_engines();
    store_engines(Instant::now() + ENGINE_CACHE_TTL, engines.clone());
    engines
}

pub(crate) fn store_engines(expires_at: Instant, engines: Vec<String>) {
    if let Ok(mut cache) = engine_cache().write() {
        *cache = Some((expires_at, engines));
    }
}

pub(crate) fn available_engines() -> Vec<String> {
    let mut engines = vec!["rust (ra_ap_ide)".to_string()];
    // gopls is useless without the go tool it drives ("no views" for every file).
    if prod_code_engine_generic::which_bin("gopls").is_ok()
        && prod_code_engine_generic::which_bin("go").is_ok()
    {
        engines.push("go (gopls)".to_string());
    }
    for engine in [
        "cpp",
        "swift",
        "python",
        "typescript",
        "java",
        "kotlin",
        "csharp",
        "php",
        "ruby",
        "dart",
        "zig",
        "elixir",
        "scala",
        "lua",
        "haskell",
        "ocaml",
        "clojure",
        "julia",
        "shell",
        "r",
        "erlang",
        "fsharp",
        "perl",
        "solidity",
        "nim",
        "d",
        "fortran",
        "sql",
        "graphql",
        "protobuf",
        "crystal",
        "groovy",
        "ada",
        "v",
        "racket",
        "terraform",
        "nix",
        "markdown",
        "yaml",
        "toml",
        "json",
        "html",
        "css",
        "dockerfile",
        "svelte",
        "vue",
        "assembly",
    ] {
        if let Some(server) = prod_code_engine_generic::GenericLspConfig::installed_server(engine) {
            engines.push(format!("{engine} ({server})"));
        }
    }
    engines.push("generic-lsp".to_string());
    engines
}

pub fn prefer_rustup_toolchain() {
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let home = PathBuf::from(home);
    let preferred: Vec<PathBuf> = [
        ".cargo/bin",
        "go/bin",
        ".local/go/bin",
        ".local/bin",
        ".npm-global/bin",
        ".bun/bin",
    ]
    .iter()
    .map(|rel| home.join(rel))
    .filter(|dir| dir.is_dir())
    .collect();
    if preferred.is_empty() {
        return;
    }
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut paths: Vec<PathBuf> = std::env::split_paths(&current)
        .filter(|p| !preferred.contains(p))
        .collect();
    for dir in preferred.iter().rev() {
        paths.insert(0, dir.clone());
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        // SAFETY: called once at startup before any other thread exists.
        unsafe { std::env::set_var("PATH", joined) };
        tracing::info!(dirs = ?preferred, "user toolchain directories put first on PATH");
    }
}
