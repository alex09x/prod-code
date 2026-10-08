/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

use super::ts_js_imported_symbols;

#[test]
fn test_default_import_preserves_local_binding() {
    let content = r#"import again from "./selected";"#;
    let syms = ts_js_imported_symbols(
        content,
        Path::new("src/caller.ts"),
        Path::new("src/selected.ts"),
        "retry",
    );
    assert_eq!(syms, vec!["again".to_string()]);
}

#[test]
fn test_default_import_rejected_when_fn_is_named_import() {
    let content = r#"import other, { retry } from "./selected";"#;
    let syms = ts_js_imported_symbols(
        content,
        Path::new("src/caller.ts"),
        Path::new("src/selected.ts"),
        "retry",
    );
    assert_eq!(syms, vec!["retry".to_string()]);
}

#[test]
fn test_default_import_accepted_when_matching_fn_name() {
    let content = r#"import retry, { other } from "./selected";"#;
    let syms = ts_js_imported_symbols(
        content,
        Path::new("src/caller.ts"),
        Path::new("src/selected.ts"),
        "retry",
    );
    assert_eq!(syms, vec!["retry".to_string()]);
}

#[test]
fn test_relative_path_resolution_distinguishes_directories() {
    let content = r#"import { retry } from "./selected";"#;
    // b/caller.ts importing ./selected should resolve to b/selected, not a/selected
    let wrong = ts_js_imported_symbols(
        content,
        Path::new("b/caller.ts"),
        Path::new("a/selected.ts"),
        "retry",
    );
    assert!(wrong.is_empty());

    let correct = ts_js_imported_symbols(
        content,
        Path::new("b/caller.ts"),
        Path::new("b/selected.ts"),
        "retry",
    );
    assert_eq!(correct, vec!["retry".to_string()]);

    let parent_content = r#"import { retry } from "../a/selected";"#;
    let cross_dir = ts_js_imported_symbols(
        parent_content,
        Path::new("b/caller.ts"),
        Path::new("a/selected.ts"),
        "retry",
    );
    assert_eq!(cross_dir, vec!["retry".to_string()]);
}

#[test]
fn test_import_tokens_in_comments_and_strings_ignored() {
    let content_comment = "// import { retry } from \"./selected\";\n";
    let syms = ts_js_imported_symbols(
        content_comment,
        Path::new("src/caller.ts"),
        Path::new("src/selected.ts"),
        "retry",
    );
    assert!(syms.is_empty());

    let content_block_comment = "/*\nimport { retry } from \"./selected\";\n*/";
    let syms_block = ts_js_imported_symbols(
        content_block_comment,
        Path::new("src/caller.ts"),
        Path::new("src/selected.ts"),
        "retry",
    );
    assert!(syms_block.is_empty());

    let content_str = "const s = \"import { retry } from './selected'\";";
    let syms_str = ts_js_imported_symbols(
        content_str,
        Path::new("src/caller.ts"),
        Path::new("src/selected.ts"),
        "retry",
    );
    assert!(syms_str.is_empty());

    let content_cjs_comment = "// const again = require(\"./selected\").retry;";
    let syms_cjs_comment = ts_js_imported_symbols(
        content_cjs_comment,
        Path::new("src/caller.ts"),
        Path::new("src/selected.ts"),
        "retry",
    );
    assert!(syms_cjs_comment.is_empty());
}

#[test]
fn test_commonjs_property_access_records_lhs_alias() {
    let content = r#"const again = require("./selected").retry;"#;
    let syms = ts_js_imported_symbols(
        content,
        Path::new("src/caller.ts"),
        Path::new("src/selected.ts"),
        "retry",
    );
    assert_eq!(syms, vec!["again".to_string()]);

    let content_ws = "const again = require (\"./selected\") . retry ;";
    let syms_ws = ts_js_imported_symbols(
        content_ws,
        Path::new("src/caller.ts"),
        Path::new("src/selected.ts"),
        "retry",
    );
    assert_eq!(syms_ws, vec!["again".to_string()]);

    let content_other_prop = r#"const again = require("./selected").other;"#;
    let syms_other = ts_js_imported_symbols(
        content_other_prop,
        Path::new("src/caller.ts"),
        Path::new("src/selected.ts"),
        "retry",
    );
    assert!(syms_other.is_empty());
}

#[test]
fn test_commonjs_destructuring_alias() {
    let content = r#"const { retry: again } = require("./selected");"#;
    let syms = ts_js_imported_symbols(
        content,
        Path::new("src/caller.ts"),
        Path::new("src/selected.ts"),
        "retry",
    );
    assert_eq!(syms, vec!["again".to_string()]);
}

#[test]
fn test_index_file_relative_resolution() {
    let content = r#"import { calculate } from "./math";"#;
    let syms = ts_js_imported_symbols(
        content,
        Path::new("src/client.ts"),
        Path::new("src/math/index.ts"),
        "calculate",
    );
    assert_eq!(syms, vec!["calculate".to_string()]);
}

#[test]
fn test_reexport_barrel_resolved() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let selected_ts = root.join("selected.ts");
    std::fs::write(&selected_ts, "export function retry() {}\n").unwrap();

    let index_ts = root.join("index.ts");
    std::fs::write(&index_ts, "export { retry } from \"./selected\";\n").unwrap();

    let consumer_ts = root.join("consumer.ts");
    let content = "import { retry } from \"./index\";\n";

    let syms = ts_js_imported_symbols(content, &consumer_ts, &selected_ts, "retry");
    assert_eq!(syms, vec!["retry".to_string()]);
}

#[test]
fn test_reexport_barrel_unrelated_symbol_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let selected_ts = root.join("selected.ts");
    std::fs::write(&selected_ts, "export function other() {}\n").unwrap();

    let another_ts = root.join("another.ts");
    std::fs::write(&another_ts, "export function retry() {}\n").unwrap();

    let index_ts = root.join("index.ts");
    std::fs::write(
        &index_ts,
        "export { other } from \"./selected\";\nexport { retry } from \"./another\";\n",
    )
    .unwrap();

    let consumer_ts = root.join("consumer.ts");
    let content = "import { retry } from \"./index\";\n";

    let syms = ts_js_imported_symbols(content, &consumer_ts, &selected_ts, "retry");
    assert!(syms.is_empty());
}

#[test]
fn test_reexport_barrel_aliased_symbol() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let selected_ts = root.join("selected.ts");
    std::fs::write(&selected_ts, "export function retry() {}\n").unwrap();

    let index_ts = root.join("index.ts");
    std::fs::write(
        &index_ts,
        "export { retry as runAgain } from \"./selected\";\n",
    )
    .unwrap();

    let consumer_ts = root.join("consumer.ts");
    let content = "import { runAgain } from \"./index\";\n";

    let syms = ts_js_imported_symbols(content, &consumer_ts, &selected_ts, "retry");
    assert_eq!(syms, vec!["runAgain".to_string()]);
}

#[test]
fn test_reexport_barrel_default_as_named() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let selected_ts = root.join("selected.ts");
    std::fs::write(&selected_ts, "export default function retry() {}\n").unwrap();

    let index_ts = root.join("index.ts");
    std::fs::write(
        &index_ts,
        "export { default as retry } from \"./selected\";\n",
    )
    .unwrap();

    let consumer_ts = root.join("consumer.ts");
    let content = "import { retry } from \"./index\";\n";

    let syms = ts_js_imported_symbols(content, &consumer_ts, &selected_ts, "retry");
    assert_eq!(syms, vec!["retry".to_string()]);
}

#[test]
fn test_cjs_callable_local_binding() {
    let content = "const again = require(\"./selected\");\nagain();\n";
    let syms = ts_js_imported_symbols(
        content,
        Path::new("consumer.js"),
        Path::new("selected.js"),
        "retry",
    );
    assert_eq!(syms, vec!["again".to_string()]);
}
