/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::helpers::at;
use crate::refactor::edits::{apply_scalar_text_edits, apply_text_edits};
use crate::refactor::execute::apply_workspace_edit;

#[test]
fn lsp_columns_count_utf16_units_and_the_scanners_count_characters() {
    let text = "let s = \"\u{1F600}\"; let x = 1;\n";
    // `x` is character 17 and UTF-16 unit 18.
    let lsp = at(0, 18, 19, "y");
    assert_eq!(
        apply_text_edits(text, lsp.as_array().unwrap()).unwrap(),
        "let s = \"\u{1F600}\"; let y = 1;\n"
    );
    let scalar = at(0, 17, 18, "y");
    assert_eq!(
        apply_scalar_text_edits(text, scalar.as_array().unwrap()).unwrap(),
        "let s = \"\u{1F600}\"; let y = 1;\n"
    );
    // A column past the end of its line stops at the line's end.
    let past = at(0, 99, 99, " // end");
    assert_eq!(
        apply_text_edits(text, past.as_array().unwrap()).unwrap(),
        "let s = \"\u{1F600}\"; let x = 1; // end\n"
    );
}

/// The same through the whole applicator: every byte of the file is what the analyzer meant.
#[test]
fn a_workspace_edit_after_non_bmp_text_lands_on_its_utf16_column() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    crate::sync::clear_sync_cache(&root);
    std::fs::write(
        root.join("lib.rs"),
        "// \u{1D11E} clef\nfn f(\u{1F600}: u8, old: u8) {}\n",
    )
    .unwrap();
    let uri = format!("file://{}/lib.rs", root.display());
    // `old` starts at character 12 and UTF-16 unit 13 of the second line, `clef` at
    // character 5 and unit 6 of the first.
    let edit = serde_json::json!({ "changes": { uri: [
        { "range": { "start": { "line": 1, "character": 13 }, "end": { "line": 1, "character": 16 } },
          "newText": "new" },
        { "range": { "start": { "line": 0, "character": 6 }, "end": { "line": 0, "character": 10 } },
          "newText": "G clef" }
    ] } });
    apply_workspace_edit(&root, &edit).unwrap();
    assert_eq!(
        std::fs::read(root.join("lib.rs")).unwrap(),
        "// \u{1D11E} G clef\nfn f(\u{1F600}: u8, new: u8) {}\n".as_bytes()
    );
    crate::sync::clear_sync_cache(&root);
}
