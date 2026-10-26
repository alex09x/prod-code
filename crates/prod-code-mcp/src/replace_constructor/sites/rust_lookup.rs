/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::rust_crate::CrateRoot;
use ra_ap_syntax::{AstNode, SyntaxKind, SyntaxNode, ast, ast::HasName};

/// Missing names can continue in an outer scope. Ambiguous imports cannot.
pub(super) type Resolution = Result<Option<SyntaxNode>, ()>;

pub(super) fn resolve(path: &ast::Path, crate_root: &CrateRoot) -> Resolution {
    resolve_names(path.syntax(), &path_names(path).ok_or(())?, crate_root)
}

pub(super) fn resolve_names(
    context: &SyntaxNode,
    names: &[String],
    crate_root: &CrateRoot,
) -> Resolution {
    Resolver {
        imports: Vec::new(),
        crate_root,
    }
    .names(context, names, 0)
}

fn is_scope(node: &SyntaxNode) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::SOURCE_FILE | SyntaxKind::ITEM_LIST | SyntaxKind::STMT_LIST
    )
}

fn module_scope(context: &SyntaxNode) -> Option<SyntaxNode> {
    context
        .ancestors()
        .find(|n| matches!(n.kind(), SyntaxKind::ITEM_LIST | SyntaxKind::SOURCE_FILE))
}

fn named_item(node: &SyntaxNode) -> Option<String> {
    let name = match ast::Item::cast(node.clone())? {
        ast::Item::Struct(item) => item.name(),
        ast::Item::Enum(item) => item.name(),
        ast::Item::Union(item) => item.name(),
        ast::Item::TypeAlias(item) => item.name(),
        ast::Item::Module(item) => item.name(),
        _ => None,
    }?;
    Some(name.text().to_string())
}

fn path_names(path: &ast::Path) -> Option<Vec<String>> {
    path.segments()
        .map(|s| Some(s.name_ref()?.text().to_string()))
        .collect()
}

struct Import {
    tree: ast::UseTree,
    names: Vec<String>,
    local: Option<String>,
}

fn imports(tree: ast::UseTree, mut prefix: Vec<String>, result: &mut Vec<Import>) {
    if let Some(path) = tree.path() {
        let Some(names) = path_names(&path) else {
            return;
        };
        if names != ["self"] || prefix.is_empty() {
            prefix.extend(names);
        }
    }
    if let Some(list) = tree.use_tree_list() {
        for child in list.use_trees() {
            imports(child, prefix.clone(), result);
        }
    } else {
        let local = if tree.star_token().is_some() {
            None
        } else {
            tree.rename()
                .and_then(|r| r.name())
                .map(|n| n.text().to_string())
                .or_else(|| prefix.last().cloned())
        };
        result.push(Import {
            tree,
            names: prefix,
            local,
        });
    }
}

struct Resolver<'a> {
    // A glob must not import the module used to resolve its own path. Skipping
    // that edge also allows finite traversal of cyclic glob reexports.
    imports: Vec<SyntaxNode>,
    crate_root: &'a CrateRoot,
}

impl Resolver<'_> {
    fn parent_scope(&self, scope: SyntaxNode, context: &SyntaxNode) -> Option<SyntaxNode> {
        scope.parent().and_then(|p| p.parent()).or_else(|| {
            // The saved file parent belongs only to this source root, never to
            // the external root reached by a preceding super segment.
            (context.ancestors().last().as_ref() == Some(&scope))
                .then(|| self.crate_root.parent())
                .flatten()
        })
    }

    fn scope(&mut self, scope: &SyntaxNode, name: &str, depth: usize) -> Resolution {
        if depth > 16 {
            return Err(());
        }
        if let Some(item) = scope
            .children()
            .find(|item| named_item(item).as_deref() == Some(name))
        {
            return Ok(Some(item));
        }
        let mut uses = Vec::new();
        for tree in scope
            .children()
            .filter_map(ast::Use::cast)
            .filter_map(|u| u.use_tree())
        {
            imports(tree, Vec::new(), &mut uses);
        }
        // Explicit bindings shadow globs, regardless of their source order.
        if let Some(import) = uses.iter().find(|i| i.local.as_deref() == Some(name)) {
            if self.imports.contains(import.tree.syntax()) {
                return Err(());
            }
            return self.import(import, depth)?.map_or(Err(()), |n| Ok(Some(n)));
        }
        let mut found = None;
        for import in uses.iter().filter(|i| i.local.is_none()) {
            if self.imports.contains(import.tree.syntax()) {
                continue;
            }
            let module = self.import(import, depth)?.ok_or(())?;
            let scope = if is_scope(&module) {
                module
            } else {
                ast::Module::cast(module)
                    .and_then(|m| m.item_list())
                    .ok_or(())?
                    .syntax()
                    .clone()
            };
            // Keep this import active while traversing its reexports as well.
            self.imports.push(import.tree.syntax().clone());
            let candidate = self.scope(&scope, name, depth + 1);
            self.imports.pop();
            if let Some(candidate) = candidate? {
                if found.as_ref().is_some_and(|item| item != &candidate) {
                    return Err(());
                }
                found = Some(candidate);
            }
        }
        Ok(found)
    }

    fn import(&mut self, import: &Import, depth: usize) -> Resolution {
        self.imports.push(import.tree.syntax().clone());
        let result = self.names(import.tree.syntax(), &import.names, depth + 1);
        self.imports.pop();
        result
    }

    fn lexical(&mut self, context: &SyntaxNode, name: &str, depth: usize) -> Resolution {
        for ancestor in context.ancestors() {
            if let Some(params) = ancestor.children().find_map(ast::GenericParamList::cast)
                && let Some(param) = params
                    .syntax()
                    .children()
                    .filter_map(ast::TypeParam::cast)
                    .find(|p| p.name().is_some_and(|n| n.text() == name))
            {
                return Ok(Some(param.syntax().clone()));
            }
            if is_scope(&ancestor) {
                if let Some(item) = self.scope(&ancestor, name, depth)? {
                    return Ok(Some(item));
                }
                // Inline modules have their own namespace, not implicit parent imports.
                if ancestor.kind() == SyntaxKind::ITEM_LIST {
                    break;
                }
            }
        }
        Ok(None)
    }

    fn names(&mut self, context: &SyntaxNode, names: &[String], depth: usize) -> Resolution {
        if depth > 16 {
            return Err(());
        }
        let mut names = names.iter();
        let Some(first) = names.next() else {
            return Ok(None);
        };
        let mut item = match first.as_str() {
            "crate" => Some(self.crate_root.node(context).ok_or(())?),
            "self" => module_scope(context),
            "super" => module_scope(context).and_then(|n| self.parent_scope(n, context)),
            "Self" => {
                let owner = context.ancestors().find_map(ast::Impl::cast).ok_or(())?;
                let Some(ast::Type::PathType(ty)) = owner.self_ty() else {
                    return Err(());
                };
                let path = ty.path().ok_or(())?;
                self.names(path.syntax(), &path_names(&path).ok_or(())?, depth + 1)?
            }
            _ => self.lexical(context, first, depth)?,
        };
        for name in names {
            let Some(mut current) = item else {
                return Ok(None);
            };
            if let Some(alias) = ast::TypeAlias::cast(current.clone()) {
                let Some(ast::Type::PathType(ty)) = alias.ty() else {
                    return Err(());
                };
                let path = ty.path().ok_or(())?;
                current = self
                    .names(path.syntax(), &path_names(&path).ok_or(())?, depth + 1)?
                    .ok_or(())?;
            }
            // A known enum variant is a distinct constructor identity. An
            // arbitrary associated path on another type remains unresolved.
            if let Some(owner) = ast::Enum::cast(current.clone()) {
                item = owner
                    .variant_list()
                    .and_then(|list| {
                        list.variants().find(|variant| {
                            variant.name().is_some_and(|n| n.text() == name.as_str())
                        })
                    })
                    .map(|variant| variant.syntax().clone());
                continue;
            }
            if name == "super" && is_scope(&current) {
                item = self.parent_scope(current, context);
                continue;
            }
            let scope = if is_scope(&current) {
                current
            } else {
                let Some(module) = ast::Module::cast(current) else {
                    return Ok(None);
                };
                // Out-of-line modules need semantic references, not parent-name guesses.
                module.item_list().ok_or(())?.syntax().clone()
            };
            item = self.scope(&scope, name, depth)?;
        }
        if let Some(alias) = item.clone().and_then(ast::TypeAlias::cast)
            && let Some(ast::Type::PathType(target)) = alias.ty()
        {
            let names = target.path().and_then(|p| path_names(&p)).ok_or(())?;
            return self.names(target.syntax(), &names, depth + 1);
        }
        Ok(item)
    }
}
