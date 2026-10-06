/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::PathBuf;

/// A checkout for the client to sync: files, and a commit so the pre-flight sync has a base.
pub struct Workspace {
    dir: tempfile::TempDir,
}

impl Workspace {
    /// An empty repository: write the files, then [`Workspace::commit`] them.
    pub fn empty() -> Self {
        let workspace = Self {
            dir: tempfile::tempdir().expect("tempdir"),
        };
        workspace.commit();
        workspace
    }

    /// Writes the files, makes it a repository and commits them.
    pub fn new(files: &[(&str, &str)]) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = Self { dir };
        for (rel, text) in files {
            workspace.write(rel, text);
        }
        workspace.commit();
        workspace
    }

    /// The canonical root. Canonical because the client canonicalises it too, and on macOS a
    /// temporary directory is reached through a symlink.
    pub fn root(&self) -> PathBuf {
        std::fs::canonicalize(self.dir.path()).unwrap_or_else(|_| self.dir.path().to_path_buf())
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.root().join(rel)
    }

    pub fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.path(rel)).expect("read")
    }

    /// Writes one file, creating its directories. Not committed: call [`Workspace::commit`].
    pub fn write(&self, rel: &str, text: &str) -> PathBuf {
        let path = self.dir.path().join(rel);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(&path, text).expect("write");
        path
    }

    /// `git init` on first use, then add and commit everything.
    pub fn commit(&self) {
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(self.dir.path())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .expect("git runs")
        };
        if !self.dir.path().join(".git").is_dir() {
            assert!(git(&["init", "-q"]).success(), "git init");
        }
        assert!(git(&["add", "-A"]).success(), "git add");
        git(&[
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "user.name=test",
            "commit",
            "-qm",
            "fixture",
        ]);
    }
}
