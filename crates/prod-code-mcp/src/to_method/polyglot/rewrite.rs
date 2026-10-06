/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::make_static::Language;
use crate::to_method::helpers::{is_ident, receiver_argument, split_call_arguments};

pub fn rewrite_static_calls_in_code(
    code: &str,
    target_method: &str,
    owner_class: &str,
    lang: Language,
) -> (String, usize) {
    let mut out = String::new();
    let mut rewritten = 0;
    let target_prefix = match lang {
        Language::TypeScript | Language::Python | Language::Swift => {
            format!("{owner_class}.{target_method}(")
        }
        Language::Cpp => {
            format!("{owner_class}::{target_method}(")
        }
        Language::Go => {
            format!("{target_method}(")
        }
    };

    for line in code.lines() {
        let trimmed = line.trim_start();
        let comment_prefix = match lang {
            Language::Python => "#",
            _ => "//",
        };
        if trimmed.starts_with(comment_prefix) || trimmed.starts_with('*') {
            out.push_str(line);
            out.push('\n');
            continue;
        }

        if !line.contains(&target_prefix) {
            out.push_str(line);
            out.push('\n');
            continue;
        }

        let mut current_line = line.to_string();
        let mut search_start = 0;
        while let Some(rel_pos) = current_line[search_start..].find(&target_prefix) {
            let pos = search_start + rel_pos;
            if pos > 0 {
                let prev_char = current_line[..pos].chars().next_back().unwrap();
                if is_ident(prev_char) || (lang == Language::Go && prev_char == '.') {
                    search_start = pos + target_prefix.len();
                    continue;
                }
            }
            if lang == Language::Go
                && current_line[..pos].trim_start().starts_with("func ")
                && !current_line[..pos].contains('{')
            {
                search_start = pos + target_prefix.len();
                continue;
            }

            let after_open = pos + target_prefix.len();
            let rest = &current_line[after_open..];
            let mut depth = 1i32;
            let mut close_pos = None;
            for (i, c) in rest.char_indices() {
                if c == '(' {
                    depth += 1;
                } else if c == ')' {
                    depth -= 1;
                    if depth == 0 {
                        close_pos = Some(after_open + i);
                        break;
                    }
                }
            }

            let Some(cp) = close_pos else {
                search_start = pos + target_prefix.len();
                continue;
            };

            let args_str = &current_line[after_open..cp];
            let args = split_call_arguments(args_str);
            if args.is_empty() {
                search_start = pos + target_prefix.len();
                continue;
            }

            let recv = receiver_argument(&args[0], lang);
            let rest_args = if args.len() > 1 {
                args[1..].join(", ")
            } else {
                String::new()
            };

            let op = if lang == Language::Cpp && (recv.starts_with('*') || recv.ends_with("->")) {
                "->"
            } else {
                "."
            };

            let formatted_recv = if recv.contains(' ') && !recv.starts_with('(') {
                format!("({recv})")
            } else {
                recv.to_string()
            };

            let call_replacement = format!("{formatted_recv}{op}{target_method}({rest_args})");
            current_line.replace_range(pos..=cp, &call_replacement);
            rewritten += 1;
            search_start = pos + call_replacement.len();
        }

        out.push_str(&current_line);
        out.push('\n');
    }

    if !code.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    (out, rewritten)
}
