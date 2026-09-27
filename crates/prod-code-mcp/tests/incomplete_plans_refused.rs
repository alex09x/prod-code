//! Signature changes and parameter objects write only complete plans (#446), directly and
//! through `code_change_signature` / `code_introduce_parameter_object`, with `force` and with
//! `verify: "compile"`.
//!
//! A reorder of two parameters of the same type compiles with a function pointer to it in the
//! program, and the pointer's callers then pass their arguments in the old order: the program
//! prints something else, and neither the analyzer nor the compiler can tell. Each scenario
//! compiles and runs the program before and after the edit the planner would make, so a refusal
//! is shown to guard a real difference. The base revision wrote it: the reference was reported
//! and written past, and on the line of a rewritten call it was not even reported.

use prod_code_mcp::protocol::{McpContentItem, McpToolCallResult};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::path::Path;
use std::process::Command;

const CARGO: &str = "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

/// `scale` called, and passed as a function pointer on a line of its own.
const APART: &str = r#"pub fn scale(x: u32, factor: u32) -> u32 {
    x * 10 + factor
}

fn apply(f: fn(u32, u32) -> u32) -> u32 {
    f(1, 2)
}

fn main() {
    let direct = scale(3, 4);
    let through = apply(scale);
    println!("{direct} {through}");
}
"#;

/// The same, with the pointer on the line of the call the rewrite changes.
const TOGETHER: &str = r#"pub fn scale(x: u32, factor: u32) -> u32 {
    x * 10 + factor
}

fn apply(f: fn(u32, u32) -> u32) -> u32 {
    f(1, 2)
}

fn main() {
    let (direct, through) = (scale(3, 4), apply(scale));
    println!("{direct} {through}");
}
"#;

/// Only calls, an import and a comment: every reference is accounted for.
const CALLS_ONLY: &str = r#"mod calc {
    pub fn scale(x: u32, factor: u32) -> u32 {
        x * 10 + factor
    }
}

use calc::scale;

fn main() {
    // see scale
    println!("{}", scale(3, 4));
}
"#;

const U32: &str = "```rust\nu32\n```\n\n---\n\nThe 32-bit unsigned integer type.";

/// The 1-based line and column of the `nth` occurrence of `needle` in `text`, plus `skip` bytes.
fn spot(text: &str, needle: &str, nth: usize, skip: usize) -> (u32, u32) {
    let at = text.match_indices(needle).nth(nth).expect(needle).0 + skip;
    let line = text[..at].matches('\n').count() as u32 + 1;
    let col = (at - text[..at].rfind('\n').map_or(0, |i| i + 1)) as u32 + 1;
    (line, col)
}

/// `text` with `from` replaced by `to`, exactly once.
fn edited(text: &str, pairs: &[(&str, &str)]) -> String {
    let mut out = text.to_string();
    for (from, to) in pairs {
        assert_eq!(out.matches(from).count(), 1, "{from}");
        out = out.replace(from, to);
    }
    out
}

/// What `program` prints, compiled with the toolchain the tests run on. A compiler that cannot
/// be started fails the test: a runtime claim that was not run is not evidence.
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
        "the program does not compile:\n{}\n{program}",
        String::from_utf8_lossy(&built.stderr)
    );
    let ran = Command::new(&bin).output().unwrap();
    assert!(ran.status.success(), "the program failed: {ran:?}");
    String::from_utf8(ran.stdout).unwrap()
}

fn workspace(main: &str) -> Workspace {
    let ws = Workspace::new(&[("Cargo.toml", CARGO), ("src/main.rs", main)]);
    ws.commit();
    ws
}

/// A gateway for `scale` in `program`: its references at `refs`, the call rewrite answering
/// with `rewritten`, `u32` confirmed as the built-in type, and no errors.
async fn gateway(
    main: &Path,
    program: &str,
    refs: &[(u32, u32)],
    rewritten: String,
) -> (ScriptedGateway, SocketAddr) {
    let (main, program, refs) = (main.to_path_buf(), program.to_string(), refs.to_vec());
    let g = ScriptedGateway::start(move |method, _| match method {
        "textDocument/references" => answers::locations(&main, &refs),
        "prodCode/structuralReplace" => answers::whole_file(&main, &program, &rewritten),
        "textDocument/hover" => answers::hover(U32),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;
    let addr = g.addr();
    (g, addr)
}

fn swap() -> Vec<prod_code_mcp::signature::Param> {
    ["factor", "x"]
        .iter()
        .map(|n| prod_code_mcp::signature::parse_param(n).unwrap())
        .collect()
}

fn text_of(result: &McpToolCallResult) -> String {
    result
        .content
        .iter()
        .map(|c| match c {
            McpContentItem::Text { text } => text.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Runs `tool` with `args`; `Err` with the text when it refused, `Ok` with it when it did not.
async fn tool(remote: SocketAddr, root: &Path, tool: &str, args: Value) -> Result<String, String> {
    match prod_code_mcp::tools::execute_tool(remote, root, tool, args).await {
        Ok(result) if result.is_error => Err(text_of(&result)),
        Ok(result) => Ok(text_of(&result)),
        Err(err) => Err(format!("{err:#}")),
    }
}

/// A reorder of `scale(x, factor)` with `scale` also passed as `fn(u32, u32) -> u32` on a line
/// of its own. The dry run names the pointer, and every write — direct, through the tool,
/// forced, compile-verified — is refused with the file as it was. The program the write would
/// leave compiles and prints something else. The base revision named it and wrote anyway.
#[tokio::test]
async fn a_function_used_as_a_value_stops_every_write_of_a_reorder() {
    refused_past_a_pointer("apart", APART).await;
}

/// The same with the pointer on the line of the call the rewrite changes: the base revision took
/// the changed line for the pointer's and did not even name it.
#[tokio::test]
async fn a_function_used_as_a_value_on_the_line_of_a_rewritten_call_is_named_and_stops_the_write() {
    refused_past_a_pointer("together", TOGETHER).await;
}

async fn refused_past_a_pointer(layout: &str, program: &str) {
    let decl = ("scale(x: u32, factor: u32)", "scale(factor: u32, x: u32)");
    let call = ("scale(3, 4)", "scale(4, 3)");
    {
        let ws = workspace(program);
        let root = ws.root();
        let main = ws.path("src/main.rs");
        let value = spot(program, "apply(scale)", 0, "apply(".len());
        let refs = [spot(program, call.0, 0, 0), value];
        let (_g, remote) = gateway(&main, program, &refs, edited(program, &[call])).await;
        let place = format!("src/main.rs:{}:{}", value.0, value.1);

        let dry =
            prod_code_mcp::signature::change(remote, &root, &main, 1, 8, &swap(), false, false)
                .await
                .unwrap_or_else(|e| panic!("{layout}: the dry run reports: {e:#}"));
        assert!(
            dry.unmatched.len() == 1 && dry.unmatched[0].starts_with(&place),
            "{layout}: the pointer {place} is named: {:?}",
            dry.unmatched
        );
        assert!(dry.unexpected.is_empty(), "{layout}: {:?}", dry.unexpected);
        assert!(
            dry.diagnostics.is_empty(),
            "{layout}: {:?}",
            dry.diagnostics
        );

        // What the write would leave compiles, and prints something else.
        let planned = &dry.rewritten.first().expect("one file").1;
        assert_eq!(planned, &edited(program, &[decl, call]), "{layout}");
        let (before, after) = (run(program), run(planned));
        assert_eq!(before, "34 12\n", "{layout}");
        assert_eq!(after, "34 21\n", "{layout}");
        println!(
            "{layout}: prints {before:?} as written and {after:?} as the write would leave it"
        );

        for force in [false, true] {
            let err =
                prod_code_mcp::signature::change(remote, &root, &main, 1, 8, &swap(), true, force)
                    .await
                    .expect_err(&format!("{layout}: written past {place} (force {force})"));
            let err = format!("{err:#}");
            assert!(
                err.contains("not complete") && err.contains(&place),
                "{layout}: {err}"
            );
            assert_eq!(ws.read("src/main.rs"), program, "{layout}: force {force}");
        }

        for extra in [json!({}), json!({ "verify": "compile" })] {
            let mut args = json!({
                "path": "src/main.rs", "line": 1, "character": 8,
                "params": ["factor", "x"], "apply": true, "force": true,
            });
            args.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            let outcome = tool(remote, &root, "code_change_signature", args).await;
            let Err(text) = outcome else {
                panic!("{layout} {extra}: the tool wrote past {place}: {outcome:?}");
            };
            assert!(text.contains(&place), "{layout} {extra}: {text}");
            assert_eq!(ws.read("src/main.rs"), program, "{layout} {extra}");
        }
    }
}

/// The control: every reference is a call, an import or a comment, the write goes through,
/// and the program prints what it printed.
#[tokio::test]
async fn a_reorder_whose_references_are_all_accounted_for_is_written_and_runs_the_same() {
    let program = CALLS_ONLY;
    let ws = workspace(program);
    let root = ws.root();
    let main = ws.path("src/main.rs");
    let call = ("scale(3, 4)", "scale(4, 3)");
    let decl = ("scale(x: u32, factor: u32)", "scale(factor: u32, x: u32)");
    let refs = [
        spot(program, "use calc::scale", 0, "use calc::".len()),
        spot(program, "see scale", 0, "see ".len()),
        spot(program, call.0, 0, 0),
    ];
    let (_g, remote) = gateway(&main, program, &refs, edited(program, &[call])).await;
    let done = prod_code_mcp::signature::change(remote, &root, &main, 2, 12, &swap(), true, false)
        .await
        .unwrap_or_else(|e| panic!("the reorder is written: {e:#}"));
    assert!(done.applied);
    assert!(done.unmatched.is_empty(), "{:?}", done.unmatched);
    let now = ws.read("src/main.rs");
    assert_eq!(now, edited(program, &[decl, call]));
    assert_eq!(run(&now), run(program));
    assert_eq!(run(&now), "34\n");
}

/// A visibility change rewrites no call, so the references it leaves as they are — the pointer
/// included — are not incomplete, and the tool writes it, forced or not.
#[tokio::test]
async fn a_visibility_change_keeps_its_references_as_they_are() {
    let ws = workspace(APART);
    let root = ws.root();
    let main = ws.path("src/main.rs");
    let refs = [
        spot(APART, "scale(3, 4)", 0, 0),
        spot(APART, "apply(scale)", 0, "apply(".len()),
    ];
    let (_g, remote) = gateway(&main, APART, &refs, APART.to_string()).await;
    let text = tool(
        remote,
        &root,
        "code_change_signature",
        json!({
            "path": "src/main.rs", "line": 1, "character": 8, "params": ["x", "factor"],
            "visibility": "pub(crate)", "apply": true,
        }),
    )
    .await
    .unwrap_or_else(|e| panic!("the visibility is changed: {e}"));
    assert!(text.contains("[applied to 1 file(s)]"), "{text}");
    let now = ws.read("src/main.rs");
    assert_eq!(
        now,
        edited(
            APART,
            &[("pub fn scale(x: u32", "pub(crate) fn scale(x: u32")]
        )
    );
    assert_eq!(run(&now), "34 12\n");
}

/// A rewrite that changes more than the references — a call the analyzer did not list — and a
/// reference at a position where the file says something else stop the write, forced or not.
#[tokio::test]
async fn an_unexplained_change_or_a_stale_reference_stops_the_write() {
    const TWO: &str = r#"pub fn scale(x: u32, factor: u32) -> u32 {
    x * 10 + factor
}

fn main() {
    let direct = scale(3, 4);
    let other = scale(5, 6);
    println!("{direct} {other}");
}
"#;
    let calls = [
        ("scale(3, 4)", "scale(4, 3)"),
        ("scale(5, 6)", "scale(6, 5)"),
    ];
    let first = spot(TWO, "scale(3, 4)", 0, 0);
    let cases = [
        (
            "a call the analyzer did not list",
            vec![first],
            "changed without being a reference: src/main.rs:7",
        ),
        (
            "a position that says `direct`",
            vec![
                first,
                spot(TWO, "direct", 0, 0),
                spot(TWO, "scale(5, 6)", 0, 0),
            ],
            "src/main.rs:6:9 (the analyzer places `scale` here, but the file says otherwise)",
        ),
    ];
    for (what, refs, why) in cases {
        let ws = workspace(TWO);
        let root = ws.root();
        let main = ws.path("src/main.rs");
        let (_g, remote) = gateway(&main, TWO, &refs, edited(TWO, &calls)).await;
        for force in [false, true] {
            let err =
                prod_code_mcp::signature::change(remote, &root, &main, 1, 8, &swap(), true, force)
                    .await
                    .expect_err(&format!("{what}: written (force {force})"));
            let err = format!("{err:#}");
            assert!(err.contains(why), "{what}: {err}");
            assert_eq!(ws.read("src/main.rs"), TWO, "{what}: force {force}");
        }
    }
}

const HOME: &str = "pub fn build(name: &str, width: u32, height: u32) -> String {\n    let area = width * height;\n    format!(\"{name} {area}\")\n}\n\npub fn caller() -> String {\n    build(\"a\", 3, 4)\n}\n";
const AS_A_VALUE: &str = "\npub fn as_a_value() -> fn(&str, u32, u32) -> String {\n    build\n}\n";
const OTHER: &str =
    "use crate::home::build;\n\npub fn twice() -> String {\n    build(\"b\", 1, 2)\n}\n";

/// A crate whose `build` is called from `src/home.rs` and imported and called in
/// `src/other.rs`, with `as_a_value` in `src/home.rs` when `value`.
fn bundle_workspace(value: bool) -> (Workspace, Vec<(&'static str, String)>) {
    let home = if value {
        format!("{HOME}{AS_A_VALUE}")
    } else {
        HOME.to_string()
    };
    let files = vec![
        ("Cargo.toml", CARGO.to_string()),
        ("src/lib.rs", "pub mod home;\npub mod other;\n".to_string()),
        ("src/home.rs", home),
        ("src/other.rs", OTHER.to_string()),
    ];
    let borrowed: Vec<(&str, &str)> = files.iter().map(|(p, t)| (*p, t.as_str())).collect();
    let ws = Workspace::new(&borrowed);
    ws.commit();
    (ws, files)
}

/// A gateway answering the references to `build` with `refs`, and to `width` and `height` with
/// their use in the body.
async fn bundle_gateway(home: &Path, refs: Value) -> (ScriptedGateway, SocketAddr) {
    let h = home.to_path_buf();
    let g = ScriptedGateway::start(move |method, params| match method {
        "textDocument/references" => {
            match params
                .pointer("/position/character")
                .and_then(|v| v.as_u64())
            {
                Some(7) => refs.clone(),
                Some(25) => answers::locations(&h, &[(2, 16)]),
                Some(37) => answers::locations(&h, &[(2, 24)]),
                _ => json!([]),
            }
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;
    let addr = g.addr();
    (g, addr)
}

async fn bundle(
    remote: SocketAddr,
    ws: &Workspace,
    apply: bool,
    force: bool,
) -> anyhow::Result<prod_code_mcp::parameter_object::ParameterObject> {
    prod_code_mcp::parameter_object::introduce(
        remote,
        &ws.root(),
        &ws.path("src/home.rs"),
        1,
        8,
        &["width".to_string(), "height".to_string()],
        "Size",
        "size",
        apply,
        force,
    )
    .await
}

fn assert_untouched(ws: &Workspace, files: &[(&str, String)], why: &str) {
    for (rel, text) in files {
        assert_eq!(&ws.read(rel), text, "{rel} was written: {why}");
    }
}

/// Bundling parameters rewrites calls; the function used as a value, a position no file has and
/// a referenced file that cannot be read are not calls it rewrote. Each stops the write — direct
/// and forced, and through the tool with `verify: "compile"` — with every file as it was. An
/// import is not one of them: the control writes past it.
#[tokio::test]
async fn a_parameter_object_is_not_written_past_a_reference_it_did_not_rewrite() {
    // The control: the calls in both files, and the import.
    let (ws, _) = bundle_workspace(false);
    let (home, other) = (ws.path("src/home.rs"), ws.path("src/other.rs"));
    let calls = [
        answers::locations(&home, &[(7, 5)]),
        answers::locations(&other, &[(1, 18), (4, 5)]),
    ];
    let all = |extra: Value| {
        Value::Array(
            calls
                .iter()
                .chain(std::iter::once(&extra))
                .flat_map(|v| v.as_array().cloned().unwrap_or_default())
                .collect(),
        )
    };
    let (_g, remote) = bundle_gateway(&home, all(json!([]))).await;
    let done = bundle(remote, &ws, true, false)
        .await
        .unwrap_or_else(|e| panic!("the bundle is written: {e:#}"));
    assert!(done.applied);
    assert!(done.unmatched.is_empty(), "{:?}", done.unmatched);
    assert_eq!(done.call_sites, 2);
    let other_now = ws.read("src/other.rs");
    assert!(
        other_now.contains("use crate::home::build;")
            && other_now.contains("build(\"b\", Size { width: 1, height: 2 })"),
        "{other_now}"
    );

    let (ws, files) = bundle_workspace(true);
    let home = ws.path("src/home.rs");
    let other = ws.path("src/other.rs");
    let gone = ws.root().join("src/gone.rs");
    let with = |extra: Value| {
        Value::Array(
            [
                answers::locations(&home, &[(7, 5)]),
                answers::locations(&other, &[(1, 18), (4, 5)]),
                extra,
            ]
            .iter()
            .flat_map(|v| v.as_array().cloned().unwrap_or_default())
            .collect(),
        )
    };
    let cases = [
        (
            "the function used as a value",
            with(answers::locations(&home, &[(11, 5)])),
            "src/home.rs:11:5",
        ),
        (
            "a position no file has",
            with(answers::locations(&home, &[(99, 1)])),
            "src/home.rs:99:1 (no such position in the file)",
        ),
        (
            "a file that cannot be read",
            with(answers::locations(&gone, &[(1, 1)])),
            "gone.rs",
        ),
    ];
    for (what, refs, why) in cases {
        let (_g, remote) = bundle_gateway(&home, refs).await;
        match bundle(remote, &ws, false, false).await {
            Ok(dry) => assert!(
                dry.unmatched.iter().any(|u| u.contains(why)),
                "{what}: {:?}",
                dry.unmatched
            ),
            Err(err) => assert!(
                format!("{err:#}").contains("cannot read") && format!("{err:#}").contains(why),
                "{what}: {err:#}"
            ),
        }
        for force in [false, true] {
            let outcome = bundle(remote, &ws, true, force).await;
            let Err(err) = outcome else {
                panic!("{what}: written (force {force})");
            };
            assert!(format!("{err:#}").contains(why), "{what}: {err:#}");
            assert_untouched(&ws, &files, &format!("{what}, force {force}"));
        }
        let outcome = tool(
            remote,
            &ws.root(),
            "code_introduce_parameter_object",
            json!({
                "path": "src/home.rs", "line": 1, "character": 8,
                "params": ["width", "height"], "name": "Size",
                "apply": true, "force": true, "verify": "compile",
            }),
        )
        .await;
        let Err(text) = outcome else {
            panic!("{what}: the compile-verified tool wrote: {outcome:?}");
        };
        assert!(text.contains(why), "{what}: {text}");
        assert_untouched(&ws, &files, &format!("{what}, verify compile"));
    }
}

/// The program for a real rust-analyzer: `scale` passed as a pointer apart from its call,
/// `shift` on the line of its call, and `mix` only called.
const LIVE: &str = r#"fn scale(x: u32, factor: u32) -> u32 {
    x * 10 + factor
}

fn shift(x: u32, by: u32) -> u32 {
    x * 100 + by
}

fn mix(a: u32, b: u32) -> u32 {
    a * 1000 + b
}

fn apply(f: fn(u32, u32) -> u32) -> u32 {
    f(1, 2)
}

fn main() {
    let direct = scale(3, 4);
    let through = apply(scale);
    let (shifted, shifted_through) = (shift(5, 6), apply(shift));
    let mixed = mix(7, 8);
    println!("{direct} {through} {shifted} {shifted_through} {mixed}");
}
"#;

const LIVE_PRINTED: &str = "34 12 506 102 7008\n";

/// `LIVE` against a real gateway and its rust-analyzer: the references and the structural
/// rewrite are the analyzer's own. Reordering `scale` or `shift` names the pointer and writes
/// nothing, forced or not; reordering `mix` is written and prints the same. It runs when
/// `PROD_CODE_LIVE_GATEWAY` holds the address of a running gateway on this machine; invoke this
/// ignored integration test explicitly with that prerequisite.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires PROD_CODE_LIVE_GATEWAY pointing to a running gateway"]
async fn a_real_rust_analyzer_reorder_is_refused_past_a_function_pointer() {
    let addr = std::env::var("PROD_CODE_LIVE_GATEWAY")
        .expect("set PROD_CODE_LIVE_GATEWAY to run this integration test")
        .parse::<SocketAddr>()
        .expect("PROD_CODE_LIVE_GATEWAY must be a socket address");
    assert_eq!(run(LIVE), LIVE_PRINTED);
    // Not a dot-directory: some tools pass over hidden ones.
    let dir = tempfile::Builder::new()
        .prefix("sig-complete-live-")
        .tempdir()
        .expect("checkout dir");
    let root = std::fs::canonicalize(dir.path()).expect("canonical root");
    for (rel, text) in [("Cargo.toml", CARGO), ("src/main.rs", LIVE)] {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(path, text).expect("write");
    }
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(&root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("git runs")
    };
    assert!(git(&["init", "-q"]).success());
    assert!(git(&["add", "-A"]).success());
    git(&[
        "-c",
        "user.email=test@example.invalid",
        "-c",
        "user.name=test",
        "commit",
        "-qm",
        "fixture",
    ]);
    let main = root.join("src/main.rs");
    let keep = |names: [&str; 2]| -> Vec<prod_code_mcp::signature::Param> {
        names
            .iter()
            .map(|n| prod_code_mcp::signature::parse_param(n).unwrap())
            .collect()
    };

    for (function, params, call, rewritten, value) in [
        (
            "scale",
            ["factor", "x"],
            "scale(3, 4)",
            "scale(4, 3)",
            spot(LIVE, "apply(scale)", 0, "apply(".len()),
        ),
        (
            "shift",
            ["by", "x"],
            "shift(5, 6)",
            "shift(6, 5)",
            spot(LIVE, "apply(shift)", 0, "apply(".len()),
        ),
    ] {
        let (line, col) = spot(LIVE, &format!("fn {function}"), 0, 3);
        // Until the analyzer has loaded the crate it lists no references; a dry run costs
        // nothing, so it is repeated until the call comes back rewritten.
        let mut dry = Err(anyhow::anyhow!("not asked"));
        for _ in 0..60 {
            dry = prod_code_mcp::signature::change(
                addr,
                &root,
                &main,
                line,
                col,
                &keep(params),
                false,
                false,
            )
            .await;
            if dry.as_ref().is_ok_and(|d| {
                d.unexpected.is_empty()
                    && d.rewritten
                        .iter()
                        .any(|(_, t)| t.contains(rewritten) && !t.contains(call))
            }) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
        }
        let dry = dry.unwrap_or_else(|e| panic!("`{function}`: the dry run reports: {e:#}"));
        eprintln!("{}", dry.render(4000));
        let place = format!("src/main.rs:{}:{}", value.0, value.1);
        assert!(
            dry.unmatched.len() == 1 && dry.unmatched[0].starts_with(&place),
            "`{function}`: the pointer {place} is named: {:?}",
            dry.unmatched
        );
        assert!(dry.unexpected.is_empty(), "{:?}", dry.unexpected);
        let planned = &dry.rewritten.first().expect("one file").1;
        let after = run(planned);
        assert_ne!(after, LIVE_PRINTED, "`{function}`: {planned}");
        eprintln!("`{function}`: prints {LIVE_PRINTED:?} as written and {after:?} as planned");
        for force in [false, true] {
            let err = prod_code_mcp::signature::change(
                addr,
                &root,
                &main,
                line,
                col,
                &keep(params),
                true,
                force,
            )
            .await
            .expect_err(&format!(
                "`{function}`: written past {place} (force {force})"
            ));
            assert!(format!("{err:#}").contains(&place), "{err:#}");
            assert_eq!(
                std::fs::read_to_string(&main).expect("read"),
                LIVE,
                "`{function}`: force {force}"
            );
        }
    }

    // The control: `mix` is only called, and the reorder is written.
    let (line, col) = spot(LIVE, "fn mix", 0, 3);
    let done = prod_code_mcp::signature::change(
        addr,
        &root,
        &main,
        line,
        col,
        &keep(["b", "a"]),
        true,
        false,
    )
    .await
    .unwrap_or_else(|e| panic!("`mix` is reordered: {e:#}"));
    assert!(done.applied && done.unmatched.is_empty() && done.unexpected.is_empty());
    let now = std::fs::read_to_string(&main).expect("read");
    assert!(
        now.contains("fn mix(b: u32, a: u32)") && now.contains("mix(8, 7)"),
        "{now}"
    );
    assert_eq!(run(&now), LIVE_PRINTED, "{now}");
}

/// A raw identifier containing the keyword `use` cannot turn a value reference into an import.
#[tokio::test]
async fn a_raw_use_identifier_does_not_hide_a_function_pointer() {
    let program = APART
        .replace("fn apply(f:", "fn apply(r#use: (), f:")
        .replace(
            "let through = apply(scale);",
            "let r#use = (); let through = apply(r#use, scale);",
        );
    let ws = workspace(&program);
    let main = ws.path("src/main.rs");
    let value = spot(&program, "apply(r#use, scale)", 0, "apply(r#use, ".len());
    let refs = [spot(&program, "scale(3, 4)", 0, 0), value];
    let (_g, remote) = gateway(
        &main,
        &program,
        &refs,
        edited(&program, &[("scale(3, 4)", "scale(4, 3)")]),
    )
    .await;
    assert_eq!(run(&program), "34 12\n");
    for verify in [json!({}), json!({"verify": "compile"})] {
        let mut args = json!({"path": "src/main.rs", "line": 1, "character": 8,
            "params": ["factor", "x"], "apply": true, "force": true});
        args.as_object_mut()
            .unwrap()
            .extend(verify.as_object().unwrap().clone());
        let result = tool(remote, &ws.root(), "code_change_signature", args).await;
        let err = result.expect_err("a raw identifier must not conceal a reference");
        assert!(err.contains("not complete"), "{err}");
        assert_eq!(ws.read("src/main.rs"), program);
    }
}

#[tokio::test]
async fn a_missing_javascript_reference_file_stops_every_apply() {
    const JS: &str = "function build(width, height) {\n    return width + height;\n}\nconst item = build(3, 4);\n";
    let ws = Workspace::new(&[
        ("package.json", "{\"type\":\"module\"}"),
        ("src/main.js", JS),
    ]);
    let home = ws.path("src/main.js");
    let gone = ws.path("src/missing.js");
    let h = home.clone();
    let gateway = ScriptedGateway::start(move |method, args| match method {
        "textDocument/references" => {
            match args.pointer("/position/character").and_then(Value::as_u64) {
                Some(9) => json!([
                    answers::locations(&h, &[(4, 14)]).as_array().unwrap()[0].clone(),
                    answers::locations(&gone, &[(1, 1)]).as_array().unwrap()[0].clone(),
                ]),
                Some(15) => answers::locations(&h, &[(2, 12)]),
                Some(22) => answers::locations(&h, &[(2, 20)]),
                _ => json!([]),
            }
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;
    for force in [false, true] {
        let result = prod_code_mcp::parameter_object::introduce(
            gateway.addr(),
            &ws.root(),
            &home,
            1,
            10,
            &["width".into(), "height".into()],
            "Size",
            "size",
            true,
            force,
        )
        .await;
        let err = result
            .expect_err("a missing indexed file is unknown evidence, not a proven absent caller");
        assert!(format!("{err:#}").contains("missing.js"), "{err:#}");
        assert_eq!(ws.read("src/main.js"), JS);
    }
}
