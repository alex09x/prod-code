/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Shell functions as definitions (#1940). No language server answers a shell symbol, so
//! `code_definition NAME` in a shell project found nothing; the scripts are read here, as the
//! outline reads them, and a function is found by its name.

use std::path::Path;

use crate::tools::outline::shell::shell_functions;
use crate::tools::symbols::{
    SymbolHit,
    across_projects::read_name_scan_text,
    matching::bare_symbol_name,
    nested_projects::{MAX_SCANNED_FILES, source_files},
};

fn is_shell_script(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("sh" | "bash" | "zsh")
    )
}

/// The shell functions named `symbol` in the shell scripts under `root`, or in the script or
/// directory `hint` names. At most ten.
pub(crate) fn find_shell_declarations(
    root: &Path,
    symbol: &str,
    hint: Option<&Path>,
) -> Vec<SymbolHit> {
    // A shell function has no owner, so a qualified name (`Type::name`, `Module.Type.name`)
    // never names one; read as its last segment it found any function of that name (#2041).
    // Bash allows a dot in a function name, so a dotted name is matched whole.
    let raw = symbol.trim();
    if raw.contains("::") {
        return Vec::new();
    }
    let name = if raw.contains('.') {
        raw
    } else {
        bare_symbol_name(raw)
    };
    if name.is_empty() {
        return Vec::new();
    }
    let files: Vec<std::path::PathBuf> = match hint {
        Some(hint) => {
            let path = if hint.is_absolute() {
                hint.to_path_buf()
            } else {
                root.join(hint)
            };
            if path.is_file() {
                vec![path]
            } else if path.is_dir() {
                source_files(&path).collect()
            } else {
                Vec::new()
            }
        }
        None => source_files(root).collect(),
    };
    let mut hits = Vec::new();
    for path in files
        .into_iter()
        .filter(|path| is_shell_script(path))
        .take(MAX_SCANNED_FILES)
    {
        let Some(text) = read_name_scan_text(&path) else {
            continue;
        };
        for (function, line, col) in shell_functions(&text) {
            if function == name {
                hits.push(SymbolHit {
                    path: path.clone(),
                    name: function,
                    kind: "Function",
                    container: None,
                    line: line as u32 + 1,
                    col: col as u32 + 1,
                });
                if hits.len() >= 10 {
                    return hits;
                }
            }
        }
    }
    hits
}
