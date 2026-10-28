/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::tools::execute_tool;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};

fn position(text: &str, needle: &str) -> (u32, u32) {
    let at = text.find(needle).unwrap();
    let before = &text[..at];
    (
        before.bytes().filter(|b| *b == b'\n').count() as u32,
        before.rsplit('\n').next().unwrap().encode_utf16().count() as u32,
    )
}

fn native_output(ws: &Workspace) -> String {
    native_output_edition(ws, "2024")
}

fn native_output_edition(ws: &Workspace, edition: &str) -> String {
    let compile = std::process::Command::new("rustc")
        .args([
            &format!("--edition={edition}"),
            "src/main.rs",
            "-o",
            "fixture",
        ])
        .current_dir(ws.root())
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = std::process::Command::new(ws.path("fixture"))
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    String::from_utf8(run.stdout).unwrap()
}

#[tokio::test]
async fn public_async_change_accepts_2021_gen_identifier() {
    const SOURCE: &str = "use std::future::Future;\nfn load(x: u32) -> u32 { x }\nfn block_on<F: Future>(f: F) -> F::Output { let mut f = std::pin::pin!(f); let mut cx = std::task::Context::from_waker(std::task::Waker::noop()); match f.as_mut().poll(&mut cx) { std::task::Poll::Ready(v) => v, _ => panic!() } }\nfn main() { let gen = 7; let answer = block_on(async { load(gen) }); println!(\"{}\", answer); }\n";
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname='demo'\nversion='0.1.0'\nedition='2021'\n",
        ),
        ("src/main.rs", SOURCE),
    ]);
    ws.commit();
    let before = native_output_edition(&ws, "2021");
    assert_eq!(before, "7\n");
    let result = apply(
        &ws,
        SOURCE,
        SOURCE.to_string(),
        "load(x",
        "load(gen)",
        serde_json::json!(["x"]),
        Some(true),
        false,
    )
    .await;
    assert!(result.as_ref().is_ok_and(|r| !r.is_error), "{result:?}");
    assert!(ws.read("src/main.rs").contains("load(gen).await"));
    assert_eq!(native_output_edition(&ws, "2021"), before);
}

async fn apply(
    ws: &Workspace,
    source: &'static str,
    rewritten: String,
    function: &'static str,
    call: &'static str,
    params: serde_json::Value,
    asyncness: Option<bool>,
    force: bool,
) -> anyhow::Result<prod_code_mcp::protocol::McpToolCallResult> {
    let file = ws.path("src/main.rs");
    let (line, col) = position(source, call);
    let answer_file = file.clone();
    let remote = match std::env::var("PROD_CODE_ISSUE1504_GATEWAY") {
        Ok(address) => address.parse().expect("the live gateway address is valid"),
        Err(_) => ScriptedGateway::start(move |method, _| match method {
            "prodCode/structuralReplace" => answers::whole_file(&answer_file, source, &rewritten),
            "textDocument/references" => answers::locations(&answer_file, &[(line + 1, col + 1)]),
            "textDocument/hover" => {
                let description = if source.contains("macro_rules! stringify") {
                    "```rust\nfixture::stringify\n```\n```rust\nmacro_rules! stringify\n```"
                } else {
                    "```rust\ncore::macros::builtin\n```\n```rust\nmacro_rules! stringify\n```"
                };
                serde_json::json!({
                    "contents": { "kind": "markdown", "value": description }
                })
            }
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => serde_json::Value::Null,
        })
        .await
        .addr(),
    };
    let (decl_line, decl_col) = position(source, function);
    let mut args = serde_json::json!({"path":"src/main.rs", "line":decl_line + 1, "character":decl_col + 1, "params":params, "apply":true, "force":force});
    if let Some(want) = asyncness {
        args["async"] = serde_json::json!(want);
    }
    execute_tool(remote, &ws.root(), "code_change_signature", args).await
}

#[tokio::test]
async fn public_async_change_keeps_async_block_runtime() {
    const SOURCE: &str = "use std::future::Future;\nfn load(x: u32) -> u32 { x }\nfn block_on<F: Future>(f: F) -> F::Output { let mut f = std::pin::pin!(f); let mut cx = std::task::Context::from_waker(std::task::Waker::noop()); match f.as_mut().poll(&mut cx) { std::task::Poll::Ready(v) => v, _ => panic!() } }\nfn main() { let answer = block_on(async { load(7) }); println!(\"{}\", answer); }\n";
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname='demo'\nversion='0.1.0'\nedition='2024'\n",
        ),
        ("src/main.rs", SOURCE),
    ]);
    ws.commit();
    let before = native_output(&ws);
    assert_eq!(before, "7\n");
    let result = apply(
        &ws,
        SOURCE,
        SOURCE.to_string(),
        "load(x",
        "load(7)",
        serde_json::json!(["x"]),
        Some(true),
        false,
    )
    .await;
    assert!(result.as_ref().is_ok_and(|r| !r.is_error), "{result:?}");
    let changed = ws.read("src/main.rs");
    assert!(
        changed.contains("async fn load") && changed.contains("load(7).await"),
        "{changed}"
    );
    assert_eq!(native_output(&ws), before);
}

#[tokio::test]
async fn public_async_change_refuses_a_synchronous_closure() {
    const SOURCE: &str = "fn load(x: u32) -> u32 { x }\nasync fn outer() { let f = || load(7); let _ = f(); }\nfn main() {}\n";
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname='demo'\nversion='0.1.0'\nedition='2024'\n",
        ),
        ("src/main.rs", SOURCE),
    ]);
    ws.commit();
    native_output(&ws);
    let result = apply(
        &ws,
        SOURCE,
        SOURCE.to_string(),
        "load(x",
        "load(7)",
        serde_json::json!(["x"]),
        Some(true),
        false,
    )
    .await;
    let error = result.unwrap_err().to_string();
    assert!(error.contains("async"), "{error}");
    assert_eq!(ws.read("src/main.rs"), SOURCE);
}

#[tokio::test]
async fn public_async_change_keeps_async_closure_runtime() {
    const SOURCE: &str = "use std::future::Future;\nfn load(x: u32) -> u32 { x }\nfn block_on<F: Future>(f: F) -> F::Output { let mut f = std::pin::pin!(f); let mut cx = std::task::Context::from_waker(std::task::Waker::noop()); match f.as_mut().poll(&mut cx) { std::task::Poll::Ready(v) => v, _ => panic!() } }\nfn main() { let f = async || load(7); let answer = block_on(f()); println!(\"{}\", answer); }\n";
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname='demo'\nversion='0.1.0'\nedition='2024'\n",
        ),
        ("src/main.rs", SOURCE),
    ]);
    ws.commit();
    let before = native_output(&ws);
    assert_eq!(before, "7\n");
    let result = apply(
        &ws,
        SOURCE,
        SOURCE.to_string(),
        "load(x",
        "load(7)",
        serde_json::json!(["x"]),
        Some(true),
        false,
    )
    .await;
    assert!(result.as_ref().is_ok_and(|r| !r.is_error), "{result:?}");
    let changed = ws.read("src/main.rs");
    assert!(
        changed.contains("async fn load") && changed.contains("load(7).await"),
        "{changed}"
    );
    assert_eq!(native_output(&ws), before);
}

#[tokio::test]
async fn stringify_reorder_runtime_control_and_public_safety_refusal() {
    let shadowed_builtin = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname='demo'\nversion='0.1.0'\nedition='2024'\n",
        ),
        (
            "src/main.rs",
            r#"macro_rules! stringify { ($($tokens:tt)*) => {{ println!("shadowed"); "value" }}; }
fn main() { println!("{}", stringify!(tokens)); }
"#,
        ),
    ]);
    shadowed_builtin.commit();
    assert_eq!(native_output(&shadowed_builtin), "shadowed\nvalue\n");

    const SOURCE: &str = "fn join(a: &str, b: &str) -> String { format!(\"{a}|{b}\") }\nfn main() { println!(\"{}\", join(stringify!(& &), stringify!(&&))); }\n";
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname='demo'\nversion='0.1.0'\nedition='2024'\n",
        ),
        ("src/main.rs", SOURCE),
    ]);
    ws.commit();
    let before = native_output(&ws);
    assert_eq!(before, "& &|&&\n");
    let rewritten = SOURCE.replace(
        "join(stringify!(& &), stringify!(&&))",
        "join(stringify!(&&), stringify!(& &))",
    );
    let result = apply(
        &ws,
        SOURCE,
        rewritten.clone(),
        "join(a",
        "join(stringify",
        serde_json::json!(["b", "a"]),
        None,
        false,
    )
    .await;
    let applied = result.expect("the built-in stringify arguments have no runtime effects");
    assert!(!applied.is_error, "{:?}", applied.content);
    let applied_source = ws.read("src/main.rs");
    assert!(applied_source.contains("fn join(b: &str, a: &str)"));
    assert!(applied_source.contains("join(stringify!(&&), stringify!(& &))"));
    assert_eq!(native_output(&ws), before);

    const SHADOWED: &str = "macro_rules! stringify { ($($tokens:tt)*) => {{ print!(\"side\"); \"value\" }}; }\nfn join(a: &str, b: &str) -> String { format!(\"{a}|{b}\") }\nfn main() { println!(\"{}\", join(stringify!(& &), stringify!(&&))); }\n";
    let shadowed = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname='demo'\nversion='0.1.0'\nedition='2024'\n",
        ),
        ("src/main.rs", SHADOWED),
    ]);
    shadowed.commit();
    let shadowed_before = native_output(&shadowed);
    let shadowed_rewrite = SHADOWED.replace(
        "join(stringify!(& &), stringify!(&&))",
        "join(stringify!(&&), stringify!(& &))",
    );
    let result = apply(
        &shadowed,
        SHADOWED,
        shadowed_rewrite,
        "join(a",
        "join(stringify",
        serde_json::json!(["b", "a"]),
        None,
        false,
    )
    .await;
    let error = result.unwrap_err().to_string();
    assert!(error.contains("evaluated in the opposite order"), "{error}");
    assert_eq!(shadowed.read("src/main.rs"), SHADOWED);
    assert_eq!(native_output(&shadowed), shadowed_before);
}
