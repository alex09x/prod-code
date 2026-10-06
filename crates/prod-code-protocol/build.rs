/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

fn main() {
    println!("cargo:rerun-if-env-changed=PROD_CODE_GIT_COMMIT");
    let git_dir = std::process::Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout)
                    .ok()
                    .map(|s| s.trim().to_string())
            } else {
                None
            }
        });
    if let Some(ref dir) = git_dir {
        let p = std::path::Path::new(dir);
        println!("cargo:rerun-if-changed={}", p.join("HEAD").display());
    }

    let common_dir = std::process::Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout)
                    .ok()
                    .map(|s| s.trim().to_string())
            } else {
                None
            }
        });

    if let Some(ref common) = common_dir {
        let common_path = std::path::Path::new(common);
        println!(
            "cargo:rerun-if-changed={}",
            common_path.join("packed-refs").display()
        );
    }

    if let Some(ref_rel) = std::process::Command::new("git")
        .args(["symbolic-ref", "-q", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|ref_name| ref_name.trim().to_string())
        .filter(|ref_rel| !ref_rel.is_empty())
    {
        if let Some(ref common) = common_dir {
            let common_path = std::path::Path::new(common);
            println!(
                "cargo:rerun-if-changed={}",
                common_path.join(&ref_rel).display()
            );
        }
        if let Some(ref dir) = git_dir.filter(|dir| common_dir.as_deref() != Some(dir.as_str())) {
            let dir_path = std::path::Path::new(dir);
            println!(
                "cargo:rerun-if-changed={}",
                dir_path.join(&ref_rel).display()
            );
        }
    }
    let commit = std::env::var("PROD_CODE_GIT_COMMIT").ok().or_else(|| {
        let output = std::process::Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .output()
            .ok()?;
        if output.status.success() {
            let s = String::from_utf8(output.stdout).ok()?;
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
        None
    });
    if let Some(c) = commit {
        println!("cargo:rustc-env=PROD_CODE_GIT_COMMIT={c}");
    }
}
