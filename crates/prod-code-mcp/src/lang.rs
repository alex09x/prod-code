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
        "astro" => "astro",
        "svelte" => "svelte",
        "vue" => "vue",
        "html" | "htm" => "html",
        "css" | "scss" | "sass" | "less" => "css",
        "xml" | "svg" => "xml",
        "c" | "h" => "c",
        "cpp" | "hpp" | "cc" | "cxx" | "hh" | "hxx" | "ixx" => "cpp",
        "m" => "objective-c",
        "mm" => "objective-cpp",
        "swift" => "swift",
        "cs" => "csharp",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "scala" | "sc" => "scala",
        "php" | "phtml" => "php",
        "rb" | "erb" | "rake" | "gemspec" => "ruby",
        "zig" | "zon" => "zig",
        "dart" => "dart",
        "lua" => "lua",
        "hs" | "lhs" => "haskell",
        "ml" | "mli" => "ocaml",
        "ex" | "exs" => "elixir",
        "clj" | "cljs" | "cljc" | "edn" => "clojure",
        "jl" => "julia",
        "r" | "R" | "Rmd" => "r",
        "erl" | "hrl" => "erlang",
        "pl" | "pm" => "perl",
        "sol" => "solidity",
        "nim" | "nims" | "nimble" => "nim",
        "d" | "di" => "d",
        "f" | "for" | "f90" | "f95" | "f03" | "f08" => "fortran",
        "cr" => "crystal",
        "groovy" | "gvy" | "gy" | "gsh" | "gradle" => "groovy",
        "adb" | "ads" => "ada",
        "v" | "vh" => "v",
        "rkt" => "racket",
        "tf" | "tfvars" => "terraform",
        "nix" => "nix",
        "s" | "S" | "asm" => "assembly",
        "sql" => "sql",
        "graphql" | "gql" => "graphql",
        "proto" => "proto",
        "thrift" => "thrift",
        "toml" => "toml",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "md" | "markdown" => "markdown",
        "sh" | "bash" | "zsh" => "shellscript",
        _ if name == "CMakeLists.txt" || path.extension().and_then(|e| e.to_str()) == Some("cmake") => "cmake",
        _ if name == "Dockerfile" || name == "Containerfile" => "dockerfile",
        _ if name == "Makefile" || name == "makefile" || name == "GNUmakefile" => "makefile",
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
        assert_eq!(language_id_for_path(Path::new("src/App.astro")), "astro");
        assert_eq!(language_id_for_path(Path::new("src/App.svelte")), "svelte");
        assert_eq!(language_id_for_path(Path::new("src/App.vue")), "vue");
        assert_eq!(language_id_for_path(Path::new("src/main.zig")), "zig");
        assert_eq!(language_id_for_path(Path::new("src/app.dart")), "dart");
        assert_eq!(language_id_for_path(Path::new("src/script.lua")), "lua");
        assert_eq!(language_id_for_path(Path::new("src/Main.hs")), "haskell");
        assert_eq!(language_id_for_path(Path::new("src/Program.cs")), "csharp");
        assert_eq!(language_id_for_path(Path::new("src/App.java")), "java");
        assert_eq!(language_id_for_path(Path::new("src/App.kt")), "kotlin");
        assert_eq!(language_id_for_path(Path::new("src/index.php")), "php");
        assert_eq!(language_id_for_path(Path::new("src/app.rb")), "ruby");
        assert_eq!(language_id_for_path(Path::new("src/lib.ex")), "elixir");
        assert_eq!(language_id_for_path(Path::new("src/core.clj")), "clojure");
        assert_eq!(language_id_for_path(Path::new("src/math.jl")), "julia");
        assert_eq!(language_id_for_path(Path::new("src/analysis.r")), "r");
        assert_eq!(language_id_for_path(Path::new("src/contract.sol")), "solidity");
        assert_eq!(language_id_for_path(Path::new("src/query.sql")), "sql");
        assert_eq!(language_id_for_path(Path::new("CMakeLists.txt")), "cmake");
        assert_eq!(language_id_for_path(Path::new("Dockerfile")), "dockerfile");
        assert_eq!(language_id_for_path(Path::new("Makefile")), "makefile");
        assert_eq!(language_id_for_path(Path::new("README")), "plaintext");
    }
}
