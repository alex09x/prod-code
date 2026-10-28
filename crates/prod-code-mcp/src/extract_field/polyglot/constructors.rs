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
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use crate::extract_field::helpers::{display, is_ident, is_in_literal_or_comment, mentions};
use crate::extract_field::polyglot::parsers::language_matches_extension;
use crate::parameter_object::Language;
use anyhow::{Context, Result};

fn is_go_function_signature(text: &str, at: usize) -> bool {
    let start = text[..at]
        .rfind(['{', '}', ';'])
        .map_or(0, |index| index + 1);
    text[start..at].match_indices("func").any(|(offset, _)| {
        let word = start + offset;
        let before = text[..word].chars().next_back();
        let after = text[word + 4..].chars().next();
        before.is_none_or(|character| !is_ident(character))
            && after.is_some_and(|character| character.is_whitespace() || character == '(')
            && !is_in_literal_or_comment(text, word, Language::Go)
    })
}

pub(super) fn go_constructor_insertion(
    content: &str,
    brace: usize,
    name: &str,
    init: &str,
) -> Option<String> {
    let close = crate::parameter_object::matching_bracket(content, brace)?;
    let inside = &content[brace + 1..close];
    if mentions(inside, &format!("{name}:")) {
        return None;
    }
    let is_multiline = inside.contains('\n');
    Some(if is_multiline {
        let indent = inside
            .lines()
            .skip(1)
            .find(|line| !line.trim().is_empty())
            .map(|line| {
                line.chars()
                    .take_while(|character| character.is_whitespace())
                    .collect::<String>()
            })
            .unwrap_or_else(|| "        ".to_string());
        format!("\n{indent}{name}: {init},")
    } else if inside.trim().is_empty() {
        format!("{name}: {init}")
    } else {
        format!("{name}: {init}, ")
    })
}

#[allow(clippy::too_many_arguments)]
pub async fn find_go_constructors(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: &str,
    struct_body_open: usize,
    owner: &str,
    name: &str,
    init: &str,
    edits: &mut BTreeMap<PathBuf, Vec<(usize, usize, String)>>,
    texts: &mut BTreeMap<PathBuf, String>,
    unmatched: &mut Vec<String>,
) -> Result<usize> {
    let type_decl = format!("type {owner}");
    let type_name_at = text[..struct_body_open]
        .rfind(&type_decl)
        .context("cannot locate the selected Go struct declaration")?
        + "type ".len();
    let (type_line, _) = crate::signature::position_at(text, type_name_at)?;
    let type_file = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let mut constructors = 0;
    for entry in ignore::WalkBuilder::new(root).build().flatten() {
        let path = entry.path();
        if !path.is_file()
            || path.extension().and_then(|extension| extension.to_str()) != Some("go")
        {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        let mut cursor = 0;
        while let Some(relative) = content[cursor..].find(owner) {
            let hit = cursor + relative;
            cursor = hit + owner.len();
            if is_in_literal_or_comment(&content, hit, Language::Go)
                || (hit > 0 && is_ident(content[..hit].chars().next_back().unwrap()))
                || content[cursor..].chars().next().is_some_and(is_ident)
                || is_go_function_signature(&content, hit)
            {
                continue;
            }
            let mut brace = cursor;
            while content[brace..]
                .chars()
                .next()
                .is_some_and(char::is_whitespace)
            {
                brace += content[brace..].chars().next().unwrap().len_utf8();
            }
            if content.as_bytes().get(brace) != Some(&b'{') {
                continue;
            }
            let (line, column) = crate::signature::position_at(&content, hit)?;
            let uri = url::Url::from_file_path(path)
                .map_err(|_| anyhow::anyhow!("invalid Go constructor path {:?}", path))?
                .to_string();
            let definitions = crate::tools::execute_lsp_query(
                remote,
                root,
                path,
                "textDocument/definition",
                serde_json::json!({
                    "textDocument": { "uri": uri },
                    "position": {
                        "line": line.saturating_sub(1),
                        "character": column.saturating_sub(1)
                    }
                }),
            )
            .await?;
            let locations: Vec<&serde_json::Value> = match &definitions {
                serde_json::Value::Array(locations) => locations.iter().collect(),
                serde_json::Value::Object(_) => vec![&definitions],
                _ => Vec::new(),
            };
            if locations.is_empty() {
                unmatched.push(format!(
                    "{}:{}: cannot resolve Go literal type `{owner}` to the selected declaration",
                    display(root, path),
                    line
                ));
                continue;
            }
            let resolves_to_selected = locations.len() == 1
                && locations.iter().any(|location| {
                    let uri = location
                        .get("uri")
                        .or_else(|| location.get("targetUri"))
                        .and_then(serde_json::Value::as_str);
                    let declaration_line = location
                        .pointer("/range/start/line")
                        .or_else(|| location.pointer("/targetSelectionRange/start/line"))
                        .and_then(serde_json::Value::as_u64)
                        .map(|line| line as u32 + 1);
                    let path_matches = uri
                        .map(crate::remote_fs::uri_to_path)
                        .map(PathBuf::from)
                        .map(|path| std::fs::canonicalize(&path).unwrap_or(path) == type_file)
                        .unwrap_or(false);
                    path_matches && declaration_line == Some(type_line)
                });
            if !resolves_to_selected {
                continue;
            }
            if crate::parameter_object::matching_bracket(&content, brace).is_none() {
                unmatched.push(format!(
                    "{}:{}: the resolved `{owner}` composite literal is incomplete",
                    display(root, path),
                    line
                ));
                continue;
            }
            let Some(insertion) = go_constructor_insertion(&content, brace, name, init) else {
                continue;
            };
            texts
                .entry(path.to_path_buf())
                .or_insert_with(|| content.clone());
            edits
                .entry(path.to_path_buf())
                .or_default()
                .push((brace + 1, 0, insertion));
            constructors += 1;
        }
    }
    Ok(constructors)
}

#[allow(clippy::too_many_arguments)]
pub fn find_constructors(
    root: &Path,
    file: &Path,
    text: &str,
    lang: Language,
    owner: &str,
    name: &str,
    init: &str,
    edits: &mut BTreeMap<PathBuf, Vec<(usize, usize, String)>>,
    texts: &mut BTreeMap<PathBuf, String>,
    unmatched: &mut Vec<String>,
) -> usize {
    let _ = (file, text, name, init, edits, texts, unmatched);
    let mut constructors = 0usize;
    for entry in ignore::WalkBuilder::new(root).build().flatten() {
        let path = entry.path();
        if !path.is_file() || !language_matches_extension(lang, path) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        match lang {
            Language::TypeScript | Language::JavaScript => {
                let needle = format!("new {owner}(");
                constructors += content.matches(&needle).count();
            }
            Language::Python | Language::Swift => {
                let needle = format!("{owner}(");
                for (at, _) in content.match_indices(&needle) {
                    if at > 0 && is_ident(content[..at].chars().next_back().unwrap()) {
                        continue;
                    }
                    constructors += 1;
                }
            }
            Language::Cpp | Language::C => {
                let needle_call = format!("{owner}(");
                let needle_brace = format!("{owner}{{");
                for (at, _) in content
                    .match_indices(&needle_call)
                    .chain(content.match_indices(&needle_brace))
                {
                    if at > 0 && is_ident(content[..at].chars().next_back().unwrap()) {
                        continue;
                    }
                    constructors += 1;
                }
            }
            Language::Rust | Language::Go | Language::Java => {}
        }
    }
    constructors
}

#[cfg(test)]
mod tests;
