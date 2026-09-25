//! prod-code for Zed: the language servers run on the build nodes, and `prod-code lsp` carries
//! them to the editor. Each server here is one language's: rust-analyzer, gopls, clangd,
//! basedpyright, the TypeScript server or sourcekit-lsp, started on the node the checkout is
//! placed on, in its copy of the checkout.
//!
//! The user's settings for the server a prod-code server stands in for apply to it as they
//! are: `lsp.rust-analyzer.initialization_options` and `.settings` reach the remote
//! rust-analyzer when `lsp.prod-code-rust` sets none of its own.

use zed_extension_api::{self as zed, LanguageServerId, Result, serde_json, settings::LspSettings};

struct ProdCode;

/// The language `prod-code lsp --language` is given for a server of this extension.
fn language(server: &LanguageServerId) -> &'static str {
    match server.as_ref() {
        "prod-code-go" => "go",
        "prod-code-cpp" => "cpp",
        "prod-code-python" => "python",
        "prod-code-typescript" => "typescript",
        "prod-code-swift" => "swift",
        _ => "rust",
    }
}

/// The name Zed knows the local server by, whose settings the remote one takes.
fn stands_in_for(server: &LanguageServerId) -> &'static str {
    match server.as_ref() {
        "prod-code-go" => "gopls",
        "prod-code-cpp" => "clangd",
        "prod-code-python" => "basedpyright",
        "prod-code-typescript" => "vtsls",
        "prod-code-swift" => "sourcekit-lsp",
        _ => "rust-analyzer",
    }
}

/// The settings of this server, or else those of the server it stands in for.
fn settings(server: &LanguageServerId, worktree: &zed::Worktree) -> (Option<LspSettings>, Option<LspSettings>) {
    (
        LspSettings::for_worktree(server.as_ref(), worktree).ok(),
        LspSettings::for_worktree(stands_in_for(server), worktree).ok(),
    )
}

impl zed::Extension for ProdCode {
    fn new() -> Self {
        ProdCode
    }

    fn language_server_command(
        &mut self,
        server: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let own = LspSettings::for_worktree(server.as_ref(), worktree).ok();
        let binary = own.as_ref().and_then(|s| s.binary.clone());
        let command = binary
            .as_ref()
            .and_then(|b| b.path.clone())
            .or_else(|| worktree.which("prod-code"))
            .ok_or_else(|| {
                "prod-code is not on the PATH: install the client from \
                 https://github.com/alex09x/prod-code/releases (or `cargo install --git \
                 https://github.com/alex09x/prod-code prod-code-client`), or set \
                 lsp.<server>.binary.path in the settings"
                    .to_string()
            })?;
        let args = binary
            .as_ref()
            .and_then(|b| b.arguments.clone())
            .unwrap_or_else(|| {
                vec![
                    "lsp".to_string(),
                    "--language".to_string(),
                    language(server).to_string(),
                ]
            });
        let mut env = worktree.shell_env();
        if let Some(extra) = binary.and_then(|b| b.env) {
            env.extend(extra);
        }
        Ok(zed::Command { command, args, env })
    }

    fn language_server_initialization_options(
        &mut self,
        server: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<serde_json::Value>> {
        let (own, native) = settings(server, worktree);
        Ok(own
            .and_then(|s| s.initialization_options)
            .or_else(|| native.and_then(|s| s.initialization_options)))
    }

    fn language_server_workspace_configuration(
        &mut self,
        server: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<serde_json::Value>> {
        let (own, native) = settings(server, worktree);
        Ok(own
            .and_then(|s| s.settings)
            .or_else(|| native.and_then(|s| s.settings)))
    }
}

zed::register_extension!(ProdCode);
