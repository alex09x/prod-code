/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::{Path, PathBuf};

#[derive(Default, PartialEq, Eq)]
pub(super) struct Search {
    quote: Vec<PathBuf>,
    user: Vec<PathBuf>,
    system: Vec<PathBuf>,
    after: Vec<PathBuf>,
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

impl Search {
    /// Use only the selected TU's compilation entry, never another TU's flags
    /// or a search of similarly named headers. Ambiguous entries stay unproven.
    pub(super) fn new(root: &Path, file: &Path) -> Self {
        for name in ["compile_commands.json", "build/compile_commands.json"] {
            let db = root.join(name);
            let Ok(text) = std::fs::read_to_string(&db) else {
                continue;
            };
            let Ok(entries) = serde_json::from_str::<Vec<serde_json::Value>>(&text) else {
                return Self::default();
            };
            let mut selected = None;
            for entry in entries {
                let Some(directory) = entry["directory"].as_str() else {
                    continue;
                };
                let directory = db.parent().unwrap_or(root).join(directory);
                let Some(source) = entry["file"].as_str() else {
                    continue;
                };
                if canonical(&directory.join(source)) != canonical(file) {
                    continue;
                }
                let args = entry["arguments"]
                    .as_array()
                    .and_then(|a| {
                        a.iter()
                            .map(|v| v.as_str().map(str::to_string))
                            .collect::<Option<Vec<_>>>()
                    })
                    .or_else(|| entry["command"].as_str().and_then(arguments));
                let Some(search) = args.and_then(|args| Self::flags(&directory, &args)) else {
                    return Self::default();
                };
                if selected.as_ref().is_some_and(|old| old != &search) {
                    return Self::default();
                }
                selected = Some(search);
            }
            // A database owns the compilation context; do not mix in a fallback
            // flags file when it cannot identify this translation unit.
            return selected.unwrap_or_default();
        }
        let mut dir = file.parent();
        while let Some(base) = dir.filter(|base| base.starts_with(root)) {
            if let Ok(text) = std::fs::read_to_string(base.join("compile_flags.txt")) {
                let args = text.lines().map(str::to_string).collect::<Vec<_>>();
                return Self::flags(base, &args).unwrap_or_default();
            }
            if base == root {
                break;
            }
            dir = base.parent();
        }
        Self::default()
    }

    fn flags(directory: &Path, args: &[String]) -> Option<Self> {
        let mut search = Self::default();
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            // Response files and sysroots need compiler expansion before paths
            // can be trusted. Do not reinterpret them as ordinary directories.
            if arg.starts_with('@') || arg.contains("sysroot") || arg == "-I-" {
                return None;
            }
            for (flag, paths) in [
                ("-iquote", &mut search.quote),
                ("-isystem", &mut search.system),
                ("-idirafter", &mut search.after),
                ("-I", &mut search.user),
            ] {
                if let Some(suffix) = arg.strip_prefix(flag) {
                    let path = if suffix.is_empty() {
                        args.next()?.as_str()
                    } else {
                        suffix
                    };
                    if path.is_empty() || path.starts_with('=') {
                        return None;
                    }
                    paths.push(canonical(&directory.join(path)));
                    break;
                }
            }
        }
        // GCC/Clang ignore a user -I occurrence of a system directory and keep
        // that directory at its system-search position.
        search.user.retain(|path| !search.system.contains(path));
        Some(search)
    }

    pub(super) fn resolve(&self, file: &Path, spec: &str, quoted: bool) -> Option<PathBuf> {
        let local = file.parent()?.to_path_buf();
        let paths = quoted
            .then_some(&local)
            .into_iter()
            .chain(self.quote.iter().filter(|_| quoted))
            .chain(self.user.iter())
            .chain(self.system.iter())
            .chain(self.after.iter());
        paths
            .map(|base| base.join(spec))
            .find(|path| path.is_file())
            .map(|path| canonical(&path))
    }
}

/// Decode ordinary compilation-database shell words without executing a shell.
/// Expansion or command composition is not a trustworthy literal search path.
fn arguments(command: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('\''), _) => word.push(c),
            (_, '$' | '`' | ';' | '|' | '&' | '<' | '>') => return None,
            (_, '\\') => word.push(chars.next()?),
            (None, '\'' | '"') => quote = Some(c),
            (None, c) if c.is_whitespace() => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            _ => word.push(c),
        }
    }
    if quote.is_some() {
        return None;
    }
    if !word.is_empty() {
        words.push(word);
    }
    Some(words)
}
