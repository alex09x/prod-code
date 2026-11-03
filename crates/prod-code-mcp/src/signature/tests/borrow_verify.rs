/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! #1810: the analyzer's check has no borrow checker, so a borrow conversion is written only
//! through `code_change_signature` with `verify: "compile"`. The analyzer here is scripted and
//! reports nothing, as the native one does for a move out of a borrow; the compile gate's shadow
//! run is the real `cargo check`, run on the files the gate proposes over a copy of the fixture.

use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::borrow_flow::{
    ENGINE, MAIN, fixture, params, position_of, references_answer, run_program,
};
use crate::protocol::{McpContentItem, McpToolCallResult};
use crate::signature::{Modifiers, change, change_with};
use crate::tools::refactor_handlers::parameters::signature::handle_change_signature;

const SOURCES: [&str; 3] = ["Cargo.toml", "src/main.rs", "src/engine.rs"];

/// The reported shape: clean to the analyzer once `request` is `&Request`, E0507 to the compiler.
const MOVES_OUT: &str = "pub struct Request(pub String);\n\npub fn take(request: Request) -> String {\n    request.0\n}\n";
const TAKES: &str =
    "mod engine;\n\nfn main() {\n    let _ = engine::take(engine::Request(String::new()));\n}\n";

/// Runs `params.command` with the real Cargo in a copy of the fixture's `SOURCES` as they are on
/// disk now, with every proposed file of the shadow run in place.
fn real_compiler(root: &Path, params: &serde_json::Value) -> serde_json::Value {
    let mut files: BTreeMap<String, String> = SOURCES
        .iter()
        .map(|rel| {
            (
                rel.to_string(),
                std::fs::read_to_string(root.join(rel)).unwrap(),
            )
        })
        .collect();
    for file in params["hypotheses"][0]["files"].as_array().unwrap() {
        let rel = file["relative_path"].as_str().unwrap().to_string();
        match file["text"].as_str() {
            Some(text) => files.insert(rel, text.to_string()),
            None => files.remove(&rel),
        };
    }
    let entries: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, t)| (p.as_str(), t.as_str()))
        .collect();
    let shadow = Workspace::new(&entries);
    let command: Vec<&str> = params["command"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert_eq!(
        command[..2],
        ["cargo", "check"],
        "the gate asks for cargo check"
    );
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let out = std::process::Command::new(cargo)
        .args(&command[1..])
        .arg("--offline")
        .env("CARGO_TARGET_DIR", shadow.root().join("target"))
        .current_dir(shadow.root().join(params["subdir"].as_str().unwrap_or("")))
        .output()
        .expect("cargo runs");
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::json!({ "exit_code": out.status.code().unwrap_or(-1), "output": output })
}

/// A gateway that answers `references` for `refs`, an analyzer check that is clean or `error`,
/// and a shadow run with [`real_compiler`]; the counter is how often the compiler was asked.
async fn gateway(
    root: &Path,
    main: &Path,
    refs: Vec<(u32, Vec<(u32, u32)>)>,
    error: Option<serde_json::Value>,
) -> (ScriptedGateway, Arc<AtomicUsize>) {
    let (root, main) = (root.to_path_buf(), main.to_path_buf());
    let compiled = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&compiled);
    let remote = ScriptedGateway::start(move |method, params| match method {
        "textDocument/references" => references_answer(&main, &refs, params),
        "textDocument/diagnostic" => error.clone().unwrap_or_else(answers::no_diagnostics),
        "prod-code/shadow-run" => {
            counted.fetch_add(1, Ordering::SeqCst);
            real_compiler(&root, params)
        }
        _ => serde_json::Value::Null,
    })
    .await;
    (remote, compiled)
}

/// `code_change_signature` on the declaration at `at` of `file`, with `extra` arguments.
async fn tool(
    remote: &ScriptedGateway,
    root: &Path,
    file: &Path,
    at: (u32, u32),
    specs: &[&str],
    extra: serde_json::Value,
) -> anyhow::Result<McpToolCallResult> {
    let mut args = serde_json::json!({
        "path": file.to_string_lossy(),
        "line": at.0,
        "character": at.1,
        "params": specs,
    });
    for (key, value) in extra.as_object().unwrap() {
        args[key] = value.clone();
    }
    handle_change_signature(remote.addr(), root, &args).await
}

fn text(result: &McpToolCallResult) -> String {
    result
        .content
        .iter()
        .map(|McpContentItem::Text { text }| text.as_str())
        .collect()
}

fn unchanged(ws: &Workspace, main: &str, engine: &str) {
    assert_eq!(ws.read("src/main.rs"), main);
    assert_eq!(ws.read("src/engine.rs"), engine);
}

/// The reviewed case. The analyzer finds nothing wrong with `fn take(request: &Request) ->
/// String { request.0 }`; the compiler does. Neither the direct API nor the tool writes it without
/// the compiler, `force` or not, and with `verify: "compile"` the real compiler refuses it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_1810_an_analyzer_clean_move_out_of_a_borrow_is_written_only_by_the_compiler() {
    let (ws, main, engine) = fixture(TAKES, MOVES_OUT);
    let root = ws.root();
    run_program(&root);
    let at = position_of(MOVES_OUT, "take(request");
    let refs = vec![(at.0, vec![position_of(TAKES, "take(engine")])];
    let (remote, compiled) = gateway(&root, &main, refs, None).await;

    let preview = tool(
        &remote,
        &root,
        &engine,
        at,
        &["&request"],
        serde_json::json!({}),
    )
    .await
    .unwrap_or_else(|e| panic!("the preview is planned: {e:#}"));
    let shown = text(&preview);
    assert!(!preview.is_error, "the analyzer accepts it: {shown}");
    assert!(shown.contains("request: &Request"), "{shown}");
    assert_eq!(
        compiled.load(Ordering::SeqCst),
        0,
        "a preview asks no compiler"
    );
    unchanged(&ws, TAKES, MOVES_OUT);

    let asked = remote.calls();
    let wanted = params(&["&request"]);
    for force in [false, true] {
        let direct = change(
            remote.addr(),
            &root,
            &engine,
            at.0,
            at.1,
            &wanted,
            true,
            force,
        );
        let err = format!("{:#}", direct.await.expect_err("the direct API refuses"));
        assert!(err.contains("verify: \"compile\"") && err.contains("Nothing was written"));
        let none = Modifiers::default();
        let with = change_with(
            remote.addr(),
            &root,
            &engine,
            at.0,
            at.1,
            &wanted,
            &none,
            true,
            force,
        );
        let err = format!("{:#}", with.await.expect_err("change_with refuses"));
        assert!(err.contains("verify: \"compile\""), "{err}");
        let err = tool(&remote, &root, &engine, at, &["&request"], {
            serde_json::json!({ "apply": true, "force": force })
        })
        .await
        .expect_err("the tool refuses an unverified apply");
        assert!(format!("{err:#}").contains("no borrow checker"), "{err:#}");
    }
    assert_eq!(
        remote.calls(),
        asked,
        "refused before the analyzer is asked"
    );
    assert_eq!(compiled.load(Ordering::SeqCst), 0);
    unchanged(&ws, TAKES, MOVES_OUT);

    let verified = tool(&remote, &root, &engine, at, &["&request"], {
        serde_json::json!({ "apply": true, "verify": "compile", "force": true })
    })
    .await
    .unwrap_or_else(|e| panic!("the gate answers: {e:#}"));
    let shown = text(&verified);
    assert!(verified.is_error, "{shown}");
    assert!(shown.contains("the compiler rejects the result"), "{shown}");
    assert!(
        shown.contains("E0507"),
        "the real compiler's error: {shown}"
    );
    assert!(shown.contains("nothing was written"), "{shown}");
    assert!(shown.contains("`force` does not override"), "{shown}");
    assert!(!shown.contains("Pass `force: true`"), "{shown}");
    assert_eq!(compiled.load(Ordering::SeqCst), 1);
    unchanged(&ws, TAKES, MOVES_OUT);
}

/// A borrow the compiler accepts is written through `verify: "compile"`: every call passes
/// `&argument`, and the program still runs with each argument evaluated once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_1810_a_compile_accepted_borrow_is_written_through_verify_compile() {
    let (ws, main, engine) = fixture(MAIN, ENGINE);
    let root = ws.root();
    assert_eq!(run_program(&root), "1 2 4 5 2\n", "before the change");
    let run_at = position_of(ENGINE, "run(&self");
    let total_at = position_of(ENGINE, "total(request");
    let run_refs = vec![
        position_of(MAIN, "run(request"),
        position_of(MAIN, "run(&engine"),
        position_of(MAIN, "run(Request"),
    ];
    let total_refs = vec![position_of(MAIN, "total(if")];
    let refs = vec![(run_at.0, run_refs), (total_at.0, total_refs)];
    let (remote, compiled) = gateway(&root, &main, refs, None).await;
    let verify = serde_json::json!({ "apply": true, "verify": "compile" });
    for (at, specs) in [
        (run_at, ["&request", "cancellation"]),
        (total_at, ["&request", "extra"]),
    ] {
        let done = tool(&remote, &root, &engine, at, &specs, verify.clone())
            .await
            .unwrap_or_else(|e| panic!("the borrow is written: {e:#}"));
        let shown = text(&done);
        assert!(!done.is_error, "{shown}");
        assert!(
            shown.contains("the compiler accepts the result too"),
            "{shown}"
        );
    }
    assert_eq!(
        compiled.load(Ordering::SeqCst),
        2,
        "one compiler run per write"
    );

    let (main_now, engine_now) = (ws.read("src/main.rs"), ws.read("src/engine.rs"));
    for call in [
        "engine.run(&request, &cancellation)",
        "Engine::run(&engine, &make(2), &cancellation)",
        "engine.run(&Request { id: 3 }, &cancellation) + 1",
        "engine::total(&(if flag { make(4) } else { make(40) }), 1)",
    ] {
        assert!(main_now.contains(call), "{call}:\n{main_now}");
    }
    assert!(
        engine_now.contains("pub fn run(&self, request: &Request, cancellation: &Cancellation)")
    );
    assert!(engine_now.contains("pub fn total(request: &Request, extra: u32)"));
    assert_eq!(main_now.matches("make(2)").count(), 1, "not duplicated");
    assert_eq!(
        run_program(&root),
        "1 2 4 5 2\n",
        "each `make` still runs once"
    );
}

/// An analyzer rejection stops the gate before the compiler is asked, and `force` does not lift
/// it for a borrow: the program would compile, and still nothing is written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_1810_force_does_not_lift_an_analyzer_rejection_at_the_gate() {
    let (ws, main, engine) = fixture(MAIN, ENGINE);
    let root = ws.root();
    let total_at = position_of(ENGINE, "total(request");
    let refs = vec![(total_at.0, vec![position_of(MAIN, "total(if")])];
    let error = answers::error_at(17, 5, "E0308", "mismatched types");
    let (remote, compiled) = gateway(&root, &main, refs, Some(error)).await;
    let forced = serde_json::json!({ "apply": true, "verify": "compile", "force": true });
    let done = tool(
        &remote,
        &root,
        &engine,
        total_at,
        &["&request", "extra"],
        forced,
    )
    .await
    .unwrap_or_else(|e| panic!("the gate answers: {e:#}"));
    let shown = text(&done);
    assert!(done.is_error, "{shown}");
    assert!(shown.contains("the compiler was not asked"), "{shown}");
    assert_eq!(compiled.load(Ordering::SeqCst), 0);
    unchanged(&ws, MAIN, ENGINE);
}
