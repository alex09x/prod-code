/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::{HashMap, HashSet, VecDeque, hash_map::Entry};
use std::io::Read;
use std::path::{Path, PathBuf};

#[path = "go_tokens.rs"]
mod go_tokens;
use go_tokens::tokens;

// Startup inspection, not Go's module solver: no downloads, Go commands or external trees.
const MAX_LOCAL_MODULES: usize = 128;
const MAX_FILE_BYTES: u64 = 1024 * 1024;

pub(super) fn read_source(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() <= MAX_FILE_BYTES as usize)
        .then(|| String::from_utf8(bytes).ok())
        .flatten()
}

struct Replacement {
    name: String,
    version: Option<String>,
    target: String,
}

#[derive(Default)]
struct Manifest {
    module: Option<String>,
    go: Option<String>,
    required: HashMap<String, String>,
    replacements: Vec<Replacement>,
}

impl Manifest {
    fn pruned(&self) -> bool {
        self.go
            .as_deref()
            .is_some_and(|go| version_cmp(go, "1.17").is_ge())
    }
}

/// One selected module owns its replacement table. Dependency modules contribute requirements
/// and imports, but their own replace directives do not apply to the main module's build.
pub(super) struct LocalDependencies {
    root: PathBuf,
    boundary: PathBuf,
    module: Option<String>,
    required: HashMap<String, String>,
    replacements: Vec<Replacement>,
    visited: HashSet<PathBuf>,
}

impl LocalDependencies {
    pub(super) fn new(root: &Path, boundary: &Path) -> Self {
        let manifest = manifest(root, boundary);
        let pruned = manifest.pruned();
        let mut dependencies = Self {
            root: root.to_path_buf(),
            boundary: boundary.to_path_buf(),
            module: manifest.module,
            required: manifest.required,
            replacements: manifest.replacements,
            visited: HashSet::from([root.to_path_buf()]),
        };
        dependencies.resolve_requirements(pruned);
        dependencies
    }

    /// Resolve contained manifest requirements before any source can report an OS constraint.
    /// In particular, an imported bridge may upgrade a version-specific replacement that would
    /// otherwise have been scanned first. Modern graphs load each main requirement's manifest,
    /// but only legacy dependencies retain their full transitive closure (Go 1.17 pruning).
    /// Unpruned edges retain their module-version vertices even when that version loses selection.
    fn resolve_requirements(&mut self, pruned: bool) {
        let roots: HashSet<_> = self.required.keys().cloned().collect();
        let mut pending: VecDeque<_> = self
            .required
            .iter()
            .map(|(name, version)| (name.clone(), version.clone(), !pruned))
            .collect();
        let mut manifests = HashSet::new();
        while let Some((name, version, full)) = pending.pop_front() {
            let Some(target) = self.local_target(&name, &version) else {
                continue;
            };
            // Wildcard replacements can share a manifest across versions. Its edges are the
            // same, but a later unpruned visit still has to expand the transitive closure.
            if !manifests.insert((target.clone(), full)) {
                continue;
            }
            if manifests.len() > MAX_LOCAL_MODULES {
                return;
            }
            let manifest = manifest(&target, &self.boundary);
            let full = full || !manifest.pruned();
            for (name, version) in manifest.required {
                let upgraded = match self.required.entry(name.clone()) {
                    Entry::Occupied(mut entry) => {
                        if version_cmp(&version, entry.get()).is_gt() {
                            entry.insert(version.clone());
                            true
                        } else {
                            false
                        }
                    }
                    Entry::Vacant(entry) => {
                        entry.insert(version.clone());
                        true
                    }
                };
                // An unselected version can contribute requirements in an unpruned graph.
                // Pruned roots only need newly selected root versions' manifests.
                if full || (upgraded && roots.contains(&name)) {
                    pending.push_back((name, version, full));
                }
            }
        }
    }

    fn local_target(&self, name: &str, version: &str) -> Option<PathBuf> {
        let replacement = self
            .replacements
            .iter()
            .find(|r| r.name == name && r.version.as_deref() == Some(version))
            .or_else(|| {
                self.replacements
                    .iter()
                    .find(|r| r.name == name && r.version.is_none())
            })?;
        let target = Path::new(&replacement.target);
        if !target.is_absolute() && !target.starts_with(".") && !target.starts_with("..") {
            return None;
        }
        // Canonical containment rejects ../ escapes and symlinks into external trees.
        let target = self.root.join(target).canonicalize().ok()?;
        (target.starts_with(&self.boundary) && target.join("go.mod").is_file()).then_some(target)
    }

    pub(super) fn imported_roots(&mut self, source: &str) -> Vec<PathBuf> {
        let mut imports = HashSet::new();
        let tokens = tokens(source);
        let mut at = 0;
        while at < tokens.len() {
            if tokens[at].0 != "import" || tokens[at].1 {
                at += 1;
                continue;
            }
            at += 1;
            // The import keyword itself does not trigger semicolon insertion.
            while tokens.get(at).is_some_and(|(s, q)| s == "\n" && !q) {
                at += 1;
            }
            let block = tokens.get(at).is_some_and(|(s, q)| s == "(" && !q);
            if block {
                at += 1;
            }
            loop {
                if block {
                    while tokens
                        .get(at)
                        .is_some_and(|(s, q)| !q && matches!(s.as_ref(), "\n" | ";"))
                    {
                        at += 1;
                    }
                }
                let Some((text, quoted)) = tokens.get(at) else {
                    break;
                };
                if !quoted {
                    if matches!(text.as_ref(), "\n" | ";" | "(" | ")") {
                        break;
                    }
                    at += 1; // Optional package name, blank identifier or dot.
                }
                let Some((path, true)) = tokens.get(at) else {
                    break;
                };
                imports.insert(path.as_ref());
                at += 1;
                // One path per specification; declarations after it are never imports.
                if !block
                    || !tokens
                        .get(at)
                        .is_some_and(|(s, q)| !q && matches!(s.as_ref(), "\n" | ";"))
                {
                    break;
                }
            }
        }
        let mut roots = Vec::new();
        let imported: HashSet<_> = imports
            .iter()
            .filter_map(|import| {
                self.required
                    .keys()
                    .chain(self.module.iter())
                    .filter(|name| {
                        *import == name.as_str()
                            || import
                                .strip_prefix(name.as_str())
                                .is_some_and(|s| s.starts_with('/'))
                    })
                    .max_by_key(|name| name.len())
                    .filter(|name| self.module.as_ref() != Some(*name))
                    .cloned()
            })
            .collect();
        for name in imported {
            if self.visited.len() >= MAX_LOCAL_MODULES {
                return roots;
            }
            let Some(target) = self.local_target(&name, &self.required[&name]) else {
                continue;
            };
            if !self.visited.insert(target.clone()) {
                continue;
            }
            roots.push(target);
        }
        roots
    }
}

/// Go module versions use semantic ordering, including numeric prerelease identifiers and
/// pseudo-version timestamps. Build metadata (such as +incompatible) does not affect ordering.
fn version_cmp(left: &str, right: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    fn number(left: &str, right: &str) -> Ordering {
        left.len().cmp(&right.len()).then_with(|| left.cmp(right))
    }
    fn parts(version: &str) -> (&str, Option<&str>) {
        let version = version
            .trim_start_matches('v')
            .split('+')
            .next()
            .unwrap_or("");
        version
            .split_once('-')
            .map_or((version, None), |(base, pre)| (base, Some(pre)))
    }
    let ((left, left_pre), (right, right_pre)) = (parts(left), parts(right));
    let mut left = left.split('.');
    let mut right = right.split('.');
    for _ in 0..3 {
        let order = number(left.next().unwrap_or("0"), right.next().unwrap_or("0"));
        if !order.is_eq() {
            return order;
        }
    }
    let (Some(left), Some(right)) = (left_pre, right_pre) else {
        return left_pre.is_none().cmp(&right_pre.is_none());
    };
    let mut left = left.split('.');
    let mut right = right.split('.');
    loop {
        let (a, b) = match (left.next(), right.next()) {
            (Some(a), Some(b)) => (a, b),
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
        };
        let numeric = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
        let order = match (numeric(a), numeric(b)) {
            (true, true) => number(a, b),
            (false, false) => a.cmp(b),
            (a, b) => b.cmp(&a),
        };
        if !order.is_eq() {
            return order;
        }
    }
}

fn manifest(root: &Path, boundary: &Path) -> Manifest {
    let mut manifest = Manifest::default();
    let source = root
        .join("go.mod")
        .canonicalize()
        .ok()
        .filter(|p| p.starts_with(boundary))
        .and_then(|p| read_source(&p))
        .unwrap_or_default();
    let tokens = tokens(&source);
    let mut block = None;
    for line in tokens
        .split(|(s, quoted)| !quoted && matches!(s.as_ref(), "\n" | ";"))
        .take(1024)
    {
        let mut words: Vec<_> = line.iter().map(|(s, _)| s.as_ref()).collect();
        if words.is_empty() {
            continue;
        }
        if words[0] == ")" {
            block = None;
            continue;
        }
        let directive = block.unwrap_or_else(|| words.remove(0));
        if words.first() == Some(&"(") {
            block = Some(directive);
            words.remove(0);
        }
        if words.last() == Some(&")") {
            block = None;
            words.pop();
        }
        match (directive, words.as_slice()) {
            ("module", [name]) => manifest.module = Some((*name).into()),
            ("go", [version]) => manifest.go = Some((*version).into()),
            ("require", [name, version]) => {
                manifest
                    .required
                    .insert((*name).to_owned(), (*version).to_owned());
            }
            ("replace", [name, "=>", target] | [name, "=>", target, _]) => {
                manifest.replacements.push(Replacement {
                    name: (*name).into(),
                    version: None,
                    target: (*target).into(),
                })
            }
            ("replace", [name, version, "=>", target] | [name, version, "=>", target, _]) => {
                manifest.replacements.push(Replacement {
                    name: (*name).into(),
                    version: Some((*version).into()),
                    target: (*target).into(),
                })
            }
            _ => {}
        }
    }
    manifest
}
