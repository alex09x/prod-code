/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_protocol::{PathTranslator, readiness::ReadySignal};
use std::path::Path;

/// How to start a language server for an editor.
#[derive(Debug, Clone)]
pub struct ServerCommand {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub ready: ReadySignal,
}

impl PartialEq for ServerCommand {
    fn eq(&self, other: &Self) -> bool {
        self.program == other.program && self.args == other.args && self.env == other.env
    }
}

impl Eq for ServerCommand {}

/// Whether editors get servers of their own: `PROD_CODE_EDITOR_SERVERS=off` serves them from
/// the shared engines instead.
pub fn enabled() -> bool {
    std::env::var("PROD_CODE_EDITOR_SERVERS").as_deref() != Ok("off")
}

/// Whether `program --version` runs: a rustup proxy exists for rust-analyzer even where the
/// component is not installed, and then fails.
pub(crate) fn runs(program: &str) -> bool {
    std::process::Command::new(program)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// The language server an editor gets for `engine` on this node, or `None` when the node has
/// none; the session is then served by the shared engines.
pub fn server_command(engine: &str) -> Option<ServerCommand> {
    server_command_inner(engine, None)
}

pub fn server_command_for_workspace(engine: &str, workspace_root: &Path) -> Option<ServerCommand> {
    server_command_inner(engine, Some(workspace_root))
}

fn server_command_inner(engine: &str, workspace_root: Option<&Path>) -> Option<ServerCommand> {
    use prod_code_engine_generic::GenericLspConfig;
    let from = |config: GenericLspConfig| ServerCommand {
        program: config.command,
        args: config.args,
        env: config.env.into_iter().collect(),
        ready: config.ready,
    };
    let command = match engine {
        "rust" => ServerCommand {
            program: "rust-analyzer".to_string(),
            args: Vec::new(),
            env: Vec::new(),
            ready: ReadySignal::Progress,
        },
        "go" => ServerCommand {
            program: prod_code_engine_go::find_gopls_binary(None)?
                .to_string_lossy()
                .into_owned(),
            args: Vec::new(),
            env: Vec::new(),
            ready: ReadySignal::Progress,
        },
        "cpp" => from(GenericLspConfig::for_cpp()),
        "python" => {
            let mut cfg = GenericLspConfig::for_python();
            let stub_path = if let Some(workspace_root) = workspace_root {
                for (k, v) in
                    crate::python_cache::python_stub_cache_env_for_workspace(workspace_root)
                {
                    cfg.env.insert(k, v);
                }
                workspace_root.join("typings")
            } else {
                for (k, v) in crate::python_cache::python_stub_cache_env() {
                    cfg.env.insert(k, v);
                }
                crate::python_cache::python_stub_cache_dir()
            };
            if let Some(opts) = cfg
                .initialization_options
                .as_mut()
                .and_then(|o| o.as_object_mut())
            {
                if let Some(py) = opts.get_mut("python").and_then(|p| p.as_object_mut()) {
                    if let Some(an) = py.get_mut("analysis").and_then(|a| a.as_object_mut()) {
                        an.insert(
                            "stubPath".to_string(),
                            serde_json::Value::String(stub_path.to_string_lossy().into_owned()),
                        );
                    }
                }
            }
            from(cfg)
        }
        "typescript" => {
            let mut cfg = GenericLspConfig::for_typescript();
            for (k, v) in crate::ts_cache::ts_types_cache_env() {
                cfg.env.insert(k, v);
            }
            from(cfg)
        }
        "swift" => {
            let mut cfg = GenericLspConfig::for_swift();
            for (k, v) in crate::swift_cache::swift_module_cache_env() {
                cfg.env.insert(k, v);
            }
            from(cfg)
        }
        "java" => from(GenericLspConfig::for_java()),
        "kotlin" => from(GenericLspConfig::for_kotlin()),
        "csharp" => from(GenericLspConfig::for_csharp()),
        "php" => from(GenericLspConfig::for_php()),
        "ruby" => from(GenericLspConfig::for_ruby()),
        "dart" => from(GenericLspConfig::for_dart()),
        "zig" => from(GenericLspConfig::for_zig()),
        "elixir" => from(GenericLspConfig::for_elixir()),
        "scala" => from(GenericLspConfig::for_scala()),
        "lua" => from(GenericLspConfig::for_lua()),
        "haskell" => from(GenericLspConfig::for_haskell()),
        "ocaml" => from(GenericLspConfig::for_ocaml()),
        "clojure" => from(GenericLspConfig::for_clojure()),
        "julia" => from(GenericLspConfig::for_julia()),
        "shell" => from(GenericLspConfig::for_shell()),
        "r" => from(GenericLspConfig::for_r()),
        "erlang" => from(GenericLspConfig::for_erlang()),
        "fsharp" => from(GenericLspConfig::for_fsharp()),
        "perl" => from(GenericLspConfig::for_perl()),
        "solidity" => from(GenericLspConfig::for_solidity()),
        "nim" => from(GenericLspConfig::for_nim()),
        "d" => from(GenericLspConfig::for_d()),
        "fortran" => from(GenericLspConfig::for_fortran()),
        "sql" => from(GenericLspConfig::for_sql()),
        "graphql" => from(GenericLspConfig::for_graphql()),
        "protobuf" => from(GenericLspConfig::for_protobuf()),
        "crystal" => from(GenericLspConfig::for_crystal()),
        "groovy" => from(GenericLspConfig::for_groovy()),
        "ada" => from(GenericLspConfig::for_ada()),
        "v" => from(GenericLspConfig::for_v()),
        "racket" => from(GenericLspConfig::for_racket()),
        "terraform" => from(GenericLspConfig::for_terraform()),
        "nix" => from(GenericLspConfig::for_nix()),
        "markdown" => from(GenericLspConfig::for_markdown()),
        "yaml" => from(GenericLspConfig::for_yaml()),
        "toml" => from(GenericLspConfig::for_toml()),
        "json" => from(GenericLspConfig::for_json()),
        "html" => from(GenericLspConfig::for_html()),
        "css" => from(GenericLspConfig::for_css()),
        "dockerfile" => from(GenericLspConfig::for_dockerfile()),
        "svelte" => from(GenericLspConfig::for_svelte()),
        "vue" => from(GenericLspConfig::for_vue()),
        "assembly" => from(GenericLspConfig::for_assembly()),
        _ => return None,
    };
    let installed = if engine == "rust" {
        runs(&command.program)
    } else {
        Path::new(&command.program).is_file()
            || prod_code_engine_generic::which_bin(&command.program).is_ok()
    };
    installed.then_some(command)
}

/// `body` as one LSP frame.
pub(crate) fn frame(body: &str) -> Vec<u8> {
    format!("Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
}

/// The editor's message as the server gets it: paths translated, and the editor's process id
/// dropped from `initialize`. That id names a process on the editor's machine; a server that
/// watches its parent would find it missing here, or find someone else's, and exit.
pub fn to_server(translator: &PathTranslator, raw: &str) -> String {
    let translated = translator.translate_lsp_to_server(raw);
    if !translated.contains("\"processId\"") {
        return translated;
    }
    match serde_json::from_str::<serde_json::Value>(&translated) {
        Ok(mut value) if value.get("method").and_then(|m| m.as_str()) == Some("initialize") => {
            if let Some(params) = value.get_mut("params").and_then(|p| p.as_object_mut()) {
                params.insert("processId".to_string(), serde_json::Value::Null);
            }
            value.to_string()
        }
        _ => translated,
    }
}
