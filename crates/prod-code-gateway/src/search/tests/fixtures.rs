/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::index::SearchIndexes;
use super::super::parser::declarations_in;
use super::super::types::{Found, Indexed};
use crate::embed::Embed;
use prod_code_protocol::{SearchRequest, SearchResponse};
use std::path::Path;
use std::time::Instant;

pub(crate) fn index_decls(rel: &str, text: &str) -> Vec<Indexed> {
    declarations_in(rel, text)
        .into_iter()
        .map(Indexed::new)
        .collect()
}

pub(crate) fn scoped_request(workspace: &str, query: &str, subpath: Option<&str>) -> SearchRequest {
    SearchRequest {
        client_workspace_root: "/client/workspace".into(),
        base_workspace_name: Some(workspace.into()),
        query: query.into(),
        limit: 20,
        subpath: subpath.map(str::to_owned),
        client_agent: None,
        client_host: None,
    }
}

pub(crate) fn hit_files(response: &SearchResponse) -> Vec<String> {
    response.hits.iter().map(|hit| hit.file.clone()).collect()
}

pub(crate) fn write_scope_fixture(root: &Path) {
    for (rel, source) in [
        (
            "src/foo/nested.rs",
            "/// Scope target in the requested directory.\npub fn in_directory() {}\n",
        ),
        (
            "src/foo.rs",
            "/// Scope target in a sibling file.\npub fn sibling_file() {}\n",
        ),
        (
            "src/foobar/outside.rs",
            "/// Scope target in a sibling string prefix.\npub fn sibling_prefix() {}\n",
        ),
        (
            "src/füß/δ.rs",
            "/// Scope target in a Unicode directory.\npub fn unicode_directory() {}\n",
        ),
    ] {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
    }
}

/// Stands in for the model: a text's vector counts the concepts its words belong to, so
/// "restore the connection" and "re-establishes the socket" meet without sharing a word.
pub(crate) struct Concepts;

pub(crate) fn concept_vector(text: &str) -> Vec<f32> {
    const CONCEPTS: [&[&str]; 3] = [
        &["reconnect", "restore", "re-establish", "again"],
        &["socket", "connection", "link", "websocket"],
        &["color", "colour", "hex", "paint"],
    ];
    let lower = text.to_lowercase();
    let mut v: Vec<f32> = CONCEPTS
        .iter()
        .map(|words| words.iter().filter(|w| lower.contains(*w)).count() as f32)
        .collect();
    v.push(0.1);
    crate::embed::normalize(&mut v);
    v
}

impl Embed for Concepts {
    fn passages(&mut self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| concept_vector(t)).collect())
    }

    fn query(&mut self, text: &str) -> anyhow::Result<Vec<f32>> {
        Ok(concept_vector(text))
    }
}

/// Waits for the background pass, which the first search starts.
pub(crate) fn embedded(indexes: &SearchIndexes, root: &Path, query: &str) -> Found {
    let deadline = Instant::now() + std::time::Duration::from_secs(10);
    loop {
        indexes.embed_pending(root, 100);
        let found = indexes.search(root, query, 5, None);
        let dense = found.dense.as_ref().expect("there is a model");
        if dense.embedded == found.declarations || Instant::now() > deadline {
            return found;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

pub(crate) enum BatchPhase {
    Started,
    Finished,
}

pub(crate) struct Blocking {
    pub(crate) phase: std::sync::mpsc::Sender<BatchPhase>,
    pub(crate) release: std::sync::mpsc::Receiver<()>,
}

pub(crate) const BLOCK: std::time::Duration = std::time::Duration::from_secs(30);

impl Embed for Blocking {
    fn passages(&mut self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        let _ = self.phase.send(BatchPhase::Started);
        let _ = self.release.recv_timeout(BLOCK);
        let _ = self.phase.send(BatchPhase::Finished);
        Ok(texts.iter().map(|t| concept_vector(t)).collect())
    }

    fn query(&mut self, text: &str) -> anyhow::Result<Vec<f32>> {
        Ok(concept_vector(text))
    }
}

/// Releases a blocked batch even if an assertion fails.
pub(crate) struct ReleaseOnDrop(pub(crate) Option<std::sync::mpsc::Sender<()>>);

impl ReleaseOnDrop {
    pub(crate) fn release(&self) {
        self.0
            .as_ref()
            .expect("the release sender is present")
            .send(())
            .expect("the background batch is waiting");
    }
}

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        if let Some(release) = &self.0 {
            let _ = release.send(());
        }
    }
}

pub(crate) fn wait_for_background(indexes: &SearchIndexes, root: &Path) {
    let deadline = Instant::now() + std::time::Duration::from_secs(10);
    while indexes
        .inner
        .embedding
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(root)
    {
        assert!(Instant::now() < deadline, "the background batch finished");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

pub(crate) fn wait_for_first_publication(indexes: &SearchIndexes, root: &Path) -> Found {
    let deadline = Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let found = indexes.search(root, "restore connection", 5, None);
        if found.dense.as_ref().is_some_and(|dense| dense.embedded > 0) {
            return found;
        }
        assert!(
            Instant::now() < deadline,
            "the first embedding publication completed"
        );
        std::thread::yield_now();
    }
}

pub(crate) fn restore_modified(path: &Path, modified: std::time::SystemTime) {
    std::fs::File::open(path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
}
