/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::collect::{canonical_root, collect_source_files, normalize_relative_path, source_path};
use super::graph::WorkspaceIndex;
use super::parser::{declarations_in, language_of};
use super::scoring::rank_with;
use super::types::{EMBED_BATCH, FileEntry, Found, Indexed, MAX_FILE_BYTES};
use crate::embed::Embed;
use prod_code_protocol::DenseStatus;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// The embedding model: not looked for yet, loaded, or known to be missing.
pub(crate) enum Model {
    Unloaded(PathBuf),
    Ready(Box<dyn Embed>),
    Missing,
}

/// Every workspace's index, built lazily and kept until the gateway stops. Cloning shares it.
#[derive(Clone)]
pub struct SearchIndexes {
    pub(crate) inner: Arc<Inner>,
}

/// The model is held twice, once for questions and once for the background pass, because a
/// search must never wait for that pass: a batch of `EMBED_BATCH` declarations keeps its model
/// busy for hundreds of milliseconds, seconds on a loaded node, and one shared model made every
/// question wait for the batch in progress. Handing one model over between smaller batches
/// would still make a question wait for the batch that holds it. The price of a second session
/// is one more copy of the model in memory (tens of megabytes for BGE-small), loaded from the
/// same directory.
pub(crate) struct Inner {
    pub(crate) by_workspace: Mutex<HashMap<PathBuf, WorkspaceIndex>>,
    /// Embeds questions; only a search takes it.
    pub(crate) query_model: Mutex<Model>,
    /// Embeds declarations; only the background pass (and `embed_pending`) takes it.
    pub(crate) passage_model: Mutex<Model>,
    /// Workspaces a background pass is embedding right now.
    pub(crate) embedding: Mutex<HashSet<PathBuf>>,
    /// Unique file-entry identities, including across workspace eviction and recreation.
    pub(crate) next_generation: AtomicU64,
}

impl Default for SearchIndexes {
    fn default() -> Self {
        Self::with(Model::Missing, Model::Missing)
    }
}

impl SearchIndexes {
    /// Lexical search only.
    pub fn new() -> Self {
        Self::default()
    }

    /// Lexical and dense search, with the model in `dir` loaded on the first query. A missing or
    /// broken model leaves the search lexical.
    pub fn with_model_dir(dir: PathBuf) -> Self {
        Self::with(Model::Unloaded(dir.clone()), Model::Unloaded(dir))
    }

    /// Lexical and dense search with these embedders: one for questions, one for declarations.
    /// They must be the same model, or the two kinds of vector do not compare.
    pub fn with_embedders(query: Box<dyn Embed>, passages: Box<dyn Embed>) -> Self {
        Self::with(Model::Ready(query), Model::Ready(passages))
    }

    fn with(query: Model, passages: Model) -> Self {
        Self {
            inner: Arc::new(Inner {
                by_workspace: Mutex::new(HashMap::new()),
                query_model: Mutex::new(query),
                passage_model: Mutex::new(passages),
                embedding: Mutex::new(HashSet::new()),
                next_generation: AtomicU64::new(0),
            }),
        }
    }

    /// Refreshes the workspace's index against the files on disk and runs the query, both
    /// ways when there is a model. Declarations without a vector are embedded in the
    /// background afterwards.
    pub fn search(&self, root: &Path, query: &str, limit: usize, subpath: Option<&str>) -> Found {
        let root = canonical_root(root);
        let query_vector = self.inner.query_vector(query);
        let found = {
            let mut guard = self.inner.lock_indexes();
            let index = guard.entry(root.clone()).or_default();
            if !index.built {
                refresh(&root, index, &self.inner.next_generation);
                index.built = true;
            } else if !index.pending.is_empty() {
                let pending = std::mem::take(&mut index.pending);
                for rel in pending {
                    reindex_one(&root, index, &rel, &self.inner.next_generation);
                }
                index.rebuild_graph();
            }
            let embedded = index.declarations().filter(|d| d.vector.is_some()).count();
            let with_vectors = query_vector.as_deref().filter(|_| embedded > 0);
            Found {
                hits: rank_with(index, query, with_vectors, limit, subpath),
                files: index.files.len(),
                declarations: index.len(),
                dense: query_vector.as_ref().map(|_| DenseStatus {
                    used: embedded > 0,
                    embedded,
                }),
                graph_fused: true,
            }
        };
        if found
            .dense
            .as_ref()
            .is_some_and(|d| d.embedded < found.declarations)
        {
            self.embed_in_background(&root);
        }
        found
    }

    /// Starts a background pass that embeds every declaration of the workspace still without a
    /// vector, unless one is already running for it.
    fn embed_in_background(&self, root: &Path) {
        {
            let mut running = self
                .inner
                .embedding
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if !running.insert(root.to_path_buf()) {
                return;
            }
        }
        let inner = Arc::clone(&self.inner);
        let root = root.to_path_buf();
        std::thread::spawn(move || {
            let started = Instant::now();
            let mut done = 0usize;
            loop {
                let n = inner.embed_pending(&root, EMBED_BATCH);
                if n == 0 {
                    break;
                }
                done += n;
            }
            tracing::info!(
                "embedded {done} declaration(s) of {} in {:.1}s",
                root.display(),
                started.elapsed().as_secs_f64()
            );
            inner
                .embedding
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&root);
        });
    }

    /// Embeds up to `max` declarations of the workspace that have no vector yet, in the calling
    /// thread. Returns how many got one.
    pub fn embed_pending(&self, root: &Path, max: usize) -> usize {
        self.inner.embed_pending(&canonical_root(root), max)
    }

    /// Records that these workspace-relative paths were written or deleted, so the next query
    /// reindexes exactly them instead of walking the tree.
    pub fn invalidate<I, S>(&self, root: &Path, paths: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let root = canonical_root(root);
        let mut guard = self.inner.lock_indexes();
        let Some(index) = guard.get_mut(&root) else {
            return;
        };
        for path in paths {
            let Some(rel) = normalize_relative_path(path.as_ref()) else {
                continue;
            };
            if language_of(rel.rsplit('/').next().unwrap_or(&rel)).is_some() {
                index.pending.push(rel);
            }
        }
    }

    /// Drops a workspace's index (its engine was evicted or its directory pruned).
    pub fn forget(&self, root: &Path) {
        self.inner.lock_indexes().remove(&canonical_root(root));
    }
}

impl Inner {
    pub(crate) fn lock_indexes(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<PathBuf, WorkspaceIndex>> {
        self.by_workspace.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn query_vector(&self, query: &str) -> Option<Vec<f32>> {
        match &mut *loaded(&self.query_model, "questions") {
            Model::Ready(model) => model
                .query(query)
                .map_err(|err| tracing::warn!("embedding the query failed: {err:#}"))
                .ok(),
            _ => None,
        }
    }

    pub(crate) fn embed_pending(&self, root: &Path, max: usize) -> usize {
        // What to embed, taken under the index lock and computed without it, so queries go on.
        let batch: Vec<(String, u64, usize, String)> = {
            let guard = self.lock_indexes();
            let Some(index) = guard.get(root) else {
                return 0;
            };
            index
                .files
                .iter()
                .flat_map(|(rel, entry)| {
                    entry
                        .decls
                        .iter()
                        .enumerate()
                        .filter(|(_, d)| d.vector.is_none())
                        .map(move |(i, d)| (rel.clone(), entry.generation, i, d.passage()))
                })
                .take(max)
                .collect()
        };
        if batch.is_empty() {
            return 0;
        }
        let texts: Vec<String> = batch.iter().map(|(_, _, _, text)| text.clone()).collect();
        let vectors = match &mut *loaded(&self.passage_model, "declarations") {
            Model::Ready(model) => match model.passages(&texts) {
                Ok(vectors) => vectors,
                Err(err) => {
                    tracing::warn!("embedding declarations failed: {err:#}");
                    return 0;
                }
            },
            _ => return 0,
        };
        let mut guard = self.lock_indexes();
        let Some(index) = guard.get_mut(root) else {
            return 0;
        };
        let mut installed = 0;
        for ((rel, generation, i, _), vector) in batch.into_iter().zip(vectors) {
            // A replacement entry has new declarations; they wait for the next batch.
            if let Some(entry) = index.files.get_mut(&rel)
                && entry.generation == generation
                && let Some(decl) = entry.decls.get_mut(i)
            {
                decl.vector = Some(vector);
                installed += 1;
            }
        }
        installed
    }
}

/// The model in `slot`, loaded on first use; `purpose` names it in the log. `Missing` when there
/// is none or it failed; the failure is logged once per slot and the search stays lexical.
fn loaded<'a>(slot: &'a Mutex<Model>, purpose: &str) -> std::sync::MutexGuard<'a, Model> {
    let mut model = slot.lock().unwrap_or_else(|e| e.into_inner());
    if let Model::Unloaded(dir) = &*model {
        *model = match crate::embed::OnnxEmbedder::load(dir) {
            Ok(loaded) => {
                tracing::info!(
                    "embedding model for {purpose} loaded from {}",
                    dir.display()
                );
                Model::Ready(Box::new(loaded))
            }
            Err(err) => {
                tracing::warn!("no dense search: {err:#}");
                Model::Missing
            }
        };
    }
    model
}

/// Walks the workspace copy and reindexes files whose stamp changed.
pub(crate) fn refresh(root: &Path, index: &mut WorkspaceIndex, generations: &AtomicU64) {
    let mut present = Vec::new();
    collect_source_files(root, root, &mut present);
    let mut seen: HashMap<String, ()> = HashMap::with_capacity(present.len());
    for (rel, path) in present {
        seen.insert(rel.clone(), ());
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if meta.len() > MAX_FILE_BYTES {
            continue;
        }
        let stamp = (
            meta.len(),
            meta.modified()
                .ok()
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0),
        );
        if index.files.get(&rel).map(|f| f.stamp) == Some(stamp) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let decls = declarations_in(&rel, &text)
            .into_iter()
            .map(Indexed::new)
            .collect();
        index.files.insert(
            rel,
            FileEntry {
                stamp,
                generation: generations.fetch_add(1, Ordering::Relaxed),
                decls,
            },
        );
    }
    index.files.retain(|rel, _| seen.contains_key(rel));
    index.rebuild_graph();
}

/// Reindexes one file after the sync layer wrote or removed it.
pub(crate) fn reindex_one(
    root: &Path,
    index: &mut WorkspaceIndex,
    rel: &str,
    generations: &AtomicU64,
) {
    let Some(path) = source_path(root, rel) else {
        index.files.remove(rel);
        return;
    };
    let Ok(meta) = std::fs::metadata(&path) else {
        index.files.remove(rel);
        return;
    };
    if meta.len() > MAX_FILE_BYTES {
        index.files.remove(rel);
        return;
    }
    let stamp = (
        meta.len(),
        meta.modified()
            .ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0),
    );
    let Ok(text) = std::fs::read_to_string(&path) else {
        index.files.remove(rel);
        return;
    };
    let decls = declarations_in(rel, &text)
        .into_iter()
        .map(Indexed::new)
        .collect();
    index.files.insert(
        rel.to_string(),
        FileEntry {
            stamp,
            generation: generations.fetch_add(1, Ordering::Relaxed),
            decls,
        },
    );
}
