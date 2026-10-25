/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

fn package(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (rel, body) in files {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
    dir
}

#[test]
fn the_probe_goes_on_its_own_line_and_its_line_number_is_reported() {
    for (text, expected_text, line) in [
        ("", "", 0),
        ("a\n", "a\n", 1),
        ("a", "a\n", 1),
        ("a\nb", "a\nb\n", 2),
        ("a\r\nb\r\n", "a\r\nb\r\n", 2),
        ("\n\n", "\n\n", 2),
    ] {
        let (probe, got_line) = with_probe_line(text);
        assert_eq!(probe, format!("{expected_text}{SWIFT_PROBE}\n"), "{text:?}");
        assert_eq!(got_line, line, "{text:?}");
        // The reported line is the one holding the probe.
        assert_eq!(
            probe.lines().nth(line as usize),
            Some(SWIFT_PROBE),
            "{text:?}"
        );
    }
}

#[test]
fn the_probe_file_is_the_first_swift_source_by_path_and_hidden_dirs_are_skipped() {
    let dir = package(&[
        ("Package.swift", "// pkg"),
        ("Sources/Zed/z.swift", "z"),
        ("Sources/App/b.swift", "b"),
        ("Sources/App/a.swift", "a"),
        ("Sources/App/readme.md", "not swift"),
        ("Sources/.hidden/0.swift", "hidden"),
        (".build/checkouts/dep/Sources/d.swift", "dep"),
    ]);
    let (path, text) = swift_probe_file(dir.path()).unwrap();
    assert_eq!(path, dir.path().join("Sources/App/a.swift"));
    assert_eq!(text, "a");

    for (name, files) in [
        ("no manifest", vec![("Sources/App/a.swift", "a")]),
        ("no sources dir", vec![("Package.swift", "p")]),
        (
            "no swift files",
            vec![("Package.swift", "p"), ("Sources/App/a.c", "c")],
        ),
        (
            "only hidden",
            vec![("Package.swift", "p"), ("Sources/.x/a.swift", "a")],
        ),
    ] {
        let dir = package(&files);
        assert_eq!(swift_probe_file(dir.path()), None, "{name}");
    }
    // A source that is not text is passed over for the next one.
    let dir = package(&[("Package.swift", "p"), ("Sources/b.swift", "b")]);
    std::fs::write(dir.path().join("Sources/a.swift"), [0xff, 0xfe]).unwrap();
    let (path, _) = swift_probe_file(dir.path()).unwrap();
    assert_eq!(path, dir.path().join("Sources/b.swift"));
}

#[tokio::test]
async fn cmake_is_not_run_without_a_project_or_when_a_database_exists() {
    let plain = package(&[("main.c", "int main(void) { return 0; }")]);
    warm_cmake_compile_commands(plain.path()).await;
    assert!(!plain.path().join("build").exists());

    let configured = package(&[
        ("CMakeLists.txt", "this is not valid cmake"),
        ("build/compile_commands.json", "[]"),
    ]);
    warm_cmake_compile_commands(configured.path()).await;
    assert_eq!(
        std::fs::read_to_string(configured.path().join("build/compile_commands.json")).unwrap(),
        "[]"
    );
    assert_eq!(
        std::fs::read_dir(configured.path().join("build"))
            .unwrap()
            .count(),
        1
    );
}

#[tokio::test]
async fn a_failing_cmake_configure_is_survived_and_leaves_the_sources_alone() {
    let broken = package(&[("CMakeLists.txt", "this is not valid cmake\n")]);
    warm_cmake_compile_commands(broken.path()).await;
    assert_eq!(
        std::fs::read_to_string(broken.path().join("CMakeLists.txt")).unwrap(),
        "this is not valid cmake\n"
    );
    assert!(!broken.path().join("build/compile_commands.json").exists());
}
