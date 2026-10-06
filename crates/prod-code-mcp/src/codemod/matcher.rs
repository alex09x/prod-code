/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeMap;

use super::source::{compute_matching_delims, tokenize_source};
use super::types::{
    CodemodMatch, CodemodRule, CompiledPattern, MatchBindings, PatternMatch, PatternToken,
    ReplacementToken, SourceToken, StructuralMatchItem, TokenKind, byte_to_line_col,
};

pub fn is_boundary_token(kind: &TokenKind, next_expected: &TokenKind) -> bool {
    if kind == next_expected {
        return false;
    }
    match kind {
        TokenKind::Punct(p) if p == ";" => true,
        TokenKind::Punct(p) if p == ":" => true,
        TokenKind::Punct(p) if matches!(p.as_str(), "=" | ":=" | "+=" | "-=" | "*=" | "/=") => true,
        TokenKind::Punct(p) if p == "," => !matches!(next_expected, TokenKind::CloseDelim(_)),
        TokenKind::Ident(id) => matches!(
            id.as_str(),
            "let"
                | "var"
                | "const"
                | "return"
                | "fn"
                | "func"
                | "function"
                | "def"
                | "class"
                | "struct"
                | "enum"
                | "interface"
                | "import"
                | "export"
                | "package"
                | "if"
                | "else"
                | "for"
                | "while"
                | "switch"
                | "case"
                | "catch"
                | "throw"
                | "yield"
                | "defer"
                | "break"
                | "continue"
        ),
        _ => false,
    }
}

/// Attempt to match pattern tokens starting at `start_idx` in `tokens`.
/// Returns `(start_byte, end_byte, end_token_idx, bindings)` on success.
pub fn match_pattern_tokens(
    tokens: &[SourceToken],
    start_idx: usize,
    matching_delims: &[Option<usize>],
    pattern_tokens: &[PatternToken],
    source: &str,
) -> Option<PatternMatch> {
    let mut code_idx = start_idx;
    let mut pat_idx = 0;
    let mut bindings: MatchBindings = BTreeMap::new();

    while pat_idx < pattern_tokens.len() {
        let pat_tok = &pattern_tokens[pat_idx];

        match pat_tok {
            PatternToken::Literal(expected_kind) => {
                if code_idx >= tokens.len() {
                    return None;
                }
                if &tokens[code_idx].kind != expected_kind {
                    return None;
                }
                code_idx += 1;
                pat_idx += 1;
            }
            PatternToken::Metavar(var_name) => {
                let next_pat_tok = pattern_tokens.get(pat_idx + 1);
                let var_start_token = code_idx;

                if code_idx >= tokens.len() {
                    return None;
                }

                // If next pattern token is a literal delimiter or punctuation,
                // consume tokens until we hit that token at the current nesting level.
                let var_end_token = match next_pat_tok {
                    Some(PatternToken::Literal(next_expected)) => {
                        let mut depth = 0;
                        let mut found = None;
                        let mut cur = code_idx;

                        while cur < tokens.len() {
                            if depth == 0
                                && cur > var_start_token
                                && newline_separates_expressions(source, tokens, cur)
                            {
                                return None;
                            }
                            match tokens[cur].kind {
                                TokenKind::OpenDelim(_) => depth += 1,
                                TokenKind::CloseDelim(_) => {
                                    if depth == 0 {
                                        // Reached an enclosing close delimiter
                                        if &tokens[cur].kind == next_expected {
                                            found = Some(cur);
                                        }
                                        break;
                                    }
                                    depth -= 1;
                                }
                                _ => {
                                    if depth == 0 {
                                        if &tokens[cur].kind == next_expected {
                                            found = Some(cur);
                                            break;
                                        }
                                        if is_boundary_token(&tokens[cur].kind, next_expected) {
                                            return None;
                                        }
                                    }
                                }
                            }
                            cur += 1;
                        }

                        let f = found?;
                        if f == var_start_token {
                            return None; // empty metavar match
                        }
                        code_idx = f; // next literal will be matched at `f`
                        f - 1
                    }
                    Some(PatternToken::Metavar(_)) | None => {
                        // Consumes a single balanced expression (token or delimited group)
                        let mut cur = code_idx;
                        if let TokenKind::OpenDelim(_) = tokens[cur].kind
                            && let Some(close_idx) = matching_delims[cur]
                        {
                            cur = close_idx;
                        }
                        code_idx = cur + 1;
                        cur
                    }
                };

                let start_b = tokens[var_start_token].start_byte;
                let end_b = tokens[var_end_token].end_byte;
                let snippet = source[start_b..end_b].trim();

                // Multi-occurrence consistency check
                if let Some(&(prev_start, prev_end)) = bindings.get(var_name) {
                    let prev_snippet = source[prev_start..prev_end].trim();
                    if prev_snippet != snippet {
                        return None; // Inconsistent metavar binding
                    }
                } else {
                    bindings.insert(var_name.clone(), (start_b, end_b));
                }

                pat_idx += 1;
            }
        }
    }

    if start_idx >= tokens.len() || code_idx == start_idx {
        return None;
    }

    let match_start_byte = tokens[start_idx].start_byte;
    let match_end_byte = tokens[code_idx - 1].end_byte;

    Some((match_start_byte, match_end_byte, code_idx, bindings))
}

pub fn newline_separates_expressions(source: &str, tokens: &[SourceToken], current: usize) -> bool {
    let previous = &tokens[current - 1];
    let next = &tokens[current];
    if !source[previous.end_byte..next.start_byte].contains('\n') {
        return false;
    }
    let can_end = matches!(
        previous.kind,
        TokenKind::Ident(_)
            | TokenKind::NumberLit(_)
            | TokenKind::StringLit(_)
            | TokenKind::CloseDelim(_)
    );
    let can_start = matches!(
        next.kind,
        TokenKind::Ident(_)
            | TokenKind::NumberLit(_)
            | TokenKind::StringLit(_)
            | TokenKind::OpenDelim(_)
    );
    can_end && can_start
}

/// Attempt to match a pattern starting at `start_idx` in `tokens`.
pub fn match_at(
    tokens: &[SourceToken],
    start_idx: usize,
    matching_delims: &[Option<usize>],
    rule: &CodemodRule,
    source: &str,
) -> Option<CodemodMatch> {
    let (match_start_byte, match_end_byte, _, bindings) = match_pattern_tokens(
        tokens,
        start_idx,
        matching_delims,
        &rule.pattern_tokens,
        source,
    )?;

    // Synthesize replacement text
    let mut rep = String::new();
    for rep_tok in &rule.replacement_tokens {
        match rep_tok {
            ReplacementToken::Text(t) => rep.push_str(t),
            ReplacementToken::Metavar(name) => {
                if let Some(&(s, e)) = bindings.get(name) {
                    rep.push_str(&source[s..e]);
                } else {
                    // Unbound metavar in replacement, keep as is
                    rep.push('$');
                    rep.push_str(name);
                }
            }
        }
    }

    Some(CodemodMatch {
        start_byte: match_start_byte,
        end_byte: match_end_byte,
        replacement: rep,
    })
}

/// Find all non-overlapping structural matches in `source` and produce rewritten text if any matches occur.
pub fn rewrite_source(source: &str, rule: &CodemodRule) -> Option<String> {
    // Quick candidate rejection
    for lit in &rule.required_literals {
        if !source.contains(lit) {
            return None;
        }
    }

    let tokens = tokenize_source(source);
    if tokens.is_empty() {
        return None;
    }

    let matching_delims = compute_matching_delims(&tokens);
    let mut matches: Vec<CodemodMatch> = Vec::new();
    let mut i = 0;

    while i < tokens.len() {
        if let Some(m) = match_at(&tokens, i, &matching_delims, rule, source) {
            // Advance tokens past the end of the match
            while i < tokens.len() && tokens[i].end_byte <= m.end_byte {
                i += 1;
            }
            matches.push(m);
        } else {
            i += 1;
        }
    }

    if matches.is_empty() {
        return None;
    }

    // Reconstruct rewritten text
    let mut output = String::with_capacity(source.len());
    let mut last_end = 0;

    for m in matches {
        if m.start_byte >= last_end {
            output.push_str(&source[last_end..m.start_byte]);
            output.push_str(&m.replacement);
            last_end = m.end_byte;
        }
    }
    if last_end < source.len() {
        output.push_str(&source[last_end..]);
    }

    Some(output)
}

/// Find all structural matches for a compiled pattern in a single source string.
pub fn find_structural_matches_in_source(
    rel_path: &str,
    source: &str,
    pattern: &CompiledPattern,
) -> Vec<StructuralMatchItem> {
    for lit in &pattern.required_literals {
        if !source.contains(lit) {
            return Vec::new();
        }
    }

    let tokens = tokenize_source(source);
    if tokens.is_empty() {
        return Vec::new();
    }

    let matching_delims = compute_matching_delims(&tokens);
    let mut matches = Vec::new();
    let mut i = 0;

    while i < tokens.len() {
        if let Some((start_b, end_b, _, bindings)) = match_pattern_tokens(
            &tokens,
            i,
            &matching_delims,
            &pattern.pattern_tokens,
            source,
        ) {
            let (line, col) = byte_to_line_col(source, start_b);
            let matched_text = source[start_b..end_b].to_string();
            let mut string_bindings = BTreeMap::new();
            for (k, (s, e)) in bindings {
                string_bindings.insert(k, source[s..e].trim().to_string());
            }

            matches.push(StructuralMatchItem {
                file: rel_path.to_string(),
                line,
                col,
                matched_text,
                bindings: string_bindings,
            });

            // Advance tokens past the end of the match
            while i < tokens.len() && tokens[i].end_byte <= end_b {
                i += 1;
            }
        } else {
            i += 1;
        }
    }

    matches
}
