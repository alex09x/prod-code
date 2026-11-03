/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! #1810 refusals against a gateway that answers from a script: every refusal leaves the files
//! as they were. The fixtures and helpers here are shared with `borrow_verify`, where the write
//! goes through the compile gate and the real compiler.

use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::path::{Path, PathBuf};

use crate::signature::{Modifiers, Param, change, change_with, parse_param};

pub(super) const ENGINE: &str = "pub struct Request {\n    pub id: u32,\n}\n\npub struct Cancellation;\n\npub struct Engine;\n\nimpl Engine {\n    pub fn run(&self, request: Request, cancellation: &Cancellation) -> u32 {\n        let _ = cancellation;\n        request.id\n    }\n}\n\npub fn total(request: Request, extra: u32) -> u32 {\n    request.id + extra\n}\n";

pub(super) const MAIN: &str = "mod engine;\n\nuse engine::{Cancellation, Engine, Request};\nuse std::sync::atomic::{AtomicUsize, Ordering};\n\nstatic MADE: AtomicUsize = AtomicUsize::new(0);\n\nfn make(id: u32) -> Request {\n    MADE.fetch_add(1, Ordering::SeqCst);\n    Request { id }\n}\n\nfn main() {\n    let (engine, cancellation, flag) = (Engine, Cancellation, true);\n    let request = Request { id: 1 };\n    let a = engine.run(request, &cancellation);\n    let b = Engine::run(&engine, make(2), &cancellation);\n    let c = engine.run(Request { id: 3 }, &cancellation) + 1;\n    let d = engine::total(if flag { make(4) } else { make(40) }, 1);\n    println!(\"{a} {b} {c} {d} {}\", MADE.load(Ordering::SeqCst));\n}\n";

/// The 1-based line and column where `needle` first occurs in `text`.
pub(super) fn position_of(text: &str, needle: &str) -> (u32, u32) {
    let at = text.find(needle).expect(needle);
    let line = text[..at].matches('\n').count() as u32 + 1;
    let col = (at - text[..at].rfind('\n').map_or(0, |i| i + 1)) as u32 + 1;
    (line, col)
}

pub(super) fn fixture(main: &str, engine: &str) -> (Workspace, PathBuf, PathBuf) {
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"issue1810\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
        ),
        ("src/main.rs", main),
        ("src/engine.rs", engine),
    ]);
    let (main, engine) = (ws.path("src/main.rs"), ws.path("src/engine.rs"));
    (ws, main, engine)
}

/// The analyzer's `references` answer at the declaration `line` (1-based) of each entry: its
/// spots in `main`.
pub(super) fn references_answer(
    main: &Path,
    refs: &[(u32, Vec<(u32, u32)>)],
    params: &serde_json::Value,
) -> serde_json::Value {
    let line = params.pointer("/position/line").and_then(|l| l.as_u64());
    refs.iter()
        .find(|(decl, _)| Some(u64::from(*decl - 1)) == line)
        .map_or(serde_json::json!([]), |(_, spots)| {
            answers::locations(main, spots)
        })
}

/// A gateway that answers `references` like [`references_answer`] and a clean check of every
/// overlay.
async fn gateway(main: &Path, refs: Vec<(u32, Vec<(u32, u32)>)>) -> ScriptedGateway {
    let main = main.to_path_buf();
    ScriptedGateway::start(move |method, params| match method {
        "textDocument/references" => references_answer(&main, &refs, params),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await
}

/// Builds and runs the fixture with the real compiler; its standard output.
pub(super) fn run_program(root: &Path) -> String {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let out = std::process::Command::new(cargo)
        .args(["run", "--quiet", "--offline"])
        .env("CARGO_TARGET_DIR", root.join("target"))
        .current_dir(root)
        .output()
        .expect("cargo runs");
    assert!(
        out.status.success(),
        "the program builds and runs:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8")
}

pub(super) fn params(specs: &[&str]) -> Vec<Param> {
    specs.iter().map(|s| parse_param(s).unwrap()).collect()
}

/// A use the rewrite cannot borrow at stops the change, even as a preview: the function as a
/// value, and an import under another name. `force` does not change that.
#[tokio::test]
async fn issue_1810_value_uses_and_aliases_refuse_before_any_write() {
    let engine = "pub struct Request;\n\npub fn take(request: Request) -> u8 {\n    let _ = request;\n    1\n}\n";
    for (main, needle, why) in [
        (
            "mod engine;\n\nfn main() {\n    let f: fn(engine::Request) -> u8 = engine::take;\n    f(engine::Request);\n    engine::take(engine::Request);\n}\n",
            "take;",
            "used as a value",
        ),
        (
            "mod engine;\n\nuse engine::take as go;\n\nfn main() {\n    engine::take(engine::Request);\n    go(engine::Request);\n}\n",
            "take as",
            "imported under another name",
        ),
    ] {
        let (ws, main_path, engine_path) = fixture(main, engine);
        let at = position_of(engine, "take(request");
        let spots = vec![position_of(main, "take(engine"), position_of(main, needle)];
        let remote = gateway(&main_path, vec![(at.0, spots)]).await;
        let wanted = params(&["&request"]);
        let root = ws.root();
        let err = change(
            remote.addr(),
            &root,
            &engine_path,
            at.0,
            at.1,
            &wanted,
            false,
            true,
        )
        .await
        .expect_err(why);
        let text = format!("{err:#}");
        assert!(
            text.contains(why) && text.contains("Nothing was written"),
            "{text}"
        );
        assert_eq!(ws.read("src/main.rs"), main, "{why}: unchanged");
        assert_eq!(ws.read("src/engine.rs"), engine, "{why}: unchanged");
    }
}

/// Other languages, an unsupported binding and a borrow combined with other changes are refused
/// before the analyzer is asked anything, and leave the files as they were.
#[tokio::test]
async fn issue_1810_unsupported_requests_refuse_before_asking_the_analyzer() {
    let ws = Workspace::new(&[
        (
            "main.go",
            "package main\n\nfunc take(request string) int { return len(request) }\n",
        ),
        (
            "app.ts",
            "export function take(request: string): number { return request.length; }\n",
        ),
        (
            "src/lib.rs",
            "pub fn take(mut request: String) -> usize {\n    request.push('x');\n    request.len()\n}\n",
        ),
    ]);
    let remote = ScriptedGateway::start(|_, _| serde_json::Value::Null).await;
    let root = ws.root();
    let none = Modifiers::default();
    let async_too = Modifiers {
        asyncness: Some(true),
        ..Modifiers::default()
    };
    for (rel, at, modifiers, why) in [
        ("main.go", (3, 6), &none, "Rust only"),
        ("app.ts", (1, 17), &none, "Rust only"),
        ("src/lib.rs", (1, 8), &none, "mutable binding"),
        ("src/lib.rs", (1, 8), &async_too, "separate requests"),
    ] {
        let before = ws.read(rel);
        let wanted = params(&["&request"]);
        let err = change_with(
            remote.addr(),
            &root,
            &ws.path(rel),
            at.0,
            at.1,
            &wanted,
            modifiers,
            true,
            true,
        )
        .await
        .expect_err(why);
        let text = format!("{err:#}");
        assert!(
            text.contains(why) && text.contains("othing was written"),
            "{rel}: {text}"
        );
        assert_eq!(ws.read(rel), before, "{rel}: unchanged");
    }
    assert_eq!(remote.calls(), 0, "no request reached the analyzer");
}

/// The attempt reported in #1810 — adding `request: &Request = &request` in place of the owned
/// `request` — is still an addition and a removal, refused because the body uses the old one.
#[tokio::test]
async fn issue_1810_the_old_add_and_remove_attempt_still_refuses_a_used_parameter() {
    let (ws, _, engine_path) = fixture(MAIN, ENGINE);
    let run_at = position_of(ENGINE, "run(&self");
    // The analyzer's answer for the parameter: its use in the body.
    let body_use = position_of(ENGINE, "request.id\n    }");
    let declaring = engine_path.clone();
    let remote = ScriptedGateway::start(move |method, _| match method {
        "textDocument/references" => answers::locations(&declaring, &[body_use]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await;
    let wanted = params(&["request: &Request = &request", "cancellation"]);
    let err = change(
        remote.addr(),
        &ws.root(),
        &engine_path,
        run_at.0,
        run_at.1,
        &wanted,
        true,
        false,
    )
    .await
    .expect_err("the old attempt is refused");
    let text = format!("{err:#}");
    assert!(text.contains("still used by the body"), "{text}");
    assert_eq!(ws.read("src/main.rs"), MAIN);
    assert_eq!(ws.read("src/engine.rs"), ENGINE);
}
