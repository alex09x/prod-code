/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn updates_apply_valid_utf8_preserve_the_base_on_unreadable_input_and_defer_busy_engines() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='sync_fixture'\nversion='0.1.0'\nedition='2021'\n[lib]\npath='lib.rs'\n",
    )
    .unwrap();
    std::fs::write(
        root.join("prod-code.toml"),
        "[rust]\nbuild_scripts=false\nproc_macro_srv='disabled'\n",
    )
    .unwrap();
    let path = root.join("lib.rs");
    std::fs::write(&path, "pub fn original() {}\n").unwrap();
    let engine = Arc::new(tokio::sync::Mutex::new(
        prod_code_engine_rust::RustEngine::load(root).unwrap(),
    ));
    let names = |engine: &prod_code_engine_rust::RustEngine| {
        engine
            .document_symbols(&path)
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&*engine.lock().await), vec!["original"]);
    std::fs::write(&path, [0xff]).unwrap();
    assert!(
        push_base_updates(
            vec![Arc::clone(&engine)],
            vec![path.clone()],
            Duration::from_secs(2)
        )
        .await
    );
    assert_eq!(names(&*engine.lock().await), vec!["original"]);
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(
        push_base_updates(
            vec![Arc::clone(&engine)],
            vec![path.clone()],
            Duration::from_secs(2)
        )
        .await
    );
    assert_eq!(names(&*engine.lock().await), vec!["original"]);
    std::fs::remove_dir(&path).unwrap();
    std::fs::write(&path, "pub fn updated() {}\n").unwrap();
    assert!(
        push_base_updates(
            vec![Arc::clone(&engine)],
            vec![path.clone()],
            Duration::from_secs(2)
        )
        .await
    );
    assert_eq!(names(&*engine.lock().await), vec!["updated"]);
    let guard = engine.lock().await;
    std::fs::write(&path, "pub fn deferred() {}\n").unwrap();
    assert!(
        !push_base_updates(
            vec![Arc::clone(&engine)],
            vec![path.clone()],
            Duration::ZERO
        )
        .await
    );
    assert_eq!(names(&guard), vec!["updated"]);
    // Deferred updates read the latest disk state, rather than retaining stale text.
    std::fs::write(&path, "pub fn latest() {}\n").unwrap();
    drop(guard);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if names(&*engine.lock().await) == vec!["latest"] {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(push_base_updates(vec![], vec![path.clone()], Duration::ZERO).await);
    assert!(push_base_updates(vec![engine], vec![], Duration::ZERO).await);
}
