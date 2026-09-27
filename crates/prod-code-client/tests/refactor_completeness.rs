//! `prod-code change-signature` and `prod-code parameter-object` write only complete plans
//! (#446): a reference the plan did not rewrite — here the function passed as a pointer, apart
//! from its call or on the same line — stops `--apply`, with `--force` and with
//! `--verify compile`, and every file stays as it was. The binary is spawned against a scripted
//! gateway; the commands go through the same MCP handlers as `code_change_signature` and
//! `code_introduce_parameter_object`. The base revision wrote each of these.

use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::path::Path;
use std::process::Output;

const CARGO: &str = "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

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

const CALLS_ONLY: &str = r#"pub fn scale(x: u32, factor: u32) -> u32 {
    x * 10 + factor
}

fn main() {
    println!("{}", scale(3, 4));
}
"#;

const U32: &str = "```rust\nu32\n```\n\n---\n\nThe 32-bit unsigned integer type.";

/// The 1-based line and column of `needle` in `text`, plus `skip` bytes.
fn spot(text: &str, needle: &str, skip: usize) -> (u32, u32) {
    let at = text.find(needle).expect(needle) + skip;
    let line = text[..at].matches('\n').count() as u32 + 1;
    let col = (at - text[..at].rfind('\n').map_or(0, |i| i + 1)) as u32 + 1;
    (line, col)
}

async fn cli(ws: &Workspace, remote: SocketAddr, args: &[&str]) -> Output {
    let home = tempfile::Builder::new()
        .prefix("completehome")
        .tempdir()
        .expect("home");
    tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"))
        .arg("--remote")
        .arg(remote.to_string())
        .args(args)
        .env("PROD_CODE_REMOTE", remote.to_string())
        .env("HOME", home.path())
        .current_dir(ws.root())
        .output()
        .await
        .expect("run prod-code")
}

fn said(out: &Output) -> String {
    format!(
        "status {:?}\n--- stdout\n{}\n--- stderr\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// A gateway for `scale` in `program`, declared at 1:8: its references at `refs`, the call
/// `scale(3, 4)` rewritten to `scale(4, 3)`, `u32` confirmed as the built-in type, no errors.
async fn signature_gateway(
    main: &Path,
    program: &str,
    refs: Vec<(u32, u32)>,
) -> (ScriptedGateway, SocketAddr) {
    let (main, program) = (main.to_path_buf(), program.to_string());
    let rewritten = program.replace("scale(3, 4)", "scale(4, 3)");
    let g = ScriptedGateway::start(move |method, _| match method {
        "workspace/symbol" => json!([answers::symbol("scale", 12, &main, 1, 8)]),
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

/// Every write of a reorder past `scale` used as a pointer on a line of its own exits non-zero
/// and writes nothing; the preview names the pointer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn change_signature_does_not_write_past_a_function_pointer() {
    refused_past_a_pointer("apart", APART).await;
}

/// The same with the pointer on the line of the call the rewrite changes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn change_signature_does_not_write_past_a_function_pointer_on_the_line_of_a_call() {
    refused_past_a_pointer("together", TOGETHER).await;
}

async fn refused_past_a_pointer(layout: &str, program: &str) {
    let reorder = [
        "change-signature",
        "scale",
        "--param",
        "factor",
        "--param",
        "x",
    ];
    {
        let ws = Workspace::new(&[("Cargo.toml", CARGO), ("src/main.rs", program)]);
        ws.commit();
        let main = ws.path("src/main.rs");
        let value = spot(program, "apply(scale)", "apply(".len());
        let refs = vec![spot(program, "scale(3, 4)", 0), value];
        let (_g, remote) = signature_gateway(&main, program, refs).await;
        let place = format!("src/main.rs:{}:{}", value.0, value.1);

        let preview = cli(&ws, remote, &reorder).await;
        let text = said(&preview);
        assert!(preview.status.success(), "{layout}: {text}");
        assert!(
            text.contains("not rewritten") && text.contains(&place),
            "{layout}: the preview names {place}: {text}"
        );
        assert_eq!(ws.read("src/main.rs"), program, "{layout}: a preview wrote");

        for extra in [
            &["--apply"][..],
            &["--apply", "--force"],
            &["--apply", "--force", "--verify", "compile"],
        ] {
            let args: Vec<&str> = reorder.iter().chain(extra).copied().collect();
            let out = cli(&ws, remote, &args).await;
            let text = said(&out);
            assert!(!out.status.success(), "{layout} {extra:?}: {text}");
            assert!(text.contains(&place), "{layout} {extra:?}: {text}");
            assert_eq!(
                ws.read("src/main.rs"),
                program,
                "{layout} {extra:?}: written past {place}"
            );
        }
    }
}

/// The control: with every reference a call, `--apply` writes the reorder and exits 0.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn change_signature_writes_a_reorder_whose_references_are_all_calls() {
    let ws = Workspace::new(&[("Cargo.toml", CARGO), ("src/main.rs", CALLS_ONLY)]);
    ws.commit();
    let main = ws.path("src/main.rs");
    let refs = vec![spot(CALLS_ONLY, "scale(3, 4)", 0)];
    let (_g, remote) = signature_gateway(&main, CALLS_ONLY, refs).await;
    let out = cli(
        &ws,
        remote,
        &[
            "change-signature",
            "scale",
            "--param",
            "factor",
            "--param",
            "x",
            "--apply",
        ],
    )
    .await;
    assert!(out.status.success(), "{}", said(&out));
    assert_eq!(
        ws.read("src/main.rs"),
        CALLS_ONLY
            .replace("scale(x: u32, factor: u32)", "scale(factor: u32, x: u32)")
            .replace("scale(3, 4)", "scale(4, 3)")
    );
}

const HOME: &str = "pub fn build(name: &str, width: u32, height: u32) -> String {\n    let area = width * height;\n    format!(\"{name} {area}\")\n}\n\npub fn caller() -> String {\n    build(\"a\", 3, 4)\n}\n\npub fn as_a_value() -> fn(&str, u32, u32) -> String {\n    build\n}\n";

/// Bundling `width` and `height` of `build`, which is also used as a value: `--apply` exits
/// non-zero with the use named and nothing written, with `--force` and `--verify compile`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parameter_object_does_not_write_past_a_function_used_as_a_value() {
    let files = [
        ("Cargo.toml", CARGO),
        ("src/lib.rs", "pub mod home;\n"),
        ("src/home.rs", HOME),
    ];
    let ws = Workspace::new(&files);
    ws.commit();
    let home = ws.path("src/home.rs");
    let h = home.clone();
    let g = ScriptedGateway::start(move |method, params| match method {
        "workspace/symbol" => json!([answers::symbol("build", 12, &h, 1, 8)]),
        "textDocument/references" => {
            match params
                .pointer("/position/character")
                .and_then(|v| v.as_u64())
            {
                Some(7) => answers::locations(&h, &[(7, 5), (11, 5)]),
                Some(25) => answers::locations(&h, &[(2, 16)]),
                Some(37) => answers::locations(&h, &[(2, 24)]),
                _ => json!([]),
            }
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;
    let bundle = [
        "parameter-object",
        "build",
        "--param",
        "width",
        "--param",
        "height",
        "--name",
        "Size",
    ];

    let preview = cli(&ws, g.addr(), &bundle).await;
    let text = said(&preview);
    assert!(preview.status.success(), "{text}");
    assert!(text.contains("src/home.rs:11:5"), "{text}");

    for extra in [
        &["--apply"][..],
        &["--apply", "--force"],
        &["--apply", "--force", "--verify", "compile"],
    ] {
        let args: Vec<&str> = bundle.iter().chain(extra).copied().collect();
        let out = cli(&ws, g.addr(), &args).await;
        let text = said(&out);
        assert!(!out.status.success(), "{extra:?}: {text}");
        assert!(text.contains("src/home.rs:11:5"), "{extra:?}: {text}");
        for (rel, original) in files {
            assert_eq!(ws.read(rel), original, "{extra:?}: {rel} was written");
        }
    }
}
