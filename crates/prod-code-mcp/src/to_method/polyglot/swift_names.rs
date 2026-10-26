/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// File-local nominal lookup. An extension supplies members, never evidence that
/// its target exists. Canonicalize declaration scopes as well as queried paths:
/// `extension Prefix { typealias Alias = C }` resolves C inside Prefix's target.
pub(super) struct Names<'a> {
    owners: &'a [Option<String>],
    extensions: &'a [bool],
    aliases: &'a [(String, Option<String>)],
}

enum Lookup {
    Known(String),
    Missing,
    Unknown,
}

impl Lookup {
    fn known(self) -> Option<String> {
        match self {
            Self::Known(name) => Some(name),
            Self::Missing | Self::Unknown => None,
        }
    }
}

impl<'a> Names<'a> {
    pub(super) fn new(
        owners: &'a [Option<String>],
        extensions: &'a [bool],
        aliases: &'a [(String, Option<String>)],
    ) -> Self {
        Self {
            owners,
            extensions,
            aliases,
        }
    }

    pub(super) fn absolute(&self, name: &str) -> Option<String> {
        self.lookup(name, &mut Vec::new()).known()
    }

    pub(super) fn relative(&self, name: &str, scope: Option<&str>) -> Option<String> {
        self.in_scope(name, scope, &mut Vec::new()).known()
    }

    fn in_scope(&self, name: &str, mut scope: Option<&str>, active: &mut Vec<String>) -> Lookup {
        loop {
            let candidate = scope.map_or_else(|| name.to_string(), |p| format!("{p}.{name}"));
            match self.lookup(&candidate, active) {
                Lookup::Missing => {}
                // An existing alias with an unknown target shadows outer names;
                // a nonexistent member path instead permits enclosing lookup.
                found => return found,
            }
            let Some(parent) = scope else {
                return Lookup::Missing;
            };
            scope = parent.rsplit_once('.').map(|(p, _)| p);
        }
    }

    fn lookup(&self, name: &str, active: &mut Vec<String>) -> Lookup {
        if active.iter().any(|path| path == name) {
            return Lookup::Unknown;
        }
        active.push(name.to_string());
        let found = self.lookup_path(name, active);
        active.pop();
        found
    }

    fn lookup_path(&self, name: &str, active: &mut Vec<String>) -> Lookup {
        let (parent, leaf) = match name.rsplit_once('.') {
            Some((prefix, leaf)) => match self.lookup(prefix, active) {
                Lookup::Known(parent) => (Some(parent), leaf),
                missing => return missing,
            },
            None => (None, name),
        };
        for (scope, owner) in self.owners.iter().enumerate() {
            if !self.extensions[scope]
                && let Some(owner) = owner
                && self.declared_here(owner, parent.as_deref(), leaf, active)
            {
                return Lookup::Known(
                    parent.map_or_else(|| leaf.to_string(), |p| format!("{p}.{leaf}")),
                );
            }
        }
        for (alias, target) in self.aliases {
            if self.declared_here(alias, parent.as_deref(), leaf, active) {
                let Some(target) = target else {
                    return Lookup::Unknown;
                };
                return match self.in_scope(target, parent.as_deref(), active) {
                    Lookup::Missing => Lookup::Unknown,
                    found => found,
                };
            }
        }
        Lookup::Missing
    }

    fn declared_here(
        &self,
        declaration: &str,
        parent: Option<&str>,
        leaf: &str,
        active: &mut Vec<String>,
    ) -> bool {
        match declaration.rsplit_once('.') {
            Some((scope, name)) if name == leaf && parent.is_some() => {
                matches!(self.lookup(scope, active), Lookup::Known(scope) if Some(scope.as_str()) == parent)
            }
            None => parent.is_none() && declaration == leaf,
            _ => false,
        }
    }
}
