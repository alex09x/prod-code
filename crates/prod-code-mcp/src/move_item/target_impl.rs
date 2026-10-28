/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::{net::SocketAddr, path::Path};

use anyhow::{Context, Result};
use ra_ap_syntax::{AstNode, Edition, ast, ast::HasName};

use super::budget::{MoveBudget, document_symbols};
use super::item::{append_item, owner_token_pos, resolve_type_def, verified_target_impl_index};
use super::ownership::{Graph, Owner};
use super::rust_syntax::impl_header_for_owner;

pub(super) struct PreparedTargetImpl {
    pub index: usize,
    pub text: String,
    pub owner_binding: OwnerBinding,
}

pub(super) struct OwnerBinding {
    owner: String,
    module: String,
    module_segments: Vec<String>,
    owner_def_path: std::path::PathBuf,
    target_path: std::path::PathBuf,
}

/// Reuses one analyzer-verified target impl or safely creates a qualified one for the same type.
#[allow(clippy::too_many_arguments)]
pub(super) async fn prepare_target_impl(
    budget: &MoveBudget,
    remote: SocketAddr,
    root: &Path,
    source_file: &Path,
    source_text: &str,
    impl_span: (usize, usize),
    target: &Path,
    target_text: &str,
    owner: &str,
    target_is_new: bool,
    graph: &Graph,
    target_owners: &[Owner],
    edition: Edition,
) -> Result<PreparedTargetImpl> {
    let verified_index = if !target_is_new {
        budget
            .run("resolving the target impl", async {
                Ok(verified_target_impl_index(
                    remote,
                    root,
                    source_file,
                    source_text,
                    impl_span,
                    target,
                    target_text,
                    owner,
                )
                .await)
            })
            .await?
    } else {
        None
    };

    if verified_index.is_none() {
        let has_unverified_impl = crate::extract_field::impl_blocks(target_text)
            .into_iter()
            .any(|(target_owner, at, open, _)| {
                target_owner == owner && !target_text[at..open].contains(" for ")
            });
        anyhow::ensure!(
            !has_unverified_impl,
            "target contains an inherent impl for `{owner}` whose type identity cannot be verified; nothing was written"
        );
    }

    let owner_pos = owner_token_pos(source_text, impl_span.0, impl_span.1, owner)
        .context("cannot locate the inherent type; nothing was written")?;
    let (owner_uri, _, _) = budget
        .run("resolving the inherent owner type", async {
            Ok(resolve_type_def(remote, root, source_file, owner_pos).await)
        })
        .await?
        .context("cannot resolve the inherent type; nothing was written")?;
    let owner_def_path = url::Url::parse(&owner_uri)?
        .to_file_path()
        .map_err(|_| anyhow::anyhow!("resolved inherent type `{owner}` is not a local file"))?;
    let owner_owners = graph.owners(&owner_def_path, None)?;
    anyhow::ensure!(
        owner_owners.iter().any(|owner| {
            target_owners
                .iter()
                .any(|target_owner| owner.identity == target_owner.identity)
        }),
        "inherent type `{owner}` and target are not in the same Cargo target; nothing was written"
    );
    let owner_module = graph.caller_spelling(target_owners, &owner_owners)?;
    let qualified_owner = format!("{owner_module}::{owner}");
    let owner_binding = OwnerBinding {
        owner: owner.to_string(),
        module: owner_module.clone(),
        module_segments: target_owners[0].module.segments.clone(),
        owner_def_path: owner_def_path.clone(),
        target_path: target.to_path_buf(),
    };

    if let Some(index) = verified_index {
        return Ok(PreparedTargetImpl {
            index,
            text: target_text.to_string(),
            owner_binding,
        });
    }

    if target.exists() && owner_def_path != target {
        let symbols = budget
            .run(
                "checking target type names",
                document_symbols(remote, root, target),
            )
            .await?;
        let shadows_owner = symbols.as_array().is_some_and(|symbols| {
            symbols.iter().any(|symbol| {
                symbol.get("name").and_then(serde_json::Value::as_str) == Some(owner)
                    && symbol
                        .get("kind")
                        .and_then(serde_json::Value::as_u64)
                        .is_some_and(|kind| matches!(kind, 2 | 3 | 5 | 10 | 11 | 23 | 26))
            })
        });
        anyhow::ensure!(
            !shadows_owner,
            "target declares a type named `{owner}`; its identity cannot be verified, so nothing was written"
        );
    }

    let header = impl_header_for_owner(source_text, impl_span, owner, &qualified_owner, edition)
        .context("cannot qualify the inherent impl owner; nothing was written")?;
    let block = format!("{header}\n}}");
    let text = if target_text.trim().is_empty() {
        format!("{block}\n")
    } else {
        append_item(target_text, &block)
    };
    let index = crate::extract_field::impl_blocks(&text)
        .len()
        .checked_sub(1)
        .ok_or_else(|| anyhow::anyhow!("could not construct target impl for `{owner}`"))?;
    Ok(PreparedTargetImpl {
        index,
        text,
        owner_binding,
    })
}

/// Give moved signatures and bodies the same owner type binding they had in the source module.
pub(super) fn ensure_owner_binding(
    target_text: &str,
    item: &str,
    binding: &OwnerBinding,
    edition: Edition,
) -> Result<(String, Option<String>)> {
    if binding.owner_def_path == binding.target_path
        || !uses_bare_owner(item, &binding.owner, edition)
    {
        return Ok((target_text.to_string(), None));
    }

    let owner_module = resolve_use_path(
        &binding
            .module
            .split("::")
            .map(str::to_string)
            .collect::<Vec<_>>(),
        &binding.module_segments,
    )
    .context("cannot resolve the owner module from the target")?;
    let mut expected = owner_module.clone();
    expected.push(binding.owner.clone());
    let (imports, globs) = target_use_bindings(target_text, edition);
    let mut found_owner = false;
    for (name, path) in imports {
        if name != binding.owner {
            continue;
        }
        let resolved = resolve_use_path(&path, &binding.module_segments);
        anyhow::ensure!(
            resolved.as_ref() == Some(&expected),
            "target already binds `{}` to a different or unresolved import; nothing was written",
            binding.owner
        );
        found_owner = true;
    }
    for path in globs {
        let resolved = resolve_use_path(&path, &binding.module_segments);
        anyhow::ensure!(
            resolved.as_ref() == Some(&owner_module),
            "target has a glob import that may shadow `{}`; its binding cannot be verified, so nothing was written",
            binding.owner
        );
        found_owner = true;
    }
    if found_owner {
        return Ok((target_text.to_string(), None));
    }

    let import = format!("use {}::{};", binding.module, binding.owner);
    let text = super::imports::add_import(target_text, &import);
    Ok((
        text,
        Some(format!("added `{import}` for the moved owner type")),
    ))
}

fn uses_bare_owner(item: &str, owner: &str, edition: Edition) -> bool {
    let parsed = super::rust_syntax::parse(item, edition);
    parsed
        .syntax()
        .descendants()
        .filter_map(ast::NameRef::cast)
        .any(|name| {
            if name.text() != owner {
                return false;
            }
            let start = usize::from(name.syntax().text_range().start());
            !item
                .get(..start)
                .unwrap_or_default()
                .trim_end()
                .ends_with("::")
        })
}

fn target_use_bindings(
    text: &str,
    edition: Edition,
) -> (Vec<(String, Vec<String>)>, Vec<Vec<String>>) {
    let parsed = super::rust_syntax::parse(text, edition);
    let mut imports = Vec::new();
    let mut globs = Vec::new();
    for use_item in parsed
        .syntax()
        .descendants()
        .filter_map(ast::Use::cast)
        .filter(|use_item| {
            use_item
                .syntax()
                .ancestors()
                .all(|node| ast::Module::cast(node).is_none())
        })
    {
        if let Some(tree) = use_item.use_tree() {
            collect_use_tree(&tree, &[], &mut imports, &mut globs);
        }
    }
    (imports, globs)
}

fn collect_use_tree(
    tree: &ast::UseTree,
    prefix: &[String],
    imports: &mut Vec<(String, Vec<String>)>,
    globs: &mut Vec<Vec<String>>,
) {
    let mut path = prefix.to_vec();
    if let Some(tree_path) = tree.path() {
        path.extend(
            tree_path
                .segments()
                .filter_map(|segment| segment.name_ref())
                .map(|name| name.text().to_string()),
        );
    }
    if tree.star_token().is_some() {
        globs.push(path);
        return;
    }
    if let Some(list) = tree.use_tree_list() {
        for nested in list.use_trees() {
            collect_use_tree(&nested, &path, imports, globs);
        }
        return;
    }

    let rename = tree.rename();
    let alias = rename
        .as_ref()
        .and_then(|rename| rename.name())
        .map(|name| name.text().to_string());
    if rename.is_some() && alias.is_none() {
        return;
    }
    if path.last().is_some_and(|segment| segment == "self") {
        path.pop();
    }
    let Some(name) = alias.or_else(|| path.last().cloned()) else {
        return;
    };
    imports.push((name, path));
}

fn resolve_use_path(path: &[String], target_module: &[String]) -> Option<Vec<String>> {
    let (first, rest) = path.split_first()?;
    let mut resolved = match first.as_str() {
        "crate" => Vec::new(),
        "self" => target_module.to_vec(),
        "super" => {
            let mut module = target_module.to_vec();
            let mut rest = path;
            while let Some("super") = rest.first().map(String::as_str) {
                if module.pop().is_none() {
                    return None;
                }
                rest = &rest[1..];
            }
            module.extend_from_slice(rest);
            return Some(module);
        }
        _ => return None,
    };
    resolved.extend_from_slice(rest);
    Some(resolved)
}
