/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};

use crate::pull_push::{ClassDecl, parse_classes_in_text, strip_override_modifiers};

pub fn transform_cpp(
    text: &str,
    sub: &ClassDecl,
    base_name: &str,
    field_name: &str,
    methods: &[String],
    base_class: Option<&ClassDecl>,
) -> Result<String> {
    let mut out = text.to_string();

    // Replace selected base constructor initializers before removing inheritance. This also
    // initializes the delegate when the original base has no default constructor.
    let mut constructor_edits = Vec::new();
    for member in sub.members.iter().filter(|member| member.name == sub.name) {
        let rewritten = rewrite_cpp_base_initializer(&member.full_text, base_name, field_name);
        if rewritten != member.full_text {
            constructor_edits.push((member.start_offset, member.end_offset, rewritten));
        }
    }
    for (start, end, replacement) in constructor_edits.into_iter().rev() {
        out.replace_range(start..end, &replacement);
    }

    // 1. Remove only the selected base specifier and preserve every other base.
    let sub_slice = &out[sub.decl_start..sub.body_start];
    let header_open = sub_slice
        .find('{')
        .context("class declaration has no body brace")?;
    let header = &sub_slice[..header_open];
    if let Some(colon_pos) = cpp_inheritance_separator(header) {
        let bases = crate::replace_constructor::split_balanced_commas(&header[colon_pos + 1..]);
        let selected_count = bases
            .iter()
            .filter(|base| cpp_base_matches(base, base_name))
            .count();
        anyhow::ensure!(
            selected_count == 1,
            "cannot uniquely identify selected C++ base `{base_name}`; nothing was written"
        );
        let remaining = bases
            .into_iter()
            .filter(|base| !cpp_base_matches(base, base_name))
            .collect::<Vec<_>>();
        let prefix = header[..colon_pos].trim_end();
        let new_header = if remaining.is_empty() {
            format!("{prefix} {}", &sub_slice[header_open..])
        } else {
            format!(
                "{prefix} : {} {}",
                remaining.join(", "),
                &sub_slice[header_open..]
            )
        };
        out.replace_range(sub.decl_start..sub.body_start, &new_header);
    }

    // Re-parse to get fresh offsets
    let classes_now = parse_classes_in_text(&out, "cpp", &sub.file_path);
    let cur_sub = classes_now
        .into_iter()
        .find(|c| c.name == sub.name)
        .with_context(|| format!("failed to re-locate class '{}'", sub.name))?;

    let indent = &cur_sub.indent;
    let mut additions = Vec::new();

    // Put the delegate before data members so its constructor runs before member
    // initializers, matching the former base-subobject initialization order.
    let class_kind = out[cur_sub.decl_start..].starts_with("struct ");
    let restored_access = if class_kind { "public:" } else { "private:" };
    let field_decl =
        format!("\n{indent}private:\n{indent}{base_name} {field_name};\n{indent}{restored_access}");
    out.insert_str(cur_sub.body_start, &field_decl);

    // 3. Generate forwarding methods
    let mut fwd_methods = Vec::new();
    for m in methods {
        let sig = if let Some(base) = base_class {
            base.members.iter().find(|bm| bm.name == *m)
        } else {
            None
        };

        let fwd = if let Some(base_m) = sig {
            let m_text = &base_m.full_text;
            let open_brace = m_text.find('{').unwrap_or(m_text.len());
            let header = m_text[..open_brace].trim();
            let parameter_open = header
                .find('(')
                .context("C++ base method has no parameter list")?;
            let parameter_close = crate::parameter_object::matching_bracket(header, parameter_open)
                .context("C++ base method has an invalid parameter list")?;
            let args = cpp_parameter_names(&header[parameter_open + 1..parameter_close])?;
            let clean_header = header
                .replace("override", "")
                .replace("final", "")
                .replace("virtual ", "")
                .replace("= 0", "")
                .trim()
                .to_string();
            format!(
                "{indent}{clean_header} {{\n{indent}    return {field_name}.{m}({args});\n{indent}}}"
            )
        } else {
            format!(
                "{indent}template <typename... Args>\n{indent}auto {m}(Args&&... args) -> decltype(auto) {{\n{indent}    return {field_name}.{m}(std::forward<Args>(args)...);\n{indent}}}"
            )
        };
        fwd_methods.push(fwd);
    }

    if !fwd_methods.is_empty() {
        additions.push(format!("public:\n{}", fwd_methods.join("\n\n")));
    }

    // Insert additions before closing brace of subclass
    let classes_updated = parse_classes_in_text(&out, "cpp", &sub.file_path);
    let final_sub = classes_updated
        .into_iter()
        .find(|c| c.name == sub.name)
        .with_context(|| format!("failed to re-locate class '{}'", sub.name))?;

    let insert_block = additions.join("\n\n");
    let insert_pos = final_sub.body_end;
    let formatted = format!("\n{insert_block}\n");
    out.insert_str(insert_pos, &formatted);

    // 4. Strip `override` and `final` from remaining subclass methods
    let classes_final = parse_classes_in_text(&out, "cpp", &sub.file_path);
    if let Some(sub_after) = classes_final.into_iter().find(|c| c.name == sub.name) {
        let body_slice = &out[sub_after.body_start..sub_after.body_end];
        let stripped_body = strip_override_modifiers(body_slice, "cpp");
        let rewritten_body =
            stripped_body.replace(&format!("{base_name}::"), &format!("{field_name}."));
        out.replace_range(sub_after.body_start..sub_after.body_end, &rewritten_body);
    }

    Ok(out)
}

pub(crate) fn cpp_inheritance_separator(header: &str) -> Option<usize> {
    let bytes = header.as_bytes();
    let mut angle_depth = 0usize;
    for (i, byte) in bytes.iter().enumerate() {
        match byte {
            b'<' => angle_depth += 1,
            b'>' => angle_depth = angle_depth.saturating_sub(1),
            b':' if angle_depth == 0
                && bytes.get(i.wrapping_sub(1)) != Some(&b':')
                && bytes.get(i + 1) != Some(&b':') =>
            {
                return Some(i);
            }
            _ => {}
        }
    }
    None
}

pub(crate) fn cpp_base_matches(specifier: &str, base_name: &str) -> bool {
    let name = specifier
        .split_whitespace()
        .filter(|part| !matches!(*part, "public" | "protected" | "private" | "virtual"))
        .last()
        .unwrap_or("")
        .split('<')
        .next()
        .unwrap_or("");
    let requested = base_name.split('<').next().unwrap_or(base_name).trim();
    name == requested
        || name.rsplit("::").next() == Some(requested.rsplit("::").next().unwrap_or(requested))
}

pub(crate) fn rewrite_cpp_base_initializer(
    text: &str,
    base_name: &str,
    field_name: &str,
) -> String {
    let Some(params_open) = text.find('(') else {
        return text.to_string();
    };
    let Some(params_close) = crate::parameter_object::matching_bracket(text, params_open) else {
        return text.to_string();
    };
    let body_open = text[params_close + 1..]
        .find('{')
        .map_or(text.len(), |at| params_close + 1 + at);
    let header = &text[params_close + 1..body_open];
    let Some(colon) = cpp_inheritance_separator(header) else {
        return text.to_string();
    };
    let initializers = crate::replace_constructor::split_balanced_commas(&header[colon + 1..]);
    let mut found = false;
    let rewritten = initializers
        .into_iter()
        .map(|initializer| {
            let init = initializer.trim();
            let name_end = init.find(['(', '{']).unwrap_or(init.len());
            let init_name = init[..name_end].trim();
            if !found
                && (init_name == base_name || init_name.rsplit("::").next() == Some(base_name))
            {
                found = true;
                init.replacen(init_name, field_name, 1)
            } else {
                initializer
            }
        })
        .collect::<Vec<_>>()
        .join(",");
    if !found {
        return text.to_string();
    }
    let colon_global = params_close + 1 + colon;
    format!(
        "{}: {}{}",
        text[..colon_global].trim_end(),
        rewritten,
        &text[body_open..]
    )
}

pub(crate) fn cpp_parameter_names(parameters: &str) -> Result<String> {
    if parameters.trim().is_empty() {
        return Ok(String::new());
    }
    crate::replace_constructor::split_balanced_commas(parameters)
        .into_iter()
        .map(|parameter| {
            let declaration = parameter
                .split_once('=')
                .map_or(parameter.as_str(), |(left, _)| left)
                .trim();
            let candidate = declaration
                .split_whitespace()
                .last()
                .unwrap_or("")
                .trim_start_matches(['*', '&']);
            anyhow::ensure!(
                !candidate.is_empty()
                    && candidate.chars().all(|c| c.is_alphanumeric() || c == '_')
                    && !matches!(candidate, "const" | "volatile" | "noexcept"),
                "cannot safely forward unnamed or complex C++ parameter `{parameter}`"
            );
            let tokens = declaration.split_whitespace().count();
            anyhow::ensure!(
                tokens >= 2,
                "cannot safely forward unnamed C++ parameter `{parameter}`"
            );
            Ok(candidate.to_string())
        })
        .collect::<Result<Vec<_>>>()
        .map(|names| names.join(", "))
}
