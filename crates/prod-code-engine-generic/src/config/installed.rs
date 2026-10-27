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
            launcher_finds(Path::new("xcrun"), "sourcekit-lsp")
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

/// How long a launcher gets to say where a tool is.
const LAUNCHER_PROBE: std::time::Duration = std::time::Duration::from_secs(10);

/// Whether `launcher --find tool` (`xcrun --find sourcekit-lsp`) names the tool in the
/// selected toolchain within [`LAUNCHER_PROBE`]. A launcher that is installed while the
/// toolchain lacks the tool, that is missing, or that hangs, finds nothing (#1449).
pub fn launcher_finds(launcher: &Path, tool: &str) -> bool {
    let Ok(mut child) = std::process::Command::new(launcher)
        .args(["--find", tool])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return false;
    };
    let deadline = std::time::Instant::now() + LAUNCHER_PROBE;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::launcher_finds;
    use std::os::unix::fs::PermissionsExt;

    fn launcher(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// A launcher that is present but cannot find the server is no server (#1449).
    #[test]
    fn a_present_launcher_without_the_server_finds_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let found = launcher(
            dir.path(),
            "found",
            "[ \"$2\" = sourcekit-lsp ] && echo /x/sourcekit-lsp",
        );
        let absent = launcher(
            dir.path(),
            "absent",
            "echo 'unable to find utility' >&2; exit 72",
        );
        assert!(launcher_finds(&found, "sourcekit-lsp"));
        assert!(!launcher_finds(&absent, "sourcekit-lsp"));
        assert!(!launcher_finds(
            &dir.path().join("missing"),
            "sourcekit-lsp"
        ));
    }
}
