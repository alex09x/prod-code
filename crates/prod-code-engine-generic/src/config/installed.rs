/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

use super::discovery::which_bin;
use super::types::GenericLspConfig;

impl GenericLspConfig {
    /// The language server this host would run for `engine` (`cpp`, `swift`, `python`,
    /// `typescript`), as a short label for the gateway status, or `None` when none of the
    /// candidates is installed.
    pub fn installed_server(engine: &str) -> Option<String> {
        let config = match engine {
            "cpp" => Self::for_cpp(),
            "swift" => Self::for_swift(),
            "python" => Self::for_python(),
            "typescript" => Self::for_typescript(),
            "java" => Self::for_java(),
            "kotlin" => Self::for_kotlin(),
            "csharp" => Self::for_csharp(),
            "php" => Self::for_php(),
            "ruby" => Self::for_ruby(),
            "dart" => Self::for_dart(),
            "zig" => Self::for_zig(),
            "elixir" => Self::for_elixir(),
            "scala" => Self::for_scala(),
            "lua" => Self::for_lua(),
            "haskell" => Self::for_haskell(),
            "ocaml" => Self::for_ocaml(),
            "clojure" => Self::for_clojure(),
            "julia" => Self::for_julia(),
            "shell" => Self::for_shell(),
            "r" => Self::for_r(),
            "erlang" => Self::for_erlang(),
            "fsharp" => Self::for_fsharp(),
            "perl" => Self::for_perl(),
            "solidity" => Self::for_solidity(),
            "nim" => Self::for_nim(),
            "d" => Self::for_d(),
            "fortran" => Self::for_fortran(),
            "sql" => Self::for_sql(),
            "graphql" => Self::for_graphql(),
            "protobuf" => Self::for_protobuf(),
            "crystal" => Self::for_crystal(),
            "groovy" => Self::for_groovy(),
            "ada" => Self::for_ada(),
            "v" => Self::for_v(),
            "racket" => Self::for_racket(),
            "terraform" => Self::for_terraform(),
            "nix" => Self::for_nix(),
            "markdown" => Self::for_markdown(),
            "yaml" => Self::for_yaml(),
            "toml" => Self::for_toml(),
            "json" => Self::for_json(),
            "html" => Self::for_html(),
            "css" => Self::for_css(),
            "dockerfile" => Self::for_dockerfile(),
            "svelte" => Self::for_svelte(),
            "vue" => Self::for_vue(),
            "assembly" => Self::for_assembly(),
            _ => return None,
        };
        let command = Path::new(&config.command);
        let installed = if command.is_absolute() {
            command.is_file()
        } else if config.command == "xcrun" {
            std::process::Command::new("xcrun")
                .args(["--find", "sourcekit-lsp"])
                .output()
                .map(|out| out.status.success())
                .unwrap_or(false)
        } else {
            which_bin(&config.command).is_ok()
        };
        if !installed {
            return None;
        }
        let label = match command.file_name().and_then(|n| n.to_str()) {
            Some("tsc") | Some("tsgo") => "tsc --lsp".to_string(),
            Some("xcrun") => "sourcekit-lsp".to_string(),
            Some(name) => name.to_string(),
            None => config.command.clone(),
        };
        Some(label)
    }
}
