/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{Graph, Owner, syntax};
use anyhow::{Context, Result};
use std::collections::BTreeSet;

impl Graph {
    pub fn scope_in(&self, owner: &Owner, text: &str) -> Result<std::ops::Range<usize>> {
        let base = self
            .nodes
            .iter()
            .find(|n| n.identity == owner.identity && n.file == owner.file && n.scope.start == 0)
            .context("file has no lexical root owner")?;
        let mut scope = 0..text.len();
        for name in &owner.module.segments[base.module.segments.len()..] {
            scope = syntax::declarations(text, scope, syntax::edition(&owner.root))
                .into_iter()
                .find(|d| &d.name == name)
                .and_then(|d| d.inline)
                .context("inline owner scope changed; nothing was written")?;
        }
        Ok(scope)
    }
}

pub(crate) fn same_roots(from: &[Owner], to: &[Owner]) -> Result<()> {
    let roots = |owners: &[Owner]| {
        owners
            .iter()
            .map(|n| n.identity.clone())
            .collect::<BTreeSet<_>>()
    };
    anyhow::ensure!(
        !from.is_empty() && !to.is_empty() && roots(from) == roots(to),
        "source and target belong to different Cargo roots (including integration-test targets); nothing was written"
    );
    for source in from {
        let destination = to.iter().find(|n| n.identity == source.identity).unwrap();
        anyhow::ensure!(
            source.condition.implies(&destination.condition),
            "destination membership is not proven in every source cfg configuration; nothing was written"
        );
    }
    Ok(())
}
