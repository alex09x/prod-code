/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::parameter_object::Language;

/// Identifies return type and async status of the innermost enclosing function.
pub fn enclosing_polyglot_info(content: &str, at: usize, lang: Language) -> Option<(String, bool)> {
    if lang == Language::Python {
        let lines: Vec<&str> = content[..at].lines().collect();
        let target_line = lines.last()?;
        let target_indent = target_line.len() - target_line.trim_start().len();
        for line in lines.iter().rev().skip(1) {
            let trimmed = line.trim();
            if trimmed.starts_with("def ") || trimmed.starts_with("async def ") {
                let indent = line.len() - line.trim_start().len();
                if indent < target_indent {
                    let is_async = trimmed.starts_with("async def ");
                    let ret = if let Some(arr) = trimmed.find("->") {
                        trimmed[arr + 2..trimmed.rfind(':').unwrap_or(trimmed.len())]
                            .trim()
                            .to_string()
                    } else {
                        "None".to_string()
                    };
                    return Some((ret, is_async));
                }
            }
        }
        return None;
    }

    // C-like bracket languages
    let mut search = at;
    while let Some(open_rel) = content[..search].rfind('{') {
        search = open_rel;
        let Some(close_b) = crate::parameter_object::matching_bracket(content, open_rel) else {
            continue;
        };
        if open_rel < at && at < close_b {
            let line_start = content[..open_rel].rfind('\n').map_or(0, |p| p + 1);
            let header = &content[line_start..open_rel];
            let is_async = header.contains("async");

            let ret_type = match lang {
                Language::TypeScript | Language::JavaScript => {
                    let end_h = if let Some(arr) = header.find("=>") {
                        arr
                    } else {
                        header.len()
                    };
                    if let Some(colon) = header[..end_h].rfind(':') {
                        header[colon + 1..end_h].trim().to_string()
                    } else {
                        String::new()
                    }
                }
                Language::Swift => {
                    if let Some(arr) = header.find("->") {
                        header[arr + 2..].trim().to_string()
                    } else {
                        "Void".to_string()
                    }
                }
                Language::Go => {
                    if let Some(func_pos) = header.find("func ") {
                        let after_func = &header[func_pos + 5..];
                        let trimmed = after_func.trim_start();
                        let offset = func_pos + 5 + (after_func.len() - trimmed.len());
                        if trimmed.starts_with('(') {
                            // Receiver present: func (r Recv) Name(params) RetType
                            if let Some(recv_close) =
                                crate::parameter_object::matching_bracket(header, offset)
                            {
                                if let Some(param_open_rel) = header[recv_close + 1..].find('(') {
                                    let param_open = recv_close + 1 + param_open_rel;
                                    if let Some(param_close) =
                                        crate::parameter_object::matching_bracket(
                                            header, param_open,
                                        )
                                    {
                                        header[param_close + 1..].trim().to_string()
                                    } else {
                                        String::new()
                                    }
                                } else {
                                    String::new()
                                }
                            } else {
                                String::new()
                            }
                        } else if let Some(param_open_rel) = trimmed.find('(') {
                            let param_open = offset + param_open_rel;
                            if let Some(param_close) =
                                crate::parameter_object::matching_bracket(header, param_open)
                            {
                                header[param_close + 1..].trim().to_string()
                            } else {
                                String::new()
                            }
                        } else {
                            String::new()
                        }
                    } else {
                        String::new()
                    }
                }
                Language::Cpp | Language::C => {
                    if let Some(paren_open) = header.find('(') {
                        let before_paren = header[..paren_open].trim();
                        let words: Vec<&str> = before_paren.split_whitespace().collect();
                        if words.len() >= 2 {
                            words[..words.len() - 1].join(" ")
                        } else {
                            words.join(" ")
                        }
                    } else {
                        String::new()
                    }
                }
                _ => String::new(),
            };
            return Some((ret_type, is_async));
        }
    }
    None
}
