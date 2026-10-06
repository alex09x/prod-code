/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::syntax::{
    derives_above, header_from, impl_header_start, impl_trait, is_trait_decl, supertraits, word_at,
};
use super::types::{Kind, Supertype, Supertypes};
use super::validate::locations;
use crate::tools::execute_lsp_query;
use anyhow::Result;
use std::collections::HashSet;
use std::future::Future;
use std::net::SocketAddr;
use std::path::Path;
use std::pin::Pin;

pub(crate) fn expand_rust_trait_supertraits<'a>(
    remote: SocketAddr,
    root: &'a Path,
    file: &'a Path,
    trait_name: &'a str,
    ref_file: &'a Path,
    ref_line: u32,
    ref_col: u32,
    level: usize,
    max_depth: usize,
    seen: &'a mut HashSet<String>,
) -> Pin<Box<dyn Future<Output = Vec<Supertype>> + Send + 'a>> {
    Box::pin(async move {
        if level >= max_depth {
            return Vec::new();
        }
        let bare = trait_name.split('<').next().unwrap_or(trait_name).trim();
        if bare.starts_with('\'') || bare.is_empty() {
            return Vec::new();
        }
        let ref_uri = match url::Url::from_file_path(ref_file) {
            Ok(u) => u.to_string(),
            Err(_) => return Vec::new(),
        };
        let def_res = execute_lsp_query(
            remote,
            root,
            ref_file,
            "textDocument/definition",
            serde_json::json!({
                "textDocument": { "uri": ref_uri },
                "position": { "line": ref_line.saturating_sub(1), "character": ref_col.saturating_sub(1) },
            }),
        )
        .await;
        let Some((def_file, def_line, def_col)) =
            def_res.ok().and_then(|v| locations(&v).into_iter().next())
        else {
            return Vec::new();
        };
        let key = format!("{}:{def_line}:{def_col}", def_file.display());
        if !seen.insert(key.clone()) {
            return Vec::new();
        }
        let text = if let Ok((bytes, _)) =
            crate::remote_fs::read_source(remote, root, &def_file.to_string_lossy()).await
        {
            String::from_utf8_lossy(&bytes).into_owned()
        } else {
            std::fs::read_to_string(&def_file).unwrap_or_default()
        };
        let lines: Vec<&str> = text.lines().collect();
        if def_line as usize >= lines.len() {
            return Vec::new();
        }
        let decl = lines[def_line as usize];
        if !is_trait_decl(decl) {
            return Vec::new();
        }
        let sub_names = supertraits(&header_from(&lines, def_line as usize));
        let mut children = Vec::with_capacity(sub_names.len());
        for name in sub_names {
            let bare_sub = name.split('<').next().unwrap_or(&name).trim();
            let mut sub_line = def_line + 1;
            let mut sub_col = def_col + 1;
            for (idx, line_str) in lines.iter().enumerate().skip(def_line as usize) {
                if let Some(pos) = line_str.find(bare_sub) {
                    sub_line = idx as u32 + 1;
                    sub_col = pos as u32 + 1;
                    break;
                }
                if line_str.contains(['{', ';']) {
                    break;
                }
            }
            let mut node = Supertype::new(name.clone(), false, None);
            let child_key = format!("{name}:{sub_line}:{sub_col}");
            if seen.contains(&child_key) {
                node.repeated = true;
            } else if level + 1 < max_depth {
                node.children = expand_rust_trait_supertraits(
                    remote,
                    root,
                    file,
                    &name,
                    &def_file,
                    sub_line,
                    sub_col,
                    level + 1,
                    max_depth,
                    seen,
                )
                .await;
            }
            children.push(node);
        }
        children
    })
}

pub(crate) async fn rust_supertypes(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    character: u32,
    position: serde_json::Value,
    depth: usize,
) -> Result<Supertypes> {
    // The declaration: where the name at the position is defined, or the position itself.
    let definition = execute_lsp_query(remote, root, file, "textDocument/definition", position)
        .await
        .ok()
        .and_then(|v| locations(&v).into_iter().next())
        .unwrap_or((
            file.to_path_buf(),
            line.saturating_sub(1),
            character.saturating_sub(1),
        ));
    let (decl_file, decl_line, decl_col) = definition;
    let text = if let Ok((bytes, _)) =
        crate::remote_fs::read_source(remote, root, &decl_file.to_string_lossy()).await
    {
        String::from_utf8_lossy(&bytes).into_owned()
    } else {
        std::fs::read_to_string(&decl_file).unwrap_or_default()
    };
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return Ok(Supertypes {
            of: "?".to_string(),
            kind: Kind::Other,
            list: Vec::new(),
            depth,
            unsupported: None,
        });
    }
    let decl_idx = (decl_line as usize).min(lines.len() - 1);
    let decl = lines.get(decl_idx).copied().unwrap_or("");
    let of = word_at(decl, decl_col + 1).unwrap_or_else(|| "?".to_string());
    if is_trait_decl(decl) {
        let raw_supertraits = supertraits(&header_from(&lines, decl_idx));
        let mut seen = HashSet::new();
        seen.insert(format!("{}:{decl_line}:{decl_col}", decl_file.display()));
        let mut list = Vec::with_capacity(raw_supertraits.len());
        for name in raw_supertraits {
            let mut node = Supertype::new(name.clone(), false, None);
            if depth > 1 {
                let bare_sub = name.split('<').next().unwrap_or(&name).trim();
                let mut sub_line = decl_line + 1;
                let mut sub_col = decl_col + 1;
                for (idx, line_str) in lines.iter().enumerate().skip(decl_idx) {
                    if let Some(pos) = line_str.find(bare_sub) {
                        sub_line = idx as u32 + 1;
                        sub_col = pos as u32 + 1;
                        break;
                    }
                    if line_str.contains(['{', ';']) {
                        break;
                    }
                }
                node.children = expand_rust_trait_supertraits(
                    remote, root, file, &name, &decl_file, sub_line, sub_col, 1, depth, &mut seen,
                )
                .await;
            }
            list.push(node);
        }
        return Ok(Supertypes {
            of,
            kind: Kind::Trait,
            list,
            depth,
            unsupported: None,
        });
    }
    let decl_uri = url::Url::from_file_path(&decl_file)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {decl_file:?}"))?
        .to_string();
    let impls = execute_lsp_query(
        remote,
        root,
        &decl_file,
        "textDocument/implementation",
        serde_json::json!({
            "textDocument": { "uri": decl_uri },
            "position": { "line": decl_line, "character": decl_col },
        }),
    )
    .await?;
    let mut list: Vec<Supertype> = derives_above(&lines, decl_idx)
        .into_iter()
        .map(|(name, l, c)| Supertype::new(name, true, Some((decl_file.clone(), l, c))))
        .collect();
    for (path, l, c) in locations(&impls) {
        let text = if let Ok((bytes, _)) =
            crate::remote_fs::read_source(remote, root, &path.to_string_lossy()).await
        {
            String::from_utf8_lossy(&bytes).into_owned()
        } else {
            std::fs::read_to_string(&path).unwrap_or_default()
        };
        let lines: Vec<&str> = text.lines().collect();
        if l as usize >= lines.len() {
            continue;
        }
        // A derive, reported at its attribute or at the type's name, is already listed.
        let Some(start) = impl_header_start(&lines, l as usize) else {
            continue;
        };
        if let Some(name) = impl_trait(&header_from(&lines, start))
            && !list.iter().any(|s| s.name == name)
        {
            list.push(Supertype::new(
                name,
                false,
                Some((path.clone(), l + 1, c + 1)),
            ));
        }
    }
    list.sort_by(|a, b| a.name.cmp(&b.name));
    if depth > 1 {
        let mut seen = HashSet::new();
        seen.insert(format!("{}:{decl_line}:{decl_col}", decl_file.display()));
        for s in &mut list {
            if let Some((path, l, c)) = &s.at {
                s.children = expand_rust_trait_supertraits(
                    remote, root, file, &s.name, path, *l, *c, 1, depth, &mut seen,
                )
                .await;
            }
        }
    }
    Ok(Supertypes {
        of,
        kind: Kind::Type,
        list,
        depth,
        unsupported: None,
    })
}
