/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! What a C/C++ checkout without a compilation database is told (#2047).
//!
//! clangd indexes the translation units a `compile_commands.json` lists. Without one it has
//! nothing to answer a name from: a lookup by name timed out, then reported "no symbol" with
//! names from other languages. The answer now says what is missing and how to produce it.

use std::path::Path;

/// Whether `root` is a C/C++ checkout that clangd can index nothing of: CMake, Meson or the
/// C/C++ engine, with no `compile_commands.json` at the root or in a directory just under it,
/// and no `compile_flags.txt`.
pub(crate) fn lacks_compile_database(root: &Path) -> bool {
    let cpp = root.join("CMakeLists.txt").is_file()
        || root.join("meson.build").is_file()
        || crate::sync::expected_engine(root) == Some("cpp");
    if !cpp || root.join("compile_flags.txt").is_file() {
        return false;
    }
    if root.join("compile_commands.json").is_file() {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return false;
    };
    !entries
        .flatten()
        .any(|entry| entry.path().join("compile_commands.json").is_file())
}

/// The note added to a C/C++ name lookup that found nothing or ran out of time.
pub(crate) fn compile_database_note(root: &Path) -> Option<String> {
    lacks_compile_database(root).then(|| {
        // Meson writes the database on its own; CMake has to be asked for it.
        let configure =
            if !root.join("CMakeLists.txt").is_file() && root.join("meson.build").is_file() {
                "`meson setup build`"
            } else {
                "`cmake -S . -B build -DCMAKE_EXPORT_COMPILE_COMMANDS=ON`"
            };
        format!(
            "This C/C++ checkout has no compile_commands.json at its root or in a directory \
             just under it (nor compile_flags.txt), so clangd likely indexes no translation unit \
             and cannot find a symbol by name. Generate one on a build node by configuring only, \
             e.g. {configure}, and link `build/compile_commands.json` to the checkout root; \
             meanwhile find the declaration with code_grep and ask code_definition by path, line \
             and character."
        )
    })
}

/// The tools whose lookup by name the note is for.
const NAME_LOOKUPS: [&str; 2] = ["code_definition", "code_references"];

/// What a failed lookup by name says: it found nothing, or ran out of time.
fn is_failed_lookup(text: &str) -> bool {
    text.contains("no symbol named")
        || text.contains("did not finish within")
        || text.contains("timed out")
}

/// `outcome` of `tool` in the checkout `root` names (asked only for a failure), with the note
/// added when it is a failed C/C++ lookup by name: an error result gets it appended, and a
/// "no symbol" error becomes such a result. Any other outcome is returned as it was.
pub(crate) fn annotate(
    tool: &str,
    root: impl FnOnce() -> std::path::PathBuf,
    outcome: anyhow::Result<crate::protocol::McpToolCallResult>,
) -> anyhow::Result<crate::protocol::McpToolCallResult> {
    use crate::protocol::{McpContentItem, McpToolCallResult};
    if !NAME_LOOKUPS.contains(&tool) {
        return outcome;
    }
    match outcome {
        Ok(mut result) if result.is_error => {
            let failed = result.content.iter().any(|item| match item {
                McpContentItem::Text { text } => is_failed_lookup(text),
            });
            if failed && let Some(note) = compile_database_note(&root()) {
                result.content.push(McpContentItem::Text { text: note });
            }
            Ok(result)
        }
        Err(error) if format!("{error:#}").contains("no symbol named") => {
            match compile_database_note(&root()) {
                Some(note) => Ok(McpToolCallResult::error(format!("{error:#}\n{note}"))),
                None => Err(error),
            }
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The note reaches a failed definition or references lookup, and nothing else.
    #[test]
    fn a_failed_lookup_by_name_gets_the_note() {
        use crate::protocol::{McpContentItem, McpToolCallResult};
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::write(root.join("CMakeLists.txt"), "project(x)\n").unwrap();
        let text = |result: &McpToolCallResult| {
            result
                .content
                .iter()
                .map(|McpContentItem::Text { text }| text.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        };
        let at = || root.to_path_buf();
        let busy = McpToolCallResult::error("code_references did not finish within 20000 ms");
        let noted = annotate("code_references", at, Ok(busy)).unwrap();
        assert!(text(&noted).contains("compile_commands.json"));

        let missing = anyhow::anyhow!("no symbol named `A::f` in the workspace index");
        let noted = annotate("code_definition", at, Err(missing)).unwrap();
        assert!(noted.is_error && text(&noted).contains("cmake -S ."));

        let found = McpToolCallResult::text("📍 Definition: a.cpp:1:1");
        assert!(!text(&annotate("code_definition", at, Ok(found)).unwrap()).contains("cmake"));
        let other = McpToolCallResult::error("no symbol named `x`");
        assert!(!text(&annotate("code_hover", at, Ok(other)).unwrap()).contains("cmake"));
    }

    /// #2047: a CMake checkout without a database is named; one with a database, or with flags,
    /// or a Rust checkout, is not.
    #[test]
    fn a_cmake_checkout_without_a_database_is_told_so() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::write(root.join("CMakeLists.txt"), "project(x)\n").unwrap();
        assert!(compile_database_note(root).is_some_and(|note| note.contains("cmake -S .")));

        std::fs::create_dir_all(root.join("build")).unwrap();
        std::fs::write(root.join("build/compile_commands.json"), "[]").unwrap();
        assert!(compile_database_note(root).is_none());

        let flags = tempfile::tempdir().unwrap();
        std::fs::write(flags.path().join("CMakeLists.txt"), "project(x)\n").unwrap();
        std::fs::write(flags.path().join("compile_flags.txt"), "-std=c++20\n").unwrap();
        assert!(compile_database_note(flags.path()).is_none());

        let rust = tempfile::tempdir().unwrap();
        std::fs::write(rust.path().join("Cargo.toml"), "[package]\n").unwrap();
        assert!(compile_database_note(rust.path()).is_none());
    }
}
