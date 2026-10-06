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

/// Tells a warm engine that the files a command just wrote are its new base, and drops every
/// engine under the workspace when one of them is a project manifest — what a sync does, for a
/// change that did not arrive as a sync.
///
/// A formatter or a generator rewrites files in the workspace copy; the client is sent the new
/// contents and records them as synced, so no later sync ever carries them here. An engine that
/// is not told keeps answering from the text it had before the command, and every position in a
/// file the command moved is off by however many lines it moved — silently, because the file it
/// is asked about is opened fresh by the client and only the *other* files come from its copy.
pub(crate) async fn refresh_engines(
    workspace_manager: &WorkspaceManager,
    server_workspace: &std::path::Path,
    files: &[FileDelta],
) {
    let loaded_rust = workspace_manager
        .get_loaded(server_workspace)
        .await
        .map(|ws| ws.mirrored_rust_engines())
        .unwrap_or_default();
    let mut project_config_changed = false;
    for delta in files {
        project_config_changed |= is_project_config_file(&delta.relative_path);
        let text = match &delta.content {
            Some(bytes) => match std::str::from_utf8(bytes) {
                Ok(text) => Some(text.to_string()),
                Err(_) => continue,
            },
            None => None,
        };
        let target = server_workspace.join(&delta.relative_path);
        for engine_lock in &loaded_rust {
            let mut engine = engine_lock.lock().await;
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                engine.update_base(&target, text.clone())
            }));
            match res {
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, file = %target.display(), "engine update after a command failed");
                }
                Err(panic) => {
                    let msg = if let Some(s) = panic.downcast_ref::<&str>() {
                        s.to_string()
                    } else if let Some(s) = panic.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "unknown panic".to_string()
                    };
                    tracing::warn!(panic = %msg, file = %target.display(), "engine update after a command panicked; continuing");
                }
                Ok(Ok(())) => {}
            }
        }
    }
    if project_config_changed {
        let dropped = workspace_manager.unload_under(server_workspace).await;
        if dropped > 0 {
            tracing::info!(
                dropped,
                "a command changed the project configuration; engines reload on next session"
            );
        }
    }
}

/// Kills a tokio child and everything it spawned (its process group), then the child itself:
/// shadow runs, which let tokio reap their children.
pub fn kill_exec_tree(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        let _ = std::process::Command::new("kill")
            .args(["-9", "--", &format!("-{pid}")])
            .status();
    }
    let _ = child.start_kill();
}

/// Runs `req.command` inside the client's server workspace, streaming stdout/stderr chunks to
/// the client and finishing with an `ExecExit`. The child is killed if the client goes away
/// or the timeout elapses.
/// The environment that lets every git worktree of a project share one C/C++ compiler cache
/// (#243). Each worktree has its own server copy at its own path, and a compiler cache keyed by
/// absolute paths shares nothing between them: the fmt library built in a second copy took
/// 22.9 s with sccache as the launcher (2 hits of 114 compiles) and 0.71 s with ccache and
/// `CCACHE_BASEDIR` set to the copy, which ccache reads on every compile. sccache reads its
/// base directories once, when its server starts, so it cannot follow worktrees that appear
/// later. CMake picks the launchers up when it configures a build directory. Nothing when the
/// node has no ccache; the caller's own variables are applied after these and win.
pub fn compiler_cache_env(workspace: &Path, ccache: bool) -> Vec<(String, String)> {
    if !ccache {
        return Vec::new();
    }
    vec![
        (
            "CCACHE_BASEDIR".to_string(),
            workspace.to_string_lossy().into_owned(),
        ),
        ("CCACHE_NOHASHDIR".to_string(), "1".to_string()),
        (
            "CCACHE_SLOPPINESS".to_string(),
            "pch_defines,time_macros".to_string(),
        ),
        ("CCACHE_PCH_EXTSUM".to_string(), "1".to_string()),
        (
            "CMAKE_C_COMPILER_LAUNCHER".to_string(),
            "ccache".to_string(),
        ),
        (
            "CMAKE_CXX_COMPILER_LAUNCHER".to_string(),
            "ccache".to_string(),
        ),
    ]
}

/// Polyglot compiler and build cache environment across Rust, Go, Python, Node, C/C++, and Swift (Roadmap 6.2, 3.4, 3.7).
pub fn polyglot_compiler_cache_env(
    workspace: &Path,
    ccache: bool,
    ram_target_dir: Option<&Path>,
) -> Vec<(String, String)> {
    let mut env = compiler_cache_env(workspace, ccache);
    if let Some(target) = ram_target_dir {
        env.push((
            "CARGO_TARGET_DIR".to_string(),
            target.to_string_lossy().into_owned(),
        ));
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home_path = PathBuf::from(home);
        let gocache = home_path.join(".cache/go-build");
        let gomodcache = home_path.join("go/pkg/mod");
        if gocache.is_dir() {
            env.push((
                "GOCACHE".to_string(),
                gocache.to_string_lossy().into_owned(),
            ));
        }
        if gomodcache.is_dir() {
            env.push((
                "GOMODCACHE".to_string(),
                gomodcache.to_string_lossy().into_owned(),
            ));
        }
        let uv_cache = home_path.join(".cache/uv");
        if uv_cache.is_dir() {
            env.push((
                "UV_CACHE_DIR".to_string(),
                uv_cache.to_string_lossy().into_owned(),
            ));
        }
        let pip_cache = home_path.join(".cache/pip");
        if pip_cache.is_dir() {
            env.push((
                "PIP_CACHE_DIR".to_string(),
                pip_cache.to_string_lossy().into_owned(),
            ));
        }
        let pnpm_store = home_path.join(".local/share/pnpm/store");
        if pnpm_store.is_dir() {
            env.push((
                "npm_config_store_dir".to_string(),
                pnpm_store.to_string_lossy().into_owned(),
            ));
        }
        let npm_cache = home_path.join(".npm");
        if npm_cache.is_dir() {
            env.push((
                "npm_config_cache".to_string(),
                npm_cache.to_string_lossy().into_owned(),
            ));
        }
        let yarn_cache = home_path.join(".cache/yarn");
        if yarn_cache.is_dir() {
            env.push((
                "YARN_CACHE_FOLDER".to_string(),
                yarn_cache.to_string_lossy().into_owned(),
            ));
        }
    }
    // Python shared virtual-environment stub cache across worktrees (Roadmap 3.6)
    env.extend(python_cache::python_stub_cache_env_for_workspace(workspace));
    // Swift shared module cache across worktrees (Roadmap 3.7)
    env.extend(swift_cache::swift_module_cache_env());
    // TypeScript shared @types and declaration cache across worktrees (Roadmap 3.5)
    env.extend(ts_cache::ts_types_cache_env());
    env
}
