/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! #442, run for real: a signature change whose call sites type-check can still change what the
//! program prints. Each scenario drives the planner (`signature::change`) against a scripted
//! gateway whose hovers have the shapes rust-analyzer answers with, and then compiles and runs
//! the program before and after the rewrite the planner would make, so a refusal is shown to
//! guard a real difference and an accepted change is shown to keep the output.
//!
//! - `fields(a.n, b.n)`: `Wrap` has no field `n`, so each read goes through `Wrap::deref`, which
//!   logs. Two field reads look like two plain places, and they are not.
//! - `refs(&a, &b)`: a `&Wrap` passed for a `&Inner` is converted by the same `deref`.
//! - `shadowed(s, t)`: `Option` is the program's own enum, with a `Drop` that logs; the name alone
//!   does not say it is the standard library's.
//! - `scalars(p /* … /* … */ … */, q)`: two `u32` locals, with a nested comment in between. The
//!   analyzer confirms `u32` is the built-in type, so the reorder is written, and the program
//!   prints the same.

use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

const PROGRAM: &str = r#"use std::ops::Deref;
use std::sync::Mutex;

static LOG: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

fn log(s: &'static str) {
    LOG.lock().unwrap().push(s);
}

pub struct Inner {
    pub n: u32,
}

pub struct Wrap(pub &'static str, pub Inner);

impl Deref for Wrap {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        log(self.0);
        &self.1
    }
}

#[allow(dead_code)]
pub enum Option<T> {
    None,
    Some(T),
}

impl<T> Drop for Option<T> {
    fn drop(&mut self) {
        log(match self {
            Option::Some(_) => "some",
            Option::None => "none",
        });
    }
}

pub fn fields(first: u32, second: u32) -> u32 {
    first * 10 + second
}

pub fn refs(first: &Inner, second: &Inner) -> u32 {
    first.n * 10 + second.n
}

pub fn shadowed(first: Option<u32>, second: Option<u32>) -> u32 {
    let _ = (&first, &second);
    7
}

pub fn scalars(first: u32, second: u32) -> u32 {
    first * 10 + second
}

fn main() {
    let (a, b) = (Wrap("a", Inner { n: 1 }), Wrap("b", Inner { n: 2 }));
    let (p, q): (u32, u32) = (3, 4);
    let (s, t) = (Option::Some(5), Option::None);
    let v = [
        fields(a.n, b.n),
        refs(&a, &b),
        shadowed(s, t),
        scalars(p /* p, /* then */ q, */, q),
    ];
    println!("{v:?} {:?}", LOG.lock().unwrap());
}
"#;

/// The hover rust-analyzer gives for the type name at a position of [`PROGRAM`]: the built-in
/// scalars as themselves, the program's own `Option` and `Inner` with the crate they are in.
fn hover(params: &serde_json::Value) -> serde_json::Value {
    let at = |key: &str| {
        params
            .pointer(&format!("/position/{key}"))
            .and_then(|v| v.as_u64())
            .unwrap_or(u64::MAX) as usize
    };
    let Some(line) = PROGRAM.lines().nth(at("line")) else {
        return serde_json::Value::Null;
    };
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    let from = at("character").min(line.len());
    let start = line[..from]
        .char_indices()
        .rev()
        .take_while(|(_, c)| ident(*c))
        .last()
        .map_or(from, |(i, _)| i);
    let word: String = line[start..].chars().take_while(|c| ident(*c)).collect();
    let markdown = match word.as_str() {
        "u32" => "```rust\nu32\n```\n\n---\n\nThe 32-bit unsigned integer type.".to_string(),
        "Option" => "```rust\nfixture\n```\n\n```rust\npub enum Option<T> {\n    None,\n    Some( /* … */ ),\n}\n```".to_string(),
        "Inner" => "```rust\nfixture\n```\n\n```rust\npub struct Inner {\n    pub n: u32,\n}\n```".to_string(),
        _ => return serde_json::Value::Null,
    };
    answers::hover(&markdown)
}

/// A gateway for one call of `PROGRAM`: `call` is where the analyzer says the call is, `rewritten`
/// is what the structural rewrite answers with, and `hovers` whether types are described at all.
async fn gateway(
    main: PathBuf,
    call: (u32, u32),
    rewritten: String,
    hovers: bool,
) -> (ScriptedGateway, SocketAddr) {
    let gateway = ScriptedGateway::start(move |method, params| match method {
        "textDocument/references" => answers::locations(&main, &[call]),
        "prodCode/structuralReplace" => answers::whole_file(&main, PROGRAM, &rewritten),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        "textDocument/hover" if hovers => hover(params),
        _ => serde_json::Value::Null,
    })
    .await;
    let addr = gateway.addr();
    (gateway, addr)
}

/// The 1-based line and column where `needle` first occurs in [`PROGRAM`].
fn position_of(needle: &str) -> (u32, u32) {
    let at = PROGRAM.find(needle).expect(needle);
    let line = PROGRAM[..at].matches('\n').count() as u32 + 1;
    let col = (at - PROGRAM[..at].rfind('\n').map_or(0, |i| i + 1)) as u32 + 1;
    (line, col)
}

fn workspace() -> (Workspace, PathBuf) {
    let ws = Workspace::empty();
    let main = ws.write("src/main.rs", PROGRAM);
    ws.commit();
    (ws, main)
}

fn keep(names: &[&str]) -> Vec<prod_code_mcp::signature::Param> {
    names
        .iter()
        .map(|n| prod_code_mcp::signature::parse_param(n).unwrap())
        .collect()
}

/// What `program` prints, compiled with the toolchain the tests run on. A compiler that cannot be
/// started fails the test: a runtime claim that was not run is not evidence.
fn run(program: &str) -> String {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("main.rs");
    let bin = dir.path().join("fixture");
    std::fs::write(&src, program).unwrap();
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let built = Command::new(&rustc)
        .args(["--edition", "2021", "-A", "warnings", "-o"])
        .arg(&bin)
        .arg(&src)
        .output()
        .unwrap_or_else(|e| panic!("cannot start {rustc:?}: {e}"));
    assert!(
        built.status.success(),
        "the fixture does not compile:\n{}\n{program}",
        String::from_utf8_lossy(&built.stderr)
    );
    let ran = Command::new(&bin).output().unwrap();
    assert!(ran.status.success(), "the fixture failed: {ran:?}");
    String::from_utf8(ran.stdout).unwrap()
}

/// What [`PROGRAM`] prints as written, compiled once for all the scenarios.
fn original_output() -> &'static str {
    static OUT: OnceLock<String> = OnceLock::new();
    OUT.get_or_init(|| run(PROGRAM))
}

/// [`PROGRAM`] with `pairs` replaced, each exactly once.
fn edited(pairs: &[(&str, &str)]) -> String {
    let mut text = PROGRAM.to_string();
    for (from, to) in pairs {
        assert_eq!(text.matches(from).count(), 1, "{from}");
        text = text.replace(from, to);
    }
    text
}

/// Swaps the two parameters of `decl` and the two arguments of `call`, the way the planner writes
/// a reorder, asks the planner, and returns its error; also checks that nothing was written and
/// that the swap really changes what the program prints.
async fn refused_swap(name: &str, decl: (&str, &str), call: (&str, &str)) -> String {
    let (ws, main) = workspace();
    let variant = edited(&[decl, call]);
    let calls_only = edited(&[call]);
    let (_g, remote) = gateway(main.clone(), position_of(call.0), calls_only, true).await;
    let (l, c) = position_of(decl.0);
    let err = prod_code_mcp::signature::change(
        remote,
        &ws.root(),
        &main,
        l,
        c,
        &keep(&["second", "first"]),
        true,
        true,
    )
    .await
    .expect_err(&format!(
        "`{name}`'s reorder changes what the program prints"
    ));
    assert_eq!(ws.read("src/main.rs"), PROGRAM, "nothing was written");
    let (before, after) = (original_output(), run(&variant));
    assert_ne!(
        before, after,
        "the fixture must show the difference the refusal guards"
    );
    println!("{name}: refused; prints {before:?} as written and {after:?} reordered");
    format!("{err:#}\n[runtime] before: {before}[runtime] after:  {after}")
}

#[tokio::test]
async fn a_field_read_through_a_user_deref_is_not_a_plain_place() {
    let text = refused_swap(
        "fields",
        (
            "fields(first: u32, second: u32)",
            "fields(second: u32, first: u32)",
        ),
        ("fields(a.n, b.n)", "fields(b.n, a.n)"),
    )
    .await;
    assert!(
        text.contains("`a.n` and `b.n` would be evaluated in the opposite order")
            && text.contains("Deref"),
        "{text}"
    );
}

#[tokio::test]
async fn a_reference_argument_can_be_converted_by_a_user_deref() {
    let text = refused_swap(
        "refs",
        (
            "refs(first: &Inner, second: &Inner)",
            "refs(second: &Inner, first: &Inner)",
        ),
        ("refs(&a, &b)", "refs(&b, &a)"),
    )
    .await;
    assert!(
        text.contains("`&a` and `&b` would be evaluated in the opposite order")
            && text.contains("Deref"),
        "{text}"
    );
}

#[tokio::test]
async fn an_option_that_is_not_the_standard_one_may_drop() {
    let text = refused_swap(
        "shadowed",
        (
            "shadowed(first: Option<u32>, second: Option<u32>)",
            "shadowed(second: Option<u32>, first: Option<u32>)",
        ),
        ("shadowed(s, t)", "shadowed(t, s)"),
    )
    .await;
    assert!(
        text.contains(
            "`shadowed` drops `second: Option<u32>` before `first: Option<u32>` when it returns"
        ) && text.contains("`Option`"),
        "{text}"
    );
}

/// Two `u32` locals the analyzer confirms are the built-in type swap freely, with a nested block
/// comment in the first argument read as one comment; without the analyzer's word on `u32` the
/// same change is refused rather than assumed.
#[tokio::test]
async fn confirmed_scalars_are_reordered_past_a_nested_comment() {
    let decl = (
        "scalars(first: u32, second: u32)",
        "scalars(second: u32, first: u32)",
    );
    let call = (
        "scalars(p /* p, /* then */ q, */, q)",
        "scalars(q, p /* p, /* then */ q, */)",
    );
    let (l, c) = position_of(decl.0);

    let (ws, main) = workspace();
    let (_g, remote) = gateway(main.clone(), position_of(call.0), edited(&[call]), false).await;
    let err = prod_code_mcp::signature::change(
        remote,
        &ws.root(),
        &main,
        l,
        c,
        &keep(&["second", "first"]),
        true,
        true,
    )
    .await
    .expect_err("an unconfirmed `u32` is not assumed to be the built-in one");
    assert!(format!("{err:#}").contains("would be evaluated in the opposite order"));
    assert_eq!(ws.read("src/main.rs"), PROGRAM, "nothing was written");

    let (ws, main) = workspace();
    let (_g, remote) = gateway(main.clone(), position_of(call.0), edited(&[call]), true).await;
    let change = prod_code_mcp::signature::change(
        remote,
        &ws.root(),
        &main,
        l,
        c,
        &keep(&["second", "first"]),
        true,
        false,
    )
    .await
    .expect("two confirmed scalars are reordered");
    assert!(change.applied);
    let now = ws.read("src/main.rs");
    assert_eq!(now, edited(&[decl, call]));
    let after = run(&now);
    assert_eq!(after, original_output(), "the output is the same");
    println!("scalars: written; prints {after:?} before and after");
}

/// A references answer at a position no file has, or naming something that is not a local file,
/// is an error rather than an overflow or a path that is quietly not there.
#[tokio::test]
async fn a_reference_outside_any_file_is_an_error() {
    let (l, c) = position_of("scalars(first");
    let entry = |uri: &str, line: u64, character: u64| {
        serde_json::json!([{
            "uri": uri,
            "range": { "start": { "line": line, "character": character },
                       "end": { "line": line, "character": character } }
        }])
    };
    let (ws, main) = workspace();
    let file = answers::uri(&main);
    let cases = [
        (
            entry(&file, u32::MAX as u64, 0),
            "reference 1 of 1 from the analyzer is at line",
        ),
        (
            entry(&file, 0, u32::MAX as u64),
            "reference 1 of 1 from the analyzer is at line",
        ),
        (
            entry("untitled:Untitled-1", 0, 0),
            "is not a local file URI",
        ),
        (
            entry("file://build-host/src/main.rs", 0, 0),
            "is not a local file URI",
        ),
        (entry("src/main.rs", 0, 0), "is not a local file URI"),
    ];
    for (reply, why) in cases {
        let answer = reply.clone();
        let gateway = ScriptedGateway::start(move |method, _| match method {
            "textDocument/references" => answer.clone(),
            _ => serde_json::Value::Null,
        })
        .await;
        let err = prod_code_mcp::signature::change(
            gateway.addr(),
            &ws.root(),
            &main,
            l,
            c,
            &keep(&["second", "first"]),
            true,
            true,
        )
        .await
        .expect_err("no such reference");
        let text = format!("{err:#}");
        assert!(text.contains(why), "{reply}: {text}");
        assert_eq!(ws.read("src/main.rs"), PROGRAM, "nothing was written");
    }
}
