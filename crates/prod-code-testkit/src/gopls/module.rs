/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn which(binary: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(binary))
            .find(|p| p.is_file())
    })
}

/// Fails the calling test unless `go` and `gopls` are on `PATH`.
pub fn require_go_toolchain() {
    let missing: Vec<&str> = ["go", "gopls"]
        .into_iter()
        .filter(|b| which(b).is_none())
        .collect();
    assert!(
        missing.is_empty(),
        "missing prerequisite: {} not on PATH. This real-server test is required and runs on \
         provisioned build nodes; a missing toolchain is a verification gap, not a pass",
        missing.join(" and ")
    );
}

/// A Go module in a directory Go tools do not skip (a `.tmp…` name is hidden from `./...`),
/// committed so the pre-flight sync has a base.
pub struct GoModule {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl GoModule {
    pub fn new(files: &[(&str, &str)]) -> Self {
        let dir = tempfile::Builder::new()
            .prefix("gosig")
            .tempdir()
            .expect("tempdir");
        let root = std::fs::canonicalize(dir.path()).expect("canonical root");
        for (rel, text) in files {
            std::fs::write(root.join(rel), text).expect("write fixture");
        }
        for args in [
            &["init", "-q"][..],
            &["add", "-A"],
            &[
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "user.name=test",
                "commit",
                "-qm",
                "fixture",
            ],
        ] {
            let ok = Command::new("git")
                .args(args)
                .current_dir(&root)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .expect("git runs")
                .success();
            assert!(ok, "git {args:?}");
        }
        Self { _dir: dir, root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    pub fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.path(rel)).expect("read")
    }

    /// Every path under the module, `.git` aside, and its bytes: a write, a new file or a
    /// deleted one all show.
    pub fn snapshot(&self) -> BTreeMap<String, Vec<u8>> {
        fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
            for entry in std::fs::read_dir(dir).expect("read_dir") {
                let path = entry.expect("entry").path();
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                if rel == ".git" {
                    continue;
                }
                if path.is_dir() {
                    walk(root, &path, out);
                } else {
                    out.insert(rel, std::fs::read(&path).expect("read"));
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(&self.root, &self.root, &mut out);
        out
    }

    /// Runs `go` in the module; the combined output, and whether it succeeded.
    pub fn go(&self, args: &[&str]) -> (bool, String) {
        let out = Command::new("go")
            .args(args)
            .current_dir(&self.root)
            .env("GOTOOLCHAIN", "local")
            .env("GOFLAGS", "-mod=mod")
            .output()
            .expect("go runs");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.success(), text)
    }

    /// `go run .`, which must succeed; what the program printed.
    pub fn run(&self) -> String {
        let (ok, text) = self.go(&["run", "."]);
        assert!(ok, "go run fails: {text}");
        text
    }
}
