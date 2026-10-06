/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Tests for session overlay isolation, buffer switching, and base updates.

use super::create_test_fixture;
use crate::engine::RustEngine;

#[test]
fn test_session_overlays_do_not_bleed_between_sessions() {
    let (temp, lib_path) = create_test_fixture();
    let mut engine = RustEngine::load(temp.path()).expect("Must load fixture");
    let base = std::fs::read_to_string(&lib_path).unwrap();
    let text_a = format!("{base}pub const ONLY_IN_SESSION_A: u8 = 1;\n");
    let text_b = format!("{base}pub const ONLY_IN_SESSION_B: u8 = 2;\n");

    engine
        .set_session_overlay(1, &lib_path, Some(text_a))
        .unwrap();
    engine
        .set_session_overlay(2, &lib_path, Some(text_b))
        .unwrap();

    let names = |engine: &RustEngine| -> Vec<String> {
        engine
            .document_symbols(&lib_path)
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect()
    };

    engine.activate_session(1).unwrap();
    let a = names(&engine);
    assert!(a.contains(&"ONLY_IN_SESSION_A".to_string()), "{a:?}");
    assert!(!a.contains(&"ONLY_IN_SESSION_B".to_string()), "{a:?}");

    assert_eq!(engine.activate_session(2).unwrap(), 1);
    let b = names(&engine);
    assert!(b.contains(&"ONLY_IN_SESSION_B".to_string()), "{b:?}");
    assert!(!b.contains(&"ONLY_IN_SESSION_A".to_string()), "{b:?}");

    // A session without its own buffer sees the untouched base.
    assert_eq!(engine.activate_session(3).unwrap(), 1);
    let plain = names(&engine);
    assert!(plain.contains(&"DEFAULT_PORT".to_string()));
    assert!(!plain.iter().any(|n| n.starts_with("ONLY_IN_SESSION")));
    assert_eq!(engine.activate_session(3).unwrap(), 0);

    // Closing the buffer returns that session to the base as well.
    engine.clear_session(1).unwrap();
    engine.activate_session(1).unwrap();
    let after_close = names(&engine);
    assert!(!after_close.contains(&"ONLY_IN_SESSION_A".to_string()));
    assert_eq!(engine.session_overlay_count(1), 0);
    assert_eq!(engine.session_overlay_count(2), 1);
}

#[test]
fn test_session_overlay_queries_and_empty_activation() {
    let (temp, lib_path) = create_test_fixture();
    let mut engine = RustEngine::load(temp.path()).expect("Must load fixture");
    assert!(!engine.has_session_overlays());
    assert!(!engine.session_has_overlays(1));
    assert_eq!(engine.activate_session(1).unwrap(), 0);

    let base = std::fs::read_to_string(&lib_path).unwrap();
    let direct_text = format!("{base}\npub fn direct_edit_func() -> u32 {{ 42 }}\n");
    // Direct edit via apply_file_change
    engine.apply_file_change(&lib_path, direct_text).unwrap();
    assert!(!engine.has_session_overlays());
    assert_eq!(engine.activate_session(1).unwrap(), 0);

    let symbols = engine.document_symbols(&lib_path).unwrap();
    assert!(symbols.iter().any(|s| s.name == "direct_edit_func"));

    // Direct reload restores from disk
    engine.reload_file(&lib_path).unwrap();
    assert!(!engine.has_session_overlays());
    let symbols_after_reload = engine.document_symbols(&lib_path).unwrap();
    assert!(
        !symbols_after_reload
            .iter()
            .any(|s| s.name == "direct_edit_func")
    );
}

#[test]
fn test_session_overlay_untracked_file_is_private() {
    let (temp, _lib_path) = create_test_fixture();
    let mut engine = RustEngine::load(temp.path()).expect("Must load fixture");
    let scratch = temp.path().join("src/scratch.rs");
    engine
        .set_session_overlay(7, &scratch, Some("pub fn scratch_only() {}\n".to_string()))
        .unwrap();

    engine.activate_session(7).unwrap();
    let mine = engine.document_symbols(&scratch).unwrap();
    assert!(mine.iter().any(|s| s.name == "scratch_only"));

    engine.activate_session(8).unwrap();
    let theirs = engine.document_symbols(&scratch).unwrap_or_default();
    assert!(theirs.is_empty(), "{theirs:?}");

    engine.activate_session(7).unwrap();
    let again = engine.document_symbols(&scratch).unwrap();
    assert!(again.iter().any(|s| s.name == "scratch_only"));
}

#[test]
fn test_update_base_adds_module_file_missing_at_load() {
    let (temp, lib_path) = create_test_fixture();
    // The crate declares a module whose file does not exist yet when the engine loads.
    let mut lib = std::fs::read_to_string(&lib_path).unwrap();
    lib.push_str("\nmod late_module;\n");
    std::fs::write(&lib_path, lib).unwrap();
    let mut engine = RustEngine::load(temp.path()).expect("Must load fixture");

    let late = temp.path().join("src/late_module.rs");
    std::fs::write(&late, "pub fn late_fn() -> u8 {{ 3 }}\n").unwrap();
    engine
        .update_base(&late, Some("pub fn late_fn() -> u8 {{ 3 }}\n".to_string()))
        .unwrap();
    let hover = engine.hover(&late, 1, 8).unwrap();
    assert!(
        hover.as_deref().is_some_and(|h| h.contains("late_fn")),
        "{hover:?}"
    );
}

#[test]
fn test_update_base_respects_open_buffers() {
    let (temp, lib_path) = create_test_fixture();
    let mut engine = RustEngine::load(temp.path()).expect("Must load fixture");
    let names = |engine: &RustEngine| -> Vec<String> {
        engine
            .document_symbols(&lib_path)
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect()
    };

    engine
        .update_base(
            &lib_path,
            Some("pub const FROM_SYNC: u8 = 1;\n".to_string()),
        )
        .unwrap();
    assert!(names(&engine).contains(&"FROM_SYNC".to_string()));

    engine
        .set_session_overlay(
            9,
            &lib_path,
            Some("pub const FROM_BUFFER: u8 = 2;\n".to_string()),
        )
        .unwrap();
    engine
        .update_base(
            &lib_path,
            Some("pub const FROM_SYNC_V2: u8 = 3;\n".to_string()),
        )
        .unwrap();
    engine.activate_session(9).unwrap();
    let with_buffer = names(&engine);
    assert!(
        with_buffer.contains(&"FROM_BUFFER".to_string()),
        "{with_buffer:?}"
    );
    assert!(
        !with_buffer.contains(&"FROM_SYNC_V2".to_string()),
        "{with_buffer:?}"
    );

    engine.clear_session(9).unwrap();
    let after = names(&engine);
    assert!(after.contains(&"FROM_SYNC_V2".to_string()), "{after:?}");
}

#[test]
fn test_retain_session_overlays_drops_stale_buffers() {
    let (temp, lib_path) = create_test_fixture();
    let mut engine = RustEngine::load(temp.path()).expect("Must load fixture");
    let scratch = temp.path().join("src/scratch.rs");
    engine
        .set_session_overlay(5, &lib_path, Some("pub const STALE: u8 = 1;\n".to_string()))
        .unwrap();
    engine
        .set_session_overlay(5, &scratch, Some("pub fn keep_me() {}\n".to_string()))
        .unwrap();
    assert_eq!(
        engine
            .retain_session_overlays(5, std::slice::from_ref(&scratch))
            .unwrap(),
        1
    );
    assert_eq!(engine.session_overlay_count(5), 1);
    engine.activate_session(5).unwrap();
    let names: Vec<String> = engine
        .document_symbols(&lib_path)
        .unwrap()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert!(names.contains(&"DEFAULT_PORT".to_string()), "{names:?}");
    assert!(!names.contains(&"STALE".to_string()), "{names:?}");
}

#[test]
fn test_session_overlay_new_module_survives_view_switches() {
    let (temp, lib_path) = create_test_fixture();
    let mut engine = RustEngine::load(temp.path()).expect("Must load fixture");
    let scratch = temp.path().join("src/scratch.rs");
    let base = std::fs::read_to_string(&lib_path).unwrap();
    let owner = format!("{base}\n#[path = \"scratch.rs\"]\nmod scratch;\n");

    engine
        .set_session_overlay(1, &lib_path, Some(owner))
        .unwrap();
    engine
        .set_session_overlay(
            1,
            &scratch,
            Some("pub fn scratch_only() -> u8 {{ 7 }}\n".to_string()),
        )
        .unwrap();
    engine.activate_session(1).unwrap();
    let first = engine.hover(&scratch, 1, 8).unwrap();
    assert!(
        first.as_deref().is_some_and(|h| h.contains("scratch_only")),
        "{first:?}"
    );

    // Another session looks at the base, then session 1 comes back.
    engine.activate_session(2).unwrap();
    let hidden = engine.hover(&scratch, 1, 8).unwrap_err();
    assert!(
        hidden.to_string().contains("File not found in VFS")
            || hidden.to_string().contains("Invalid position"),
        "{hidden:#}"
    );
    assert!(engine.hover(&scratch, 1, 1).is_err());
    engine.activate_session(1).unwrap();
    let again = engine.hover(&scratch, 1, 8).unwrap();
    assert!(
        again.as_deref().is_some_and(|h| h.contains("scratch_only")),
        "{again:?}"
    );

    // Session 1 disconnects; a fresh session re-syncs the same files and must resolve too.
    engine.clear_session(1).unwrap();
    engine.activate_session(2).unwrap();
    let owner = format!("{base}\n#[path = \"scratch.rs\"]\nmod scratch;\n");
    engine
        .set_session_overlay(3, &lib_path, Some(owner))
        .unwrap();
    engine
        .set_session_overlay(
            3,
            &scratch,
            Some("pub fn scratch_only() -> u8 {{ 7 }}\n".to_string()),
        )
        .unwrap();
    engine.activate_session(3).unwrap();
    let fresh = engine.hover(&scratch, 1, 8).unwrap();
    assert!(
        fresh.as_deref().is_some_and(|h| h.contains("scratch_only")),
        "{fresh:?}"
    );
}
