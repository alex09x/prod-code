/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

/// When the server of a language is loaded ahead of a request.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum When {
    /// Again after a restart, if it was in use before it.
    #[default]
    Restart,
    /// Only when a request asks for it.
    Never,
}

/// What is done for one language.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LanguagePolicy {
    #[serde(default)]
    pub when: When,
    /// A command run in the workspace after it was loaded again, as an argument vector with no
    /// shell: the build that writes the index a server answers from (`swift build
    /// --build-tests` for a `sourcekit-lsp` that has no background indexing). Empty: none.
    #[serde(default)]
    pub prebuild: Vec<String>,
    /// How long that command may run before it is stopped.
    #[serde(default = "default_prebuild_budget_secs")]
    pub budget_secs: u64,
}

fn default_prebuild_budget_secs() -> u64 {
    600
}

impl Default for LanguagePolicy {
    fn default() -> Self {
        Self {
            when: When::default(),
            prebuild: Vec::new(),
            budget_secs: default_prebuild_budget_secs(),
        }
    }
}

/// The `[warmup]` section of `gateway.toml`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct WarmupConfig {
    /// Load again after a restart the workspaces that were in use before it.
    pub after_restart: bool,
    /// At most this many, the most recently used first.
    pub max_workspaces: usize,
    /// Only those used within this many hours.
    pub max_age_hours: u64,
    /// Not while the one-minute load is above this percentage of the node's cores (80 is a load
    /// of 0.8 per core; a node already past its cores can be set above 100).
    pub skip_above_load_percent: u32,
    /// Loads running at once.
    pub max_concurrent: usize,
    /// Per language, by the engine's name (`rust`, `go`, `swift`, …); a language not listed is
    /// loaded.
    pub languages: BTreeMap<String, LanguagePolicy>,
}

impl Default for WarmupConfig {
    fn default() -> Self {
        Self {
            after_restart: true,
            max_workspaces: 4,
            max_age_hours: 24,
            skip_above_load_percent: 80,
            max_concurrent: 1,
            languages: BTreeMap::new(),
        }
    }
}

impl WarmupConfig {
    /// Whether a workspace served by `engine` is loaded ahead of a request.
    pub fn loads(&self, engine: &str) -> bool {
        self.languages
            .get(engine)
            .is_none_or(|policy| policy.when != When::Never)
    }

    /// A mistake in the section, naming the key.
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=800).contains(&self.skip_above_load_percent) {
            return Err(format!(
                "warmup.skip_above_load_percent must be from 1 to 800 (a percentage of the cores), not {}",
                self.skip_above_load_percent
            ));
        }
        if self.max_concurrent == 0 {
            return Err("warmup.max_concurrent must be at least 1".to_string());
        }
        if self.max_age_hours == 0 {
            return Err("warmup.max_age_hours must be at least 1".to_string());
        }
        if let Some(name) = self.languages.keys().find(|k| k.trim().is_empty()) {
            return Err(format!(
                "warmup.languages has an empty language name {name:?}"
            ));
        }
        for (name, policy) in &self.languages {
            if policy
                .prebuild
                .first()
                .is_some_and(|program| program.trim().is_empty())
            {
                return Err(format!(
                    "warmup.languages.{name}.prebuild must start with the program to run, not an empty string"
                ));
            }
            if !(1..=3600).contains(&policy.budget_secs) {
                return Err(format!(
                    "warmup.languages.{name}.budget_secs must be from 1 to 3600 seconds, not {}",
                    policy.budget_secs
                ));
            }
        }
        Ok(())
    }

    /// The command to run in a workspace of `engine` after it was loaded again, and how long it
    /// may take; `None` when the language has none.
    pub fn prebuild(&self, engine: &str) -> Option<(&[String], std::time::Duration)> {
        let policy = self.languages.get(engine)?;
        (!policy.prebuild.is_empty()).then(|| {
            (
                &policy.prebuild[..],
                std::time::Duration::from_secs(policy.budget_secs),
            )
        })
    }
}
