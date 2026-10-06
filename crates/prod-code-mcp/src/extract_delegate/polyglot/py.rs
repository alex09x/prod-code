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

use super::params::extract_param_names_py;
use crate::extract_delegate::common::{is_ident, is_ident_str, reindent};

pub fn restructure_py(
    text: &str,
    symbol_opt: Option<&str>,
    line_opt: Option<u32>,
    fields: &[String],
    methods: &[String],
    helper: &str,
    field: &str,
) -> Result<(String, String)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut class_idx = None;
    let mut owner_name = String::new();

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("class ") && trimmed.contains(':') {
            let after = &trimmed[6..];
            let name: String = after.chars().take_while(|c| is_ident(*c)).collect();
            if name.is_empty() {
                continue;
            }
            if let Some(sym) = symbol_opt
                && sym != name
            {
                continue;
            }
            if let Some(l) = line_opt
                && l > 0
                && l != (idx as u32 + 1)
            {
                continue;
            }
            owner_name = name;
            class_idx = Some(idx);
            break;
        }
    }

    let c_idx = class_idx.with_context(|| {
        if let Some(sym) = symbol_opt {
            format!("class `{sym}` not found")
        } else {
            "no class found at the specified location".to_string()
        }
    })?;

    let class_line = lines[c_idx];
    let class_indent = class_line.len() - class_line.trim_start().len();

    let mut end_class_idx = lines.len();
    for (idx, line) in lines.iter().enumerate().skip(c_idx + 1) {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let ind = line.len() - line.trim_start().len();
        if ind <= class_indent {
            end_class_idx = idx;
            break;
        }
    }

    let mut ctor_assigned_fields: BTreeMap<String, String> = BTreeMap::new();
    let mut class_fields: BTreeMap<String, String> = BTreeMap::new();

    struct PyMethod {
        params: String,
        full_text: String,
        body: String,
        start_line: usize,
        end_line: usize,
    }

    let mut moved_methods: BTreeMap<String, PyMethod> = BTreeMap::new();
    let mut idx = c_idx + 1;

    while idx < end_class_idx {
        let line = lines[idx];
        let trimmed = line.trim_start();
        if trimmed.starts_with("def ") || trimmed.starts_with("async def ") {
            let after_def = if let Some(s) = trimmed.strip_prefix("async def ") {
                s
            } else {
                &trimmed[4..]
            };
            if let Some(p_open) = after_def.find('(') {
                let mname = after_def[..p_open].trim().to_string();
                if let Some(p_close) = after_def.rfind(')') {
                    let params = after_def[p_open + 1..p_close].trim().to_string();
                    let m_indent = line.len() - trimmed.len();
                    let mut m_end = end_class_idx;
                    for (k, ml) in lines.iter().enumerate().take(end_class_idx).skip(idx + 1) {
                        if ml.trim().is_empty() || ml.trim_start().starts_with('#') {
                            continue;
                        }
                        let mind = ml.len() - ml.trim_start().len();
                        if mind <= m_indent {
                            m_end = k;
                            break;
                        }
                    }
                    let m_lines = &lines[idx..m_end];
                    let full_text = m_lines.join("\n");
                    let body_lines = if m_lines.len() > 1 {
                        &m_lines[1..]
                    } else {
                        &[]
                    };
                    let body = body_lines.join("\n");

                    if mname == "__init__" {
                        for bl in body_lines {
                            let bt = bl.trim();
                            if bt.starts_with("self.")
                                && let Some((lhs, rhs)) = bt.split_once('=')
                            {
                                let fname = lhs.trim().trim_start_matches("self.").trim();
                                if is_ident_str(fname) {
                                    ctor_assigned_fields
                                        .insert(fname.to_string(), rhs.trim().to_string());
                                }
                            }
                        }
                    } else if methods.contains(&mname) {
                        moved_methods.insert(
                            mname.clone(),
                            PyMethod {
                                params,
                                full_text,
                                body,
                                start_line: idx,
                                end_line: m_end,
                            },
                        );
                    }
                    idx = m_end;
                    continue;
                }
            }
        } else if trimmed.contains('=') || trimmed.contains(':') {
            let first = trimmed.split(&['=', ':'][..]).next().unwrap_or("").trim();
            if is_ident_str(first) {
                class_fields.insert(first.to_string(), trimmed.to_string());
            }
        }
        idx += 1;
    }

    for f in fields {
        anyhow::ensure!(
            class_fields.contains_key(f)
                || ctor_assigned_fields.contains_key(f)
                || text.contains(&format!("self.{f}")),
            "`{owner_name}` has no field `{f}`"
        );
    }
    anyhow::ensure!(
        !class_fields.contains_key(field) && !ctor_assigned_fields.contains_key(field),
        "`{owner_name}` already has a field `{field}`"
    );

    for m in methods {
        anyhow::ensure!(
            moved_methods.contains_key(m),
            "`{owner_name}` has no method `{m}`"
        );
    }

    for (mname, minfo) in &moved_methods {
        for (pos, _) in minfo.body.match_indices("self.") {
            let after = &minfo.body[pos + 5..];
            let ident: String = after.chars().take_while(|c| is_ident(*c)).collect();
            if !ident.is_empty() && !fields.contains(&ident) && !methods.contains(&ident) {
                anyhow::bail!("`{mname}` uses `{ident}`, which does not move to `{helper}`");
            }
        }
    }

    let mut helper_lines = Vec::new();
    helper_lines.push(format!("class {helper}:"));
    let helper_init_params = fields
        .iter()
        .map(|f| format!("{f}=None"))
        .collect::<Vec<_>>()
        .join(", ");
    helper_lines.push(format!("    def __init__(self, {helper_init_params}):"));
    for f in fields {
        helper_lines.push(format!("        self.{f} = {f}"));
    }
    for m in methods {
        let minfo = moved_methods.get(m).unwrap();
        helper_lines.push(String::new());
        let reindented = reindent(&minfo.full_text, "    ");
        helper_lines.push(reindented);
    }
    let helper_text = helper_lines.join("\n");

    let mut new_class_lines = Vec::new();
    let mut inside_init = false;
    let mut init_replaced = false;

    let owner_init_args = fields
        .iter()
        .map(|f| {
            if let Some(rhs) = ctor_assigned_fields.get(f) {
                rhs.clone()
            } else {
                f.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(", ");

    let mut k = c_idx;
    while k < end_class_idx {
        let line = lines[k];
        let trimmed = line.trim();

        if trimmed.starts_with("def __init__") {
            inside_init = true;
            new_class_lines.push(line.to_string());
            k += 1;
            continue;
        }

        if inside_init {
            if trimmed.starts_with("def ") {
                inside_init = false;
            } else {
                let is_moved_assign = fields.iter().any(|f| {
                    trimmed.starts_with(&format!("self.{f} ="))
                        || trimmed.starts_with(&format!("self.{f}="))
                });
                if is_moved_assign {
                    if !init_replaced {
                        new_class_lines.push(format!(
                            "        self.{field} = {helper}({owner_init_args})"
                        ));
                        init_replaced = true;
                    }
                    k += 1;
                    continue;
                }
            }
        }

        let is_in_moved_method = moved_methods
            .values()
            .any(|minfo| minfo.start_line <= k && k < minfo.end_line);
        if is_in_moved_method {
            k += 1;
            continue;
        }

        let is_moved_class_field = fields.iter().any(|f| {
            trimmed.starts_with(&format!("{f} =")) || trimmed.starts_with(&format!("{f}:"))
        });
        if is_moved_class_field {
            k += 1;
            continue;
        }

        new_class_lines.push(line.to_string());
        k += 1;
    }

    for m in methods {
        let minfo = moved_methods.get(m).unwrap();
        let arg_names = extract_param_names_py(&minfo.params);
        let other_params = if minfo.params.is_empty() || minfo.params == "self" {
            String::new()
        } else {
            let after_self = minfo
                .params
                .strip_prefix("self")
                .unwrap_or(&minfo.params)
                .trim();
            if after_self.starts_with(',') {
                after_self.to_string()
            } else if !after_self.is_empty() {
                format!(", {after_self}")
            } else {
                String::new()
            }
        };
        new_class_lines.push(String::new());
        new_class_lines.push(format!("    def {m}(self{other_params}):"));
        new_class_lines.push(format!(
            "        return self.{field}.{m}({})",
            arg_names.join(", ")
        ));
    }

    let mut final_lines = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if i == c_idx {
            final_lines.push(helper_text.clone());
            final_lines.push(String::new());
            final_lines.push(String::new());
            for cl in &new_class_lines {
                final_lines.push(cl.clone());
            }
        } else if i > c_idx && i < end_class_idx {
            continue;
        } else {
            final_lines.push(line.to_string());
        }
    }

    Ok((final_lines.join("\n"), owner_name))
}
