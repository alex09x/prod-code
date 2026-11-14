/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Searching a library, toolchain or SDK directory that only the build node has (#2021): the
//! registry copy of a dependency is on the node that compiles it, not in the checkout.

use super::execute::{build_rg_args, parse_rg_json_output};
use super::{GrepOptions, GrepReport};
use anyhow::{Result, bail};
use prod_code_protocol::source_roots::{HOME_SOURCE_ROOTS, SYSTEM_SOURCE_ROOTS};
use std::net::SocketAddr;
use std::path::Path;

/// The guard's exit code for a path the node does not have.
const NOT_ON_NODE: i32 = 64;
/// The guard's exit code for a path outside every source location.
const NOT_A_SOURCE_LOCATION: i32 = 65;

/// Whether `search_path`, which the checkout does not have, may name something on the node: an
/// absolute path outside the workspace. A missing path inside the workspace is a mistyped one.
pub(super) fn names_node_path(root: &Path, search_path: &Path) -> bool {
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    search_path.is_absolute()
        && !search_path.starts_with(root)
        && !search_path.starts_with(canonical_root)
}

/// What the node runs before ripgrep. The path is resolved there, symbolic links included, and
/// searched only when it lies in a source location the gateway also reads files from, so a
/// search reaches what `code_source` reaches and nothing else on the host. The path and the
/// ripgrep arguments are positional parameters: none of them is part of the script's text.
pub(super) fn guard_script() -> String {
    let mut roots = String::new();
    for relative in HOME_SOURCE_ROOTS {
        roots.push_str(&format!(" \"$HOME/{relative}\""));
    }
    for root in SYSTEM_SOURCE_ROOTS {
        roots.push_str(&format!(" \"{root}\""));
    }
    format!(
        "resolved=$(realpath -- \"$1\" 2>/dev/null) || exit {NOT_ON_NODE}\n\
         [ -e \"$resolved\" ] || exit {NOT_ON_NODE}\n\
         shift\n\
         for root in{roots}; do\n\
         case \"$resolved\" in \"$root\"|\"$root\"/*) exec rg \"$@\" \"$resolved\" ;; esac\n\
         done\n\
         exit {NOT_A_SOURCE_LOCATION}\n"
    )
}

/// Runs the search on the node `addr` over `search_path`, an absolute path the checkout does
/// not have. Hits carry the node's path, which `code_source` reads.
pub(super) async fn grep_node_source(
    addr: SocketAddr,
    root: &Path,
    search_path: &Path,
    opts: &GrepOptions,
    max_results: usize,
    context: usize,
) -> Result<GrepReport> {
    let target = search_path.to_string_lossy().into_owned();
    let mut rg_args = build_rg_args(&target, opts);
    // The node resolves the target and appends it itself.
    rg_args.pop();
    let mut command = vec![
        "sh".to_string(),
        "-c".to_string(),
        guard_script(),
        "sh".to_string(),
        target.clone(),
    ];
    command.extend(rg_args);

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let outcome = crate::exec::run_remote(
        addr,
        root,
        None,
        command,
        Vec::new(),
        30,
        false,
        |is_err, chunk| {
            if is_err {
                stderr.extend_from_slice(chunk);
            } else {
                stdout.extend_from_slice(chunk);
            }
        },
    )
    .await?;
    if let Some(error) = &outcome.exit.error {
        bail!("the build node could not search {target}: {error}");
    }
    if outcome.exit.timed_out {
        bail!("searching {target} on the build node timed out; name a narrower directory");
    }
    match outcome.exit.exit_code {
        Some(0 | 1) => Ok(parse_rg_json_output(
            root,
            &stdout,
            opts,
            max_results,
            context,
            "remote-ripgrep",
        )),
        Some(NOT_ON_NODE) => bail!(
            "search path does not exist: {target} (neither in this checkout nor on the build \
             node that serves it)"
        ),
        Some(NOT_A_SOURCE_LOCATION) => bail!(
            "search path is outside the workspace: {target}. On the build node, code_grep \
             searches only library, toolchain and SDK sources (a dependency cache, a toolchain, \
             system headers); read a file there with `prod-code source <path>` (code_source)"
        ),
        code => {
            let detail = String::from_utf8_lossy(&stderr);
            if detail.contains("regex parse error") {
                bail!(
                    "invalid regex pattern `{}`: {}",
                    opts.pattern,
                    detail.trim()
                );
            }
            bail!(
                "the search of {target} on the build node failed (exit {code:?}): {}",
                detail.trim()
            )
        }
    }
}

#[cfg(test)]
#[path = "node_source_tests.rs"]
mod tests;
