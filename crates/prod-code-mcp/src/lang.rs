//! LSP `languageId` for a file path, shared by the CLI and the MCP tools.

use std::path::Path;

/// The LSP `languageId` a file is opened with, from its extension.
pub fn language_id_for_path(path: &Path) -> &'static str {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "rs" => "rust",
        "go" => "go",
        "py" | "pyi" => "python",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "typescriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "c" | "h" => "c",
        "cpp" | "hpp" | "cc" | "cxx" | "hh" | "hxx" | "ixx" => "cpp",
        "m" => "objective-c",
        "mm" => "objective-cpp",
        "swift" => "swift",
        "proto" => "proto",
        "toml" => "toml",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "sh" | "bash" | "zsh" => "shellscript",
        _ if name == "CMakeLists.txt" => "cmake",
        _ => "plaintext",
    }
}

/// Whether a file is a C or C++ header: what callers include to learn a declaration, and so
/// where a type they need has to be declared.
pub fn is_header(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).unwrap_or(""),
        "h" | "hh" | "hpp" | "hxx" | "inl"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_are_told_from_sources() {
        assert!(is_header(Path::new("src/home.h")));
        assert!(is_header(Path::new("include/shapes/home.hpp")));
        assert!(!is_header(Path::new("src/home.c")));
        assert!(!is_header(Path::new("src/home.cpp")));
    }

    #[test]
    fn maps_common_extensions() {
        assert_eq!(language_id_for_path(Path::new("src/lib.rs")), "rust");
        assert_eq!(
            language_id_for_path(Path::new("src/index.ts")),
            "typescript"
        );
        assert_eq!(language_id_for_path(Path::new("src/util.h")), "c");
        assert_eq!(
            language_id_for_path(Path::new("Sources/App/main.swift")),
            "swift"
        );
        assert_eq!(language_id_for_path(Path::new("CMakeLists.txt")), "cmake");
        assert_eq!(language_id_for_path(Path::new("README")), "plaintext");
    }
}
