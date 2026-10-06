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

use anyhow::{Context, Result};

use super::params::extract_param_names_cpp;
use crate::extract_delegate::common::{is_ident, is_ident_str, reindent};
use crate::parameter_object::Language;

pub fn restructure_cpp(
    text: &str,
    symbol_opt: Option<&str>,
    line_opt: Option<u32>,
    fields: &[String],
    methods: &[String],
    helper: &str,
    field: &str,
) -> Result<(String, String)> {
    let (owner_start, open_brace, close_brace, owner) = {
        let mut found = None;
        for keyword in ["class ", "struct "] {
            for (i, _) in text.match_indices(keyword) {
                if i > 0 && text[..i].chars().next_back().is_some_and(is_ident) {
                    continue;
                }
                let after = &text[i + keyword.len()..];
                let name: String = after.chars().take_while(|c| is_ident(*c)).collect();
                if name.is_empty() {
                    continue;
                }
                if let Some(sym) = symbol_opt
                    && sym != name
                {
                    continue;
                }
                let Some(open_rel) = after.find('{') else {
                    continue;
                };
                let open = i + keyword.len() + open_rel;
                let Some(close) = crate::parameter_object::matching_bracket(text, open) else {
                    continue;
                };
                if let Some(l) = line_opt
                    && l > 0
                {
                    let (sl, _) = crate::signature::position_at(text, i)?;
                    let (el, _) = crate::signature::position_at(text, close)?;
                    if l < sl || l > el {
                        continue;
                    }
                }
                found = Some((i, open, close, name));
                break;
            }
            if found.is_some() {
                break;
            }
        }
        found.with_context(|| {
            if let Some(sym) = symbol_opt {
                format!("class or struct `{sym}` not found")
            } else {
                "no class/struct found at the specified location".to_string()
            }
        })?
    };

    let body = &text[open_brace + 1..close_brace];

    let mut field_decls: BTreeMap<String, String> = BTreeMap::new();
    let mut field_types: BTreeMap<String, String> = BTreeMap::new();
    let mut brace_depth = 0i32;
    let mut body_offset = 0usize;
    for line in body.split_inclusive('\n') {
        let trimmed = line.trim();
        if brace_depth == 0
            && !trimmed.starts_with("//")
            && !trimmed.starts_with("/*")
            && !trimmed.starts_with('*')
            && let Some(semi) = trimmed.strip_suffix(';')
        {
            let decl = semi.trim();
            if !decl.contains('(') {
                let parts: Vec<&str> = decl.split_whitespace().collect();
                if parts.len() >= 2 {
                    let fname = parts.last().unwrap().trim_matches(|c| !is_ident(c));
                    let ty = parts[..parts.len() - 1].join(" ");
                    if is_ident_str(fname) {
                        field_decls.insert(fname.to_string(), line.to_string());
                        field_types.insert(fname.to_string(), ty);
                    }
                }
            }
        }
        for (offset, ch) in line.char_indices() {
            let absolute = body_offset + offset;
            if crate::inline_parameter::is_in_comment(body, absolute, Language::Cpp)
                || crate::inline_parameter::is_in_string(body, absolute, Language::Cpp)
            {
                continue;
            }
            match ch {
                '{' => brace_depth += 1,
                '}' => brace_depth -= 1,
                _ => {}
            }
        }
        body_offset += line.len();
    }

    for f in fields {
        anyhow::ensure!(field_decls.contains_key(f), "`{owner}` has no field `{f}`");
    }
    anyhow::ensure!(
        !field_decls.contains_key(field),
        "`{owner}` already has a field `{field}`"
    );

    struct CppMethod {
        sig: String,
        params: String,
        full_text: String,
        body: String,
        start_line: usize,
        end_line: usize,
    }
    let mut moved_methods: BTreeMap<String, CppMethod> = BTreeMap::new();
    let mut offset = 0;
    while offset < body.len() {
        let slice = &body[offset..];
        let Some(paren_rel) = slice.find('(') else {
            break;
        };
        let paren_pos = offset + paren_rel;
        let before_paren = body[offset..paren_pos].trim();
        let last_word = before_paren.split_whitespace().last().unwrap_or("");
        if is_ident_str(last_word)
            && last_word != owner
            && last_word != "if"
            && last_word != "while"
            && last_word != "for"
            && let Some(close_paren_rel) = body[paren_pos..].find(')')
        {
            let close_paren = paren_pos + close_paren_rel;
            let params = body[paren_pos + 1..close_paren].trim().to_string();
            if let Some(open_b_rel) = body[close_paren + 1..].find('{') {
                let open_b = close_paren + 1 + open_b_rel;
                if let Some(close_b_rel) =
                    crate::parameter_object::matching_bracket(&body[open_b..], 0)
                {
                    let close_b = open_b + close_b_rel;
                    let line_start = body
                        [..offset + slice[..paren_rel].rfind('\n').map_or(0, |x| x + 1)]
                        .rfind('\n')
                        .map_or(0, |x| x + 1);
                    let sig = body[line_start..open_b].trim().to_string();
                    let m_body = body[open_b + 1..close_b].to_string();
                    let full_text = body[line_start..=close_b].trim().to_string();
                    let start_line = body[..line_start].matches('\n').count();
                    let end_line = body[..close_b].matches('\n').count();
                    let mname = last_word.to_string();
                    if methods.contains(&mname) {
                        moved_methods.insert(
                            mname,
                            CppMethod {
                                sig,
                                params,
                                full_text,
                                body: m_body,
                                start_line,
                                end_line,
                            },
                        );
                    }
                    offset = close_b + 1;
                    continue;
                }
            }
        }
        offset = paren_pos + 1;
    }

    for m in methods {
        anyhow::ensure!(
            moved_methods.contains_key(m),
            "`{owner}` has no method `{m}`"
        );
    }

    for (mname, minfo) in &moved_methods {
        for fname in field_decls.keys() {
            if !fields.contains(fname) {
                let direct_use =
                    minfo.body.contains(&format!("this->{fname}")) || minfo.body.contains(fname);
                if direct_use {
                    anyhow::bail!("`{mname}` uses `{fname}`, which does not move to `{helper}`");
                }
            }
        }
    }

    let mut helper_lines = Vec::new();
    helper_lines.push(format!("class {helper} {{"));
    helper_lines.push("public:".to_string());
    for f in fields {
        let ty = field_types.get(f).map(|s| s.as_str()).unwrap_or("auto");
        helper_lines.push(format!("    {ty} {f};"));
    }
    helper_lines.push(String::new());
    helper_lines.push(format!("    {helper}() = default;"));
    let ctor_params = fields
        .iter()
        .map(|f| {
            let ty = field_types.get(f).map(|s| s.as_str()).unwrap_or("auto");
            format!("{ty} {f}")
        })
        .collect::<Vec<_>>()
        .join(", ");
    let ctor_init = fields
        .iter()
        .map(|f| format!("{f}({f})"))
        .collect::<Vec<_>>()
        .join(", ");
    helper_lines.push(format!("    {helper}({ctor_params}) : {ctor_init} {{}}"));
    for m in methods {
        let minfo = moved_methods.get(m).unwrap();
        helper_lines.push(String::new());
        helper_lines.push(reindent(&minfo.full_text, "    "));
    }
    helper_lines.push("};".to_string());
    let helper_text = helper_lines.join("\n");

    let mut new_body_lines = Vec::new();
    let mut delegate_placed = false;

    for (line_index, line) in body.lines().enumerate() {
        if moved_methods
            .values()
            .any(|minfo| minfo.start_line <= line_index && line_index <= minfo.end_line)
        {
            continue;
        }
        let trimmed = line.trim();
        let is_moved_field = fields.iter().any(|f| {
            if let Some(decl) = field_decls.get(f) {
                decl.trim() == trimmed
            } else {
                false
            }
        });
        if is_moved_field {
            if !delegate_placed {
                new_body_lines.push(format!("    {helper} {field};"));
                delegate_placed = true;
            }
            continue;
        }

        if trimmed.starts_with(':') || (trimmed.contains(':') && trimmed.contains('(')) {
            let mut updated_line = line.to_string();
            let mut init_args = Vec::new();
            for f in fields {
                if let Some(pos) = updated_line.find(&format!("{f}(")) {
                    let after = &updated_line[pos + f.len() + 1..];
                    if let Some(end) = after.find(')') {
                        let arg = &after[..end];
                        init_args.push(arg.to_string());
                    }
                }
            }
            if !init_args.is_empty() {
                for f in fields {
                    if let Some(pos) = updated_line.find(&format!("{f}(")) {
                        let after = &updated_line[pos + f.len() + 1..];
                        if let Some(end) = after.find(')') {
                            let full_term = &updated_line[pos..pos + f.len() + 1 + end + 1];
                            updated_line = updated_line.replace(full_term, "");
                        }
                    }
                }
                updated_line = updated_line.replace(", ,", ",");
                if let Some(colon_pos) = updated_line.find(':') {
                    let before_colon = &updated_line[..colon_pos + 1];
                    let after_colon = updated_line[colon_pos + 1..].trim();
                    let joined = init_args.join(", ");
                    if after_colon.is_empty() || after_colon.starts_with('{') {
                        updated_line = format!("{before_colon} {field}({joined}) {after_colon}");
                    } else {
                        updated_line = format!("{before_colon} {field}({joined}), {after_colon}");
                    }
                }
            }
            new_body_lines.push(updated_line);
            continue;
        }

        new_body_lines.push(line.to_string());
    }

    if !delegate_placed {
        new_body_lines.insert(0, format!("    {helper} {field};"));
    }

    for m in methods {
        let minfo = moved_methods.get(m).unwrap();
        let arg_names = extract_param_names_cpp(&minfo.params);
        new_body_lines.push(String::new());
        new_body_lines.push(format!(
            "    {} {{\n        return {field}.{m}({});\n    }}",
            minfo.sig,
            arg_names.join(", ")
        ));
    }

    let new_body = new_body_lines.join("\n");
    let mut out = text.to_string();
    out.replace_range(open_brace + 1..close_brace, &format!("\n{new_body}\n"));
    out.insert_str(owner_start, &format!("{helper_text}\n\n"));
    Ok((out, owner))
}
