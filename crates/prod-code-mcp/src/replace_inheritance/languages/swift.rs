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

pub fn transform_swift(
    text: &str,
    sub: &ClassDecl,
    base_name: &str,
    field_name: &str,
    methods: &[String],
    base_class: Option<&ClassDecl>,
) -> Result<String> {
    let mut out = text.to_string();

    // 1. Remove only the selected base while preserving protocol conformances.
    let sub_slice = &out[sub.decl_start..sub.body_start];
    let header_open = sub_slice
        .find('{')
        .context("class declaration has no body brace")?;
    let header = &sub_slice[..header_open];
    if let Some(colon_pos) = header.find(':') {
        let bases = header[colon_pos + 1..]
            .split(',')
            .map(str::trim)
            .filter(|base| !base.is_empty())
            .collect::<Vec<_>>();
        let selected_count = bases
            .iter()
            .filter(|base| swift_base_matches(base, base_name))
            .count();
        anyhow::ensure!(
            selected_count == 1,
            "cannot uniquely identify selected Swift base `{base_name}`; nothing was written"
        );
        let remaining = bases
            .into_iter()
            .filter(|base| !swift_base_matches(base, base_name))
            .collect::<Vec<_>>();
        let prefix = header[..colon_pos].trim_end();
        let new_header = if remaining.is_empty() {
            format!("{prefix} {}", &sub_slice[header_open..])
        } else {
            format!(
                "{prefix}: {} {}",
                remaining.join(", "),
                &sub_slice[header_open..]
            )
        };
        out.replace_range(sub.decl_start..sub.body_start, &new_header);
    }

    // Re-parse to get fresh offsets
    let classes_now = parse_classes_in_text(&out, "swift", &sub.file_path);
    let cur_sub = classes_now
        .into_iter()
        .find(|c| c.name == sub.name)
        .with_context(|| format!("failed to re-locate class '{}'", sub.name))?;

    let indent = &cur_sub.indent;
    let mut additions = Vec::new();

    // 2. Delegate field declaration
    let field_decl = format!("{indent}private let {field_name}: {base_name}");
    additions.push(field_decl);

    // 3. Constructor handling
    let sub_body = &out[cur_sub.body_start..cur_sub.body_end];
    if sub_body.contains("init(") {
        let replaced_body =
            sub_body.replace("super.init(", &format!("self.{field_name} = {base_name}("));
        out.replace_range(cur_sub.body_start..cur_sub.body_end, &replaced_body);
    } else {
        let new_init =
            format!("{indent}init() {{\n{indent}    self.{field_name} = {base_name}()\n{indent}}}");
        additions.push(new_init);
    }

    // 4. Generate forwarding methods
    for m in methods {
        let base_method = base_class
            .and_then(|base| base.members.iter().find(|member| member.name == *m))
            .with_context(|| {
                format!("cannot safely forward Swift method `{m}` without its base signature")
            })?;
        additions.push(swift_forwarding_method(
            &base_method.full_text,
            m,
            field_name,
            indent,
        )?);
    }

    // Insert additions before closing brace of subclass
    let classes_updated = parse_classes_in_text(&out, "swift", &sub.file_path);
    let final_sub = classes_updated
        .into_iter()
        .find(|c| c.name == sub.name)
        .with_context(|| format!("failed to re-locate class '{}'", sub.name))?;

    let insert_block = additions.join("\n\n");
    let insert_pos = final_sub.body_end;
    let formatted = format!("\n{insert_block}\n");
    out.insert_str(insert_pos, &formatted);

    // 5. Strip `override` from remaining subclass methods
    let classes_final = parse_classes_in_text(&out, "swift", &sub.file_path);
    if let Some(sub_after) = classes_final.into_iter().find(|c| c.name == sub.name) {
        let body_slice = &out[sub_after.body_start..sub_after.body_end];
        let stripped_body = strip_override_modifiers(body_slice, "swift");
        let rewritten_body = stripped_body.replace("super.", &format!("self.{field_name}."));
        out.replace_range(sub_after.body_start..sub_after.body_end, &rewritten_body);
    }

    Ok(out)
}

fn swift_forwarding_method(
    declaration: &str,
    expected_name: &str,
    field_name: &str,
    indent: &str,
) -> Result<String> {
    let declaration_func_start = declaration
        .find("func ")
        .context("Swift base method has no `func` declaration")?;
    let body_start = declaration[declaration_func_start..]
        .find('{')
        .map_or(declaration.len(), |offset| declaration_func_start + offset);
    let header = declaration[..body_start].trim();
    let func_start = header
        .find("func ")
        .context("Swift base method header has no `func` declaration")?;
    let open = header[func_start..]
        .find('(')
        .map(|offset| func_start + offset)
        .context("Swift base method has no parameter list")?;
    let close = crate::parameter_object::matching_bracket(header, open)
        .context("Swift base method has an invalid parameter list")?;
    let method_name = header[func_start + "func ".len()..open].trim();
    anyhow::ensure!(
        method_name == expected_name,
        "Swift method signature `{method_name}` does not match `{expected_name}` (header: `{header}`)"
    );
    let mut call_args = Vec::new();
    let raw_parameters = &header[open + 1..close];
    for parameter in crate::replace_constructor::split_balanced_commas(raw_parameters) {
        let parameter = parameter.trim();
        if parameter.is_empty() {
            continue;
        }
        anyhow::ensure!(
            !parameter.contains("..."),
            "cannot safely forward variadic Swift parameter `{parameter}`"
        );
        let (labels, ty) = parameter
            .split_once(':')
            .with_context(|| format!("cannot parse Swift parameter `{parameter}`"))?;
        let names = labels.split_whitespace().collect::<Vec<_>>();
        let local = names.last().copied().unwrap_or("");
        anyhow::ensure!(
            !local.is_empty() && local.chars().all(|c| c.is_alphanumeric() || c == '_'),
            "cannot safely forward Swift parameter `{parameter}`"
        );
        let label = if names.len() > 1 { names[0] } else { local };
        let value = if ty.trim_start().starts_with("inout ") {
            format!("&{local}")
        } else {
            local.to_string()
        };
        call_args.push(if label == "_" {
            value
        } else {
            format!("{label}: {value}")
        });
    }
    let suffix = header[close + 1..].trim();
    let mut call_prefix = String::new();
    if suffix.contains("throws") || suffix.contains("rethrows") {
        call_prefix.push_str("try ");
    }
    if suffix.contains("async") {
        call_prefix.push_str("await ");
    }
    let has_value = suffix
        .split_once("->")
        .map(|(_, result)| result.trim().split_whitespace().next().unwrap_or(""))
        .is_some_and(|result| result != "Void" && result != "()" && !result.is_empty());
    let return_prefix = if has_value { "return " } else { "" };
    let mut clean_header = header.to_string();
    for modifier in ["override ", "final "] {
        clean_header = clean_header.replace(modifier, "");
    }
    Ok(format!(
        "{indent}{clean_header} {{\n{indent}    {return_prefix}{call_prefix}self.{field_name}.{expected_name}({})\n{indent}}}",
        call_args.join(", ")
    ))
}

fn swift_base_matches(specifier: &str, base_name: &str) -> bool {
    let specifier = specifier
        .trim()
        .split('<')
        .next()
        .unwrap_or(specifier)
        .trim();
    let base = base_name
        .trim()
        .split('<')
        .next()
        .unwrap_or(base_name)
        .trim();
    specifier == base
        || specifier.rsplit('.').next() == Some(base)
        || specifier.rsplit('.').next() == base.rsplit('.').next()
}
