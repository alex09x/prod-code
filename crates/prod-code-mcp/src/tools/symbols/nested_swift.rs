/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Swift declarations that a root match of the same name would hide (#2014).

use super::across_projects::files_naming_qualified;
use super::declared_at::declared_at;
use super::nested_projects::read_name_scan_text;
use super::search::declarations_in;
use super::types::SymbolHit;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

/// How long the checkout's Swift files are read for a declaration of the name.
const SCAN_BUDGET: Duration = Duration::from_secs(2);

/// How long their outlines are asked for.
const OUTLINE_BUDGET: Duration = Duration::from_secs(5);

/// The declarations of `name` in the checkout's Swift files, for a root of another language that
/// has already answered with a declaration of that name. sourcekit-lsp lists workspace symbols
/// only from a build index, so a Swift package nested in a C, Go or JavaScript checkout is found
/// by reading its files; the root's own `applyTheme` used to end the search there, and the Swift
/// method of that name went unlisted. Only files that declare the name are outlined, so a
/// checkout without one costs a walk and no query.
pub(crate) async fn nested_swift_declarations(
    remote: SocketAddr,
    root: &Path,
    name: &str,
    qualifier: Option<&str>,
    deadline: tokio::time::Instant,
) -> Vec<SymbolHit> {
    if crate::sync::expected_engine(root) == Some("swift") {
        return Vec::new();
    }
    let now = tokio::time::Instant::now();
    let files: Vec<_> = files_naming_qualified(
        root,
        "swift",
        name,
        qualifier,
        deadline.min(now + SCAN_BUDGET),
    )
    .into_iter()
    .filter(|path| {
        read_name_scan_text(path)
            .is_some_and(|text| text.lines().any(|line| declared_at(line, name).is_some()))
    })
    .collect();
    if files.is_empty() {
        return Vec::new();
    }
    let outline_deadline = deadline.min(tokio::time::Instant::now() + OUTLINE_BUDGET);
    match tokio::time::timeout_at(
        outline_deadline,
        declarations_in(remote, root, &files, name),
    )
    .await
    {
        Ok(Ok(found)) => found,
        _ => Vec::new(),
    }
}

#[cfg(test)]
#[path = "nested_swift_tests.rs"]
mod tests;
