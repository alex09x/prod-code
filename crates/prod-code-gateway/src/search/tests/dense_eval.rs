/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::graph::WorkspaceIndex;
use super::super::index::SearchIndexes;
use super::super::runner::run_search;
use super::super::scoring::{rank, rank_with};
use super::super::types::FileEntry;
use super::fixtures::{
    BatchPhase, Blocking, Concepts, ReleaseOnDrop, concept_vector, embedded, index_decls,
    restore_modified, scoped_request, wait_for_background, wait_for_first_publication,
};
use std::path::Path;

#[test]
fn a_question_sharing_no_words_is_found_by_meaning_once_embedded() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(
        root.join("net.rs"),
        "/// Re-establishes the socket after the link drops.\npub fn reconnect_on_close() {}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("paint.rs"),
        "/// Reads a hex colour such as #ff8800.\npub fn parse_colour(s: &str) {}\n",
    )
    .unwrap();
    let question = "restore the connection when the server goes away";

    let lexical_only = SearchIndexes::new().search(root, question, 5, None);
    assert!(lexical_only.hits.is_empty());
    assert!(lexical_only.dense.is_none());

    let indexes = SearchIndexes::with_embedders(Box::new(Concepts), Box::new(Concepts));
    let first = indexes.search(root, question, 5, None);
    assert_eq!(first.declarations, 2);
    let found = embedded(&indexes, root, question);
    let dense = found.dense.as_ref().unwrap();
    assert!(dense.used);
    assert_eq!(dense.embedded, 2);
    assert_eq!(found.hits[0].name, "reconnect_on_close", "{:?}", found.hits);

    // A file that changes loses its vectors until the next pass embeds it again.
    std::fs::write(
        root.join("net.rs"),
        "/// Re-establishes the socket after the link drops, again.\npub fn reconnect_on_close() {}\npub fn close() {}\n",
    )
    .unwrap();
    indexes.invalidate(root, ["net.rs"]);
    let found = embedded(&indexes, root, question);
    assert_eq!(found.declarations, 3);
    assert_eq!(found.dense.as_ref().unwrap().embedded, 3);
    assert_eq!(found.hits[0].name, "reconnect_on_close");

    // Nothing to embed once every declaration has its vector, and nothing for an unknown
    // workspace.
    assert_eq!(indexes.embed_pending(root, 100), 0);
    assert_eq!(
        indexes.embed_pending(Path::new("/no/such/workspace"), 100),
        0
    );
    indexes.forget(root);
    assert_eq!(indexes.embed_pending(root, 100), 0);
}

#[cfg(unix)]
#[test]
fn search_skips_linked_sources_cycles_and_unsafe_invalidations() {
    use std::os::unix::fs::symlink;

    let storage = tempfile::tempdir().unwrap();
    let checkout = storage.path().join("checkout");
    let outside = storage.path().join("outside");
    let outside_file = outside.join("sentinel.rs");
    let outside_source =
        "/// Outside sentinel must never be indexed.\npub fn outside_sentinel() {}\n";
    std::fs::create_dir_all(checkout.join("nested")).unwrap();
    std::fs::create_dir_all(outside.join("ancestor")).unwrap();
    std::fs::write(
        checkout.join("plain.rs"),
        "/// Ordinary declaration remains searchable.\npub fn ordinary_source() {}\n",
    )
    .unwrap();
    std::fs::write(
        checkout.join("nested/child.rs"),
        "/// Stale ancestor declaration.\npub fn stale_ancestor() {}\n",
    )
    .unwrap();
    std::fs::write(&outside_file, outside_source).unwrap();
    std::fs::write(
        outside.join("ancestor/child.rs"),
        "/// Outside ancestor sentinel.\npub fn outside_ancestor() {}\n",
    )
    .unwrap();
    symlink(&outside_file, checkout.join("linked.rs")).unwrap();
    symlink(&outside, checkout.join("linked_dir")).unwrap();
    symlink(&checkout, checkout.join("loop")).unwrap();

    let indexes = SearchIndexes::new();
    let request = scoped_request("checkout", "outside sentinel", None);
    let (sent, received) = std::sync::mpsc::channel();
    let search_indexes = indexes.clone();
    let storage_root = storage.path().to_path_buf();
    std::thread::spawn(move || {
        let _ = sent.send(run_search(&search_indexes, &storage_root, &request));
    });
    let response = received
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("linked directory cycle finishes within the watchdog");
    assert!(response.hits.is_empty());
    assert_eq!(response.indexed_files, 2);
    assert_eq!(response.indexed_declarations, 2);

    let alias = storage.path().join("checkout-alias");
    symlink(&checkout, &alias).unwrap();
    let ordinary = indexes.search(&alias, "ordinary declaration", 5, None);
    assert_eq!(ordinary.hits[0].name, "ordinary_source");

    symlink(&outside_file, checkout.join("plain.rs.new")).unwrap();
    std::fs::remove_file(checkout.join("plain.rs")).unwrap();
    std::fs::rename(checkout.join("plain.rs.new"), checkout.join("plain.rs")).unwrap();
    indexes.invalidate(
        &checkout,
        ["plain.rs", "../outside/sentinel.rs", "/tmp/nope.rs"],
    );
    let linked = indexes.search(&checkout, "ordinary declaration", 5, None);
    assert!(linked.hits.iter().all(|hit| hit.name != "ordinary_source"));
    assert_eq!(
        linked.files, 1,
        "linked file remained indexed: {:?}",
        linked.hits
    );

    std::fs::remove_dir_all(checkout.join("nested")).unwrap();
    symlink(outside.join("ancestor"), checkout.join("nested")).unwrap();
    indexes.invalidate(&checkout, ["nested/child.rs"]);
    let ancestor = indexes.search(&checkout, "stale ancestor", 5, None);
    assert!(
        ancestor.hits.is_empty(),
        "linked ancestor left stale hits: {:?}",
        ancestor.hits
    );
    assert_eq!(
        std::fs::read_to_string(&outside_file).unwrap(),
        outside_source
    );

    let dense = SearchIndexes::with_embedders(Box::new(Concepts), Box::new(Concepts));
    let first = dense.search(&checkout, "outside sentinel", 5, None);
    assert_eq!(
        first.declarations, 0,
        "linked sources became embedding input"
    );
    wait_for_background(&dense, &checkout);
    assert_eq!(dense.embed_pending(&checkout, 100), 0);
}

#[test]
fn stale_batch_does_not_publish_to_replaced_declarations() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let path = root.join("net.rs");
    let old = "/// Re-establishes the socket after a drop.\npub fn old_socket() {}\n/// Paints the hex colour.\npub fn old_paint() {}\n";
    let mut current =
        "/// Adds a note.\npub fn inserted_item() {}\n/// Re-establishes the socket.\npub fn moved_socket() {}\n".to_string();
    assert!(current.len() <= old.len());
    current.push_str(&" ".repeat(old.len() - current.len()));
    assert_eq!(current.len(), old.len());
    std::fs::write(&path, old).unwrap();
    std::fs::write(
        root.join("stable.rs"),
        "/// Counts the comet.\npub fn stable_item() {}\n",
    )
    .unwrap();
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    let (phase, batch_phase) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let indexes = SearchIndexes::with_embedders(
        Box::new(Concepts),
        Box::new(Blocking {
            phase,
            release: released,
        }),
    );
    let release = ReleaseOnDrop(Some(release));

    indexes.search(&root, "restore connection", 5, None);
    assert!(matches!(
        batch_phase
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the old batch started"),
        BatchPhase::Started
    ));

    // This is a delete/recreate with the old length and exact modification time. The new
    // declaration also moves the reconnect passage to a different ordinal.
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, &current).unwrap();
    restore_modified(&path, modified);
    indexes.invalidate(&root, ["net.rs"]);
    let reindexed = indexes.search(&root, "restore connection", 5, None);
    assert_eq!(reindexed.declarations, 3);
    assert_eq!(reindexed.dense.as_ref().unwrap().embedded, 0);

    release.release();
    // The old batch either publishes and finishes, or the replacement batch has already
    // begun. In both cases inspect the actual post-publication state before waiting for a
    // replacement batch: without generation guards the stale vectors land right here.
    assert!(matches!(
        batch_phase
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the old batch finished"),
        BatchPhase::Finished
    ));

    // The untouched file keeps its valid vector, but neither old vector can land on the
    // replacement declarations.
    let stale = wait_for_first_publication(&indexes, &root);
    assert_eq!(stale.dense.as_ref().unwrap().embedded, 1);
    assert!(stale.hits.iter().all(|hit| hit.name != "inserted_item"));

    release.release();
    wait_for_background(&indexes, &root);
    let current = embedded(&indexes, &root, "restore connection");
    assert!(current.dense.as_ref().unwrap().used);
    assert_eq!(current.hits[0].name, "moved_socket", "{:?}", current.hits);
}

#[test]
fn stale_batch_does_not_survive_workspace_forget_and_recreation() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let path = root.join("net.rs");
    let old = "/// Re-establishes the socket.\npub fn old_socket() {}\n";
    let mut current = "/// Paints the hex colour.\npub fn new_paint() {}\n".to_string();
    assert!(current.len() <= old.len());
    current.push_str(&" ".repeat(old.len() - current.len()));
    assert_eq!(current.len(), old.len());
    std::fs::write(&path, old).unwrap();
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    let (phase, batch_phase) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let indexes = SearchIndexes::with_embedders(
        Box::new(Concepts),
        Box::new(Blocking {
            phase,
            release: released,
        }),
    );
    let release = ReleaseOnDrop(Some(release));

    indexes.search(&root, "restore connection", 5, None);
    assert!(matches!(
        batch_phase
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the old batch started"),
        BatchPhase::Started
    ));
    indexes.forget(&root);
    std::fs::write(&path, &current).unwrap();
    restore_modified(&path, modified);
    let recreated = indexes.search(&root, "color", 5, None);
    assert_eq!(recreated.declarations, 1);
    assert_eq!(recreated.dense.as_ref().unwrap().embedded, 0);

    release.release();
    wait_for_background(&indexes, &root);
    let stale = indexes.search(&root, "restore connection", 5, None);
    assert!(!stale.hits.iter().any(|hit| hit.name == "new_paint"));

    release.release();
    wait_for_background(&indexes, &root);
    let current = embedded(&indexes, &root, "color");
    assert!(current.dense.as_ref().unwrap().used);
    assert_eq!(current.hits[0].name, "new_paint", "{:?}", current.hits);
}

#[test]
fn a_question_does_not_wait_for_the_batch_being_embedded() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    std::fs::write(
        root.join("net.rs"),
        "/// Re-establishes the socket after the link drops.\npub fn reconnect_on_close() {}\n",
    )
    .unwrap();
    let question = "restore the connection when the server goes away";
    let (phase, batch_phase) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let indexes = SearchIndexes::with_embedders(
        Box::new(Concepts),
        Box::new(Blocking {
            phase,
            release: released,
        }),
    );

    // The first search starts the background pass, whose batch then blocks.
    indexes.search(&root, question, 5, None);
    assert!(matches!(
        batch_phase
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the background pass started a batch"),
        BatchPhase::Started
    ));

    // A question asked meanwhile is answered without waiting for that batch, and says how
    // far the embedding has got.
    let (answer, answered) = std::sync::mpsc::channel();
    {
        let indexes = indexes.clone();
        let root = root.clone();
        std::thread::spawn(move || {
            let _ = answer.send(indexes.search(&root, question, 5, None));
        });
    }
    let found = answered
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("the search returned while a batch was still being embedded");
    let dense = found.dense.as_ref().expect("there is a model");
    assert!(!dense.used);
    assert_eq!(dense.embedded, 0);
    assert_eq!(found.declarations, 1);

    // Released, the batch lands and the next question is ranked by meaning too.
    drop(release);
    let found = embedded(&indexes, &root, question);
    let dense = found.dense.as_ref().unwrap();
    assert!(dense.used);
    assert_eq!(dense.embedded, 1);
    assert_eq!(found.hits[0].name, "reconnect_on_close");
}

#[test]
fn a_declaration_both_halves_find_outranks_one_found_by_one() {
    let mut index = WorkspaceIndex::default();
    let mut decls = index_decls(
        "a.rs",
        "/// Reconnects the websocket.\npub fn reconnect_socket() {}\n/// Reconnects the database pool.\npub fn reconnect_pool() {}\n/// Paints the hex colour.\npub fn paint() {}\n",
    );
    for d in &mut decls {
        d.vector = Some(concept_vector(&d.passage()));
    }
    assert!(decls[0].passage().contains("reconnect socket"));
    assert!(decls[0].passage().contains("Reconnects the websocket."));
    index.files.insert(
        "a.rs".to_string(),
        FileEntry {
            stamp: (0, 0),
            generation: 0,
            decls,
        },
    );
    let query = "reconnect the link";
    let q = concept_vector(query);
    let lexical = rank(&index, query, 5, None);
    assert_eq!(lexical.len(), 2, "both say reconnect: {lexical:?}");
    let fused = rank_with(&index, query, Some(&q), 5, None);
    assert_eq!(fused[0].name, "reconnect_socket", "{fused:?}");
    assert_eq!(
        fused.len(),
        3,
        "the dense half ranks everything with a vector"
    );
    // A question of stopwords alone still has a dense answer.
    assert!(rank(&index, "the", 5, None).is_empty());
    assert!(!rank_with(&index, "the", Some(&q), 5, None).is_empty());
}
