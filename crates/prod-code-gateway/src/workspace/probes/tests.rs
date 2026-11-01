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

/// #1500: a SwiftPM target with a custom `path:` and no `Sources/` still has a probe file,
/// from the directory its manifest names; a path outside the package is not followed.
#[test]
fn a_target_with_a_custom_path_has_a_probe_file() {
    let manifest = "// swift-tools-version:5.9\nimport PackageDescription\nlet package = Package(\n    name: \"Shop\",\n    targets: [\n        .target(name: \"Escape\", path: \"../elsewhere\"),\n        .executableTarget(name: \"Shop\", path: \"App/Code\"),\n    ]\n)\n";
    let dir = package(&[
        ("Package.swift", manifest),
        ("App/Code/main.swift", "print(1)\n"),
        ("App/Code/.hidden/x.swift", "hidden"),
    ]);
    let (path, text) = swift_probe_file(dir.path()).expect("the custom target's source");
    assert_eq!(path, dir.path().join("App/Code/main.swift"));
    assert_eq!(text, "print(1)\n");
    assert_eq!(
        declared_target_paths(manifest).collect::<Vec<_>>(),
        ["App/Code"],
        "`../elsewhere` is outside the package"
    );

    // #1766: undeclared directories under `Sources/` (e.g. `Sources/RenderDemo/main.swift`) and
    // paths listed in `exclude:` are skipped in favor of declared SwiftPM targets.
    let tako_manifest = "// swift-tools-version: 5.9\nimport PackageDescription\nlet package = Package(\n    name: \"Tako\",\n    targets: [\n        .binaryTarget(name: \"TakoCoreFFI\", path: \"../TakoCore.xcframework\"),\n        .executableTarget(name: \"TakoApp\", dependencies: [\"TakoCoreFFI\"], path: \"Sources/Tako\", exclude: [\"Shaders/Excluded.swift\"]),\n        .testTarget(name: \"TakoTests\", dependencies: [\"TakoApp\"], path: \"Tests\")\n    ]\n)\n";
    let tako_dir = package(&[
        ("Package.swift", tako_manifest),
        ("Sources/RenderDemo/main.swift", "import Foundation\n"),
        ("Sources/Tako/Shaders/Excluded.swift", "excluded\n"),
        (
            "Sources/Tako/App/AppDelegate.swift",
            "class AppDelegate {}\n",
        ),
    ]);
    let (probe_path, probe_text) =
        swift_probe_file(tako_dir.path()).expect("declared target probe file");
    assert_eq!(
        probe_path,
        tako_dir.path().join("Sources/Tako/App/AppDelegate.swift")
    );
    assert_eq!(probe_text, "class AppDelegate {}\n");
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

/// An executable that acts as `cmake` when called with `-S` (running `body` in the build
/// directory's parent) and does nothing otherwise. It is run once before it is returned: a file
/// just written cannot be executed while a process forked by a parallel test still holds the
/// descriptor it was written through.
#[cfg(unix)]
fn fake_cmake(dir: &Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("fake-cmake");
    std::fs::write(
        &path,
        format!("#!/bin/sh\n[ \"$1\" = \"-S\" ] || exit 0\n{body}\n"),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    for _ in 0..500 {
        match std::process::Command::new(&path).status() {
            Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) => {
                std::thread::sleep(Duration::from_millis(10))
            }
            result => {
                assert!(result.unwrap().success());
                break;
            }
        }
    }
    path
}

#[cfg(unix)]
#[tokio::test]
async fn cmake_outcomes_are_reported_and_only_a_success_leaves_a_database() {
    let tools = tempfile::tempdir().unwrap();
    let ok = fake_cmake(
        tools.path(),
        "mkdir -p build\nprintf '%s\\n' \"$*\" > build/args.txt\necho '[]' > build/compile_commands.json",
    );
    let sleeper_dir = tempfile::tempdir().unwrap();
    let sleeper = fake_cmake(sleeper_dir.path(), "exec sleep 3");
    let failing_dir = tempfile::tempdir().unwrap();
    let failing = fake_cmake(
        failing_dir.path(),
        "echo first >&2\necho the-last-line >&2\nexit 3",
    );
    let missing = tools.path().join("no-such-cmake");

    // (program, limit in milliseconds, how the configure ends, whether a database exists after)
    let cases: [(&Path, u64, CmakeRun, bool); 4] = [
        (&ok, 20_000, CmakeRun::Configured, true),
        (
            &failing,
            20_000,
            CmakeRun::Failed("the-last-line".into()),
            false,
        ),
        (
            &missing,
            20_000,
            CmakeRun::NotRunnable("No such file or directory (os error 2)".into()),
            false,
        ),
        (&sleeper, 100, CmakeRun::TimedOut, false),
    ];
    for (program, limit_ms, expected, database) in cases {
        let project = package(&[("CMakeLists.txt", "project(sample)\n")]);
        let started = std::time::Instant::now();
        let run = configure_cmake(project.path(), program, Duration::from_millis(limit_ms)).await;
        assert_eq!(run, expected);
        assert_eq!(
            project.path().join("build/compile_commands.json").is_file(),
            database,
            "{expected:?}"
        );
        if run == CmakeRun::TimedOut {
            assert!(started.elapsed() < Duration::from_millis(2500));
        }
        if database {
            assert_eq!(
                std::fs::read_to_string(project.path().join("build/args.txt")).unwrap(),
                "-S . -B build -DCMAKE_EXPORT_COMPILE_COMMANDS=ON\n"
            );
            // A second call finds the database and leaves it alone.
            assert_eq!(
                configure_cmake(project.path(), program, Duration::from_secs(20)).await,
                CmakeRun::Skipped
            );
        }
    }
}
