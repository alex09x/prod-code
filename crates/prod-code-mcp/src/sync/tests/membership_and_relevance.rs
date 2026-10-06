/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::sync::engine::detect::engine_at;
use crate::sync::engine::workspace::*;
use crate::sync::relevance::*;
use crate::sync::*;
#[test]
fn test_lockfiles_are_relevant() {
    assert!(is_relevant_code_or_manifest_file("Cargo.lock"));
    assert!(is_relevant_code_or_manifest_file("CMakeLists.txt"));
    assert!(is_relevant_code_or_manifest_file(
        "build/compile_commands.json"
    ));
    assert!(is_relevant_code_or_manifest_file(".clangd"));
    assert!(is_relevant_code_or_manifest_file("requirements.txt"));
    assert!(is_relevant_code_or_manifest_file(
        "App.xcodeproj/project.pbxproj"
    ));
    assert!(!is_relevant_code_or_manifest_file("notes.txt"));
    assert!(is_relevant_code_or_manifest_file("web/yarn.lock"));
    assert!(is_relevant_code_or_manifest_file("go.sum"));
    assert!(is_relevant_code_or_manifest_file("scripts/rustc-wrapper"));
    assert!(is_relevant_code_or_manifest_file(
        "scripts/collect-report.sh"
    ));
}

mod workspace_membership_tests {
    use super::*;

    const ROOT: &str = r#"[workspace]
resolver = "2"
members = [
    "crates/prod-code-protocol",
    "crates/prod-code-mcp",
]
# a fixture is a test bed, not a member
exclude = ["fixtures"]

[workspace.package]
version = "0.2.2"
"#;

    #[test]
    fn the_lists_are_read_across_lines_and_one_liners() {
        let (members, excludes) = workspace_lists(ROOT);
        assert_eq!(
            members,
            ["crates/prod-code-protocol", "crates/prod-code-mcp"]
        );
        assert_eq!(excludes, ["fixtures"]);

        let (members, excludes) =
            workspace_lists("[workspace]\nmembers = [\"a\", \"b/*\"]\nexclude = [\"vendor\"]\n");
        assert_eq!(members, ["a", "b/*"]);
        assert_eq!(excludes, ["vendor"]);
    }

    #[test]
    fn a_member_belongs_to_the_workspace_and_an_excluded_crate_does_not() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("Cargo.toml"), ROOT).expect("root manifest");
        for rel in [
            "crates/prod-code-mcp",
            "fixtures/polyglot-order/core",
            "examples/demo",
        ] {
            std::fs::create_dir_all(root.join(rel)).expect("dirs");
            std::fs::write(
                root.join(rel).join("Cargo.toml"),
                "[package]\nname = \"x\"\n",
            )
            .expect("crate manifest");
        }

        assert!(
            !excluded_from_root_workspace(root, &root.join("crates/prod-code-mcp")),
            "a member belongs to the workspace and must stay with it"
        );
        assert!(
            excluded_from_root_workspace(root, &root.join("fixtures/polyglot-order/core")),
            "a crate under an excluded directory is its own project"
        );
        assert!(
            excluded_from_root_workspace(root, &root.join("examples/demo")),
            "and so is one the members list simply does not mention"
        );
        assert!(
            !excluded_from_root_workspace(root, &root.join("crates")),
            "a directory with no manifest of its own is not a crate at all"
        );
    }

    #[test]
    fn a_glob_member_covers_its_children_and_only_them() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\n",
        )
        .expect("root manifest");
        for rel in ["crates/one", "crates/one/nested", "other"] {
            std::fs::create_dir_all(root.join(rel)).expect("dirs");
            std::fs::write(
                root.join(rel).join("Cargo.toml"),
                "[package]\nname = \"x\"\n",
            )
            .expect("crate manifest");
        }
        assert!(!excluded_from_root_workspace(
            root,
            &root.join("crates/one")
        ));
        assert!(
            excluded_from_root_workspace(root, &root.join("crates/one/nested")),
            "`crates/*` is one level, not a subtree"
        );
        assert!(excluded_from_root_workspace(root, &root.join("other")));
    }

    #[test]
    fn a_plain_package_has_no_workspace_to_belong_to() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"solo\"\n").expect("manifest");
        std::fs::create_dir_all(root.join("sub")).expect("dirs");
        std::fs::write(
            root.join("sub").join("Cargo.toml"),
            "[package]\nname = \"sub\"\n",
        )
        .expect("manifest");
        assert!(
            excluded_from_root_workspace(root, &root.join("sub")),
            "a crate under a plain package answers for itself"
        );
    }

    /// A Makefile next to C sources is C/C++, below every other manifest, and one with no C
    /// sources is not; an XcodeGen `project.yml` is Swift, another `project.yml` is not (#404).
    #[test]
    fn a_make_c_project_and_an_xcodegen_spec_are_recognised() {
        let make = tempfile::tempdir().expect("tempdir");
        std::fs::write(make.path().join("Makefile"), "all:\n\tcc main.c\n").expect("makefile");
        assert_eq!(expected_engine(make.path()), None, "no C sources");
        std::fs::write(make.path().join("main.c"), "int main(void){}\n").expect("source");
        assert_eq!(expected_engine(make.path()), Some("cpp"));
        std::fs::write(make.path().join("pyproject.toml"), "[project]\n").expect("manifest");
        assert_eq!(
            expected_engine(make.path()),
            Some("python"),
            "a Makefile ranks below every manifest"
        );

        let xcodegen = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            xcodegen.path().join("project.yml"),
            "name: App\ntargets:\n  App:\n    type: application\n",
        )
        .expect("spec");
        assert_eq!(expected_engine(xcodegen.path()), Some("swift"));
        let other = tempfile::tempdir().expect("tempdir");
        std::fs::write(other.path().join("project.yml"), "name: docs\n").expect("yml");
        assert_eq!(engine_at(other.path()), None);
    }

    #[test]
    fn cargo_examples_and_tests_under_state_and_data_are_relevant() {
        assert!(is_relevant_code_or_manifest_file(
            "examples/state/computed_states.rs"
        ));
        assert!(is_relevant_code_or_manifest_file(
            "examples/data/loading.rs"
        ));
        assert!(is_relevant_code_or_manifest_file(
            "tests/state/test_state.rs"
        ));
        assert!(is_relevant_code_or_manifest_file(
            "benches/state/bench_state.rs"
        ));
    }
}
