/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::helper_path::Module;
use crate::fixture::builder::lexer::{Source, bare};
use anyhow::{Context, Result};

// A semantic field reference ties the existing constructor/pattern (and its
// imports) to the owner. Reuse that path when cfg aliases or crate roots differ.
#[allow(clippy::too_many_arguments)]
pub(super) async fn spelling<F: std::future::Future<Output = Option<String>>>(
    homes: &[Module],
    uses: &[Module],
    home_scope: &[String],
    using_scope: &[String],
    text: &str,
    occurrence: usize,
    owner: &str,
    helper: &str,
    active_owner: impl FnOnce() -> F,
) -> Result<String> {
    let candidates = super::binding_path::prefixes(text, occurrence, owner)?;
    let choices: Vec<_> = candidates
        .iter()
        .filter(|candidate| {
            uses.iter().all(|using| {
                homes
                    .iter()
                    .any(|home| resolves(&candidate.prefix, home, using, home_scope, using_scope))
            })
        })
        .collect();
    if let Some(first) = choices.first() {
        // An unknown cfg import may override the ordinary binding, or be
        // inactive. Neither source order nor a possible physical owner proves
        // which one the analyzer's field reference belongs to.
        let ambiguous = choices.iter().any(|c| c.conditional)
            && choices.iter().any(|c| !same(&c.prefix, &first.prefix));
        let prefix = if ambiguous {
            anyhow::ensure!(
                uses.iter().all(|u| u.target == uses[0].target),
                "conditional bindings differ across Cargo targets; nothing was planned"
            );
            let markdown = active_owner().await.context(
                "cannot resolve the active conditional owner binding; nothing was planned",
            )?;
            let identity = owner_identity(&markdown, owner).context(
                "the analyzer did not identify the conditional owner module; nothing was planned",
            )?;
            &choices
                .iter()
                .find(|candidate| {
                    uses.iter().all(|using| {
                        homes.iter().any(|home| {
                            let path: Vec<_> = std::iter::once(home.name.clone())
                                .chain(home.segments.iter().cloned())
                                .chain(home_scope.iter().cloned())
                                .collect();
                            same(&path, &identity)
                                && resolves(&candidate.prefix, home, using, home_scope, using_scope)
                        })
                    })
                })
                .context("the active owner has no helper path in this scope; nothing was planned")?
                .prefix
        } else {
            &first.prefix
        };
        return Ok(format!("{}::{helper}", prefix.join("::")));
    }
    anyhow::ensure!(
        !candidates.iter().any(|c| c.explicit),
        "cannot resolve the explicit constructor binding to its nominal owner; nothing was planned"
    );
    // With no occurrence path, only a definite, identical spelling in *every*
    // containing target is sufficient. A single unknown path keeps source-only
    // and conditional-only layouts usable without choosing between cfg aliases.
    let eligible = |h: &&Module| {
        h.definite
            || homes
                .iter()
                .filter(|other| other.target == h.target)
                .count()
                == 1
    };
    for home in homes.iter().filter(eligible) {
        for root in ["crate", home.name.as_str()] {
            let prefix: Vec<_> = std::iter::once(root.to_owned())
                .chain(home.segments.iter().cloned())
                .chain(home_scope.iter().cloned())
                .collect();
            if uses.iter().all(|using| {
                homes
                    .iter()
                    .filter(eligible)
                    .any(|h| resolves(&prefix, h, using, home_scope, using_scope))
            }) {
                return Ok(format!("{}::{helper}", prefix.join("::")));
            }
        }
    }
    anyhow::bail!(
        "cannot resolve one helper path in every containing Cargo target; nothing was planned"
    )
}

fn owner_identity(markdown: &str, owner: &str) -> Option<Vec<String>> {
    // rust-analyzer's field hover names the containing type in its own code
    // block, before the field declaration. Do not treat arbitrary prose as an
    // identity, and do not infer a path from a bare field/type spelling.
    let mut lines = markdown.lines().skip_while(|l| l.trim() != "```rust");
    lines.next()?;
    let path: Vec<_> = lines
        .next()?
        .trim()
        .split("::")
        .map(str::to_owned)
        .collect();
    (lines.next()?.trim() == "```"
        && path.len() > 1
        && path.iter().all(|s| {
            !bare(s).is_empty() && bare(s).chars().all(|c| c == '_' || c.is_alphanumeric())
        })
        && bare(path.last()?) == bare(owner))
    .then(|| path[..path.len() - 1].to_vec())
}

fn same(a: &[String], b: &[String]) -> bool {
    a.iter().map(|s| bare(s)).eq(b.iter().map(|s| bare(s)))
}

fn resolves(
    prefix: &[String],
    home: &Module,
    using: &Module,
    home_scope: &[String],
    using_scope: &[String],
) -> bool {
    let owner: Vec<_> = home.segments.iter().chain(home_scope).cloned().collect();
    let mut local: Vec<_> = using.segments.iter().chain(using_scope).cloned().collect();
    let absolute = prefix.first().is_some_and(String::is_empty);
    let prefix = if absolute { &prefix[1..] } else { prefix };
    let Some(root) = prefix.first() else {
        return false;
    };
    let mut at = 1;
    match root.as_str() {
        "crate" | "self" | "super" if absolute => return false,
        "crate" => local.clear(),
        "self" => {}
        "super" => {
            if local.pop().is_none() {
                return false;
            }
            while prefix.get(at).is_some_and(|s| s == "super") {
                if local.pop().is_none() {
                    return false;
                }
                at += 1;
            }
        }
        _ => {
            let relative: Vec<_> = local.iter().chain(prefix).cloned().collect();
            if !absolute && home.target == using.target && same(&relative, &owner) {
                return true;
            }
            if bare(root) != bare(&home.name) || !home.library || !same(&prefix[1..], &owner) {
                return false;
            }
            // A crate's own library name is in scope only with an explicit alias.
            return home.target != using.target || self_alias(&using.target, root);
        }
    }
    local.extend_from_slice(&prefix[at..]);
    home.target == using.target && same(&local, &owner)
}

fn self_alias(target: &std::path::Path, name: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(target) else {
        return false;
    };
    let Ok(source) = Source::new(&text) else {
        return false;
    };
    (0..source.tokens.len()).any(|at| {
        source.is(at, "extern")
            && source.is(at + 1, "crate")
            && source.is(at + 2, "self")
            && source.is(at + 3, "as")
            && source.is(at + 4, name)
            && source.is(at + 5, ";")
    })
}
