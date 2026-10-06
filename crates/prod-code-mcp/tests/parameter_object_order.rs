/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Bundling keeps the order a call evaluates its arguments in, or refuses (#436), driven end to
//! end against a scripted gateway in Python, whose keyword arguments may come in any order. The
//! rewritten module is run with `python3` where the test runs, and prints what the original did.

use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::net::SocketAddr;
use std::path::Path;

/// The 1-based line and column of the first `needle` in `text`, `skip` bytes into it.
fn spot(text: &str, needle: &str, skip: usize) -> (u32, u32) {
    let at = text
        .find(needle)
        .unwrap_or_else(|| panic!("`{needle}` is in the fixture"))
        + skip;
    let line = text[..at].matches('\n').count() as u32 + 1;
    let col = (at - text[..at].rfind('\n').map_or(0, |n| n + 1)) as u32 + 1;
    (line, col)
}

const PLOT: &str = "seen = []


def mark(v):
    seen.append(v)
    return v


def plot(a, b, c):
    return f\"{a}{b}{c}\"


def total(a, b, c):
    return a + b + c


print(plot(c=mark(\"c\"), a=mark(\"a\"), b=mark(\"b\")))
print(total(mark(1), mark(2), mark(3)))
print(\",\".join(str(s) for s in seen))
";

/// A gateway with the references basedpyright gives in `PLOT`; the ones to `plot` fail when
/// `fail` says so.
async fn gateway(file: &Path, fail: bool) -> SocketAddr {
    let f = file.to_path_buf();
    let plot = spot(PLOT, "plot(a", 0);
    let total = spot(PLOT, "total(a", 0);
    let at = |start: (u32, u32), skip: u32| (start.0, start.1 + skip);
    let table = move |asked: (u32, u32)| -> Option<serde_json::Value> {
        let spots = match asked {
            _ if asked == plot && fail => return Some(answers::failure("server crashed")),
            _ if asked == plot => vec![spot(PLOT, "plot(c=", 0)],
            _ if asked == at(plot, 5) => vec![spot(PLOT, "{a}", 1)],
            _ if asked == at(plot, 8) => vec![spot(PLOT, "{b}", 1)],
            _ if asked == at(plot, 11) => vec![spot(PLOT, "{c}", 1)],
            _ if asked == total => vec![spot(PLOT, "total(mark", 0)],
            _ if asked == at(total, 6) => vec![spot(PLOT, "a + b", 0)],
            _ if asked == at(total, 9) => vec![spot(PLOT, "b + c", 0)],
            _ if asked == at(total, 12) => vec![spot(PLOT, "c\n", 0)],
            _ => return None,
        };
        Some(answers::locations(&f, &spots))
    };
    ScriptedGateway::start(move |method, params| {
        let at = |p: &str| params.pointer(p).and_then(|v| v.as_u64()).unwrap_or(0) as u32 + 1;
        match method {
            "textDocument/references" => table((at("/position/line"), at("/position/character")))
                .unwrap_or_else(|| serde_json::json!([])),
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => serde_json::Value::Null,
        }
    })
    .await
    .addr()
}

async fn bundle(
    remote: SocketAddr,
    ws: &Workspace,
    at: (u32, u32),
    params: &[&str],
    apply: bool,
) -> anyhow::Result<prod_code_mcp::parameter_object::ParameterObject> {
    let params: Vec<String> = params.iter().map(|p| p.to_string()).collect();
    prod_code_mcp::parameter_object::introduce(
        remote,
        &ws.root(),
        &ws.path("app/plot.py"),
        at.0,
        at.1,
        &params,
        "Pair",
        "pair",
        apply,
        false,
    )
    .await
}

/// Keywords passed as `c=…, a=…` go into the literal as `c=…, a=…`: the call evaluates `c`
/// first, and so does the rewritten one. Before #436 the literal took the declaration's order.
#[tokio::test]
async fn python_keywords_keep_the_order_the_call_evaluates_them_in() {
    let ws = Workspace::new(&[
        (
            "pyproject.toml",
            "[project]\nname = \"po-order\"\nversion = \"0.1.0\"\n",
        ),
        ("app/__init__.py", ""),
        ("app/plot.py", PLOT),
    ]);
    let remote = gateway(&ws.path("app/plot.py"), false).await;
    let done = bundle(remote, &ws, spot(PLOT, "plot(a", 0), &["a", "c"], false)
        .await
        .expect("contiguous keywords are bundled");
    let text = done
        .rewritten
        .iter()
        .find(|(p, _)| p.ends_with("plot.py"))
        .map(|(_, t)| t.clone())
        .expect("plot.py is rewritten");
    assert!(
        text.contains("print(plot(pair=Pair(c=mark(\"c\"), a=mark(\"a\")), b=mark(\"b\")))"),
        "{text}"
    );
    assert!(
        text.contains("def plot(pair: Pair, b):\n    return f\"{pair.a}{b}{pair.c}\""),
        "{text}"
    );

    let Some(before) = run_python(PLOT) else {
        eprintln!("skipping the run: `python3` is not installed here");
        return;
    };
    assert_eq!(before, "abc\n6\nc,a,b,1,2,3\n");
    assert_eq!(
        run_python(&text).as_deref(),
        Some(before.as_str()),
        "{text}"
    );
}

/// `total(mark(1), mark(2), mark(3))` with `a` and `c` bundled would evaluate `mark(3)` before
/// `mark(2)`; that is refused with the call and the two arguments, and a failed reference query
/// stops the change instead of leaving the calls it did not list behind. Nothing is written.
#[tokio::test]
async fn python_refuses_a_reordered_call_and_a_failed_reference_query() {
    let ws = Workspace::new(&[
        (
            "pyproject.toml",
            "[project]\nname = \"po-order\"\nversion = \"0.1.0\"\n",
        ),
        ("app/__init__.py", ""),
        ("app/plot.py", PLOT),
    ]);
    let file = ws.path("app/plot.py");
    let remote = gateway(&file, false).await;
    let err = bundle(remote, &ws, spot(PLOT, "total(a", 0), &["a", "c"], true)
        .await
        .expect_err("the call would be evaluated in another order");
    let err = format!("{err:#}");
    assert!(
        err.contains(
            "`total` at app/plot.py:18:7 passes `mark(2)` between the bundled arguments; in the \
             object `mark(3)` would be evaluated before it"
        ),
        "{err}"
    );

    let failing = gateway(&file, true).await;
    let err = bundle(failing, &ws, spot(PLOT, "plot(a", 0), &["a", "b"], true)
        .await
        .expect_err("a failed query is not an empty one");
    let err = format!("{err:#}");
    assert!(
        err.contains("the references to `plot` at app/plot.py:9:5 could not be listed"),
        "{err}"
    );
    assert!(err.contains("server crashed"), "{err}");
    assert_eq!(ws.read("app/plot.py"), PLOT, "nothing was written");
}

/// Runs a Python module and returns what it printed; `None` when there is no `python3`.
fn run_python(text: &str) -> Option<String> {
    let dir = tempfile::tempdir().expect("script dir");
    let path = dir.path().join("plot.py");
    std::fs::write(&path, text).expect("write the script");
    let out = std::process::Command::new("python3")
        .arg(&path)
        .output()
        .ok()?;
    assert!(
        out.status.success(),
        "python3 failed: {}\n{text}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}
