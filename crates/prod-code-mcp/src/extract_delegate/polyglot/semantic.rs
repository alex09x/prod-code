/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result, ensure};
use std::collections::{BTreeMap, BTreeSet};

use crate::extract_delegate::common::is_ident;
use crate::parameter_object::Language;

/// Positions always refer to the original source. Restructurers pass original subslices
/// here, before removing/reordering anything, so an identical token elsewhere cannot be
/// substituted for a removed or unsupported occurrence.
pub(crate) struct Source<'a> {
    pub text: &'a str,
    pub lang: Language,
    pub field: &'a str,
    refs: BTreeMap<usize, String>,
    handled: BTreeSet<usize>,
    pub accesses: usize,
}

impl<'a> Source<'a> {
    pub fn new(text: &'a str, lang: Language, field: &'a str) -> Self {
        Self {
            text,
            lang,
            field,
            refs: BTreeMap::new(),
            handled: BTreeSet::new(),
            accesses: 0,
        }
    }

    pub fn add(&mut self, line: u32, col: u32, name: &str) -> Result<()> {
        let offset = crate::signature::parse::offset_of(self.text, line, col)
            .context("analyzer reference is outside the original source")?;
        let after = self
            .text
            .get(offset..)
            .context("reference splits a character")?;
        ensure!(
            after.starts_with(name)
                && !after[name.len()..].chars().next().is_some_and(is_ident)
                && !self.text[..offset]
                    .chars()
                    .next_back()
                    .is_some_and(is_ident),
            "analyzer reference {line}:{col} does not identify field `{name}`"
        );
        ensure!(
            super::lexical::reference_is_code(self.text, offset, self.lang),
            "analyzer reference {line}:{col} identifies literal/comment text, not field `{name}`"
        );
        ensure!(
            !super::invocation::invoked(self.text, offset, name.len(), self.lang),
            "callable field `{name}` at {line}:{col}: receiver/evaluation semantics are unproved"
        );
        if let Some(previous) = self.refs.insert(offset, name.to_string()) {
            ensure!(previous == name, "conflicting analyzer field identities");
        }
        Ok(())
    }

    fn bounds(&self, slice: &str) -> std::ops::Range<usize> {
        let start = slice.as_ptr() as usize - self.text.as_ptr() as usize;
        start..start + slice.len()
    }

    pub fn positions(&self, slice: &str) -> Vec<(usize, String)> {
        self.refs
            .range(self.bounds(slice))
            .filter(|(p, _)| !self.handled.contains(p))
            .map(|(p, n)| (*p, n.clone()))
            .collect()
    }

    /// A declaration or moved method retains its own field identity in the helper.
    /// Its caller supplies only the exact consumed source region.
    pub fn consumed(&mut self, slice: &str) {
        for (p, _) in self.positions(slice) {
            self.handled.insert(p);
        }
    }

    pub fn moved_method(&mut self, slice: &str, receiver: &str) -> Result<()> {
        for (p, name) in self.positions(slice) {
            let before = self.text[..p].trim_end();
            let receiver_before = before
                .strip_suffix(&format!("{receiver}."))
                .or_else(|| before.strip_suffix(&format!("{receiver}->")));
            ensure!(
                receiver_before.is_some_and(|s| !s.chars().next_back().is_some_and(is_ident))
                    || (matches!(self.lang, Language::Cpp | Language::C | Language::Swift)
                        && !before.ends_with('.')
                        && !before.ends_with("->")),
                "moved method's reference to `{name}` has an unproved receiver"
            );
            self.handled.insert(p);
        }
        Ok(())
    }

    pub fn rewrite(&mut self, slice: &str) -> Result<String> {
        let start = self.bounds(slice).start;
        let mut out = slice.to_string();
        for (p, name) in self.positions(slice).into_iter().rev() {
            if self.handled.contains(&p) {
                continue;
            }
            let before = self.text[..p].trim_end();
            ensure!(
                before.ends_with('.')
                    || before.ends_with("->")
                    || matches!(self.lang, Language::Cpp | Language::C | Language::Swift),
                "field `{name}` at byte {p} is not a supported qualified access"
            );
            out.insert_str(p - start, &format!("{}.", self.field));
            if self.handled.insert(p) {
                self.accesses += 1;
            }
        }
        Ok(out)
    }

    pub fn finish(&self) -> Result<()> {
        ensure!(
            self.refs.keys().all(|p| self.handled.contains(p)),
            "analyzer field reference was not consumed or rewritten at its original position"
        );
        Ok(())
    }
}
