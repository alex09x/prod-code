/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::tests::{graph, write};
use super::*;

#[test]
fn resolves_supported_reference_spellings_and_excludes_self_and_unknown() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        root,
        "app/App.csproj",
        "<Project>\r\n<ProjectReference Include=\"Bare\" />\r\n<ProjectReference Include=\"Local.csproj\" />\r\n<ProjectReference Include=\"../Slash/Slash.csproj\" />\r\n<ProjectReference Include=\"..\\Back\\Back.csproj\" />\r\n<ProjectReference Include=\"App.csproj\" />\r\n<ProjectReference Include=\"Missing.csproj\" />\r\n</Project>",
    );
    for name in ["Bare", "Local", "Slash", "Back", "独立"] {
        write(root, &format!("{name}/{name}.csproj"), "<Project />");
    }
    write(root, "invalid/Unreadable.csproj", "placeholder");
    std::fs::write(root.join("invalid/Unreadable.csproj"), [0xff]).unwrap();
    let mut adj = BTreeMap::new();
    collect_dotnet_dependencies(root, &mut adj);
    assert_eq!(
        adj,
        graph(
            root,
            &[
                ("App", "app", &["Bare", "Local", "Slash", "Back"]),
                ("Bare", "Bare", &[]),
                ("Local", "Local", &[]),
                ("Slash", "Slash", &[]),
                ("Back", "Back", &[]),
                ("独立", "独立", &[]),
                ("Unreadable", "invalid", &[]),
            ]
        )
    );
}

#[test]
fn skips_generated_hidden_and_too_deep_projects() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, "Top.csproj", "<Project />");
    for dir in [
        "bin",
        "obj",
        "artifacts",
        ".git",
        ".hidden",
        "a/bin",
        "a/obj",
        "a/artifacts",
    ] {
        write(root, &format!("{dir}/Ignored.csproj"), "<Project />");
    }
    // Windows-style path components must also be excluded on a Unix checkout.
    for dir in ["a\\bin\\b", "a\\obj\\b", "a\\.git\\b", "a\\artifacts\\b"] {
        write(root, &format!("{dir}/Ignored.csproj"), "<Project />");
    }
    write(root, "a/b/c/d/e/f/g/Boundary.csproj", "<Project />");
    write(root, "a/b/c/d/e/f/g/h/TooDeep.csproj", "<Project />");
    write(root, "other/NotProject.txt", "<Project />");
    write(root, "other/Upper.CSPROJ", "<Project />");
    let mut adj = BTreeMap::new();
    collect_dotnet_dependencies(root, &mut adj);
    assert_eq!(
        adj,
        graph(
            root,
            &[("Top", "", &[]), ("Boundary", "a/b/c/d/e/f/g", &[])]
        )
    );
}

#[test]
fn markers_without_projects_and_existing_graph_are_preserved() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut adj = BTreeMap::new();
    collect_dotnet_dependencies(root, &mut adj);
    assert_eq!(adj, BTreeMap::new());
    for marker in [
        "global.json",
        "Directory.Build.props",
        "Directory.Build.targets",
    ] {
        write(root, marker, "");
        collect_dotnet_dependencies(root, &mut adj);
        assert_eq!(adj, BTreeMap::new());
        std::fs::remove_file(root.join(marker)).unwrap();
    }
    write(root, "Ignored.csproj", "<Project />");
    let expected = graph(root, &[("already-present", "present", &["dependency"])]);
    adj = expected.clone();
    collect_dotnet_dependencies(root, &mut adj);
    assert_eq!(adj, expected);
}
