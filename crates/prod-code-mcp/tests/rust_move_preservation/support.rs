/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, process::Command};

pub fn snapshot(root: &Path) -> Value {
    fn visit(root: &Path, dir: &Path, out: &mut BTreeMap<String, Value>) {
        if !dir.exists() {
            return;
        }
        for entry in std::fs::read_dir(dir).unwrap() {
            let p = entry.unwrap().path();
            if p.file_name()
                .is_some_and(|n| n == "target" || n == ".git" || n == "probe.rs")
            {
                continue;
            }
            let key = p.strip_prefix(root).unwrap().to_string_lossy().to_string();
            let meta = std::fs::symlink_metadata(&p).unwrap();
            if meta.is_symlink() {
                #[cfg(unix)]
                let identity = {
                    use std::os::unix::fs::MetadataExt;
                    let target = if p.is_dir() {
                        p.join("mod.rs")
                    } else {
                        p.clone()
                    };
                    std::fs::metadata(target).ok().map(|m| (m.dev(), m.ino()))
                };
                #[cfg(not(unix))]
                let identity: Option<(u64, u64)> = None;
                out.insert(key, json!({"link": std::fs::read_link(&p).unwrap(), "target_identity": identity,
                    "target": std::fs::canonicalize(&p).ok().map(|p| p.strip_prefix(root).unwrap_or(&p).to_path_buf()),
                    "contents": std::fs::read_to_string(p.join("mod.rs")).ok()}));
            } else if meta.is_dir() {
                visit(root, &p, out);
            } else if p.extension().is_some_and(|e| e == "rs")
                || p.file_name().is_some_and(|n| n == "Cargo.toml")
            {
                out.insert(key, json!(std::fs::read_to_string(p).unwrap()));
            }
        }
    }
    let mut out = BTreeMap::new();
    out.insert(
        "Cargo.toml".to_string(),
        json!(std::fs::read_to_string(root.join("Cargo.toml")).unwrap()),
    );
    visit(root, root, &mut out);
    json!(out)
}

fn run(mut cmd: Command, root: &Path, commands: &mut Vec<Value>) -> bool {
    let argv: Vec<_> = std::iter::once(cmd.get_program())
        .chain(cmd.get_args())
        .map(|s| {
            s.to_string_lossy()
                .replace(&root.to_string_lossy().to_string(), "<fixture>")
        })
        .collect();
    let output = cmd.output().unwrap();
    commands.push(json!({"argv": argv, "exit": output.status.code(),
        "stdout": String::from_utf8_lossy(&output.stdout),
        "stderr": String::from_utf8_lossy(&output.stderr)}));
    output.status.success()
}

/// Compile every physical Cargo root separately, then a downstream consumer with assertions.
/// This is real compiler evidence, independent of scripted analyzer acceptance.
pub fn compiler(root: &Path, id: &str, after: bool) -> Value {
    let mut commands = Vec::new();
    let lib = root.join("libmove_audit.rlib");
    let mut cmd = Command::new("rustc");
    cmd.args([
        "--edition=2021",
        "--crate-name=move_audit",
        "--crate-type=lib",
        "--cfg",
        "feature=\"enabled\"",
    ])
    .arg(root.join("src/lib.rs"))
    .arg("-o")
    .arg(&lib);
    let mut passed = run(cmd, root, &mut commands);
    if root.join("src/main.rs").exists() {
        let mut cmd = Command::new("rustc");
        cmd.arg("--edition=2021")
            .arg(root.join("src/main.rs"))
            .arg("-o")
            .arg(root.join("binary"));
        passed &= run(cmd, root, &mut commands);
    }
    let probe = match id {
        "item-multiline-derive-reattaches" => format!(
            "fn main() {{ let _ = move_audit::{}::Moved.clone(); }}",
            if after { "dest" } else { "source" }
        ),
        "same-line-attribute-hides-public-module" => format!(
            "fn main() {{ assert_eq!(move_audit::{}api(), 7); }}",
            if after { "util::" } else { "" }
        ),
        "super-string-macro-nested-scope" => format!(
            "fn main() {{ assert_eq!(move_audit::{}::b::HELP, \"super::help\"); assert_eq!(move_audit::{}::b::inner::value(), 2); assert_eq!(move_audit::{}::b::TOKEN, \"super::help\"); }}",
            if after { "c" } else { "a" },
            if after { "c" } else { "a" },
            if after { "c" } else { "a" }
        ),
        "module-cfg-block-doc-reattach" => {
            "fn main() { let _ = move_audit::a::Stayed; }".to_string()
        }
        "module-shared-by-lib-and-bin" | "module-owned-only-by-binary" => {
            "fn main() {}".to_string()
        }
        _ => "fn main() {}".to_string(),
    };
    // A feature-off compile checks that the moved cfg still belongs to its declaration.
    if id == "module-cfg-block-doc-reattach" {
        let mut cmd = Command::new("rustc");
        cmd.args([
            "--edition=2021",
            "--crate-name=move_audit",
            "--crate-type=lib",
        ])
        .arg(root.join("src/lib.rs"))
        .arg("-o")
        .arg(&lib);
        passed &= run(cmd, root, &mut commands);
    }
    if passed {
        std::fs::write(root.join("probe.rs"), &probe).unwrap();
        let mut cmd = Command::new("rustc");
        cmd.arg("--edition=2021")
            .arg(root.join("probe.rs"))
            .arg("--extern")
            .arg(format!("move_audit={}", lib.display()))
            .arg("-o")
            .arg(root.join("probe"));
        let built = run(cmd, root, &mut commands);
        passed &= built;
        if built {
            passed &= run(Command::new(root.join("probe")), root, &mut commands);
        }
    }
    json!({"passed": passed, "commands": commands, "probe_source": probe})
}

pub fn save_evidence(records: &[Value]) {
    if let Ok(path) = std::env::var("MOVE_EVIDENCE") {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Temporary fixture paths are coordinates, not public machine identities.
        let mut text = serde_json::to_string_pretty(records).unwrap();
        let re = regex::Regex::new(
            r#"/tmp/\.tmp[A-Za-z0-9]+|/private/var/folders/[^" ]+/T/\.tmp[A-Za-z0-9]+"#,
        )
        .unwrap();
        text = re.replace_all(&text, "<fixture>").into_owned();
        std::fs::write(path, text).unwrap();
    }
}

pub fn save_named(id: &str, record: Value) {
    if let Ok(path) = std::env::var("MOVE_EVIDENCE") {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(path);
        let name = path.file_stem().unwrap().to_string_lossy();
        let path = path.with_file_name(format!("{name}-{id}.json"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text = serde_json::to_string_pretty(&record).unwrap();
        let re = regex::Regex::new(r"/tmp/\.tmp[A-Za-z0-9]+").unwrap();
        std::fs::write(path, re.replace_all(&text, "<fixture>").as_bytes()).unwrap();
    }
}

pub fn cargo(root: &Path) -> Value {
    let mut commands = Vec::new();
    let mut command = Command::new("cargo");
    command
        .args([
            "test",
            "--offline",
            "--all-targets",
            "--quiet",
            "--manifest-path",
        ])
        .arg(root.join("Cargo.toml"));
    let passed = run(command, root, &mut commands);
    json!({"passed": passed, "commands": commands, "sources": snapshot(root)})
}
