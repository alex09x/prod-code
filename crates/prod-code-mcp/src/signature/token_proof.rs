/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::signature::call_sites::opaque_at;
use ra_ap_syntax::{AstNode, Edition, SourceFile, SyntaxKind, ast};

/// Comments and a syntax-token fingerprint of a call. Macro punctuation also retains jointness.
pub fn call_tokens(text: &str) -> Option<(Vec<&str>, Vec<u8>)> {
    let mut comments = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if let Some((end, comment)) = opaque_at(text, i, 0).ok()? {
            if comment {
                comments.push(&text[i..end]);
            }
            i = end;
        } else {
            i += 1;
        }
    }
    let prefix = "fn __prod_code_proof() { let _ = ";
    let wrapped = format!("{prefix}{text}; }}");
    let parsed = SourceFile::parse(&wrapped, Edition::Edition2024);
    let parsed = if parsed.errors().is_empty() {
        parsed
    } else {
        SourceFile::parse(&wrapped, Edition::Edition2021)
    };
    if !parsed.errors().is_empty() {
        return None;
    }
    let mut code = Vec::new();
    let mut previous: Option<(usize, bool, bool)> = None;
    for token in parsed
        .syntax_node()
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
    {
        let range = token.text_range();
        let start = u32::from(range.start()) as usize;
        let end = u32::from(range.end()) as usize;
        if start < prefix.len() || end > prefix.len() + text.len() {
            continue;
        }
        if matches!(token.kind(), SyntaxKind::WHITESPACE | SyntaxKind::COMMENT) {
            continue;
        }
        let spelling = token.text();
        let punct = spelling.len() == 1
            && spelling.as_bytes()[0].is_ascii_punctuation()
            && !b"()[]{}\"".contains(&spelling.as_bytes()[0]);
        let in_macro = token
            .parent_ancestors()
            .any(|n| ast::TokenTree::can_cast(n.kind()));
        let joint = previous.is_some_and(|(end, old_punct, old_macro)| {
            punct && old_punct && in_macro && old_macro && end == start
        });
        code.extend_from_slice(&(token.kind() as u16).to_le_bytes());
        code.push(u8::from(joint));
        code.extend_from_slice(&(spelling.len() as u32).to_le_bytes());
        code.extend_from_slice(spelling.as_bytes());
        previous = Some((end, punct, in_macro));
    }
    Some((comments, code))
}
