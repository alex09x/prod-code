/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::engine::RustEngine;
use std::path::Path;

/// One analyzer for registry and path dependencies, owner qualification, precedence,
/// trait methods, and useful near-miss candidates (#1218).
pub(super) fn assert_cases(engine: &RustEngine, dep: &Path, ws: &Path, path_dep: &Path) {
    let dep_file = dep.join("src/lib.rs");
    let local_file = ws.join("src/lib.rs");
    let path_file = path_dep.join("src/lib.rs");
    for (query, file, expected) in [
        (
            "Analysis::parent_module",
            &dep_file,
            vec![("parent_module", 11, 12, "gadget::Analysis")],
        ),
        (
            "gadget::Analysis::parent_module",
            &dep_file,
            vec![("parent_module", 11, 12, "gadget::Analysis")],
        ),
        (
            "Dep::method",
            &dep_file,
            vec![("method", 15, 12, "gadget::Dep")],
        ),
        (
            "gadget::Dep::method",
            &dep_file,
            vec![("method", 15, 12, "gadget::Dep")],
        ),
        (
            "SharedName::method",
            &local_file,
            vec![("method", 9, 12, "app::SharedName")],
        ),
        (
            "gadget::SharedName::method",
            &dep_file,
            vec![("method", 22, 12, "gadget::SharedName")],
        ),
        (
            "DepTrait::trait_method",
            &dep_file,
            vec![("trait_method", 19, 8, "gadget::DepTrait")],
        ),
        (
            "gadget::DepTrait::trait_method",
            &dep_file,
            vec![("trait_method", 19, 8, "gadget::DepTrait")],
        ),
        (
            "Dep::methdo",
            &dep_file,
            vec![
                ("method", 15, 12, "gadget::Dep"),
                ("method_more", 16, 12, "gadget::Dep"),
            ],
        ),
        (
            "PathDep::method",
            &path_file,
            vec![("method", 3, 12, "path_dep::PathDep")],
        ),
        (
            "path_dep::PathDep::method",
            &path_file,
            vec![("method", 3, 12, "path_dep::PathDep")],
        ),
    ] {
        let found = engine.workspace_symbols(query, 10).unwrap();
        let actual: Vec<_> = found
            .iter()
            .map(|hit| {
                assert_eq!(&hit.path, file, "{query}: {found:?}");
                (
                    hit.name.as_str(),
                    hit.line,
                    hit.col,
                    hit.container.as_deref().unwrap(),
                )
            })
            .collect();
        assert_eq!(actual, expected, "{query}");
        for hit in found {
            let definitions = engine
                .goto_definition(&hit.path, hit.line, hit.col)
                .unwrap();
            assert_eq!(definitions.len(), 1, "{query}: {definitions:?}");
            assert_eq!(definitions[0].path, hit.path, "{query}");
            assert_eq!(
                (definitions[0].line, definitions[0].col),
                (hit.line, hit.col),
                "{query}"
            );
            let text = std::fs::read_to_string(&hit.path).unwrap();
            assert!(
                text.lines()
                    .nth(hit.line as usize - 1)
                    .unwrap()
                    .contains(&format!("fn {}(", hit.name)),
                "{query}"
            );
        }
    }
}
