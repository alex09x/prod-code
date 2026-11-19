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
    if root.join("compile_commands.json").exists() {
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
        "This C/C++ checkout has no compile_commands.json (nor compile_flags.txt), so clangd \
         indexes no translation unit and cannot find a symbol by name. Generate one on a build \
         node by configuring only, e.g. `cmake -S . -B build -DCMAKE_EXPORT_COMPILE_COMMANDS=ON`, \
         and link `build/compile_commands.json` to the checkout root; meanwhile find the \
         declaration with code_grep and ask code_definition by path, line and character."
            .to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
