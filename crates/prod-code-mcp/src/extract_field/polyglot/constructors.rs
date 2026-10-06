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
use std::path::{Path, PathBuf};

use crate::extract_field::helpers::{display, is_ident, is_in_literal_or_comment, mentions};
use crate::extract_field::polyglot::parsers::language_matches_extension;
use crate::parameter_object::Language;

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
    let mut constructors = 0usize;
    if lang == Language::Go {
        let target_package = text.lines().find_map(|line| {
            line.trim_start()
                .strip_prefix("package ")
                .and_then(|p| p.split_whitespace().next())
        });
        let target_dir = file.parent().unwrap_or(root);
        let target_dir =
            std::fs::canonicalize(target_dir).unwrap_or_else(|_| target_dir.to_path_buf());
        for entry in ignore::WalkBuilder::new(root).build().flatten() {
            let path = entry.path();
            if !path.is_file() || path.extension().and_then(|s| s.to_str()) != Some("go") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(path) else {
                continue;
            };
            let needle = format!("{owner}{{");
            if !content.contains(&needle) {
                continue;
            }
            let mut file_edits = Vec::new();
            let mut cur = 0;
            while let Some(pos) = content[cur..].find(&needle) {
                let hit = cur + pos;
                cur = hit + needle.len();
                if is_in_literal_or_comment(&content, hit, lang) {
                    continue;
                }
                if hit > 0 && is_ident(content[..hit].chars().next_back().unwrap()) {
                    continue;
                }
                if content[..hit].trim_end().ends_with('.') {
                    unmatched.push(format!(
                        "{}:{}: Go literal type `{owner}` is package-qualified and cannot be matched to the selected declaration",
                        display(root, path),
                        content[..hit].lines().count()
                    ));
                    continue;
                }
                let candidate_package = content.lines().find_map(|line| {
                    line.trim_start()
                        .strip_prefix("package ")
                        .and_then(|p| p.split_whitespace().next())
                });
                let candidate_dir = path.parent().unwrap_or(root);
                let candidate_dir = std::fs::canonicalize(candidate_dir)
                    .unwrap_or_else(|_| candidate_dir.to_path_buf());
                if target_package.is_none()
                    || candidate_package != target_package
                    || candidate_dir != target_dir
                {
                    unmatched.push(format!(
                        "{}:{}: unqualified Go literal type `{owner}` is in another package and cannot be resolved safely",
                        display(root, path),
                        content[..hit].lines().count()
                    ));
                    continue;
                }
                let open = hit + owner.len();
                let Some(close) = crate::parameter_object::matching_bracket(&content, open) else {
                    continue;
                };
                let inside = &content[open + 1..close];
                if mentions(inside, &format!("{name}:")) {
                    continue;
                }
                let is_multiline = inside.contains('\n');
                let insertion = if is_multiline {
                    let indent = inside
                        .lines()
                        .skip(1)
                        .find(|l| !l.trim().is_empty())
                        .map(|l| {
                            l.chars()
                                .take_while(|c| c.is_whitespace())
                                .collect::<String>()
                        })
                        .unwrap_or_else(|| "        ".to_string());
                    format!("\n{indent}{name}: {init},")
                } else if inside.trim().is_empty() {
                    format!("{name}: {init}")
                } else {
                    format!("{name}: {init}, ")
                };
                file_edits.push((open + 1, 0, insertion));
                constructors += 1;
            }
            if !file_edits.is_empty() {
                texts
                    .entry(path.to_path_buf())
                    .or_insert_with(|| content.clone());
                edits
                    .entry(path.to_path_buf())
                    .or_default()
                    .extend(file_edits);
            }
        }
    } else {
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
                _ => {}
            }
        }
    }
    constructors
}
