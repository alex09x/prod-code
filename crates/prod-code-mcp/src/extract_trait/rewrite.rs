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

use super::generics::{Delimiters, method_param_span};
use super::lex::{
    attribute_end, contains_code_word, is_conditional_attribute, lexical_code,
    outer_attributes_before,
};
use super::types::{ImplBlock, valid_ident};

#[path = "visibility.rs"]
mod visibility_scope;

pub use visibility_scope::visibility;
use visibility_scope::widest_visibility;
pub(super) use visibility_scope::{
    add_import_at_inline_scope, inline_scope_at, relative_import_path,
};

/// An item's text split into its leading lines of comments, docs and attributes, and the
/// declaration that follows them.
pub fn declaration(item: &str) -> (Vec<&str>, &str) {
    let mut leading = Vec::new();
    let mut rest = item;
    loop {
        let t = rest.trim_start();
        if t.starts_with("//") {
            let end = t.find('\n').map_or(t.len(), |n| n + 1);
            leading.push(t[..end].trim_end());
            rest = &t[end..];
        } else if t.starts_with("/*") {
            let code = lexical_code(t);
            let end = code.iter().position(|is_code| *is_code).unwrap_or(t.len());
            leading.push(t[..end].trim_end());
            rest = &t[end..];
        } else if let Some(end) = attribute_end(t) {
            leading.push(t[..end].trim_end());
            rest = &t[end..];
        } else {
            return (leading, t);
        }
    }
}

/// A function's signature: its declaration up to the body's opening brace.
pub fn signature(decl: &str) -> Option<&str> {
    let name_at = decl.find("fn ")? + 3;
    let name_at = name_at + (decl[name_at..].len() - decl[name_at..].trim_start().len());
    let (_, close) = method_param_span(decl, name_at)?;
    let code = lexical_code(decl);
    let mut nesting = Delimiters::default();
    let mut body = None;
    for (offset, c) in decl[close + 1..].char_indices() {
        let at = close + 1 + offset;
        if !code[at] {
            continue;
        }
        if c == '{' && nesting.top_level() {
            body = Some(at);
            break;
        }
        nesting.advance(decl, at)?;
    }
    let body = body?;
    Some(decl[..body].trim_end())
}

/// An item's text at `indent`. The text starts at its first character, so only the first line
/// lacks the indentation; the others keep theirs, which is right because the new blocks sit at
/// the same depth as the old one.
fn indented(item: &str, indent: &str) -> String {
    format!("{indent}{}", item.trim())
}

/// The block rewritten: the methods named move out of `imp` into `trait {name}` and its
/// `impl {name} for Type`, placed right after the block; the block itself goes when nothing is
/// left in it. Returns the new text and the names of the methods left behind.
pub fn rewrite(
    text: &str,
    imp: &ImplBlock,
    methods: &[String],
    name: &str,
) -> Result<(String, Vec<String>)> {
    anyhow::ensure!(valid_ident(name), "`{name}` is not a valid Rust identifier");
    for method in methods {
        anyhow::ensure!(
            valid_ident(method),
            "`{method}` is not a valid Rust method identifier"
        );
    }
    let available: Vec<&str> = imp.items.iter().filter_map(|i| i.name.as_deref()).collect();
    for m in methods {
        anyhow::ensure!(
            available.contains(&m.as_str()),
            "`{}` has no method `{m}`; it has {}",
            imp.self_ty,
            available.join(", ")
        );
    }
    let impl_attrs = outer_attributes_before(text, imp.start);
    anyhow::ensure!(
        impl_attrs
            .iter()
            .all(|(start, end)| is_conditional_attribute(&text[*start..*end])),
        "attributes on impl blocks are not supported yet; remove or move the attribute first"
    );
    let anchor = impl_attrs.first().map_or(imp.start, |(start, _)| *start);
    let line_start = text[..anchor].rfind('\n').map_or(0, |i| i + 1);
    let before_impl = &text[line_start..anchor];
    let outer = if before_impl.chars().all(char::is_whitespace) {
        before_impl
    } else {
        ""
    };
    let replacement_start = if outer.is_empty() { anchor } else { line_start };
    let impl_prefix: String = impl_attrs
        .iter()
        .map(|(start, end)| format!("{outer}{}\n", text[*start..*end].trim()))
        .collect();
    let inner = format!("{outer}    ");
    let mut decls = Vec::new();
    let mut bodies = Vec::new();
    let mut kept = Vec::new();
    let mut kept_names = Vec::new();
    let mut selected_visibilities = Vec::new();
    for item in &imp.items {
        let chunk = &text[item.start..item.end];
        let Some(chosen) = item.name.as_ref().filter(|n| methods.contains(n)) else {
            kept.push(indented(chunk, &inner));
            kept_names.extend(item.name.clone());
            continue;
        };
        let (leading, decl) = declaration(chunk);
        let (vis, bare) = visibility(decl);
        selected_visibilities.push(vis);
        let sig =
            signature(bare).with_context(|| format!("cannot read the signature of `{chosen}`"))?;
        let name_at = sig.find("fn ").context("missing function name")? + 3;
        let name_at = name_at + (sig[name_at..].len() - sig[name_at..].trim_start().len());
        let (_, parameters_end) =
            method_param_span(sig, name_at).context("cannot read method parameter boundaries")?;
        anyhow::ensure!(
            !contains_code_word(&sig[parameters_end..], "impl"),
            "method `{chosen}` has an opaque return type; extraction can change its lifetime capture"
        );
        let trait_leading: Vec<&str> = leading
            .iter()
            .copied()
            .filter(|l| l.starts_with("///") || is_conditional_attribute(l))
            .collect();
        let impl_leading: Vec<&str> = leading
            .iter()
            .copied()
            .filter(|l| !l.starts_with("///"))
            .collect();
        let mut decl_text = trait_leading
            .iter()
            .map(|l| format!("{inner}{l}\n"))
            .collect::<String>();
        decl_text.push_str(&indented(&format!("{sig};"), &inner));
        decls.push(decl_text);
        let mut body = impl_leading
            .iter()
            .map(|l| format!("{inner}{l}\n"))
            .collect::<String>();
        body.push_str(&indented(bare, &inner));
        bodies.push(body);
    }
    let widest = widest_visibility(text, imp.start, &selected_visibilities);
    let vis = if widest.is_empty() {
        String::new()
    } else {
        format!("{widest} ")
    };
    let block = |header: String, items: &[String]| {
        format!(
            "{impl_prefix}{outer}{header} {{\n{}\n{outer}}}",
            items.join("\n\n")
        )
    };
    let where_clause = if imp.where_clause.is_empty() {
        String::new()
    } else if imp.where_on_newline {
        format!("\n{}", imp.where_clause)
    } else {
        format!(" {}", imp.where_clause)
    };
    let generic_args = if imp.generic_args.is_empty() {
        String::new()
    } else {
        format!("<{}>", imp.generic_args.join(", "))
    };
    let mut replacement = String::new();
    if !kept.is_empty() {
        let inherent = if imp.header.starts_with('<') {
            format!("impl{}", imp.header)
        } else {
            format!("impl {}", imp.header)
        };
        replacement.push_str(&block(inherent, &kept));
        replacement.push_str("\n\n");
    }
    replacement.push_str(&block(
        format!("{vis}trait {name}{}{where_clause}", imp.generics),
        &decls,
    ));
    replacement.push_str("\n\n");
    replacement.push_str(&block(
        format!(
            "impl{} {name}{generic_args} for {}{where_clause}",
            imp.generics, imp.self_ty
        ),
        &bodies,
    ));
    let mut out = String::with_capacity(text.len() + replacement.len());
    out.push_str(&text[..replacement_start]);
    out.push_str(&replacement);
    out.push_str(&text[imp.close + 1..]);
    Ok((out, kept_names))
}
