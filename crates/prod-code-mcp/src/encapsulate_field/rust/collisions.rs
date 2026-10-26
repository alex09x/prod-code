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
use std::{
    collections::BTreeSet,
    net::SocketAddr,
    path::{Path, PathBuf},
};

/// Insertion scope and impl ownership are independent: a descendant may import the
/// selected type under any name, or define an unrelated equally named type.
#[allow(clippy::too_many_arguments)]
pub(super) async fn check(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: &str,
    struct_at: usize,
    field: &str,
    setter: bool,
    local: &[(usize, usize)],
) -> Result<()> {
    let mask =
        crate::caller_migration::lexical_code_mask(text, crate::parameter_object::Language::Rust);
    let code = String::from_utf8(
        text.bytes()
            .enumerate()
            .map(|(i, b)| if mask[i] || b == b'\n' { b } else { b' ' })
            .collect(),
    )
    .context("invalid Rust code mask")?;
    let methods: Vec<_> = std::iter::once(field.to_owned())
        .chain(setter.then(|| format!("set_{field}")))
        .map(|name| {
            let pattern = format!(r"\bfn\s+(?:r#)?{}\s*[(<]", regex::escape(&name));
            (
                name,
                regex::Regex::new(&pattern).expect("escaped accessor name"),
            )
        })
        .collect();
    for (owner, start, open, close) in impl_blocks(&code) {
        // Same-scope impls already have the existing collision guard. Resolve every
        // other potentially colliding impl, even when its owner is an alias/path.
        if local.contains(&(open, close)) || code[start..open].contains(" for ") {
            continue;
        }
        let Some((method, _)) = methods
            .iter()
            .find(|(_, re)| re.is_match(&code[open..close]))
        else {
            continue;
        };
        let at = owner_position(&code, start, open, &owner).context(
            "cannot locate impl owner for accessor collision check; nothing was written",
        )?;
        let (owner_file, declaration) = nominal_owner(remote, root, file, at).await?;
        anyhow::ensure!(
            owner_file != std::fs::canonicalize(file)?
                || declaration != struct_at + "struct ".len(),
            "selected type already has a `fn {method}` in {}; rename it or the field first",
            crate::extract_field::helpers::display(root, file)
        );
    }
    Ok(())
}

/// A distinct definition location may be a type alias for the selected struct.
/// Follow its RHS semantically; only a nominal declaration proves a distinct owner.
async fn nominal_owner(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    at: usize,
) -> Result<(PathBuf, usize)> {
    let mut file = file.to_path_buf();
    let mut at = at;
    let mut visited = BTreeSet::new();
    for _ in 0..32 {
        let text = std::fs::read_to_string(&file)?;
        let (line, col) = crate::signature::position_at(&text, at)?;
        let answer = crate::tools::execute_lsp_query(
            remote,
            root,
            &file,
            "textDocument/definition",
            serde_json::json!({
                "textDocument": {"uri": prod_code_protocol::path::file_uri(&file)},
                "position": {"line": line - 1, "character": col - 1},
            }),
        )
        .await
        .context(
            "cannot resolve impl ownership for accessor collision check; nothing was written",
        )?;
        let targets = crate::slice::lsp::parse_locations(&answer)
            .map_err(anyhow::Error::msg)
            .context("invalid impl ownership answer; nothing was written")?;
        anyhow::ensure!(
            targets.len() == 1,
            "cannot prove impl ownership for accessor collision check; nothing was written"
        );
        let target = &targets[0];
        file = match &target.source {
            crate::slice::lsp::Source::File(path) => std::fs::canonicalize(path)?,
            _ => anyhow::bail!("cannot prove nominal impl owner; nothing was written"),
        };
        let text = std::fs::read_to_string(&file)?;
        let pos = target.range.start;
        at = crate::signature::offset_of(&text, pos.line + 1, pos.character + 1)
            .context("impl owner definition is outside the source; nothing was written")?;
        anyhow::ensure!(
            visited.insert((file.clone(), at)),
            "cyclic impl owner definition; nothing was written"
        );
        let mask = crate::caller_migration::lexical_code_mask(
            &text,
            crate::parameter_object::Language::Rust,
        );
        let code = String::from_utf8(
            text.bytes()
                .enumerate()
                .map(|(i, b)| if mask[i] || b == b'\n' { b } else { b' ' })
                .collect(),
        )?;
        let prefix = code[..at].trim_end();
        let kind = ["struct", "enum", "union", "type"]
            .into_iter()
            .find(|word| {
                prefix
                    .strip_suffix(word)
                    .is_some_and(|lead| !lead.ends_with(super::super::case::is_ident))
            })
            .context("definition does not prove nominal impl ownership; nothing was written")?;
        anyhow::ensure!(
            code[at..].starts_with(super::super::case::is_ident),
            "impl owner definition is not a type name; nothing was written"
        );
        if kind != "type" {
            return Ok((file, at));
        }
        at = alias_target(&code, at)?;
    }
    anyhow::bail!("impl owner alias chain is too deep; nothing was written")
}

/// Locate the terminal path identifier in an ordinary alias RHS, without looking
/// up same-spelled names elsewhere. Unknown alias forms refuse before generation.
fn alias_target(code: &str, at: usize) -> Result<usize> {
    let end = code[at..]
        .find(';')
        .map(|n| at + n)
        .context("alias declaration does not end; nothing was written")?;
    let mut depth = 0usize;
    let equals = code[at..end]
        .char_indices()
        .find_map(|(off, ch)| {
            match ch {
                '<' => depth += 1,
                '>' => depth = depth.saturating_sub(1),
                '=' if depth == 0 => return Some(at + off),
                _ => {}
            }
            None
        })
        .context("alias has no type RHS; nothing was written")?;
    let rhs = code[equals + 1..end].trim_start();
    let start = end - rhs.len();
    let path = regex::Regex::new(
        r"^(?:::)?(?:r#)?[\p{L}_][\p{L}\p{N}_]*(?:\s*::\s*(?:r#)?[\p{L}_][\p{L}\p{N}_]*)*",
    )?
    .find(rhs)
    .context("cannot resolve alias type path; nothing was written")?;
    let last = path.as_str().rsplit("::").next().unwrap().trim_start();
    Ok(start + path.end() - last.len())
}

/// Walk executable impl headers regardless of line layout. The shared extraction
/// locator only accepts item starts at line boundaries and misses inline modules.
fn impl_blocks(code: &str) -> Vec<(String, usize, usize, usize)> {
    let mut blocks = Vec::new();
    for (start, _) in code.match_indices("impl") {
        let after = start + "impl".len();
        if code[..start].ends_with(super::super::case::is_ident)
            || !(code[after..].starts_with('<') || code[after..].starts_with(char::is_whitespace))
        {
            continue;
        }
        let Some(open) = code[after..].find(['{', ';']).map(|off| after + off) else {
            continue;
        };
        if code.as_bytes()[open] != b'{' {
            continue;
        }
        if let (Some(owner), Some(close)) = (
            crate::extract_field::self_type(&code[after..open]),
            crate::parameter_object::matching_bracket(code, open),
        ) {
            blocks.push((owner, start, open, close));
        }
    }
    blocks
}

fn owner_position(text: &str, start: usize, open: usize, owner: &str) -> Option<usize> {
    let mut at = start + "impl".len();
    at += text[at..open].len() - text[at..open].trim_start().len();
    if text[at..].starts_with('<') {
        let mut depth = 0;
        for (off, ch) in text[at..open].char_indices() {
            match ch {
                '<' => depth += 1,
                '>' => {
                    depth -= 1;
                    if depth == 0 {
                        at += off + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    let path = text[at..open].split('<').next()?.split("where").next()?;
    path.rfind(owner).map(|off| at + off)
}
