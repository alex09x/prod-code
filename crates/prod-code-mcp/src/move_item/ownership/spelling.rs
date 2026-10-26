/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{Graph, Owner, normalize};
use anyhow::{Context, Result};
use std::collections::BTreeSet;

/// Resolve one spelling that preserves the binding in every compilation of a shared file.
/// Names never establish crate identity, including two targets backed by the same file.
impl Graph {
    pub fn caller_spelling(&self, callers: &[Owner], destinations: &[Owner]) -> Result<String> {
        let mut spellings = BTreeSet::new();
        for caller in callers {
            let path = if let Some(local) =
                destinations.iter().find(|d| d.identity == caller.identity)
            {
                std::iter::once("crate".to_owned())
                    .chain(local.module.segments.clone())
                    .collect::<Vec<_>>()
                    .join("::")
            } else {
                let mut external = BTreeSet::new();
                for destination in destinations.iter().filter(|d| d.kind == "lib") {
                    for binding in bindings(caller, destination)? {
                        external.insert(
                            std::iter::once(binding)
                                .chain(destination.module.segments.clone())
                                .collect::<Vec<_>>()
                                .join("::"),
                        );
                    }
                }
                anyhow::ensure!(
                    external.len() == 1,
                    "external Cargo target binding is unknown or ambiguous (caller kind {}, destination kinds {:?}); nothing was written",
                    caller.kind,
                    destinations
                        .iter()
                        .map(|d| (d.kind, d.manifest == caller.manifest))
                        .collect::<Vec<_>>()
                );
                external.into_iter().next().unwrap()
            };
            spellings.insert(path);
        }
        anyhow::ensure!(
            spellings.len() == 1,
            "shared caller requires different target bindings; nothing was written"
        );
        Ok(spellings.into_iter().next().unwrap())
    }
}

fn bindings(caller: &Owner, destination: &Owner) -> Result<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    if caller.manifest == destination.manifest && caller.kind != "custom-build" {
        names.insert(destination.module.krate.clone());
    } else {
        let doc: toml::Value = toml::from_str(&std::fs::read_to_string(&caller.manifest)?)?;
        let dir = caller
            .manifest
            .parent()
            .context("manifest has no directory")?;
        for key in ["dependencies", "dev-dependencies"] {
            if key == "dev-dependencies"
                && !caller.test_crate
                && caller.kind != "example"
                && caller.kind != "bench"
            {
                continue;
            }
            if let Some(table) = doc.get(key).and_then(|v| v.as_table()) {
                for (binding, spec) in table {
                    if let Some(path) = spec.get("path").and_then(|p| p.as_str())
                        && normalize(&dir.join(path).join("Cargo.toml")) == destination.manifest
                    {
                        // Cargo's package key names a dependency, but an unrenamed
                        // dependency is exposed under its library target name.
                        let name = if spec.get("package").is_some() {
                            binding.replace('-', "_")
                        } else {
                            destination.module.krate.clone()
                        };
                        names.insert(name);
                    }
                }
            }
        }
    }
    // In 2015 the explicit extern declaration supplies the root binding (possibly renamed).
    // In newer editions the extern prelude provides an absolute binding, independent of
    // local modules and runtime decoys with the same name.
    if super::syntax::edition(&caller.root) == ra_ap_syntax::Edition::Edition2015 {
        use ra_ap_syntax::{AstNode, ast};
        let text = super::syntax::read(&caller.root)?;
        let tree = super::syntax::parse(&text, ra_ap_syntax::Edition::Edition2015);
        let mut explicit = BTreeSet::new();
        for node in tree.syntax().children().filter_map(ast::ExternCrate::cast) {
            let tokens: Vec<_> = node
                .syntax()
                .descendants_with_tokens()
                .filter_map(|n| n.into_token())
                .filter(|t| !t.kind().is_trivia())
                .collect();
            if let Some(index) = tokens.iter().position(|t| t.text() == "crate")
                && let Some(name) = tokens.get(index + 1)
                && names.contains(name.text())
            {
                let alias = if tokens.get(index + 2).is_some_and(|t| t.text() == "as") {
                    tokens
                        .get(index + 3)
                        .context("extern crate alias is missing")?
                        .text()
                } else {
                    name.text()
                };
                explicit.insert(format!("::{alias}"));
            }
        }
        Ok(explicit)
    } else {
        Ok(names.into_iter().map(|name| format!("::{name}")).collect())
    }
}
