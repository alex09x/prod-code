/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;

use crate::make_static::helpers::{extract_receiver, receiver_has_effects};
use crate::make_static::types::Language;

pub fn method_declaration_position(
    code: &str,
    lang: Language,
    owner: &str,
    method: &str,
) -> Result<(u32, u32)> {
    let lines: Vec<&str> = code.lines().collect();
    let mut type_start = None;
    let mut type_end = lines.len();
    if lang != Language::Go {
        let type_prefixes: &[&str] = match lang {
            Language::TypeScript => &["class ", "export class ", "export default class "],
            Language::Python => &["class "],
            Language::Cpp => &["class ", "struct "],
            Language::Swift => &["class ", "struct ", "actor "],
            Language::Go => &[],
        };
        for (idx, line) in lines.iter().enumerate() {
            let trimmed = line.trim_start();
            let type_keyword = match lang {
                Language::TypeScript => "class",
                Language::Python => "class",
                Language::Cpp => {
                    if trimmed.starts_with("struct ") {
                        "struct"
                    } else {
                        "class"
                    }
                }
                Language::Swift => {
                    if trimmed.starts_with("actor ") {
                        "actor"
                    } else if trimmed.starts_with("struct ") {
                        "struct"
                    } else {
                        "class"
                    }
                }
                Language::Go => unreachable!(),
            };
            let declared_type = type_prefixes
                .iter()
                .any(|prefix| trimmed.starts_with(prefix))
                .then(|| {
                    let mut words = trimmed.split_whitespace();
                    words
                        .position(|word| word == type_keyword)
                        .and_then(|_| words.next())
                        .map(|word| word.trim_matches(|c: char| !c.is_alphanumeric() && c != '_'))
                })
                .flatten();
            if declared_type == Some(owner) {
                type_start = Some(idx);
                if lang == Language::Python {
                    let indent = line.len() - trimmed.len();
                    type_end = lines
                        .iter()
                        .enumerate()
                        .skip(idx + 1)
                        .find(|(_, nested)| {
                            !nested.trim().is_empty()
                                && !nested.trim_start().starts_with('#')
                                && nested.len() - nested.trim_start().len() <= indent
                        })
                        .map_or(lines.len(), |(end, _)| end);
                } else {
                    let mut depth = 0i32;
                    for (end, nested) in lines.iter().enumerate().skip(idx) {
                        depth += nested.chars().filter(|c| *c == '{').count() as i32;
                        depth -= nested.chars().filter(|c| *c == '}').count() as i32;
                        if end > idx && depth == 0 {
                            type_end = end;
                            break;
                        }
                    }
                }
                break;
            }
        }
    }
    let start = type_start.unwrap_or(0);
    for (idx, line) in lines.iter().enumerate().take(type_end).skip(start) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('#') || trimmed.starts_with('*') {
            continue;
        }
        let name_at = match lang {
            Language::TypeScript => {
                let before = trimmed.split_once('(').map(|(before, _)| before.trim());
                (before.and_then(|before| before.split_whitespace().last()) == Some(method))
                    .then(|| line.find(method))
                    .flatten()
            }
            Language::Python => (trimmed.starts_with(&format!("def {method}("))
                || trimmed.starts_with(&format!("async def {method}(")))
            .then(|| line.find(method))
            .flatten(),
            Language::Cpp => {
                let before = trimmed.split_once('(').map(|(before, _)| before.trim());
                (before.is_some_and(|before| {
                    before.split_whitespace().last().is_some_and(|name| {
                        name.trim_start_matches('*').trim_start_matches('&') == method
                    })
                }))
                .then(|| line.find(method))
                .flatten()
            }
            Language::Swift => trimmed
                .contains(&format!("func {method}("))
                .then(|| line.find(method))
                .flatten(),
            Language::Go => trimmed.strip_prefix("func (").and_then(|after| {
                let close = after.find(')')?;
                let receiver = after[..close]
                    .split_whitespace()
                    .last()?
                    .trim_start_matches('*');
                let tail = after[close + 1..].trim_start();
                (receiver == owner && tail.starts_with(&format!("{method}(")))
                    .then(|| line.find(method))
                    .flatten()
            }),
        };
        if let Some(col) = name_at {
            let offset = code
                .split_inclusive('\n')
                .take(idx)
                .map(str::len)
                .sum::<usize>()
                + col;
            return crate::signature::position_at(code, offset);
        }
    }
    anyhow::bail!("cannot locate the selected `{owner}.{method}` declaration")
}

pub fn rewrite_calls_in_code(
    code: &str,
    target_method: &str,
    owner_class: &str,
    lang: Language,
    file_rel: &str,
    blocked: &mut Vec<String>,
    semantic_references: Option<&mut std::collections::HashSet<(u32, u32)>>,
) -> (String, usize) {
    let mut out = String::new();
    let mut rewritten = 0;
    let needle_dot = format!(".{target_method}(");
    let needle_arrow = format!("->{target_method}(");
    let lexical_language = match lang {
        Language::TypeScript => crate::parameter_object::Language::TypeScript,
        Language::Python => crate::parameter_object::Language::Python,
        Language::Cpp => crate::parameter_object::Language::Cpp,
        Language::Swift => crate::parameter_object::Language::Swift,
        Language::Go => crate::parameter_object::Language::Go,
    };

    let mut semantic_references = semantic_references;
    let mut absolute_offset = 0usize;
    for (line_idx, raw_line) in code.split_inclusive('\n').enumerate() {
        let line = raw_line.strip_suffix('\n').unwrap_or(raw_line);
        let ending = &raw_line[line.len()..];
        let line_start = absolute_offset;
        absolute_offset += raw_line.len();
        let trimmed = line.trim_start();
        let comment_prefix = match lang {
            Language::Python => "#",
            _ => "//",
        };
        if trimmed.starts_with(comment_prefix) || trimmed.starts_with('*') {
            out.push_str(raw_line);
            continue;
        }

        let has_needle =
            line.contains(&needle_dot) || (lang == Language::Cpp && line.contains(&needle_arrow));
        if !has_needle {
            out.push_str(raw_line);
            continue;
        }

        let mut current_line = String::new();
        let mut cursor = 0usize;
        loop {
            let next_dot = line[cursor..]
                .find(&needle_dot)
                .map(|p| (cursor + p, false));
            let next_arrow = (lang == Language::Cpp)
                .then(|| {
                    line[cursor..]
                        .find(&needle_arrow)
                        .map(|p| (cursor + p, true))
                })
                .flatten();
            let Some((pos, is_arrow)) = (match (next_dot, next_arrow) {
                (Some(dot), Some(arrow)) => Some(if dot.0 <= arrow.0 { dot } else { arrow }),
                (Some(dot), None) => Some(dot),
                (None, Some(arrow)) => Some(arrow),
                (None, None) => None,
            }) else {
                break;
            };
            let op_len = if is_arrow { 2 } else { 1 };
            let end = pos + op_len + target_method.len() + 1;
            if crate::inline_parameter::is_in_string(code, line_start + pos, lexical_language) {
                current_line.push_str(&line[cursor..end]);
                cursor = end;
                continue;
            }
            let method_offset = line_start + pos + op_len;
            let method_position = crate::signature::position_at(code, method_offset).ok();
            let Some(references) = semantic_references.as_deref_mut() else {
                current_line.push_str(&line[cursor..end]);
                cursor = end;
                continue;
            };
            let Some(method_position) = method_position else {
                current_line.push_str(&line[cursor..end]);
                cursor = end;
                continue;
            };
            if !references.remove(&method_position) {
                current_line.push_str(&line[cursor..end]);
                cursor = end;
                continue;
            }
            let before = &line[..pos];

            if let Some(recv) = extract_receiver(before) {
                if recv == owner_class {
                    current_line.push_str(&line[cursor..end]);
                    cursor = end;
                    continue;
                }
                let site = format!("{file_rel}:{}:{}", line_idx + 1, pos + 1);
                if receiver_has_effects(recv) {
                    blocked.push(format!("{site} `{recv}` is evaluated for what it does"));
                }
                let target_call = match lang {
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
                let recv_start = pos - recv.len();
                current_line.push_str(&line[cursor..recv_start]);
                current_line.push_str(&target_call);
                rewritten += 1;
                cursor = end;
            } else {
                current_line.push_str(&line[cursor..end]);
                cursor = end;
            }
        }
        current_line.push_str(&line[cursor..]);
        out.push_str(&current_line);
        out.push_str(ending);
    }
    (out, rewritten)
}
