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

#[test]
fn yaml_frontmatter_diagnostics() {
    for (line, expected) in [
        (
            "\tkey: value",
            "YAML frontmatter forbids tab characters for indentation; use spaces",
        ),
        (
            "key:\tvalue",
            "YAML frontmatter forbids tab characters for indentation; use spaces",
        ),
        (
            "not a mapping",
            "invalid frontmatter line: expected `key: value` or list item, got `not a mapping`",
        ),
        (
            "key: \"open",
            "unclosed double quote `\"` in frontmatter value",
        ),
        (
            "key: 'open",
            "unclosed single quote `'` in frontmatter value",
        ),
    ] {
        let text = format!("---\n{line}\n---\n");
        let r = validate_markdown("note.md", &text);
        assert_eq!(r.errors, 1, "{line}");
        assert_eq!(r.file, "note.md");
        assert_eq!((r.items[0].line, r.items[0].col), (2, 1));
        assert_eq!(r.items[0].message, expected);
        assert_eq!(r.items[0].code.as_deref(), Some("markdown-frontmatter"));
    }
    let r = validate_markdown("note.md", "---\nkey: a\nkey: b\n---\n");
    assert_eq!(r.errors, 1);
    assert_eq!(r.items[0].message, "duplicate frontmatter key `key`");
    assert_eq!(r.items[0].line, 3);
}

#[test]
fn valid_yaml_and_fence_boundaries() {
    for text in [
        "",
        "---\n# comment\n\nkey: \"escaped\\\"quote\"\nlist:\n- entry\n  nested: 'ok'\nblock: |\nfolded: >\n---\n",
        "+++\ntitle = \"α\"\n+++\r\n[valid](url)",
        "~~~\n[ignored](\n~~~\n",
        "````rust\n```\n````   \n",
        "    ```\n",
        "text](broken",
        "[label](url)",
        "  ~~~\ntext\n  ~~~",
    ] {
        let r = validate_markdown("note.md", text);
        assert_eq!(r.items.len(), 0, "{text}: {:?}", r.items);
        assert_eq!((r.errors, r.warnings), (0, 0));
    }
}

#[test]
fn toml_and_body_errors() {
    let r = validate_markdown("note.md", "+++\ntitle = 1\n");
    assert_eq!(r.errors, 1);
    assert_eq!(
        r.items[0].message,
        "unclosed TOML frontmatter: opening `+++` on line 1 has no matching closing `+++`"
    );
    let r = validate_markdown("note.md", "+++\na = 1\nb = [\n+++\n");
    assert_eq!(r.errors, 1);
    assert_eq!((r.items[0].line, r.items[0].col), (3, 1));
    assert_eq!(r.items[0].code.as_deref(), Some("markdown-frontmatter"));
    assert!(
        r.items[0]
            .message
            .starts_with("TOML frontmatter syntax error:")
    );
    let r = validate_markdown("note.md", "---\nkey: value\n---\nα [x](\r\n~~~rust\n");
    assert_eq!(r.errors, 2);
    assert_eq!(
        r.items
            .iter()
            .map(|d| (d.line, d.col, d.message.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (4, 8, "unclosed link destination on line 4; expected `)`"),
            (
                5,
                1,
                "unclosed code fence: opened at line 5 with `~~~`, reaching end of file without closing fence"
            )
        ]
    );
}
