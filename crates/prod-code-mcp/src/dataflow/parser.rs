/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::keywords::KEYWORDS;

/// Strips comments and string literals from a line of code to allow safe identifier scanning.
pub(crate) fn strip_literals_and_comments(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    let mut i = 0;

    while i < n {
        // Line comments: // or # or --
        if (chars[i] == '/' && i + 1 < n && chars[i + 1] == '/')
            || (chars[i] == '-' && i + 1 < n && chars[i + 1] == '-')
            || (chars[i] == '#' && (i == 0 || chars[i - 1].is_whitespace()))
        {
            break;
        }

        // String literals: "...", '...', `...`
        if chars[i] == '"' || chars[i] == '\'' || chars[i] == '`' {
            let quote = chars[i];
            i += 1;
            while i < n {
                if chars[i] == '\\' && i + 1 < n {
                    i += 2;
                } else if chars[i] == quote {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
            out.push(' ');
            continue;
        }

        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Extracts identifier tokens from text, skipping property and method member accesses (e.g. `.price`).
pub(crate) fn extract_used_variables(text: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut i = 0;

    while i < n {
        if chars[i].is_ascii_alphabetic() || chars[i] == '_' {
            let start = i;
            while i < n && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();

            // Check if this word is preceded by '.' (property access like `obj.prop` or `obj.method()`)
            let mut j = start;
            let mut is_property = false;
            while j > 0 {
                j -= 1;
                if chars[j] == '.' {
                    // Check if it's '..' range in Rust; '..' is not property access
                    is_property = !(j > 0 && chars[j - 1] == '.');
                    break;
                } else if !chars[j].is_whitespace() {
                    break;
                }
            }

            if !is_property
                && !KEYWORDS.contains(&word.as_str())
                && !word.chars().all(|c| c.is_ascii_digit())
            {
                ids.push(word);
            }
        } else {
            i += 1;
        }
    }
    ids
}

/// Extracts parameter names from a function signature line across common languages.
pub(crate) fn extract_function_parameters(signature: &str) -> Vec<String> {
    let mut params = Vec::new();
    let Some(open_paren) = signature.find('(') else {
        return extract_used_variables(signature);
    };
    let Some(close_paren) = signature.rfind(')') else {
        return extract_used_variables(signature);
    };
    if open_paren >= close_paren {
        return extract_used_variables(signature);
    }
    let param_section = &signature[open_paren + 1..close_paren];
    for raw_part in param_section.split(',') {
        let part = raw_part.trim();
        if part.is_empty() || part == "self" || part == "&self" || part == "&mut self" {
            continue;
        }
        // Extract the parameter name
        // Rust / TS: `name: Type`
        let param_name = if let Some(colon) = part.find(':') {
            part[..colon].trim()
        } else {
            // Go: `name Type` or Python: `name`
            part.split_whitespace().next().unwrap_or(part)
        };
        // Clean leading modifiers like mut, &, etc.
        let cleaned = param_name
            .trim_start_matches("mut ")
            .trim_start_matches('&')
            .trim();
        for id in extract_used_variables(cleaned) {
            params.push(id);
        }
    }
    params
}
