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

use crate::pull_push::{ClassDecl, parse_classes_in_text};

pub fn transform_python(
    text: &str,
    sub: &ClassDecl,
    base_name: &str,
    field_name: &str,
    methods: &[String],
    _base_class: Option<&ClassDecl>,
) -> Result<String> {
    let mut out = text.to_string();

    // 1. Remove base from class header
    // e.g. `class Dog(Animal):` -> `class Dog:` or `class Dog(Animal, Other):` -> `class Dog(Other):`
    let sub_slice = &out[sub.decl_start..sub.body_start];
    let class_line_end = sub_slice.find('\n').unwrap_or(sub_slice.len());
    let class_header = &sub_slice[..class_line_end];

    if let Some(open_paren) = class_header.find('(')
        && let Some(close_paren) = class_header.find(')')
    {
        let inside_bases = &class_header[open_paren + 1..close_paren];
        let remaining_bases: Vec<&str> = inside_bases
            .split(',')
            .map(str::trim)
            .filter(|b| *b != base_name && !b.is_empty())
            .collect();

        let new_header = if remaining_bases.is_empty() {
            format!("{}:", class_header[..open_paren].trim_end())
        } else {
            format!(
                "{}({}):",
                class_header[..open_paren].trim_end(),
                remaining_bases.join(", ")
            )
        };

        let header_start = sub.decl_start;
        let header_end = sub.decl_start + class_line_end;
        out.replace_range(header_start..header_end, &new_header);
    }

    // Re-parse to get fresh sub offsets
    let updated_classes = parse_classes_in_text(&out, "python", &sub.file_path);
    let updated_sub = updated_classes
        .into_iter()
        .find(|c| c.name == sub.name)
        .with_context(|| format!("failed to re-locate class '{}'", sub.name))?;

    let indent = &updated_sub.indent;
    let mut additions = Vec::new();

    // 2. Check for `__init__`
    let init_method = updated_sub.members.iter().find(|m| m.name == "__init__");
    if let Some(init_decl) = init_method {
        // In existing __init__, replace `super().__init__(...)` with `self.{field_name} = {base_name}(...)`
        let init_slice = &out[init_decl.start_offset..init_decl.end_offset];
        if init_slice.contains("super().__init__(") {
            let replaced_init = init_slice.replace(
                "super().__init__(",
                &format!("self.{field_name} = {base_name}("),
            );
            out.replace_range(init_decl.start_offset..init_decl.end_offset, &replaced_init);
        } else if init_slice.contains(&format!("{base_name}.__init__(self")) {
            let replaced_init =
                rewrite_python_explicit_base_init(init_slice, base_name, field_name)?;
            out.replace_range(init_decl.start_offset..init_decl.end_offset, &replaced_init);
        } else {
            // Prepend `self.{field_name} = {base_name}()` inside __init__
            if let Some(def_colon) = init_slice.find(':') {
                let insert_idx = init_decl.start_offset + def_colon + 1;
                let assign = format!("\n{indent}    self.{field_name} = {base_name}()");
                out.insert_str(insert_idx, &assign);
            }
        }
    } else {
        // Generate new __init__
        let new_init = format!(
            "{indent}def __init__(self, *args, **kwargs):\n{indent}    self.{field_name} = {base_name}(*args, **kwargs)"
        );
        additions.push(new_init);
    }

    // 3. Generate forwarding methods
    for m in methods {
        let fwd = format!(
            "{indent}def {m}(self, *args, **kwargs):\n{indent}    return self.{field_name}.{m}(*args, **kwargs)"
        );
        additions.push(fwd);
    }

    // Insert additions into subclass body
    if !additions.is_empty() {
        let classes_now = parse_classes_in_text(&out, "python", &sub.file_path);
        let cur_sub = classes_now
            .into_iter()
            .find(|c| c.name == sub.name)
            .with_context(|| format!("failed to re-locate class '{}'", sub.name))?;

        let body_slice = &out[cur_sub.body_start..cur_sub.body_end];
        let block = additions.join("\n\n");
        if body_slice.trim() == "pass" {
            let pass_start = cur_sub.body_start + body_slice.find("pass").unwrap_or(0);
            let pass_end = pass_start + 4;
            out.replace_range(pass_start..pass_end, &block);
        } else {
            let insert_pos = cur_sub.body_end;
            let formatted = format!("\n\n{block}");
            out.insert_str(insert_pos, &formatted);
        }
    }

    // 4. Rewrite any remaining `super().` calls inside subclass to `self.{field_name}.`
    let final_classes = parse_classes_in_text(&out, "python", &sub.file_path);
    if let Some(final_sub) = final_classes.into_iter().find(|c| c.name == sub.name) {
        let body_slice = &out[final_sub.body_start..final_sub.body_end];
        if body_slice.contains("super().") {
            let rewritten_body = body_slice.replace("super().", &format!("self.{field_name}."));
            out.replace_range(final_sub.body_start..final_sub.body_end, &rewritten_body);
        }
    }

    Ok(out)
}

fn rewrite_python_explicit_base_init(
    body: &str,
    base_name: &str,
    field_name: &str,
) -> Result<String> {
    let needle = format!("{base_name}.__init__");
    let mut rewritten = body.to_string();
    let mut search_from = 0usize;
    while let Some(relative) = rewritten[search_from..].find(&needle) {
        let start = search_from + relative;
        if crate::extract_field::is_in_literal_or_comment(
            &rewritten,
            start,
            crate::parameter_object::Language::Python,
        ) {
            search_from = start + needle.len();
            continue;
        }
        let open = start + needle.len();
        if rewritten.as_bytes().get(open) != Some(&b'(') {
            search_from = open;
            continue;
        }
        let close = crate::parameter_object::matching_bracket(&rewritten, open)
            .context("malformed explicit Python base initializer")?;
        let args = &rewritten[open + 1..close];
        let mut parts = crate::replace_constructor::split_balanced_commas(args);
        if parts.first().map(|first| first.trim()) != Some("self") {
            anyhow::bail!(
                "explicit base initializer does not start with `self`; nothing was rewritten"
            );
        }
        parts.remove(0);
        let forwarded = parts.join(", ").trim().to_string();
        let replacement = if forwarded.is_empty() {
            format!("self.{field_name} = {base_name}()")
        } else {
            format!("self.{field_name} = {base_name}({forwarded})")
        };
        rewritten.replace_range(start..close + 1, &replacement);
        search_from = start + replacement.len();
    }
    Ok(rewritten)
}
