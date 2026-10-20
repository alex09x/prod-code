/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Workspace symbol resolution, search across projects, and LSP coordinate translation.

pub mod across_projects;
pub mod alias;
pub mod declared_at;
pub mod matching;
pub mod nested_projects;
pub mod resolve;
pub mod search;
pub mod sources;
pub mod type_members;
pub mod types;
pub mod unindexed;

pub use matching::DEFAULT_AMBIGUOUS_CANDIDATE_LIMIT;
pub use resolve::{resolve_symbol, resolve_symbol_candidates};
pub use search::workspace_symbol_search;
pub use types::SymbolHit;

#[allow(unused_imports)]
pub(crate) use across_projects::*;
#[allow(unused_imports)]
pub(crate) use alias::*;
#[allow(unused_imports)]
pub(crate) use declared_at::*;
#[allow(unused_imports)]
pub(crate) use matching::*;
#[allow(unused_imports)]
pub(crate) use nested_projects::*;
#[allow(unused_imports)]
pub(crate) use search::*;
#[allow(unused_imports)]
pub(crate) use sources::*;
#[allow(unused_imports)]
pub(crate) use type_members::*;
#[allow(unused_imports)]
pub(crate) use types::*;
#[allow(unused_imports)]
pub(crate) use unindexed::*;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn go_collect_members_and_source_members_prefer_receiver_methods_over_field_keys() {
        // gopls top-level method symbols use `(*Store).ConstructionStatus` (#1110).
        let symbols = json!([{
            "name": "(*Store).ConstructionStatus",
            "kind": 6,
            "selectionRange": {
                "start": { "line": 14, "character": 18 },
                "end": { "line": 14, "character": 36 }
            }
        }]);
        let mut hits = Vec::new();
        let root = std::path::Path::new("/tmp");
        let file = std::path::Path::new("/tmp/construction_status.go");
        collect_members(
            &symbols,
            root,
            file,
            &["Store"],
            "ConstructionStatus",
            &[],
            &mut hits,
        )
        .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].2, 15);
        assert_eq!(hits[0].3, 19);

        let dir = tempfile::tempdir().unwrap();
        let callsite_file = dir.path().join("turn_test.go");
        std::fs::write(
            &callsite_file,
            "package pluginstore\nfunc testTurn(s *Store) {\n\t_ = Status{\n\t\tConstructionStatus: s.ConstructionStatus(1),\n\t}\n}\n",
        )
        .unwrap();
        let callsite_hits =
            find_source_members(dir.path(), &callsite_file, &["Store"], "ConstructionStatus");
        assert!(
            callsite_hits.is_empty(),
            "composite-literal field key in non-type file must not match as Store declaration: {callsite_hits:?}"
        );

        let method_file = dir.path().join("construction_status.go");
        std::fs::write(
            &method_file,
            "package pluginstore\n\nfunc (s *Store) ConstructionStatus(id int64) string {\n\treturn \"ok\"\n}\n",
        )
        .unwrap();
        let method_hits =
            find_source_members(dir.path(), &method_file, &["Store"], "ConstructionStatus");
        assert_eq!(method_hits.len(), 1);
        assert_eq!(method_hits[0].line, 3);
        assert_eq!(method_hits[0].container.as_deref(), Some("Store"));
    }

    #[test]
    fn local_declarations_extract_rust_impl_methods_and_swift_symbols() {
        let dir = tempfile::tempdir().unwrap();
        let rust_file = dir.path().join("decode.rs");
        std::fs::write(
            &rust_file,
            "pub struct BlockMeta;\n\nimpl BlockMeta {\n    pub fn decode(input: &[u8]) -> Self {\n        BlockMeta\n    }\n}\n",
        )
        .unwrap();
        let rust_decls = search::local_declarations_in_file(&rust_file, "decode");
        assert!(rust_decls.iter().any(|d| {
            d.name == "decode"
                && d.container
                    .as_deref()
                    .is_some_and(|c| c.contains("BlockMeta"))
                && qualifier_matches(dir.path(), d, &["BlockMeta"])
        }));

        let swift_file = dir.path().join("AppState.swift");
        let swift_source = "/*\nfunc ghostInComment() {}\n*/\nfinal class AppState {\n    let snippet = \"\"\"\n    A lone \" quote\n    }\n    func ghostInString() {}\n    \"\"\"\n    let raw = #\"raw \" quote\n    func ghostInRawString() {}\n    \"#\n    func reloadSession() {}\n}\n";
        std::fs::write(&swift_file, swift_source).unwrap();
        let swift_class_decls = search::local_declarations_in_file(&swift_file, "AppState");
        assert!(swift_class_decls.iter().any(|d| d.name == "AppState"));
        assert!(
            search::local_declarations_in_file(&swift_file, "ghostInComment").is_empty(),
            "declarations inside block comments must be ignored"
        );
        assert!(
            search::local_declarations_in_file(&swift_file, "ghostInString").is_empty(),
            "declarations inside multiline strings with interior quotes must be ignored"
        );
        assert!(
            search::local_declarations_in_file(&swift_file, "ghostInRawString").is_empty(),
            "declarations inside raw strings with interior quotes must be ignored"
        );
        let swift_method_decls = search::local_declarations_in_file(&swift_file, "reloadSession");
        assert!(
            swift_method_decls.iter().any(|d| {
                d.name == "reloadSession" && d.container.as_deref() == Some("AppState")
            }),
            "braces inside strings must not close the enclosing container: {swift_method_decls:?}"
        );
        let outline = crate::tools::outline::file::local_swift_outline_symbols(swift_source);
        let outline_names: Vec<&str> = outline
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.get("name").and_then(|n| n.as_str()))
            .collect();
        assert_eq!(outline_names, vec!["AppState", "reloadSession"]);
    }
}
