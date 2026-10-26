/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{
    ownership::{Graph, Owner},
    rust_syntax,
};
use anyhow::Result;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

pub(super) struct ItemPlan {
    pub graph: Graph,
    pub from: Vec<Owner>,
    pub to: Vec<Owner>,
    pub parents: Vec<Owner>,
    pub new_module_parent: Option<PathBuf>,
    pub target_text: String,
}

impl ItemPlan {
    pub fn new(
        root: &Path,
        file: &Path,
        target: &Path,
        line: u32,
        col: u32,
        text: &str,
    ) -> Result<Self> {
        let graph = Graph::load(root, file)?;
        graph.ensure_unaliased(root, [file, target])?;
        let from = graph.owners(file, crate::signature::offset_of(text, line, col))?;
        let (to, parents) = graph.destination(target, &from)?;
        let new_module_parent = if target.exists() {
            None
        } else {
            parents
                .first()
                .map(|p| p.file.clone())
                .or_else(|| to[0].declaration.as_ref().map(|(p, _)| p.clone()))
        };
        let target_text = if target.exists() {
            rust_syntax::read(target)?
        } else {
            String::new()
        };
        Ok(Self {
            graph,
            from,
            to,
            parents,
            new_module_parent,
            target_text,
        })
    }

    pub fn declare(
        &self,
        rewritten: &mut BTreeMap<PathBuf, String>,
        target: &Path,
        public: bool,
    ) -> Result<()> {
        let module_name = target.file_stem().unwrap_or_default().to_string_lossy();
        let mut attributes = BTreeMap::new();
        for owner in &self.parents {
            if target.parent() != Some(owner.module_dir.as_path()) {
                let literal =
                    super::ownership::filesystem::path_literal(owner.attribute_base(), target)?;
                let key = (owner.file.clone(), owner.scope.start);
                if let Some(previous) = attributes.insert(key, literal.clone()) {
                    anyhow::ensure!(
                        previous == literal,
                        "shared target has incompatible path attributes; nothing was written"
                    );
                }
            }
        }
        let mut declared = BTreeSet::new();
        for owner in &self.parents {
            if !declared.insert((owner.file.clone(), owner.scope.start)) {
                continue;
            }
            let mut text = rewritten
                .get(&owner.file)
                .cloned()
                .unwrap_or(rust_syntax::read(&owner.file)?);
            if owner.scope.start == 0 {
                text = super::module::declare_module_in(
                    &text,
                    &module_name,
                    public,
                    rust_syntax::edition(&owner.root),
                );
                if let Some(literal) = attributes.get(&(owner.file.clone(), owner.scope.start)) {
                    let scope = 0..text.len();
                    let decl =
                        rust_syntax::declarations(&text, scope, rust_syntax::edition(&owner.root))
                            .into_iter()
                            .find(|d| d.name == module_name)
                            .unwrap();
                    text.insert_str(decl.span.start, &format!("#[path={literal}]\n"));
                }
            } else {
                let scope = self.graph.scope_in(owner, &text)?;
                text.insert_str(
                    scope.end - 1,
                    &format!(
                        "\n{}{}mod {module_name};\n",
                        attributes
                            .get(&(owner.file.clone(), owner.scope.start))
                            .map(|literal| format!("#[path={literal}]\n"))
                            .unwrap_or_default(),
                        if public { "pub " } else { "" }
                    ),
                );
            }
            rewritten.insert(owner.file.clone(), text);
        }
        Ok(())
    }
}
