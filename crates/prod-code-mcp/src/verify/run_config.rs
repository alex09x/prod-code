/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! What a project says about how its checks run, from `prod-code.toml` (#1961):
//!
//! ```toml
//! [rust]
//! toolchain = "1.97.1"   # RUSTUP_TOOLCHAIN for the Rust commands; it must be on the node
//! runner = "mitts"       # "cargo" (default) or "mitts"
//!
//! [commands.python]
//! test = ["python3", "-m", "pytest", "-q", "{filter}"]
//! ```
//!
//! A `[commands.<language>]` entry (`test`, `check`, `lint` or `bench`) is the argv to run: it
//! replaces the planned command and the planner's scope narrowing is not applied to it.
//! `{filter}` stands for the test filter. Without the file, or without these keys, the planner
//! behaves as it did.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Result, bail};
use serde::Deserialize;

use super::types::VerifyKind;

const FILE_NAME: &str = "prod-code.toml";
const KINDS: [&str; 4] = ["test", "check", "lint", "bench"];

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct RunConfig {
    rust: RustRun,
    commands: BTreeMap<String, BTreeMap<String, Vec<String>>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
struct RustRun {
    toolchain: Option<String>,
    runner: Option<String>,
}

fn kind_key(kind: VerifyKind) -> &'static str {
    match kind {
        VerifyKind::Test => "test",
        VerifyKind::Check => "check",
        VerifyKind::Lint => "lint",
        VerifyKind::Bench => "bench",
    }
}

impl RunConfig {
    /// Reads `<root>/prod-code.toml`. A missing file is the default; a malformed one is
    /// reported and ignored, as the Rust analysis options are.
    pub fn load(root: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(root.join(FILE_NAME)) else {
            return Self::default();
        };
        match toml::from_str::<Self>(&text) {
            Ok(config) => {
                for (language, kinds) in &config.commands {
                    for kind in kinds.keys().filter(|kind| !KINDS.contains(&kind.as_str())) {
                        tracing::warn!(
                            language,
                            kind,
                            "prod-code.toml: [commands.{language}] names no such command; \
                             use test, check, lint or bench"
                        );
                    }
                }
                config
            }
            Err(error) => {
                tracing::warn!(%error, "prod-code.toml run settings ignored");
                Self::default()
            }
        }
    }

    /// Whether the project names the command for `kind` in `language`.
    pub fn has_command(&self, language: &str, kind: VerifyKind) -> bool {
        self.commands
            .get(language)
            .is_some_and(|kinds| kinds.contains_key(kind_key(kind)))
    }

    /// The project's own command for `kind` in `language`, with `{filter}` filled in. An
    /// argument that names `{filter}` is dropped when there is no filter; a filter that the
    /// command has no place for is refused, since it would not be applied.
    pub fn command_for(
        &self,
        language: &str,
        kind: VerifyKind,
        filter: Option<&str>,
    ) -> Result<Option<Vec<String>>> {
        let Some(template) = self
            .commands
            .get(language)
            .and_then(|kinds| kinds.get(kind_key(kind)))
        else {
            return Ok(None);
        };
        let place = format!("[commands.{language}] {}", kind_key(kind));
        if template.is_empty() || template[0].trim().is_empty() {
            bail!("prod-code.toml: {place} is empty; give the command as a list of arguments");
        }
        let places_filter = template.iter().any(|arg| arg.contains("{filter}"));
        if filter.is_some() && !places_filter {
            bail!(
                "prod-code.toml: {place} has no {{filter}}, so the filter cannot be applied; \
                 add {{filter}} to it or run without a filter"
            );
        }
        let command = template
            .iter()
            .filter_map(|arg| match (arg.contains("{filter}"), filter) {
                (true, Some(filter)) => Some(arg.replace("{filter}", filter)),
                (true, None) => None,
                (false, _) => Some(arg.clone()),
            })
            .collect();
        Ok(Some(command))
    }

    fn mitts(&self) -> Result<bool> {
        match self.rust.runner.as_deref() {
            None | Some("cargo") => Ok(false),
            Some("mitts") => Ok(true),
            Some(other) => bail!(
                "prod-code.toml: [rust] runner = \"{other}\" is not a runner; use \"cargo\" or \"mitts\""
            ),
        }
    }

    /// `command`, planned for `language`, after the project's runner and toolchain apply. A
    /// command the project wrote itself (`custom`) is run as written, except that the Rust
    /// toolchain still applies.
    pub fn finish(
        &self,
        language: &str,
        custom: bool,
        mut command: Vec<String>,
    ) -> Result<Vec<String>> {
        if language != "rust" {
            return Ok(command);
        }
        if !custom && self.mitts()? {
            command = cargo_to_mitts(command)?;
        }
        if let Some(toolchain) = self.rust.toolchain.as_deref() {
            if toolchain.trim().is_empty() || toolchain.chars().any(char::is_control) {
                bail!("prod-code.toml: [rust] toolchain must be a rustup toolchain name");
            }
            let mut wrapped = vec!["env".to_string(), format!("RUSTUP_TOOLCHAIN={toolchain}")];
            wrapped.extend(command);
            command = wrapped;
        }
        Ok(command)
    }
}

/// The Mitts command for a planned Cargo one. Mitts takes `check`, `clippy` and `test` with the
/// flags the planner uses; it takes the test filter as a name rather than after `--`, and it
/// passes nothing after `--` to the tests, so a test argument other than one name is refused
/// instead of dropped. Other commands (`bench`) stay as they are.
fn cargo_to_mitts(mut command: Vec<String>) -> Result<Vec<String>> {
    if command.first().map(String::as_str) != Some("cargo") {
        return Ok(command);
    }
    match command.get(1).map(String::as_str) {
        Some("check" | "clippy") => {
            command[0] = "mitts".to_string();
        }
        Some("test") => {
            command[0] = "mitts".to_string();
            if let Some(at) = command.iter().position(|arg| arg == "--") {
                let tail: Vec<String> = command.drain(at..).skip(1).collect();
                match tail.as_slice() {
                    [] => {}
                    [name] if !name.starts_with('-') => command.push(name.clone()),
                    _ => bail!(
                        "prod-code.toml: [rust] runner = \"mitts\" takes one test name, not `{}`; \
                         use runner = \"cargo\" or write [commands.rust] test",
                        tail.join(" ")
                    ),
                }
            }
        }
        _ => {}
    }
    Ok(command)
}

#[cfg(test)]
#[path = "run_config_tests.rs"]
mod tests;
