/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::move_item::move_item;
use std::io::BufRead;
use std::net::SocketAddr;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

#[path = "common/live_gateway.rs"]
mod live_gateway;

struct Gateway {
    child: Child,
    reader: Option<std::thread::JoinHandle<()>>,
    addr: SocketAddr,
    _storage: tempfile::TempDir,
}

impl Gateway {
    fn start() -> Self {
        let storage = tempfile::tempdir().unwrap();
        let child = live_gateway::command(storage.path(), "move_views_match_disk")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut gateway = Self {
            child,
            reader: None,
            addr: "127.0.0.1:0".parse().unwrap(),
            _storage: storage,
        };
        let stdout = gateway.child.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        gateway.reader = Some(std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Some(address) = line.split("listening on ").nth(1)
                    && let Ok(address) = address.trim().parse::<SocketAddr>()
                {
                    let _ = tx.send(address);
                }
            }
        }));
        gateway.addr = rx.recv_timeout(Duration::from_secs(60)).unwrap();
        gateway
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            reader.join().unwrap();
        }
    }
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

const ITEM: &str = "pub struct DexOracleState {\n    pub value: u32,\n    // Snapshot metadata.\n    // 01\n    // 02\n    // 03\n    // 04\n    // 05\n    // 06\n    // 07\n    // 08\n    // 09\n    // 10\n    // 11\n    // 12\n    // 13\n    // 14\n    // 15\n    // 16\n    // 17\n    // 18\n}\n";
const USES: &str = "\npub struct Holder {\n    pub state: /* 😀 */ DexOracleState,\n}\n\npub fn make(value: u32) -> /* 😀 */ DexOracleState {\n    /* 😀 */ DexOracleState { value }\n}\n";

// Case table: main/overlay × first apply/dry run then apply × ASCII/UTF-16 × LF/CRLF.
// Every row preserves field, signature and body uses with exactly the expected import.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn move_views_match_disk() {
    if live_gateway::serve_if_child().await {
        return;
    }
    let gateway = Gateway::start();
    // Start with the reproduced workflow, then cover its nearest controls.
    for (worktree, preview, unicode, ending) in [
        (true, true, true, "\n"),
        (false, true, true, "\n"),
        (true, false, true, "\n"),
        (false, false, true, "\n"),
        (true, true, false, "\n"),
        (false, true, false, "\n"),
        (true, false, false, "\n"),
        (false, false, false, "\n"),
        (true, true, true, "\r\n"),
        (false, true, true, "\r\n"),
        (true, false, true, "\r\n"),
        (false, false, true, "\r\n"),
        (true, true, false, "\r\n"),
        (false, true, false, "\r\n"),
        (true, false, false, "\r\n"),
        (false, false, false, "\r\n"),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let base = temp
            .path()
            .join(format!("base-{worktree}-{preview}-{unicode}"));
        let uses = if unicode {
            USES.to_string()
        } else {
            USES.replace("/* 😀 */ ", "")
        };
        let source_text = format!("{ITEM}{uses}").replace('\n', ending);
        std::fs::create_dir_all(base.join("src")).unwrap();
        for (path, text) in [
            (
                "Cargo.toml",
                "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
            ),
            ("prod-code.toml", "[rust]\nproc_macro_srv = \"disabled\"\n"),
            ("src/lib.rs", "pub mod dex_oracle;\n"),
            ("src/dex_oracle.rs", source_text.as_str()),
        ] {
            std::fs::write(base.join(path), text).unwrap();
        }
        git(&base, &["init", "-q"]);
        git(&base, &["add", "."]);
        git(
            &base,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-qm",
                "fixture",
            ],
        );
        let root = if worktree {
            // Warm the main checkout before attaching its detached overlay. Its text is
            // deliberately different, so a reference from the wrong view cannot pass.
            let source = base.join("src/dex_oracle.rs");
            prod_code_mcp::signature::references(gateway.addr, &base, &source, 1, 12)
                .await
                .unwrap();
            let copy = temp.path().join("detached");
            git(
                &base,
                &[
                    "worktree",
                    "add",
                    "--detach",
                    copy.to_str().unwrap(),
                    "HEAD",
                ],
            );
            std::fs::write(
                copy.join("src/dex_oracle.rs"),
                format!("// detached view\n\n{source_text}"),
            )
            .unwrap();
            copy
        } else {
            base
        };
        let source = root.join("src/dex_oracle.rs");
        let target = root.join("src/dex_oracle_types.rs");
        let original = std::fs::read_to_string(&source).unwrap();
        let line = if worktree { 3 } else { 1 };
        let mut references =
            prod_code_mcp::signature::references(gateway.addr, &root, &source, line, 12)
                .await
                .unwrap();
        references.sort();
        let expected_refs = uses
            .lines()
            .enumerate()
            .filter_map(|(index, text)| {
                let at = text.find("DexOracleState")?;
                Some((
                    source.clone(),
                    line + 21 + index as u32 + 1,
                    text[..at].encode_utf16().count() as u32 + 1,
                ))
            })
            .collect::<Vec<_>>();
        assert_eq!(
            references, expected_refs,
            "worktree={worktree}, unicode={unicode}"
        );
        let dry = if preview {
            let dry = move_item(
                gateway.addr,
                &root,
                &source,
                line,
                12,
                &target,
                false,
                false,
            )
            .await
            .unwrap();
            assert!(!dry.applied);
            assert_eq!(std::fs::read_to_string(&source).unwrap(), original);
            assert!(!target.exists());
            let mut after =
                prod_code_mcp::signature::references(gateway.addr, &root, &source, line, 12)
                    .await
                    .unwrap();
            after.sort();
            assert_eq!(
                after, expected_refs,
                "preview must preserve analyzer positions"
            );
            Some(dry)
        } else {
            None
        };
        let applied = move_item(gateway.addr, &root, &source, line, 12, &target, true, false)
            .await
            .unwrap_or_else(|e| panic!("worktree={worktree}, preview={preview}: {e:#}"));
        assert!(applied.unmatched.is_empty());
        assert!(applied.applied);
        assert_eq!(applied.moved_lines, 22);
        assert!(applied.diagnostics.is_empty(), "{:?}", applied.diagnostics);
        if let Some(dry) = dry {
            assert!(dry.unmatched.is_empty(), "{:?}", dry.unmatched);
            assert_eq!(dry.rewritten, applied.rewritten);
        }
        for (path, text) in &applied.rewritten {
            assert_eq!(std::fs::read_to_string(path).unwrap(), *text);
        }
        let expected = original.replace("\r\n", "\n").replacen(ITEM, "", 1);
        let insert = expected.find("pub struct Holder").unwrap();
        let expected = format!(
            "{}use crate::dex_oracle_types::DexOracleState;\n{}",
            &expected[..insert],
            &expected[insert..]
        );
        assert_eq!(std::fs::read_to_string(&source).unwrap(), expected);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), ITEM);
        assert_eq!(
            std::fs::read_to_string(root.join("src/lib.rs")).unwrap(),
            "pub mod dex_oracle;\npub mod dex_oracle_types;\n"
        );
        if worktree {
            assert_eq!(
                std::fs::read_to_string(temp.path().join(format!(
                    "base-{worktree}-{preview}-{unicode}/src/dex_oracle.rs"
                )))
                .unwrap(),
                source_text
            );
        }
        eprintln!(
            "move case passed: worktree={worktree}, preview={preview}, unicode={unicode}, ending={ending:?}"
        );
    }
}
