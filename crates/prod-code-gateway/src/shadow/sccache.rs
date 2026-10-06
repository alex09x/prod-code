/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_protocol::FileDelta;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::hypothesis_view::{HypothesisView, Node};
use super::types::Job;

/// Why an overlay hypothesis must not run, or `None`. An sccache daemon started outside the
/// hypothesis's mount namespace compiles against the real workspace, so sccache has to stay in
/// client-side mode, where the wrapper process inside the namespace compiles (#426).
pub(crate) fn check_isolation_conflicts(job: &Job) -> Option<String> {
    let var = |key: &str| effective_var(&job.env, key);
    sccache_client_side(&var, &job.workspace, &job.files)
        .err()
        .map(|why| format!("cannot run an overlay shadow: {why}"))
}

/// A variable as the overlay command sees it: the request's env wins over the gateway's, and
/// `SCCACHE_CLIENT_SIDE` is the `1` that `run_overlay` sets unless the request overrides it.
pub(crate) fn effective_var(env: &[(String, String)], key: &str) -> Option<OsString> {
    if let Some((_, value)) = env.iter().rev().find(|(k, _)| k == key) {
        return Some(value.into());
    }
    if key == "SCCACHE_CLIENT_SIDE" {
        return Some("1".into());
    }
    std::env::var_os(key)
}

/// `Ok` when an sccache client started with these variables compiles in client-side mode,
/// resolved as sccache 0.17 `Config::load` does: a non-empty `SCCACHE_CLIENT_SIDE` wins over
/// the config file's `client_side_mode`, and the mode is off while `SCCACHE_LOG` is set or the
/// config file names `dist.scheduler_url`. Distributed compilation is configured only in that
/// file, never through the environment.
pub(crate) fn sccache_client_side(
    var: &dyn Fn(&str) -> Option<OsString>,
    workspace: &Path,
    files: &[FileDelta],
) -> std::result::Result<(), String> {
    // sccache reads the variable with `env::var(..).ok()` and ignores an empty value.
    let from_env = match var("SCCACHE_CLIENT_SIDE")
        .and_then(|v| v.into_string().ok())
        .filter(|v| !v.is_empty())
    {
        None => None,
        Some(v) => match v.to_lowercase().as_str() {
            "true" | "on" | "1" => Some(true),
            "false" | "off" | "0" => {
                return Err(
                    "SCCACHE_CLIENT_SIDE turns sccache's client-side mode off, and \
                            sccache would compile in its daemon outside the hypothesis's \
                            mount namespace"
                        .to_string(),
                );
            }
            _ => {
                return Err(
                    "SCCACHE_CLIENT_SIDE is not one of true, on, 1 (sccache rejects \
                            any value other than true, on, 1, false, off, 0)"
                        .to_string(),
                );
            }
        },
    };
    if var("SCCACHE_LOG").is_some() {
        return Err(
            "SCCACHE_LOG is set, and sccache turns client-side mode off while it logs; \
                    unset it for shadow runs"
                .to_string(),
        );
    }
    let path = sccache_config_path(var)?;
    let (scheduler, from_file) = match sccache_config_content(&path, workspace, files)? {
        Some(bytes) => parse_sccache_config(&path, &bytes)?,
        None => (false, false),
    };
    if scheduler {
        return Err(format!(
            "the sccache config {} sets dist.scheduler_url; distributed compilation runs \
             outside the hypothesis's mount namespace and is not supported in shadow runs",
            path.display()
        ));
    }
    if !from_env.unwrap_or(from_file) {
        return Err(format!(
            "sccache's client-side mode is off: SCCACHE_CLIENT_SIDE is empty and the config {} \
             does not set client_side_mode = true",
            path.display()
        ));
    }
    Ok(())
}

/// The config file sccache reads: `SCCACHE_CONF`, otherwise the Linux config directory of
/// the `directories` crate (`$XDG_CONFIG_HOME` when absolute, else `$HOME/.config`).
pub(crate) fn sccache_config_path(
    var: &dyn Fn(&str) -> Option<OsString>,
) -> std::result::Result<PathBuf, String> {
    if let Some(conf) = var("SCCACHE_CONF") {
        let path = PathBuf::from(conf);
        if !path.is_absolute() {
            return Err(
                "SCCACHE_CONF must be an absolute path in shadow runs: sccache \
                        resolves a relative one against each compiler's working directory"
                    .to_string(),
            );
        }
        return Ok(path);
    }
    let config_home = var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            var("HOME")
                .filter(|h| !h.is_empty())
                .map(|h| PathBuf::from(h).join(".config"))
        })
        .ok_or(
            "cannot locate the sccache config: none of SCCACHE_CONF, XDG_CONFIG_HOME and \
                HOME is set",
        )?;
    Ok(config_home.join("sccache").join("config"))
}

/// The config file's bytes as the hypothesis sees them (`None`: no file). The path is followed
/// through `..` and symlinks in the hypothesis's own view, so an alias such as
/// `ws/ci/../sccache.toml` or a symlink from outside into the workspace reads the proposed file
/// the overlay command would read (#426).
pub(crate) fn sccache_config_content(
    path: &Path,
    workspace: &Path,
    files: &[FileDelta],
) -> std::result::Result<Option<Vec<u8>>, String> {
    let unreadable =
        |why: &str| format!("cannot read the sccache config {}: {why}", path.display());
    let view = HypothesisView::new(workspace, files);
    let (physical, node) = view.resolve(path, true).map_err(|why| unreadable(&why))?;
    match node {
        Node::Missing => Ok(None),
        Node::Proposed(bytes) => Ok(Some(bytes.to_vec())),
        Node::Disk => match std::fs::read(&physical) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(unreadable(&e.kind().to_string())),
        },
        Node::Dir { .. } => Err(unreadable("is a directory")),
        Node::Link(_) => Err(unreadable("is a symbolic link")),
    }
}

/// `(dist.scheduler_url is set, client_side_mode)` from a config file: JSON when its extension
/// is `json`, TOML otherwise, as sccache decides. Parser messages are not passed on because
/// they quote the file, which can hold cache credentials.
pub(crate) fn parse_sccache_config(
    path: &Path,
    bytes: &[u8],
) -> std::result::Result<(bool, bool), String> {
    let invalid = |what: &str| format!("the sccache config {} {what}", path.display());
    let text = std::str::from_utf8(bytes).map_err(|_| invalid("is not UTF-8 text"))?;
    let value: serde_json::Value = if path.extension().is_some_and(|e| e == "json") {
        serde_json::from_str(text).map_err(|_| invalid("is not valid JSON"))?
    } else {
        toml::from_str::<toml::Table>(text)
            .ok()
            .and_then(|table| serde_json::to_value(table).ok())
            .ok_or_else(|| invalid("is not valid TOML"))?
    };
    let root = value.as_object().ok_or_else(|| invalid("is not a table"))?;
    let scheduler = match root.get("dist") {
        None => false,
        Some(serde_json::Value::Object(dist)) => {
            dist.get("scheduler_url").is_some_and(|url| !url.is_null())
        }
        Some(_) => return Err(invalid("has a `dist` that is not a table")),
    };
    let client_side = match root.get("client_side_mode") {
        None => false,
        Some(serde_json::Value::Bool(on)) => *on,
        Some(_) => return Err(invalid("has a `client_side_mode` that is not a boolean")),
    };
    Ok((scheduler, client_side))
}

/// `sccache` on PATH or in `~/.cargo/bin`.
pub fn find_sccache() -> Option<PathBuf> {
    let on_path = std::process::Command::new("sh")
        .args(["-c", "command -v sccache"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()));
    on_path.filter(|p| p.is_file()).or_else(|| {
        let home = std::env::var_os("HOME")?;
        let p = PathBuf::from(home).join(".cargo/bin/sccache");
        p.is_file().then_some(p)
    })
}

/// Ensures that if `sccache` is installed, its server is running cleanly on the host
/// before entering a private mount namespace.
pub fn ensure_sccache_server() {
    if let Some(sccache) = find_sccache() {
        let stats = std::process::Command::new(&sccache)
            .arg("--show-stats")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        let healthy = stats.map(|s| s.success()).unwrap_or(false);
        if !healthy {
            let _ = std::process::Command::new(&sccache)
                .arg("--stop-server")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            let _ = std::process::Command::new(&sccache)
                .arg("--start-server")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }
}
