/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! #2018: an editor's diagnostics pass is made with the engine and run without it, so the lock
//! that guards the engine is free while a large file is checked.

use crate::RustEngine;

const SOURCE: &str =
    "pub fn first() -> u32 { let _: u32 = \"text\"; 1 }\npub fn second() -> u32 { 2 }\n";

#[test]
fn a_pass_made_from_the_engine_runs_without_it_and_a_write_cancels_it() {
    let (_temp, path) = super::create_test_fixture();
    std::fs::write(&path, SOURCE).unwrap();
    let mut engine = RustEngine::load(path.parent().unwrap().parent().unwrap()).unwrap();
    let expected = engine.editor_diagnostics(&path).unwrap();
    assert!(
        expected
            .as_array()
            .is_some_and(|all| all.iter().any(|d| d["code"] == "E0308")),
        "{expected}"
    );

    // The pass keeps nothing borrowed from the engine: it is run on another thread while this
    // one still holds the engine, as a session does once it has let go of the engine's lock.
    let pass = engine.editor_diagnostics_pass(&path);
    let ran = std::thread::spawn(move || pass.run()).join().unwrap();
    assert_eq!(ran.unwrap(), expected);

    // A write that arrives before the pass runs leaves it nothing to answer from. The write
    // waits for the pass to let go of the database, so the pass runs on its own thread, a
    // moment after the write has begun; its error is the one a caller takes as "a newer edit
    // has a pass of its own".
    let stale = engine.editor_diagnostics_pass(&path);
    let late = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        stale.run()
    });
    engine
        .apply_file_change(&path, SOURCE.replace("\"text\"", "3"))
        .unwrap();
    let error = late
        .join()
        .unwrap()
        .expect_err("a cancelled pass has no answer");
    assert!(crate::is_salsa_cancelled(&error), "{error:#}");

    // And the pass made after the write sees the write.
    let fresh = engine.editor_diagnostics_pass(&path).run().unwrap();
    assert!(
        !fresh
            .as_array()
            .is_some_and(|all| all.iter().any(|d| d["code"] == "E0308")),
        "{fresh}"
    );
}
