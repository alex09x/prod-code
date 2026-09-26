//! Bundling Rust parameters keeps the order their values are dropped in, or refuses (#441).
//!
//! A function drops its parameters in the reverse of the order it declares them, and a struct
//! drops its fields in the order it declares them, so `fn take(a: Guard, b: Guard)` taking a
//! `Pair { a, b }` would drop `a` first where it dropped `b` first. Each case here is bundled by
//! the planner against a scripted gateway, and the original and the rewritten program are both
//! compiled with `rustc` and run where the test runs: every `Guard` prints when it is dropped, and
//! the two programs have to print the same.

use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::process::Command;

const DROPS: &str = "struct Guard(&'static str);

impl Drop for Guard {
    fn drop(&mut self) {
        println!(\"drop {}\", self.0);
    }
}

fn take(a: Guard, b: Guard) {
    println!(\"take {} {}\", a.0, b.0);
}

fn around(x: Guard, a: Guard, b: Guard, y: Guard) {
    println!(\"around {} {} {} {}\", x.0, a.0, b.0, y.0);
}

fn early(a: Guard, b: Guard, c: Guard, stop: Option<u32>) -> Option<u32> {
    let moved = b;
    let n = stop?;
    drop(moved);
    println!(\"early {}\", c.0);
    if n == 0 {
        return None;
    }
    Some(n + a.0.len() as u32)
}

fn gap(a: Guard, n: u32, b: Guard) {
    println!(\"gap {} {} {}\", a.0, n, b.0);
}

fn tag(name: &str, a: Guard, count: u32) {
    println!(\"tag {} {} {}\", name, a.0, count);
}

fn hold(a: Guard, b: Guard) -> impl FnOnce() {
    let f = move || {
        let g = a;
        println!(\"hold {}\", g.0);
    };
    println!(\"made {}\", b.0);
    f
}

fn apart(a: Guard, x: Guard, b: Guard) {
    println!(\"apart {} {} {}\", a.0, x.0, b.0);
}

async fn later(a: Guard, b: Guard) {
    println!(\"later {} {}\", a.0, b.0);
}

fn main() {
    take(Guard(\"A\"), Guard(\"B\"));
    around(Guard(\"X\"), Guard(\"A\"), Guard(\"B\"), Guard(\"Y\"));
    println!(\"{:?}\", early(Guard(\"A\"), Guard(\"B\"), Guard(\"C\"), None));
    println!(\"{:?}\", early(Guard(\"A\"), Guard(\"B\"), Guard(\"C\"), Some(0)));
    println!(\"{:?}\", early(Guard(\"A\"), Guard(\"B\"), Guard(\"C\"), Some(1)));
    gap(Guard(\"A\"), 1, Guard(\"B\"));
    tag(\"t\", Guard(\"A\"), 2);
    let f = hold(Guard(\"A\"), Guard(\"B\"));
    println!(\"returned\");
    f();
    let (first, middle, last) = (Guard(\"A\"), Guard(\"X\"), Guard(\"B\"));
    apart(first, middle, last);
    drop(later(Guard(\"A\"), Guard(\"B\")));
}
";

/// What `DROPS` prints: `B` before `A` at the end of every call, and the future of `later`,
/// dropped before it is polled, dropping `A` before `B`.
const PRINTED: &str = "take A B
drop B
drop A
around X A B Y
drop Y
drop B
drop A
drop X
drop B
drop C
drop A
None
drop B
early C
drop C
drop A
None
drop B
early C
drop C
drop A
Some(2)
gap A 1 B
drop B
drop A
tag t A 2
drop A
made B
drop B
returned
hold A
drop A
apart A X B
drop B
drop X
drop A
drop A
drop B
";

/// The 1-based line and column of byte `at`.
fn spot(text: &str, at: usize) -> (u32, u32) {
    let line = text[..at].matches('\n').count() as u32 + 1;
    let col = (at - text[..at].rfind('\n').map_or(0, |n| n + 1)) as u32 + 1;
    (line, col)
}

/// Where `word` is written as a whole name between `from` and `to`, outside string literals.
fn words(text: &str, from: usize, to: usize, word: &str) -> Vec<usize> {
    let bytes = text.as_bytes();
    let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut in_string = false;
    let mut out = Vec::new();
    for at in from..to {
        if bytes[at] == b'"' {
            in_string = !in_string;
        }
        if !in_string
            && text[at..].starts_with(word)
            && !ident(bytes[at - 1])
            && !bytes.get(at + word.len()).is_some_and(|b| ident(*b))
        {
            out.push(at);
        }
    }
    out
}

/// The references rust-analyzer gives in a source written like `DROPS`: a function is called
/// from `main`, and a parameter is used in its function's body.
fn references(text: &str) -> HashMap<(u32, u32), Vec<(u32, u32)>> {
    let main = text.find("fn main()").expect("a main");
    let mut table = HashMap::new();
    let mut at = 0;
    while let Some(n) = text[at..main].find("fn ") {
        let decl = at + n + 3;
        at = decl;
        let open = decl + text[decl..].find('(').expect("a parameter list");
        let name = &text[decl..open];
        if name == "drop" {
            continue;
        }
        let calls = words(text, main, text.len(), name)
            .into_iter()
            .map(|c| spot(text, c))
            .collect();
        table.insert(spot(text, decl), calls);
        let close = open + text[open..].find(')').expect("the list ends");
        let end = close + text[close..].find("\n}\n").expect("the body ends");
        let mut param = open + 1;
        for raw in text[open + 1..close].split(", ") {
            let pname = &raw[..raw.find(':').expect("a typed parameter")];
            let uses = words(text, close, end, pname)
                .into_iter()
                .map(|u| spot(text, u))
                .collect();
            table.insert(spot(text, param), uses);
            param += raw.len() + 2;
        }
    }
    table
}

async fn gateway(file: &Path, text: &str) -> SocketAddr {
    let (file, table) = (file.to_path_buf(), references(text));
    ScriptedGateway::start(move |method, params| {
        let at = |p: &str| params.pointer(p).and_then(|v| v.as_u64()).unwrap_or(0) as u32 + 1;
        match method {
            "textDocument/references" => table
                .get(&(at("/position/line"), at("/position/character")))
                .map_or_else(|| serde_json::json!([]), |s| answers::locations(&file, s)),
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => serde_json::Value::Null,
        }
    })
    .await
    .addr()
}

fn workspace(edition: &str) -> Workspace {
    Workspace::new(&[
        (
            "Cargo.toml",
            &format!("[package]\nname = \"drops\"\nversion = \"0.1.0\"\nedition = \"{edition}\"\n"),
        ),
        ("src/main.rs", DROPS),
    ])
}

/// Bundles `params` of `function` in `src/main.rs` into `name`, bound as `binding`.
async fn bundle(
    ws: &Workspace,
    function: &str,
    params: &[&str],
    name: &str,
    apply: bool,
) -> anyhow::Result<prod_code_mcp::parameter_object::ParameterObject> {
    let file = ws.path("src/main.rs");
    let remote = gateway(&file, DROPS).await;
    let at = DROPS
        .find(&format!("fn {function}("))
        .unwrap_or_else(|| panic!("`{function}` is declared"))
        + 3;
    let (line, col) = spot(DROPS, at);
    let params: Vec<String> = params.iter().map(|p| p.to_string()).collect();
    prod_code_mcp::parameter_object::introduce(
        remote,
        &ws.root(),
        &file,
        line,
        col,
        &params,
        name,
        &name.to_lowercase(),
        apply,
        false,
    )
    .await
}

fn main_rs(done: &prod_code_mcp::parameter_object::ParameterObject) -> String {
    assert!(done.diagnostics.is_empty(), "{:?}", done.diagnostics);
    done.rewritten
        .iter()
        .find(|(p, _)| p.ends_with("main.rs"))
        .map(|(_, t)| t.clone())
        .expect("main.rs is rewritten")
}

/// Compiles `source` with `rustc` and runs it, and returns what it printed. A failure of either
/// panics with the exit status and everything the compiler or the program wrote.
fn run(source: &str, edition: &str) -> String {
    let dir = tempfile::tempdir().expect("a directory for the program");
    let (main, program) = (dir.path().join("main.rs"), dir.path().join("program"));
    std::fs::write(&main, source).expect("write the program");
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let built = Command::new(rustc)
        .args(["--edition", edition, "-o"])
        .arg(&program)
        .arg(&main)
        .output()
        .expect("rustc starts");
    assert!(
        built.status.success(),
        "rustc: {}\n{}\n{source}",
        built.status,
        String::from_utf8_lossy(&built.stderr)
    );
    let ran = Command::new(&program).output().expect("the program starts");
    assert!(
        ran.status.success(),
        "the program: {}\n{}\n{}\n{source}",
        ran.status,
        String::from_utf8_lossy(&ran.stdout),
        String::from_utf8_lossy(&ran.stderr)
    );
    String::from_utf8_lossy(&ran.stdout).into_owned()
}

/// Two owned parameters next to each other become fields in the reverse of their order, so
/// the struct drops `b` before `a` as the function did; the call still evaluates `a` first.
#[tokio::test]
async fn adjacent_owned_parameters_are_dropped_in_the_order_they_were() {
    assert_eq!(run(DROPS, "2021"), PRINTED);
    let ws = workspace("2021");
    let done = bundle(&ws, "take", &["a", "b"], "Pair", false)
        .await
        .expect("adjacent parameters are bundled");
    let text = main_rs(&done);
    assert_eq!(run(&text, "2021"), PRINTED, "{text}");
    assert!(
        text.contains("pub struct Pair {\n    pub b: Guard,\n    pub a: Guard,\n}"),
        "{text}"
    );
    assert!(text.contains("fn take(pair: Pair) {"), "{text}");
    assert!(
        text.contains("take(Pair { a: Guard(\"A\"), b: Guard(\"B\") });"),
        "{text}"
    );
}

/// Owned parameters on either side of the bundle, a value moved out of it before an early
/// return by `?` or `return`, a scalar between two bundled values, a move closure that takes one
/// field, and a borrow and a scalar bundled around an owned value: each program prints what the
/// original did.
#[tokio::test]
async fn neighbours_moves_early_returns_and_closures_keep_the_drop_order() {
    let ws = workspace("2021");
    let cases: &[(&str, &[&str], &str, &str)] = &[
        (
            "around",
            &["a", "b"],
            "Pair",
            "around(Guard(\"X\"), Pair { a: Guard(\"A\"), b: Guard(\"B\") }, Guard(\"Y\"));",
        ),
        (
            "early",
            &["a", "b", "c"],
            "Trio",
            "let moved = trio.b;\n    let n = stop?;",
        ),
        (
            "gap",
            &["a", "b"],
            "Pair",
            "gap(Pair { a: Guard(\"A\"), b: Guard(\"B\") }, 1);",
        ),
        (
            "tag",
            &["name", "count"],
            "Labels",
            "pub struct Labels<'a> {\n    pub name: &'a str,\n    pub count: u32,\n}",
        ),
        ("hold", &["a", "b"], "Pair", "let g = pair.a;"),
    ];
    for (function, params, name, expected) in cases {
        let done = bundle(&ws, function, params, name, false)
            .await
            .unwrap_or_else(|e| panic!("`{function}` is bundled: {e:#}"));
        let text = main_rs(&done);
        assert!(text.contains(expected), "{function}: {text}");
        assert_eq!(run(&text, "2021"), PRINTED, "{function}: {text}");
    }
    assert_eq!(ws.read("src/main.rs"), DROPS, "nothing was written");
}

/// An owned parameter between the bundled ones would be dropped before both of them rather
/// than between them, and an `async` function's future drops its parameters in one order when
/// it is dropped unpolled and in the other once it has run, which no order of the fields keeps.
/// Both are refused before anything is written, and so is a closure in a crate of edition 2018,
/// which captures the whole struct where it captured one parameter.
#[tokio::test]
async fn an_order_no_struct_keeps_is_refused_before_anything_is_written() {
    let ws = workspace("2021");
    let err = bundle(&ws, "apart", &["a", "b"], "Pair", true)
        .await
        .expect_err("`x` is dropped between `b` and `a`");
    let err = format!("{err:#}");
    assert!(
        err.contains(
            "`apart` drops the parameters that may have a destructor in the order `b`, `x`, `a`; \
             with `a`, `b` in `Pair` it would drop them in the order `x`, `b`, `a`"
        ),
        "{err}"
    );
    assert!(err.contains("nothing was rewritten"), "{err}");

    let err = bundle(&ws, "later", &["a", "b"], "Pair", true)
        .await
        .expect_err("no field order keeps both of the future's orders");
    let err = format!("{err:#}");
    assert!(
        err.contains(
            "`later` is `async`: a future dropped before it is polled drops the parameters that \
             may have a destructor in the order `a`, `b`; with `a`, `b` in `Pair` it would drop \
             them in the order `b`, `a`"
        ),
        "{err}"
    );
    assert!(err.contains("nothing was rewritten"), "{err}");
    assert_eq!(ws.read("src/main.rs"), DROPS, "nothing was written");

    let old = workspace("2018");
    let err = bundle(&old, "hold", &["a", "b"], "Pair", true)
        .await
        .expect_err("edition 2018 captures all of `pair`");
    let err = format!("{err:#}");
    assert!(
        err.contains(
            "`hold` is in a crate of edition 2018, where a closure or an `async` block that uses \
             a field of `pair` captures all of it"
        ),
        "{err}"
    );
    assert_eq!(old.read("src/main.rs"), DROPS, "nothing was written");
    let done = bundle(&old, "take", &["a", "b"], "Pair", false)
        .await
        .expect("a body without a closure is bundled in edition 2018 too");
    let text = main_rs(&done);
    assert_eq!(run(&text, "2018"), PRINTED, "{text}");
}
