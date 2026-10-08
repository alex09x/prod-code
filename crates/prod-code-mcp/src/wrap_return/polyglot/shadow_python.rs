/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub(crate) fn is_python_shadowed(content: &str, at: usize, name: &str) -> bool {
    let lines: Vec<&str> = content[..at].lines().collect();
    let target_line = match lines.last() {
        Some(l) => *l,
        None => return false,
    };
    let target_indent = target_line.len() - target_line.trim_start().len();
    for (i, line) in lines.iter().enumerate().rev().skip(1) {
        let trimmed = line.trim();
        if trimmed.starts_with("def ") || trimmed.starts_with("async def ") {
            let indent = line.len() - line.trim_start().len();
            if indent < target_indent {
                let mut header = line.to_string();
                let mut body_start = i + 1;
                while !header.contains(')') && body_start < lines.len() {
                    header.push(' ');
                    header.push_str(lines[body_start].trim());
                    body_start += 1;
                }
                if let (Some(open), Some(close)) = (header.find('('), header.rfind(')')) {
                    let params = &header[open + 1..close];
                    for p in params.split(',') {
                        let p = p
                            .trim()
                            .split(':')
                            .next()
                            .unwrap()
                            .split('=')
                            .next()
                            .unwrap()
                            .trim();
                        if p == name {
                            return true;
                        }
                    }
                }
                let mut nested_fn_indent: Option<usize> = None;
                for body_line in &lines[body_start..lines.len() - 1] {
                    if body_line.trim().is_empty() {
                        continue;
                    }
                    let line_indent = body_line.len() - body_line.trim_start().len();
                    if let Some(fn_indent) = nested_fn_indent {
                        if line_indent > fn_indent {
                            continue;
                        } else {
                            nested_fn_indent = None;
                        }
                    }

                    let b_trimmed = body_line.trim();
                    if b_trimmed.starts_with("def ")
                        || b_trimmed.starts_with("async def ")
                        || b_trimmed.starts_with("class ")
                    {
                        nested_fn_indent = Some(line_indent);
                        continue;
                    }

                    if line_binds_python_name(b_trimmed, name) {
                        return true;
                    }
                }
                break;
            }
        }
    }
    false
}

fn line_binds_python_name(line: &str, name: &str) -> bool {
    let code = line.split('#').next().unwrap_or("").trim();
    for stmt in code.split(';') {
        let s = stmt.trim();
        if let Some(rest) = s.strip_prefix("from ") {
            if let Some((_mod, clause)) = rest.split_once(" import ") {
                let clause = clause.trim().trim_start_matches('(').trim_end_matches(')');
                for item in clause.split(',') {
                    let local = if let Some((_, alias)) = item.split_once(" as ") {
                        alias.trim()
                    } else {
                        item.trim()
                    };
                    if local == name {
                        return true;
                    }
                }
            }
        } else if let Some(rest) = s.strip_prefix("import ") {
            for item in rest.split(',') {
                let local = if let Some((_, alias)) = item.split_once(" as ") {
                    alias.trim()
                } else {
                    item.trim().split('.').next().unwrap_or("").trim()
                };
                if local == name {
                    return true;
                }
            }
        } else if let Some((lhs, _)) = s.split_once('=') {
            let var = lhs.trim().split(':').next().unwrap().trim();
            if var == name {
                return true;
            }
        }
    }
    false
}
