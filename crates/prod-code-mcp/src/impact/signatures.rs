/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::diff::Hunk;

pub(crate) fn normalize_signature(lines: &[&str], language: &str) -> String {
    let mut parts = Vec::new();
    for line in lines {
        let mut trimmed = line.trim();
        if language == "python" {
            if let Some(pos) = trimmed.find('#') {
                trimmed = trimmed[..pos].trim();
            }
        } else if let Some(pos) = trimmed.find("//") {
            trimmed = trimmed[..pos].trim();
        }
        if !trimmed.is_empty() {
            parts.push(trimmed);
        }
    }
    let joined = parts.join(" ");
    let mut out = String::new();
    let mut prev_ws = false;
    for c in joined.chars() {
        if c.is_whitespace() {
            if !prev_ws {
                out.push(' ');
                prev_ws = true;
            }
        } else {
            out.push(c);
            prev_ws = false;
        }
    }
    let trimmed =
        out.trim_end_matches(|c: char| c == '{' || c == ':' || c == ';' || c.is_whitespace());
    trimmed.replace("( ", "(").replace(" )", ")").to_string()
}

pub(crate) fn extract_signature_span(
    lines: &[&str],
    line: u32,
    _name: &str,
    language: &str,
) -> Option<(u32, u32, String)> {
    if line == 0 || (line as usize) > lines.len() {
        return None;
    }
    let line_idx = (line - 1) as usize;
    let mut start_idx = line_idx;
    while start_idx > 0 && start_idx + 3 >= line_idx {
        let prev = lines[start_idx - 1].trim();
        if prev.starts_with('@')
            || prev.starts_with("#[")
            || prev.starts_with("template")
            || prev.ends_with("async")
            || prev.ends_with("pub")
            || prev.ends_with("export")
        {
            start_idx -= 1;
        } else {
            break;
        }
    }

    let mut end_idx = line_idx;
    let mut paren_depth = 0i32;
    let mut angle_depth = 0i32;
    let mut bracket_depth = 0i32;
    let mut param_started = false;

    let max_scan = (line_idx + 25).min(lines.len());
    for idx in start_idx..max_scan {
        let line_text = lines[idx];
        let trimmed = line_text.trim();
        if trimmed.starts_with("//") || trimmed.starts_with('#') || trimmed.starts_with('*') {
            continue;
        }

        let mut in_str = false;
        let mut str_char = ' ';
        let mut chars = line_text.char_indices().peekable();

        while let Some((byte_idx, c)) = chars.next() {
            if in_str {
                if c == '\\' {
                    let _ = chars.next();
                } else if c == str_char {
                    in_str = false;
                }
                continue;
            }
            if c == '"' || c == '\'' || c == '`' {
                in_str = true;
                str_char = c;
                continue;
            }

            if c == '/' && chars.peek().map(|&(_, next_c)| next_c) == Some('/') {
                break;
            }
            if language == "python" && c == '#' {
                break;
            }

            match c {
                '(' => {
                    paren_depth += 1;
                    param_started = true;
                }
                ')' => {
                    if paren_depth > 0 {
                        paren_depth -= 1;
                    }
                }
                '<' if !trimmed.starts_with("<-") => {
                    angle_depth += 1;
                }
                '>' if angle_depth > 0 => {
                    angle_depth -= 1;
                }
                '[' => {
                    bracket_depth += 1;
                }
                ']' => {
                    if bracket_depth > 0 {
                        bracket_depth -= 1;
                    }
                }
                ':' if language == "python" && param_started && paren_depth == 0 => {
                    end_idx = idx;
                    let mut sig_lines: Vec<&str> = lines[start_idx..idx].to_vec();
                    sig_lines.push(&line_text[..=byte_idx]);
                    let sig_text = normalize_signature(&sig_lines, language);
                    return Some(((start_idx + 1) as u32, (end_idx + 1) as u32, sig_text));
                }
                '{' | ';'
                    if language != "python"
                        && param_started
                        && paren_depth == 0
                        && angle_depth == 0
                        && bracket_depth == 0 =>
                {
                    end_idx = idx;
                    let mut sig_lines: Vec<&str> = lines[start_idx..idx].to_vec();
                    sig_lines.push(&line_text[..=byte_idx]);
                    let sig_text = normalize_signature(&sig_lines, language);
                    return Some(((start_idx + 1) as u32, (end_idx + 1) as u32, sig_text));
                }
                _ => {}
            }
        }
        end_idx = idx;
    }

    let sig_text = normalize_signature(&lines[start_idx..=end_idx], language);
    Some(((start_idx + 1) as u32, (end_idx + 1) as u32, sig_text))
}

pub(crate) fn base_line_for_new_line(hunks: &[Hunk], new_line: u32) -> Option<u32> {
    let mut hunks = hunks.to_vec();
    hunks.sort_by_key(|hunk| hunk.start);
    let mut delta = 0i64;
    for hunk in hunks {
        if new_line < hunk.start {
            break;
        }
        if hunk.added > 0 && new_line < hunk.start.saturating_add(hunk.added) {
            if hunk.removed == 0 {
                return None;
            }
            let old_start = (hunk.start as i64).checked_sub(delta)?;
            let offset = new_line.saturating_sub(hunk.start);
            let offset = offset.min(hunk.removed - 1);
            return u32::try_from(old_start + offset as i64).ok();
        }
        delta += i64::from(hunk.added) - i64::from(hunk.removed);
    }
    u32::try_from(i64::from(new_line).checked_sub(delta)?).ok()
}

pub(crate) fn find_function_in_text(
    lines: &[&str],
    name: &str,
    language: &str,
    preferred_line: u32,
) -> Option<u32> {
    let mut best: Option<(u32, u32)> = None;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") || trimmed.starts_with('#') || trimmed.starts_with('*') {
            continue;
        }
        let matched = match language {
            "python" => {
                if let Some(rest) = trimmed.strip_prefix("def ") {
                    let rest = rest.trim_start();
                    rest.starts_with(name)
                        && rest[name.len()..]
                            .chars()
                            .next()
                            .is_none_or(|c| !c.is_alphanumeric() && c != '_')
                } else if let Some(rest) = trimmed.strip_prefix("async def ") {
                    let rest = rest.trim_start();
                    rest.starts_with(name)
                        && rest[name.len()..]
                            .chars()
                            .next()
                            .is_none_or(|c| !c.is_alphanumeric() && c != '_')
                } else {
                    false
                }
            }
            "rust" => {
                let has_fn = trimmed.contains("fn ");
                if has_fn {
                    let pattern1 = format!("fn {name}");
                    let pattern2 = format!("fn r#{name}");
                    if let Some(pos) = trimmed.find(&pattern1) {
                        let after = pos + pattern1.len();
                        trimmed[after..]
                            .chars()
                            .next()
                            .is_none_or(|c| !c.is_alphanumeric() && c != '_')
                    } else if let Some(pos) = trimmed.find(&pattern2) {
                        let after = pos + pattern2.len();
                        trimmed[after..]
                            .chars()
                            .next()
                            .is_none_or(|c| !c.is_alphanumeric() && c != '_')
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            "go" => {
                if let Some(rest) = trimmed.strip_prefix("func ") {
                    let rest = rest.trim_start();
                    if let Some(tail) = rest.strip_prefix(name) {
                        tail.chars()
                            .next()
                            .is_none_or(|c| !c.is_alphanumeric() && c != '_')
                    } else if let Some(close_paren) = rest.find(')') {
                        let after = rest[close_paren + 1..].trim_start();
                        after.strip_prefix(name).is_some_and(|tail| {
                            tail.chars()
                                .next()
                                .is_none_or(|c| !c.is_alphanumeric() && c != '_')
                        })
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            "typescript" | "javascript" => {
                let has_decl = trimmed.contains("function")
                    || trimmed.contains("=>")
                    || trimmed.contains(&format!("{name}("))
                    || trimmed.contains(&format!("{name} ="))
                    || trimmed.contains(&format!("{name}:"))
                    || trimmed.contains(&format!("{name} :"))
                    || trimmed.contains(&format!("{name}<"));
                if has_decl {
                    if let Some(pos) = trimmed.find(name) {
                        let before_ok = if pos == 0 {
                            true
                        } else {
                            let prev = trimmed[..pos].chars().last().unwrap();
                            !prev.is_alphanumeric() && prev != '_' && prev != '.'
                        };
                        let after_pos = pos + name.len();
                        let after_ok = trimmed[after_pos..]
                            .chars()
                            .next()
                            .is_none_or(|c| !c.is_alphanumeric() && c != '_');
                        before_ok && after_ok
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            _ => {
                let has_decl = trimmed.contains("func ")
                    || trimmed.contains("function ")
                    || trimmed.contains("def ")
                    || trimmed.contains("fn ")
                    || trimmed.contains(&format!("{name}("))
                    || trimmed.contains(&format!("{name}<"));
                if has_decl {
                    if let Some(pos) = trimmed.find(name) {
                        let before_ok = if pos == 0 {
                            true
                        } else {
                            let prev = trimmed[..pos].chars().last().unwrap();
                            !prev.is_alphanumeric() && prev != '_' && prev != '.'
                        };
                        let after_pos = pos + name.len();
                        let after_ok = trimmed[after_pos..]
                            .chars()
                            .next()
                            .is_none_or(|c| !c.is_alphanumeric() && c != '_');
                        before_ok && after_ok
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
        };
        if matched {
            let line = (i + 1) as u32;
            let distance = line.abs_diff(preferred_line);
            if best.is_none_or(|(_, best_distance)| distance < best_distance) {
                best = Some((line, distance));
            }
        }
    }
    best.map(|(line, _)| line)
}
