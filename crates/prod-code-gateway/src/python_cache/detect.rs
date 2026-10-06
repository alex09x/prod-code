/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::fs;
use std::path::{Path, PathBuf};

/// Checks whether `root` represents or contains a Python project.
pub fn is_python_project(root: &Path) -> bool {
    const PYTHON_MARKERS: &[&str] = &[
        "pyproject.toml",
        "setup.py",
        "setup.cfg",
        "requirements.txt",
        "Pipfile",
        "pyrightconfig.json",
        "mypy.ini",
        ".python-version",
    ];

    for marker in PYTHON_MARKERS {
        if root.join(marker).is_file() {
            return true;
        }
    }

    if root.join(".venv").join("pyvenv.cfg").is_file()
        || root.join("venv").join("pyvenv.cfg").is_file()
        || root.join("pyvenv.cfg").is_file()
    {
        return true;
    }

    // Check for any top-level or immediate subfolder python files
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("py") {
                return true;
            }
        }
    }

    false
}

/// Discovers PEP 561 type stub directories (`*-stubs`) in Python virtual environments.
pub fn find_venv_stubs(venv_root: &Path) -> Vec<PathBuf> {
    let mut stubs = Vec::new();
    let lib_dir = venv_root.join("lib");
    let Ok(entries) = fs::read_dir(&lib_dir) else {
        return stubs;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with("python") && path.is_dir() {
            let sp = path.join("site-packages");
            if let Ok(sp_entries) = fs::read_dir(&sp) {
                for sp_entry in sp_entries.flatten() {
                    let sp_path = sp_entry.path();
                    let sp_name = sp_entry.file_name();
                    let sp_name_str = sp_name.to_string_lossy();
                    if sp_name_str.ends_with("-stubs") && sp_path.is_dir() {
                        stubs.push(sp_path);
                    }
                }
            }
        }
    }
    stubs
}
