/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The source-reading fallbacks of `code_definition` keep a dotted qualifier (#2041).

use super::range::find_unindexed_rust_declarations;
use super::shell::find_shell_declarations;

fn checkout() -> tempfile::TempDir {
    let temp = tempfile::Builder::new()
        .prefix("qualified")
        .tempdir()
        .unwrap();
    let write = |rel: &str, text: &str| {
        let path = temp.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write(
        "src/terminal/state.rs",
        "pub struct Terminal;\nimpl Terminal {\n    pub fn feed(&mut self, bytes: &[u8]) {}\n}\n",
    );
    write(
        "tests/grid_dump.rs",
        "fn feed(term: &mut Terminal, data: &str) {}\n",
    );
    write("scripts/run.sh", "feed() {\n  echo fed\n}\n");
    write("scripts/dotted.sh", "Tako.feed() {\n  echo dotted\n}\n");
    temp
}

/// #2041: a Swift member asked for by `Module.Type.name` matched every Rust `feed` and every
/// shell `feed` of the checkout. Its type names none of them, so none is the answer.
#[test]
fn a_dotted_qualifier_is_kept_by_the_source_fallbacks() {
    let temp = checkout();
    let root = temp.path();
    let rust = find_unindexed_rust_declarations(root, "Tako.SurfaceView.feed", None);
    assert!(rust.is_empty(), "{rust:?}");
    let shell = find_shell_declarations(root, "Tako.SurfaceView.feed", None);
    assert!(shell.is_empty(), "{shell:?}");
    assert!(find_shell_declarations(root, "Terminal::feed", None).is_empty());
}

/// #2045: `Exec::cancel` is not the `#[path] mod cancel;` of a file that also has `impl Exec`;
/// the method inside that `impl` is still found.
#[test]
fn a_module_beside_an_impl_is_not_its_member() {
    let temp = tempfile::Builder::new()
        .prefix("modimpl")
        .tempdir()
        .unwrap();
    let root = temp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub struct Exec;\nmod liq;\n").unwrap();
    std::fs::write(
        root.join("src/liq.rs"),
        "use super::*;\n\n#[path = \"liq_cancel.rs\"]\nmod cancel;\n\nimpl Exec {\n    pub fn sweep(&self) {}\n}\n",
    )
    .unwrap();
    let hits = find_unindexed_rust_declarations(root, "Exec::cancel", None);
    assert!(hits.is_empty(), "{hits:?}");
    let sweep = find_unindexed_rust_declarations(root, "Exec::sweep", None);
    assert_eq!(sweep.len(), 1, "{sweep:?}");
    assert_eq!(sweep[0].line, 7);
    // The module is still the module's.
    assert_eq!(
        find_unindexed_rust_declarations(root, "liq::cancel", None).len(),
        1
    );
}

/// #2102: `tokio::sync::mpsc::Receiver::recv_many` names tokio's type. A local `impl<T>
/// Receiver<T>` in `src/mpsc.rs` has a `recv_many` too, but its module is `mpsc`, not
/// `tokio::sync::mpsc`, so it is not the answer; nor is it `zzz::Receiver::recv_many`. Its own
/// paths still find it, and so does a module path to a free function.
#[test]
fn modules_named_before_the_owner_must_be_the_files() {
    let temp = checkout();
    let root = temp.path();
    std::fs::write(
        root.join("src/mpsc.rs"),
        "pub struct Receiver<T>(T);\nimpl<T> Receiver<T> {\n    pub async fn recv_many(&mut self, limit: usize) -> usize { limit }\n}\n",
    )
    .unwrap();
    for symbol in [
        "tokio::sync::mpsc::Receiver::recv_many",
        "zzz::Receiver::recv_many",
        "zzz::mpsc::Receiver::recv_many",
    ] {
        let hits = find_unindexed_rust_declarations(root, symbol, None);
        assert!(hits.is_empty(), "{symbol}: {hits:?}");
    }
    for symbol in [
        "mpsc::Receiver::recv_many",
        "crate::mpsc::Receiver::recv_many",
        "Receiver::recv_many",
    ] {
        let hits = find_unindexed_rust_declarations(root, symbol, None);
        assert_eq!(hits.len(), 1, "{symbol}: {hits:?}");
        assert!(hits[0].path.ends_with("src/mpsc.rs"), "{symbol}");
    }
    for symbol in ["terminal::state::feed", "terminal::state::Terminal::feed"] {
        let hits = find_unindexed_rust_declarations(root, symbol, None);
        assert_eq!(hits.len(), 1, "{symbol}: {hits:?}");
        assert!(hits[0].path.ends_with("src/terminal/state.rs"), "{symbol}");
    }
}

/// The qualifier still finds what it names, spelled with a dot or with `::`, and a bare name
/// still finds every declaration of it.
#[test]
fn a_qualifier_that_names_the_owner_still_finds_it() {
    let temp = checkout();
    let root = temp.path();
    for symbol in ["Terminal::feed", "Terminal.feed", "state::feed"] {
        let hits = find_unindexed_rust_declarations(root, symbol, None);
        assert_eq!(hits.len(), 1, "{symbol}: {hits:?}");
        assert!(hits[0].path.ends_with("src/terminal/state.rs"), "{symbol}");
    }
    assert_eq!(
        find_unindexed_rust_declarations(root, "feed", None).len(),
        2
    );
    assert_eq!(find_shell_declarations(root, "feed", None).len(), 1);
    // Bash allows a dot in a function name: that name, whole, is still found.
    assert_eq!(find_shell_declarations(root, "Tako.feed", None).len(), 1);
}
