/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::matcher::{find_structural_matches_in_source, rewrite_source};
use super::scope::resolve_workspace_scope;
use super::source::tokenize_source;
use super::types::{CodemodRule, CompiledPattern, TokenKind};

#[test]
fn test_go_error_wrapping_codemod() {
    let rule =
        CodemodRule::parse("errors.Wrap($err, $msg) ==>> fmt.Errorf(\"%s: %w\", $msg, $err)")
            .expect("valid rule");
    let src = r#"package main

import "errors"

func test() error {
    err := read()
    if err != nil {
        return errors.Wrap(err, "read failed")
    }
    return nil
}
"#;
    let rewritten = rewrite_source(src, &rule).expect("should match");
    assert!(rewritten.contains(r#"return fmt.Errorf("%s: %w", "read failed", err)"#));
}

#[test]
fn test_typescript_logger_codemod() {
    let rule = CodemodRule::parse("console.log($msg) ==>> logger.info($msg)").expect("valid rule");
    let src = r#"function login(user: User) {
    console.log("user logged in: " + user.id);
}
"#;
    let rewritten = rewrite_source(src, &rule).expect("should match");
    assert!(rewritten.contains(r#"logger.info("user logged in: " + user.id);"#));
}

#[test]
fn test_python_pathlib_codemod() {
    let rule = CodemodRule::parse("os.path.join($a, $b) ==>> Path($a) / $b").expect("valid rule");
    let src = r#"import os

def get_config():
    p = os.path.join(base_dir, "config.json")
    return p
"#;
    let rewritten = rewrite_source(src, &rule).expect("should match");
    assert!(rewritten.contains(r#"p = Path(base_dir) / "config.json""#));
}

#[test]
fn test_cpp_smart_pointer_codemod() {
    let rule = CodemodRule::parse(
        "std::make_shared<$T>($args) ==>> std::allocate_shared<$T>(alloc, $args)",
    )
    .expect("valid rule");
    let src = r#"#include <memory>

void make() {
    auto ptr = std::make_shared<Widget>(42, "test");
}
"#;
    let rewritten = rewrite_source(src, &rule).expect("should match");
    assert!(rewritten.contains(r#"auto ptr = std::allocate_shared<Widget>(alloc, 42, "test");"#));
}

#[test]
fn test_swift_os_log_codemod() {
    let rule = CodemodRule::parse("print($x) ==>> os_log($x)").expect("valid rule");
    let src = r#"func log() {
    print("operation succeeded")
}
"#;
    let rewritten = rewrite_source(src, &rule).expect("should match");
    assert!(rewritten.contains(r#"os_log("operation succeeded")"#));
}

#[test]
fn test_multi_occurrence_consistency() {
    let rule = CodemodRule::parse("compare($a, $a) ==>> 0").expect("valid rule");
    let match_src = "let res = compare(x, x);";
    let no_match_src = "let res = compare(x, y);";

    assert_eq!(
        rewrite_source(match_src, &rule),
        Some("let res = 0;".to_string())
    );
    assert_eq!(rewrite_source(no_match_src, &rule), None);
}

#[test]
fn test_multiline_whitespace_invariance() {
    let rule = CodemodRule::parse("calc($a, $b) ==>> compute($b, $a)").expect("valid rule");
    let src = r#"let val = calc(
    firstArgument + 1,
    secondArgument * 2
);
"#;
    let rewritten = rewrite_source(src, &rule).expect("should match");
    assert!(rewritten.contains("compute(secondArgument * 2, firstArgument + 1)"));
}

#[test]
fn test_structural_search_ast() {
    let pattern = CompiledPattern::parse("$a.unwrap()").expect("valid pattern");
    let src = r#"
fn run() {
    let x = opt.unwrap();
    let y = calc(1, 2);
    let z = map.get(&k).unwrap();
}
"#;
    let matches = find_structural_matches_in_source("test.rs", src, &pattern);
    assert_eq!(matches.len(), 2);
    assert_eq!(matches[0].bindings.get("a").unwrap(), "opt");
    assert_eq!(matches[1].bindings.get("a").unwrap(), "get(&k)");
}

#[test]
fn test_structural_search_unicode_multibyte_chars() {
    // Multi-byte UTF-8 characters like '€' (3 bytes), '✓' (3 bytes), non-ASCII docstrings
    let pattern = CompiledPattern::parse("$x.price()").expect("valid pattern");
    let src = r#"
/// Price in €/kg or £/lb or ¥
fn test_currency() {
    let apple = item.price();
    let label = "Apple Price (€/kg)";
}
"#;
    let matches = find_structural_matches_in_source("test.rs", src, &pattern);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].bindings.get("x").unwrap(), "item");

    let tokens = tokenize_source("let symbol = €; let name = café;");
    assert!(
        tokens
            .iter()
            .any(|t| matches!(&t.kind, TokenKind::Punct(p) if p == "€"))
    );
    assert!(
        tokens
            .iter()
            .any(|t| matches!(&t.kind, TokenKind::Ident(id) if id.contains("caf")))
    );

    // Pattern and codemod rule parsing with multi-byte Unicode characters
    let rule = CodemodRule::parse("$x.price(€) ==>> $x.cost(¥)").expect("valid rule with unicode");
    let src = "let r = item.price(€);";
    let rewritten = rewrite_source(src, &rule).expect("should match and rewrite");
    assert_eq!(rewritten, "let r = item.cost(¥);");
}

#[test]
fn metavariables_stop_at_statement_boundaries() {
    let rule = CodemodRule::parse("$a.unwrap() ==>> handle($a)").unwrap();
    let source = "let keep = 1; obj.unwrap();\n";
    let rewritten = rewrite_source(source, &rule).unwrap();
    assert_eq!(rewritten, "let keep = 1; handle(obj);\n");
}

#[test]
fn codemod_scope_rejects_parent_absolute_and_symlink_paths() {
    let workspace = tempfile::tempdir().unwrap();
    let sibling = tempfile::tempdir().unwrap();
    let inside = workspace.path().join("src.rs");
    std::fs::write(&inside, "fn inside() {}\n").unwrap();
    let outside = sibling.path().join("outside.rs");
    std::fs::write(&outside, "fn outside() {}\n").unwrap();

    assert_eq!(
        resolve_workspace_scope(workspace.path(), "src.rs").unwrap(),
        inside.canonicalize().unwrap()
    );
    assert_eq!(
        resolve_workspace_scope(workspace.path(), ".").unwrap(),
        workspace.path().canonicalize().unwrap()
    );
    assert_eq!(
        resolve_workspace_scope(workspace.path(), "./.").unwrap(),
        workspace.path().canonicalize().unwrap()
    );
    assert!(resolve_workspace_scope(workspace.path(), "").is_err());
    assert!(resolve_workspace_scope(workspace.path(), "   ").is_err());
    assert!(resolve_workspace_scope(workspace.path(), "../outside.rs").is_err());
    assert!(resolve_workspace_scope(workspace.path(), outside.to_str().unwrap()).is_err());

    #[cfg(unix)]
    {
        let link = workspace.path().join("external.rs");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        assert!(resolve_workspace_scope(workspace.path(), "external.rs").is_err());
    }
}
