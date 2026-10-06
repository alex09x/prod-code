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

use super::parse::{argument_names, impl_blocks, parse_struct};
use super::types::Field;
use crate::extract_delegate::common::{is_ident, split_top};

/// Rewrites every struct literal `Name { .. }` (and `Self { .. }` inside `self_ranges`) in
/// `text` whose entries include one of `moved`: those entries go into `field: Helper { .. }`.
pub fn rewrite_literals(
    text: &str,
    name: &str,
    self_ranges: &[(usize, usize)],
    moved: &[String],
    field: &str,
    helper: &str,
) -> Result<String> {
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for word in [name, "Self"] {
        for (i, _) in text.match_indices(word) {
            let before = text[..i].trim_end();
            if text[..i].chars().next_back().is_some_and(is_ident)
                || text[i + word.len()..].chars().next().is_some_and(is_ident)
                || before.ends_with("struct")
                || before.ends_with("impl")
                || before.ends_with("for")
                || before.ends_with("->")
                || (word == "Self" && !self_ranges.iter().any(|(s, e)| *s < i && i < *e))
            {
                continue;
            }
            let rest = &text[i + word.len()..];
            let trimmed = rest.trim_start();
            if !trimmed.starts_with('{') {
                continue;
            }
            let open = i + word.len() + (rest.len() - trimmed.len());
            let Some(close) = crate::parameter_object::matching_bracket(text, open) else {
                continue;
            };
            let body = &text[open + 1..close];
            let entries: Vec<&str> = split_top(body, ',')
                .into_iter()
                .map(|(s, e)| body[s..e].trim())
                .filter(|e| !e.is_empty())
                .collect();
            let key = |e: &str| -> String { e.split(':').next().unwrap_or(e).trim().to_string() };
            let inner: Vec<&str> = entries
                .iter()
                .copied()
                .filter(|e| moved.contains(&key(e)))
                .collect();
            if inner.is_empty() {
                continue;
            }
            anyhow::ensure!(
                !entries.iter().any(|e| e.starts_with("..")),
                "a literal or pattern of `{name}` uses `..`; rewrite it by hand"
            );
            let indent: String = body
                .trim_start_matches(['\n'])
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            let nested = format!("{field}: {helper} {{ {} }}", inner.join(", "));
            let mut all: Vec<String> = Vec::new();
            let mut placed = false;
            for e in &entries {
                if moved.contains(&key(e)) {
                    if !placed {
                        all.push(nested.clone());
                        placed = true;
                    }
                } else {
                    all.push(e.to_string());
                }
            }
            let new_body = if body.contains('\n') {
                let close_indent: String = text[..close]
                    .rsplit('\n')
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .collect();
                format!(
                    "\n{indent}{},\n{close_indent}",
                    all.join(&format!(",\n{indent}"))
                )
            } else {
                format!(" {} ", all.join(", "))
            };
            edits.push((open + 1, close, new_body));
        }
    }
    edits.sort_by_key(|e| std::cmp::Reverse(e.0));
    let mut out = text.to_string();
    for (s, e, t) in edits {
        out.replace_range(s..e, &t);
    }
    Ok(out)
}

pub fn restructure(
    text: &str,
    at: usize,
    fields: &[String],
    methods: &[String],
    helper: &str,
    field: &str,
) -> Result<String> {
    let decl = parse_struct(text, at)?;
    for f in fields {
        anyhow::ensure!(
            decl.fields.iter().any(|d| &d.name == f),
            "`{}` has no field `{f}`; it has {}",
            decl.name,
            decl.fields
                .iter()
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    anyhow::ensure!(
        decl.fields.iter().all(|d| d.name != field),
        "`{}` already has a field `{field}`",
        decl.name
    );
    let moved_fields: Vec<&Field> = decl
        .fields
        .iter()
        .filter(|f| fields.contains(&f.name))
        .collect();
    let widest = if moved_fields.iter().any(|f| f.vis == "pub") {
        "pub"
    } else {
        moved_fields
            .iter()
            .map(|f| f.vis.as_str())
            .find(|v| !v.is_empty())
            .unwrap_or("")
    };
    let indent = "    ";
    let mut kept: Vec<String> = Vec::new();
    let mut placed = false;
    for f in &decl.fields {
        if fields.contains(&f.name) {
            if !placed {
                let v = if widest.is_empty() {
                    String::new()
                } else {
                    format!("{widest} ")
                };
                kept.push(format!("{indent}{v}{field}: {helper},"));
                placed = true;
            }
        } else {
            kept.push(format!("{indent}{},", f.text));
        }
    }
    let struct_vis = if decl.vis.is_empty() {
        String::new()
    } else {
        format!("{} ", decl.vis)
    };
    let new_struct_body = format!("{{\n{}\n}}", kept.join("\n"));
    let helper_decl = format!(
        "{}{struct_vis}struct {helper} {{\n{}\n}}\n",
        decl.derive
            .as_ref()
            .map(|d| format!("{d}\n"))
            .unwrap_or_default(),
        moved_fields
            .iter()
            .map(|f| format!("{indent}{},", f.text))
            .collect::<Vec<_>>()
            .join("\n")
    );

    // Methods: taken out of every `impl Name` block, a forwarding method left in their place.
    let mut out = text.to_string();
    let mut moved_items: Vec<String> = Vec::new();
    let mut found: Vec<String> = Vec::new();
    for (_, open, close) in impl_blocks(text, &decl.name).into_iter().rev() {
        let items = crate::extract_trait::items(text, open, close);
        for item in items.iter().rev() {
            let Some(mname) = item.name.as_ref().filter(|n| methods.contains(n)) else {
                continue;
            };
            let chunk = &text[item.start..item.end];
            let (leading, decl_text) = crate::extract_trait::declaration(chunk);
            let sig = crate::extract_trait::signature(decl_text)
                .with_context(|| format!("cannot read the signature of `{mname}`"))?;
            anyhow::ensure!(
                sig.contains("&self") || sig.contains("&mut self"),
                "`{mname}` does not take `&self` or `&mut self`; only methods on a borrowed \
                 receiver can forward to the helper"
            );
            // It may use only the moved fields and the other moved methods.
            let body = &decl_text[sig.len()..];
            for (i, _) in body.match_indices("self.") {
                let used: String = body[i + 5..].chars().take_while(|c| is_ident(*c)).collect();
                anyhow::ensure!(
                    fields.contains(&used) || methods.contains(&used),
                    "`{mname}` uses `self.{used}`, which does not move to `{helper}`"
                );
            }
            let names = argument_names(sig).with_context(|| {
                format!("`{mname}` has a parameter pattern this cannot forward")
            })?;
            let item_indent: String = text[..item.start]
                .rsplit('\n')
                .next()
                .unwrap_or("")
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            let attrs: String = leading
                .iter()
                .map(|l| format!("{l}\n{item_indent}"))
                .collect();
            let stub = format!(
                "{attrs}{sig} {{\n{item_indent}    self.{field}.{mname}({})\n{item_indent}}}",
                names.join(", ")
            );
            out.replace_range(item.start..item.end, &stub);
            moved_items.push(format!("{item_indent}{}", chunk.trim()));
            found.push(mname.clone());
        }
    }
    for m in methods {
        anyhow::ensure!(
            found.contains(m),
            "`{}` has no method `{m}` in an inherent `impl` block",
            decl.name
        );
    }
    moved_items.reverse();

    // The struct itself: rewritten last, it comes before every impl block in the offsets above
    // only if it is declared first; find it again in the text as it is now.
    let decl_now = parse_struct(
        &out,
        out.find(&format!("struct {} ", decl.name)).unwrap_or(at),
    )?;
    let helper_impl = if moved_items.is_empty() {
        String::new()
    } else {
        format!("\nimpl {helper} {{\n{}\n}}\n", moved_items.join("\n\n"))
    };
    out.replace_range(
        decl_now.open..decl_now.close + 1,
        format!("{new_struct_body}\n\n{helper_decl}{helper_impl}").trim_end(),
    );
    Ok(out)
}
