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

pub fn transform_typescript(
    text: &str,
    sub: &ClassDecl,
    base_name: &str,
    field_name: &str,
    methods: &[String],
    base_class: Option<&ClassDecl>,
) -> Result<String> {
    let mut out = text.to_string();

    // 1. Remove `extends <Base>` from class header
    let sub_slice = &out[sub.decl_start..sub.body_start];
    if let Some(extends_pos) = sub_slice.find("extends") {
        let after_extends = &sub_slice[extends_pos + 7..];
        let base_pos = after_extends
            .find(base_name)
            .with_context(|| format!("cannot find base '{base_name}' in extends clause"))?;
        let mut remove_start = sub.decl_start + extends_pos;
        if remove_start > sub.decl_start && out.as_bytes()[remove_start - 1] == b' ' {
            remove_start -= 1;
        }
        let remove_end = sub.decl_start + extends_pos + 7 + base_pos + base_name.len();
        out.replace_range(remove_start..remove_end, "");
    }

    // Re-parse to get fresh offsets
    let classes_now = parse_classes_in_text(&out, &sub.language, &sub.file_path);
    let cur_sub = classes_now
        .into_iter()
        .find(|c| c.name == sub.name)
        .with_context(|| format!("failed to re-locate class '{}'", sub.name))?;

    let indent = &cur_sub.indent;
    let mut additions = Vec::new();

    // 2. Delegate field declaration
    let field_decl = format!("{indent}private {field_name}: {base_name};");
    additions.push(field_decl);

    // 3. Constructor handling
    let sub_body = &out[cur_sub.body_start..cur_sub.body_end];
    if sub_body.contains("constructor(") {
        // If constructor exists, replace `super(` with `this.{field_name} = new {base_name}(`
        let replaced_body =
            sub_body.replace("super(", &format!("this.{field_name} = new {base_name}("));
        out.replace_range(cur_sub.body_start..cur_sub.body_end, &replaced_body);
    } else {
        // Generate constructor
        let new_ctor = format!(
            "{indent}constructor(...args: any[]) {{\n{indent}    this.{field_name} = new {base_name}(...args as any);\n{indent}}}"
        );
        additions.push(new_ctor);
    }

    // 4. Generate forwarding methods
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
            let clean_header = header.replace("override ", "").replace("override\t", "");
            format!(
                "{indent}{clean_header} {{\n{indent}    return this.{field_name}.{m}(...arguments as any);\n{indent}}}"
            )
        } else {
            format!(
                "{indent}{m}(...args: any[]): any {{\n{indent}    return (this.{field_name} as any).{m}(...args);\n{indent}}}"
            )
        };
        additions.push(fwd);
    }

    // Insert additions before closing brace of subclass
    let classes_updated = parse_classes_in_text(&out, &sub.language, &sub.file_path);
    let final_sub = classes_updated
        .into_iter()
        .find(|c| c.name == sub.name)
        .with_context(|| format!("failed to re-locate class '{}'", sub.name))?;

    let insert_block = additions.join("\n\n");
    let insert_pos = final_sub.body_end;
    let formatted = format!("\n{insert_block}\n");
    out.insert_str(insert_pos, &formatted);

    // 5. Strip `override` from remaining subclass methods
    let classes_final = parse_classes_in_text(&out, &sub.language, &sub.file_path);
    if let Some(sub_after) = classes_final.into_iter().find(|c| c.name == sub.name) {
        let body_slice = &out[sub_after.body_start..sub_after.body_end];
        let stripped_body = strip_override_modifiers(body_slice, "typescript");
        let rewritten_body = stripped_body.replace("super.", &format!("this.{field_name}."));
        out.replace_range(sub_after.body_start..sub_after.body_end, &rewritten_body);
    }

    Ok(out)
}
