/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{guard_script, names_node_path};
use std::path::{Path, PathBuf};
use std::process::Command;

/// A home directory with a registry crate in it and a ripgrep that prints its arguments.
struct Node {
    _dir: tempfile::TempDir,
    home: PathBuf,
    bin: PathBuf,
}

impl Node {
    fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("NodeSource")
            .tempdir()
            .unwrap();
        // Resolved, as the guard resolves the path it is given.
        let base = std::fs::canonicalize(dir.path()).unwrap();
        let home = base.join("home");
        let bin = base.join("bin");
        std::fs::create_dir_all(home.join(".cargo/registry/src/demo-1.0.0/src")).unwrap();
        std::fs::write(
            home.join(".cargo/registry/src/demo-1.0.0/src/lib.rs"),
            "pub fn demo() {}\n",
        )
        .unwrap();
        std::fs::create_dir_all(home.join("private")).unwrap();
        std::fs::write(home.join("private/notes.txt"), "demo\n").unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        let rg = bin.join("rg");
        std::fs::write(&rg, "#!/bin/sh\nprintf 'rg:%s\\n' \"$@\"\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&rg, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self {
            _dir: dir,
            home,
            bin,
        }
    }

    /// The guard run over `path` with two ripgrep arguments: its exit code and what it printed.
    fn guard(&self, path: &Path) -> (Option<i32>, String) {
        let system_path = std::env::var("PATH").unwrap_or_default();
        let output = Command::new("sh")
            .arg("-c")
            .arg(guard_script())
            .arg("sh")
            .arg(path)
            .args(["-e", "demo; echo injected"])
            .env("HOME", &self.home)
            .env("PATH", format!("{}:{system_path}", self.bin.display()))
            // Go's own locations, set as a node sets them: outside the home's defaults.
            .env("GOROOT", self.home.join("sdk/go"))
            .env(
                "GOPATH",
                format!(
                    "{}:{}",
                    self.home.join("gopath-a").display(),
                    self.home.join("gopath-b").display()
                ),
            )
            .output()
            .unwrap();
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
        )
    }
}

#[cfg(unix)]
#[test]
fn the_guard_searches_a_registry_directory_and_nothing_outside_a_source_location() {
    let node = Node::new();
    let crate_dir = node.home.join(".cargo/registry/src/demo-1.0.0");

    // A dependency's directory is searched, at its resolved path, and the pattern stays one
    // argument whatever it contains.
    let (code, out) = node.guard(&crate_dir.join("src/../src"));
    assert_eq!(code, Some(0), "{out}");
    assert_eq!(
        out,
        format!(
            "rg:-e\nrg:demo; echo injected\nrg:{}\n",
            crate_dir.join("src").display()
        )
    );

    // A directory of the same host that is no source location is not.
    let (code, out) = node.guard(&node.home.join("private"));
    assert_eq!((code, out.as_str()), (Some(65), ""));

    // Neither is one reached through a link inside a source location.
    std::os::unix::fs::symlink(node.home.join("private"), crate_dir.join("link")).unwrap();
    let (code, out) = node.guard(&crate_dir.join("link"));
    assert_eq!((code, out.as_str()), (Some(65), ""));

    // And a path the node does not have says so.
    let (code, out) = node.guard(&crate_dir.join("src/missing"));
    assert_eq!((code, out.as_str()), (Some(64), ""));

    // A directory whose name only begins like a source location is none.
    let sibling = node.home.join(".cargo/registry-notes");
    std::fs::create_dir_all(&sibling).unwrap();
    let (code, out) = node.guard(&sibling);
    assert_eq!((code, out.as_str()), (Some(65), ""));
}

/// The gateway reads Go's standard library under `GOROOT` and the module cache and sources
/// under each entry of `GOPATH`; a search reaches the same, and nothing else under them.
#[cfg(unix)]
#[test]
fn the_guard_searches_the_nodes_goroot_and_gopath_sources() {
    let node = Node::new();
    let searched = [
        node.home.join("sdk/go/src/fmt"),
        node.home.join("gopath-a/src/example.com/demo"),
        node.home.join("gopath-b/pkg/mod/example.com/demo@v1.0.0"),
    ];
    for directory in &searched {
        std::fs::create_dir_all(directory).unwrap();
        let (code, out) = node.guard(directory);
        assert_eq!(code, Some(0), "{}: {out}", directory.display());
        assert!(
            out.ends_with(&format!("rg:{}\n", directory.display())),
            "{out}"
        );
    }
    // `bin` of a GOPATH entry holds no source.
    let binaries = node.home.join("gopath-b/bin");
    std::fs::create_dir_all(&binaries).unwrap();
    let (code, out) = node.guard(&binaries);
    assert_eq!((code, out.as_str()), (Some(65), ""));
}

#[test]
fn only_an_absolute_path_outside_the_workspace_is_looked_for_on_the_node() {
    let root = Path::new("/work/checkout");
    assert!(names_node_path(
        root,
        Path::new("/home/build/.cargo/registry/src/index/demo-1.0.0/src")
    ));
    // A missing path inside the workspace is a mistyped one, answered locally.
    assert!(!names_node_path(root, Path::new("/work/checkout/src/gone")));
    assert!(!names_node_path(root, Path::new("src/gone")));
}

/// A source directory that this machine has too (system headers, the same user's registry
/// cache) is still the node's to search; a path of the workspace is not, however it is
/// spelled or wherever a link in it leads.
#[cfg(unix)]
#[test]
fn a_directory_this_machine_also_has_is_still_searched_on_the_node() {
    let dir = tempfile::Builder::new()
        .prefix("NodeSource")
        .tempdir()
        .unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let root = base.join("real/checkout");
    let outside = base.join("sdk/include");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    assert!(names_node_path(&root, &outside));

    // Another spelling of a path inside the workspace is searched in the workspace.
    std::os::unix::fs::symlink(base.join("real"), base.join("alias")).unwrap();
    assert!(!names_node_path(&root, &base.join("alias/checkout/src")));
    // A link inside the workspace that leads out of it is refused there, not sent on.
    std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();
    assert!(!names_node_path(&root, &root.join("escape")));
}
