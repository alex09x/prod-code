/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::across::{
    ImportMatch, imports_declaration, is_occurrence_shadowed, mask_executable_code,
    masked_code_string,
};
use std::path::Path;

#[test]
fn imports_declaration_resolves_relative_paths_and_filters_strings_urls_and_shadowing() {
    let caller = Path::new("/ws/src/pages/index.astro");
    let decl_ts = Path::new("/ws/src/data/caseStudies.ts");
    let decl_idx = Path::new("/ws/src/data/caseStudies/index.ts");
    let astro_text = "---\nimport {\n  type CaseStudy,\n  caseStudies,\n} from '../data/caseStudies';\nconst label = \"caseStudies\";\nconst r = total! / caseStudies.length / 2 + π / caseStudies.length / 2;\nfunction render(caseStudies: string) {\n  return caseStudies;\n}\n---\n<a href=\"https://example.org\">{/[}]/.test(caseStudies[0]) ? `Value: ${caseStudies.length / 2}` : /caseStudies/.source}</a>\n";
    assert_eq!(
        imports_declaration(astro_text, caller, decl_ts, "caseStudies"),
        ImportMatch::Proven
    );
    assert_eq!(
        imports_declaration(astro_text, caller, decl_idx, "CaseStudy"),
        ImportMatch::Proven
    );
    let page_b = Path::new("/ws/src/b/Page.astro");
    let rel_imp = "import { foo } from './data.js';\n";
    assert_eq!(
        imports_declaration(rel_imp, page_b, Path::new("/ws/src/a/data.js"), "foo"),
        ImportMatch::None
    );
    assert_eq!(
        imports_declaration(rel_imp, page_b, Path::new("/ws/src/b/data.js"), "foo"),
        ImportMatch::Proven
    );
    assert_eq!(
        imports_declaration(rel_imp, page_b, Path::new("/ws/src/b/data/index.js"), "foo"),
        ImportMatch::None
    );

    let mask = mask_executable_code(astro_text, caller);
    let masked = masked_code_string(astro_text, &mask);
    let lines: Vec<&str> = astro_text.lines().collect();
    let mut uses = Vec::new();
    let mut line_start = 0usize;
    for (idx, line) in astro_text.split('\n').enumerate() {
        for (byte_in_line, _) in line.match_indices("caseStudies") {
            let off = line_start + byte_in_line;
            if mask[off..off + "caseStudies".len()].iter().all(|&b| b)
                && !is_occurrence_shadowed(&masked, &lines, idx, off, "caseStudies")
            {
                uses.push(idx + 1);
            }
        }
        line_start += line.len() + 1;
    }
    assert_eq!(uses, vec![4, 7, 7, 12, 12]);

    let vue_text = "<template><button title=\"foo\" :prop=\"foo\" @click=\"foo()\">{{ foo }}</button></template>";
    let vue_mask = mask_executable_code(vue_text, Path::new("/ws/App.vue"));
    let vue_foos: Vec<bool> = vue_text
        .match_indices("foo")
        .map(|(off, _)| vue_mask[off])
        .collect();
    assert_eq!(vue_foos, vec![false, true, true, true]);

    let py_text = "# foo in comment\nmsg = f\"literal foo {foo()}\"\n";
    let py_mask = mask_executable_code(py_text, Path::new("/ws/app.py"));
    let py_foos: Vec<bool> = py_text
        .match_indices("foo")
        .map(|(off, _)| py_mask[off])
        .collect();
    assert_eq!(py_foos, vec![false, false, true]);
}

#[test]
fn nested_git_worktree_is_excluded_from_source_files_and_worktree_check() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::write(root.join(".git"), "gitdir: /tmp/main\n").unwrap();
    let src_dir = root.join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    let main_file = src_dir.join("app.ts");
    std::fs::write(&main_file, "export const x = 1;\n").unwrap();

    let nested_wt = root.join(".prod/worktrees/wt-123");
    std::fs::create_dir_all(nested_wt.join("src")).unwrap();
    std::fs::write(nested_wt.join(".git"), "gitdir: /tmp/wt-123\n").unwrap();
    let wt_file = nested_wt.join("src/app.ts");
    std::fs::write(&wt_file, "export const x = 1;\n").unwrap();

    assert!(!crate::tools::symbols::is_in_nested_git_worktree(
        root, &main_file
    ));
    assert!(crate::tools::symbols::is_in_nested_git_worktree(
        root, &wt_file
    ));

    let files: Vec<_> = crate::tools::symbols::source_files(root).collect();
    assert!(files.contains(&main_file));
    assert!(!files.contains(&wt_file));
}

#[test]
fn resolve_file_path_preserves_external_toolchain_and_maps_remote_mirror() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("my-repo");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn f() {}\n").unwrap();

    let external = "/opt/rustup/toolchains/stable/lib/rustlib/src/rust/library/core/src/error.rs";
    assert_eq!(
        crate::tools::compile_gate::resolve_file_path(&root, external),
        std::path::PathBuf::from(external)
    );

    let mirror = "/srv/prod-code/workspaces/my-repo/src/lib.rs";
    assert_eq!(
        crate::tools::compile_gate::resolve_file_path(&root, mirror),
        root.join("src/lib.rs")
    );
}

#[test]
fn nested_swift_project_and_swift_declarations_work_at_deep_paths() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"root\"\n").unwrap();
    let swift_pkg = root.join("apps/macos/Sources/AppPkg");
    let deep_dir = swift_pkg.join("Features/Terminal/Views/Components/Rows");
    std::fs::create_dir_all(&deep_dir).unwrap();
    std::fs::write(
        swift_pkg.join("Package.swift"),
        "// swift-tools-version: 6.0\n",
    )
    .unwrap();
    let swift_file = deep_dir.join("GhosttyTerminalNSView.swift");
    std::fs::write(
        &swift_file,
        "import AppKit\nfinal class GhosttyTerminalNSView: NSView {}\nextension GhosttyTerminalNSView {}\n",
    )
    .unwrap();

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let nested = crate::tools::symbols::projects_naming(root, "GhosttyTerminalNSView", deadline);
    assert!(nested.iter().any(|(rep, subpath, engine)| {
        rep == &swift_file && subpath == "apps/macos/Sources/AppPkg" && *engine == "swift"
    }));
    assert!(
        crate::tools::symbols::declared_at(
            "final class GhosttyTerminalNSView: NSView {",
            "GhosttyTerminalNSView"
        )
        .is_some()
    );
    assert!(
        crate::tools::symbols::declared_at(
            "extension GhosttyTerminalNSView {",
            "GhosttyTerminalNSView"
        )
        .is_some()
    );
}
