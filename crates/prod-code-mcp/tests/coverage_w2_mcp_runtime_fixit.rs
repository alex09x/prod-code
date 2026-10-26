/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `check_and_fix` against a scripted node: the compiler's fixes for Rust, a linter's own fix
//! mode for the other languages, and what each says when there is nothing to do.

use futures_util::{SinkExt, StreamExt};
use prod_code_mcp::fixit::{Fixed, check_and_fix};
use prod_code_mcp::verify::VerifyKind;
use prod_code_protocol::{
    ExecChanges, ExecChunk, ExecExit, ExecRequest, FileDelta, ProdCodeCodec, SyncProbeResponse,
    SyncResponse, WireMessage,
};
use prod_code_testkit::Workspace;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio_util::codec::Framed;

/// An unused import, as cargo reports it: the suggestion is in a child.
const UNUSED: &str = r#"{"reason":"compiler-message","message":{"level":"warning","code":{"code":"unused_imports"},"message":"unused import: `std::fmt`","spans":[{"file_name":"src/lib.rs","byte_start":4,"byte_end":12,"line_start":1,"is_primary":true,"text":[{"text":"use std::fmt;"}],"suggested_replacement":null,"suggestion_applicability":null}],"children":[{"message":"remove the whole `use` item","spans":[{"file_name":"src/lib.rs","byte_start":0,"byte_end":14,"line_start":1,"is_primary":true,"text":[{"text":"use std::fmt;"}],"suggested_replacement":"","suggestion_applicability":"MachineApplicable"}],"children":[]}]}}"#;

const CARGO_TOML: &str = "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
const RUFF_FIX: [&str; 6] = ["ruff", "check", ".", "--fix", "--output-format", "concise"];

/// What the node prints, which files it hands back, and how it exits for one command.
#[derive(Clone, Default)]
struct Reply {
    stdout: &'static str,
    stderr: &'static str,
    exit: i32,
    changes: Vec<(&'static str, &'static str)>,
    /// A local edit made while the command runs, before its changes come back.
    local_edit: Option<(std::path::PathBuf, &'static str)>,
}

/// A node answering the commands it is sent from `script`, in order, over as many connections
/// as it takes; once the script is spent it hangs up on the next command. Returns its address
/// and the log of every command it was sent.
async fn node(script: Vec<Reply>) -> (SocketAddr, Arc<Mutex<Vec<ExecRequest>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        let script = Arc::new(script);
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                return;
            };
            let (script, log) = (script.clone(), log.clone());
            tokio::spawn(async move {
                let mut framed = Framed::new(socket, ProdCodeCodec::new());
                while let Some(Ok(message)) = framed.next().await {
                    match message {
                        WireMessage::SyncProbeRequest(probe) => {
                            let reply = SyncProbeResponse {
                                server_workspace_root: probe.client_workspace_root,
                                seeded: false,
                                files_deleted: 0,
                                missing: vec![],
                            };
                            let _ = framed.send(WireMessage::SyncProbeResponse(reply)).await;
                        }
                        WireMessage::SyncRequest(sync) => {
                            let reply = SyncResponse {
                                server_workspace_root: sync.client_workspace_root,
                                files_updated: 0,
                                files_deleted: 0,
                                bytes_transferred: 0,
                                duration_ms: 0,
                                workspace_was_fresh: false,
                                stale_paths: vec![],
                            };
                            let _ = framed.send(WireMessage::SyncResponse(reply)).await;
                        }
                        WireMessage::ExecRequest(request) => {
                            let index = {
                                let mut log = log.lock().unwrap();
                                log.push(request.clone());
                                log.len() - 1
                            };
                            let Some(reply) = script.get(index) else {
                                return;
                            };
                            answer(&mut framed, &request, reply).await;
                        }
                        _ => return,
                    }
                }
            });
        }
    });
    (addr, seen)
}

async fn answer(
    framed: &mut Framed<tokio::net::TcpStream, ProdCodeCodec>,
    req: &ExecRequest,
    r: &Reply,
) {
    for (stderr, text) in [(false, r.stdout), (true, r.stderr)] {
        if !text.is_empty() {
            let chunk = ExecChunk {
                stderr,
                data: Some(text.as_bytes().to_vec()),
            };
            let _ = framed.send(WireMessage::ExecChunk(chunk)).await;
        }
    }
    if let Some((path, text)) = &r.local_edit {
        std::fs::write(path, text).unwrap();
    }
    if !r.changes.is_empty() {
        let files = r
            .changes
            .iter()
            .map(|(path, text)| FileDelta {
                relative_path: (*path).into(),
                content: Some(text.as_bytes().to_vec()),
                is_executable: false,
            })
            .collect();
        let _ = framed
            .send(WireMessage::ExecChanges(ExecChanges { files }))
            .await;
    }
    let exit = ExecExit {
        exit_code: Some(r.exit),
        duration_ms: 3,
        server_workspace_root: req.client_workspace_root.clone(),
        timed_out: false,
        error: None,
        usage: None,
        platform: None,
    };
    let _ = framed.send(WireMessage::ExecExit(exit)).await;
}

async fn fix(
    addr: SocketAddr,
    ws: &Workspace,
    hint: Option<&Path>,
    kind: VerifyKind,
) -> anyhow::Result<Fixed> {
    check_and_fix(addr, &ws.root(), hint, kind, 30).await
}

fn rust_workspace() -> Workspace {
    Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        ("src/lib.rs", "use std::fmt;\nfn f() {}\n"),
    ])
}

fn command(request: &ExecRequest) -> String {
    request.command.join(" ")
}

#[tokio::test]
async fn a_machine_applicable_rust_fix_is_written_and_the_check_runs_again() {
    let ws = rust_workspace();
    let (addr, seen) = node(vec![
        Reply {
            stdout: UNUSED,
            ..Reply::default()
        },
        Reply::default(),
    ])
    .await;
    let fixed = fix(addr, &ws, None, VerifyKind::Check).await.unwrap();

    assert_eq!(ws.read("src/lib.rs"), "fn f() {}\n");
    assert_eq!(fixed.before.language, "rust");
    assert_eq!(fixed.before.fixes.len(), 1);
    assert_eq!(fixed.outcomes.len(), 1);
    let outcome = &fixed.outcomes[0];
    assert_eq!(outcome.file, "src/lib.rs");
    assert_eq!(outcome.line, 1);
    assert_eq!(
        outcome.message,
        "unused import: `std::fmt`: remove the whole `use` item"
    );
    assert_eq!(outcome.skipped, None);
    assert_eq!(fixed.note, None);
    let after = fixed
        .after
        .as_ref()
        .expect("the check runs again after a write");
    assert_eq!(after.exit_code, Some(0));
    assert!(fixed.ok());

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(command(&seen[0]), command(&seen[1]));
    assert!(
        command(&seen[0]).starts_with("cargo check"),
        "{}",
        command(&seen[0])
    );
}

#[tokio::test]
async fn nothing_is_written_or_rerun_when_the_compiler_offers_no_fix() {
    let ws = rust_workspace();
    let (addr, seen) = node(vec![Reply {
        exit: 101,
        ..Reply::default()
    }])
    .await;
    let fixed = fix(addr, &ws, None, VerifyKind::Lint).await.unwrap();

    assert!(fixed.outcomes.is_empty() && fixed.after.is_none() && fixed.note.is_none());
    assert_eq!(fixed.before.exit_code, Some(101));
    assert!(!fixed.ok());
    assert_eq!(ws.read("src/lib.rs"), "use std::fmt;\nfn f() {}\n");
    assert_eq!(seen.lock().unwrap().len(), 1);

    // The file changed since the compiler read it: the fix is skipped and nothing is rerun.
    let (addr, seen) = node(vec![Reply {
        stdout: UNUSED,
        ..Reply::default()
    }])
    .await;
    ws.write("src/lib.rs", "use std::io;\nfn f() {}\n");
    let fixed = fix(addr, &ws, None, VerifyKind::Check).await.unwrap();
    assert_eq!(fixed.outcomes.len(), 1);
    assert_eq!(
        fixed.outcomes[0].skipped.as_deref(),
        Some("the file changed since the compiler read it")
    );
    assert!(fixed.after.is_none());
    assert_eq!(seen.lock().unwrap().len(), 1);
    assert_eq!(ws.read("src/lib.rs"), "use std::io;\nfn f() {}\n");
}

#[tokio::test]
async fn only_lint_applies_fixes_for_a_language_with_a_linter_of_its_own() {
    let ws = Workspace::new(&[
        ("pyproject.toml", "[tool.ruff]\n"),
        ("app.py", "import os\n"),
    ]);
    let (addr, seen) = node(vec![Reply {
        exit: 1,
        ..Reply::default()
    }])
    .await;
    let fixed = fix(addr, &ws, None, VerifyKind::Check).await.unwrap();
    assert_eq!(fixed.before.language, "python");
    assert_eq!(
        fixed.note.as_deref(),
        Some("fixes: only `lint --fix` applies fixes for python")
    );
    assert!(fixed.outcomes.is_empty() && fixed.after.is_none());
    assert_eq!(seen.lock().unwrap().len(), 1);
    assert_eq!(ws.read("app.py"), "import os\n");
}

#[tokio::test]
async fn a_linters_fix_mode_runs_on_the_node_and_its_rewrites_come_back() {
    let ws = Workspace::new(&[
        ("pyproject.toml", "[tool.ruff]\n"),
        ("app.py", "import os\n"),
    ]);
    let (addr, seen) = node(vec![
        Reply {
            stdout: "app.py:1:8: F401 `os` imported but unused\n",
            exit: 1,
            ..Reply::default()
        },
        Reply {
            changes: vec![("app.py", "\n")],
            ..Reply::default()
        },
        Reply::default(),
    ])
    .await;
    let fixed = fix(addr, &ws, None, VerifyKind::Lint).await.unwrap();

    let ruff = RUFF_FIX.join(" ");
    assert_eq!(fixed.before.language, "python");
    assert_eq!(
        fixed.note,
        Some(format!("fixes: `{ruff}` rewrote 1 file(s)"))
    );
    assert_eq!(fixed.outcomes.len(), 1);
    assert_eq!(fixed.outcomes[0].file, "app.py");
    assert_eq!(fixed.outcomes[0].line, 0);
    assert_eq!(fixed.outcomes[0].message, format!("rewritten by `{ruff}`"));
    assert_eq!(fixed.outcomes[0].skipped, None);
    assert_eq!(fixed.after.as_ref().map(|a| a.exit_code), Some(Some(0)));
    assert_eq!(ws.read("app.py"), "\n");

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 3);
    assert_eq!(seen[1].command, RUFF_FIX);
    assert_eq!(seen[1].env, [("NO_COLOR".to_string(), "1".to_string())]);
    assert!(seen[1].pull_changes);
    assert_eq!(seen[1].subdir, None);
}

/// A local edit made while the fixer runs wins the writeback: the report names the kept file as
/// skipped, with why, and never as fixed (#1414).
#[tokio::test]
async fn a_file_kept_for_a_local_edit_is_reported_and_not_fixed() {
    let ws = Workspace::new(&[
        ("pyproject.toml", "[tool.ruff]\n"),
        ("app.py", "import os\n"),
        ("lib.py", "import sys\n"),
    ]);
    let (addr, _) = node(vec![
        Reply {
            stdout: "app.py:1:8: F401 `os` imported but unused\n",
            exit: 1,
            ..Reply::default()
        },
        Reply {
            changes: vec![("app.py", "\n"), ("lib.py", "\n")],
            local_edit: Some((ws.root().join("app.py"), "import os  # mine\n")),
            ..Reply::default()
        },
        Reply::default(),
    ])
    .await;
    let fixed = fix(addr, &ws, None, VerifyKind::Lint).await.unwrap();

    let ruff = RUFF_FIX.join(" ");
    assert_eq!(
        fixed.note,
        Some(format!(
            "fixes: `{ruff}` rewrote 1 file(s); 1 file(s) kept with their local state, the fix \
             not written: app.py"
        ))
    );
    assert_eq!(
        ws.read("app.py"),
        "import os  # mine\n",
        "the local edit stays"
    );
    assert_eq!(ws.read("lib.py"), "\n");
    let kept = fixed.outcomes.iter().find(|o| o.file == "app.py").unwrap();
    assert!(kept.skipped.is_some(), "{kept:?}");
    let report = fixed.render(50);
    assert!(report.contains("  fixed lib.py\n"), "{report}");
    assert!(report.contains("  skipped app.py: fixed by"), "{report}");
    assert!(!report.contains("fixed app.py"), "{report}");
}

#[tokio::test]
async fn a_fix_mode_that_rewrites_nothing_is_not_checked_again() {
    let ws = Workspace::new(&[("pyproject.toml", "[tool.ruff]\n"), ("app.py", "x = 1\n")]);
    let (addr, seen) = node(vec![Reply::default(), Reply::default()]).await;
    let fixed = fix(addr, &ws, None, VerifyKind::Lint).await.unwrap();
    assert_eq!(
        fixed.note,
        Some(format!("fixes: `{}` rewrote 0 file(s)", RUFF_FIX.join(" ")))
    );
    assert!(fixed.outcomes.is_empty() && fixed.after.is_none());
    assert_eq!(seen.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn a_linter_without_a_fix_mode_changes_nothing() {
    let ws = Workspace::new(&[
        ("go.mod", "module demo\n\ngo 1.22\n"),
        ("main.go", "package main\n"),
    ]);
    let (addr, seen) = node(vec![Reply {
        exit: 1,
        ..Reply::default()
    }])
    .await;
    let fixed = fix(addr, &ws, None, VerifyKind::Lint).await.unwrap();
    assert_eq!(fixed.before.language, "go");
    assert_eq!(
        fixed.note.as_deref(),
        Some("fixes: the go linter has no fix mode; nothing was changed")
    );
    assert!(fixed.outcomes.is_empty() && fixed.after.is_none());
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn the_fix_runs_in_the_project_the_hint_is_in() {
    // A Python service inside a Rust checkout: its linter runs in its own directory.
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        ("src/lib.rs", "fn f() {}\n"),
        ("svc/pyproject.toml", "[tool.ruff]\n"),
        ("svc/app.py", "x = 1\n"),
    ]);
    let (addr, seen) = node(vec![Reply::default(), Reply::default()]).await;
    let hint = ws.path("svc/app.py");
    let fixed = fix(addr, &ws, Some(&hint), VerifyKind::Lint).await.unwrap();
    assert_eq!(fixed.before.language, "python");
    let seen = seen.lock().unwrap();
    assert_eq!(seen[1].command, RUFF_FIX);
    assert_eq!(seen[1].subdir.as_deref(), Some("svc"));
}

#[tokio::test]
async fn a_node_that_hangs_up_fails_the_fix_without_writing() {
    let ws = rust_workspace();
    let (addr, _) = node(vec![]).await;
    let err = fix(addr, &ws, None, VerifyKind::Check).await.unwrap_err();
    assert!(
        format!("{err:#}").contains("gateway closed the connection during exec"),
        "{err:#}"
    );

    let ws = Workspace::new(&[("pyproject.toml", "[tool.ruff]\n"), ("app.py", "x = 1\n")]);
    let (addr, _) = node(vec![Reply::default()]).await;
    let err = fix(addr, &ws, None, VerifyKind::Lint).await.unwrap_err();
    assert!(
        format!("{err:#}").contains("gateway closed the connection during exec"),
        "{err:#}"
    );
}
