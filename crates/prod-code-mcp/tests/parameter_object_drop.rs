//! Bundling Rust parameters keeps the order their values are dropped in, or refuses (#441).
//!
//! A function drops its parameters in the reverse of the order it declares them, and a struct
//! drops its fields in the order it declares them, so `fn take(a: Guard, b: Guard)` taking a
//! `Pair { a, b }` would drop `a` first where it dropped `b` first. Each case here is bundled by
//! the planner against a scripted gateway, and the original and the rewritten program are both
//! compiled with `rustc` and run where the test runs: every `Guard` prints when it is dropped, and
//! the two programs have to print the same.
//!
//! A primitive's name does not prove a type has no destructor: a program may declare or import
//! its own `struct bool` with `Drop`. The planner asks the analyzer, whose hover on a parameter
//! says whether its type has drop glue. The scripted hovers are the ones rust-analyzer gave on a
//! build node (`prod-code hover` on each parameter), and the last test asks a real one.

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

async /* the future owns both
of them */ fn noted(a: Guard, b: Guard) {
    println!(\"noted {} {}\", a.0, b.0);
}

pub(crate)
async
fn spread(a: Guard, b: Guard) {
    println!(\"spread {} {}\", a.0, b.0);
}

pub(crate) // async once
/* not async */ fn plain(a: Guard, b: Guard) {
    println!(\"plain {} {}\", a.0, b.0);
}

async fn counted(a: Guard, n: u32) {
    println!(\"counted {} {}\", a.0, n);
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
    drop(noted(Guard(\"A\"), Guard(\"B\")));
    drop(spread(Guard(\"A\"), Guard(\"B\")));
    drop(counted(Guard(\"A\"), 3));
    plain(Guard(\"A\"), Guard(\"B\"));
}
";

/// What `DROPS` prints: `B` before `A` at the end of every call, and the futures of `later`,
/// `noted` and `spread`, dropped before they are polled, dropping `A` before `B`; the future of
/// `counted` drops its only value.
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
drop A
drop B
drop A
drop B
drop A
plain A B
drop B
drop A
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

/// 1-based positions, and what the analyzer finds at each.
type At<T> = HashMap<(u32, u32), T>;

/// What rust-analyzer finds in a source written like `DROPS`: the references (a function is
/// called from `main`, and a parameter is used in its function's body), and the declaration of
/// each parameter, which a hover on its name shows.
fn tables(text: &str) -> (At<Vec<(u32, u32)>>, At<String>) {
    let main = text.find("fn main()").expect("a main");
    let mut table = HashMap::new();
    let mut declared = HashMap::new();
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
            declared.insert(spot(text, param), raw.to_string());
            param += raw.len() + 2;
        }
    }
    (table, declared)
}

/// How the scripted analyzer answers a hover on the parameter declared as `raw` (`n: u32`) in
/// the program `text`.
type Hover = fn(&str, &str) -> serde_json::Value;

/// The hover rust-analyzer gives a parameter: its declaration, then whether its type has drop
/// glue. A name means the program's own type where the program declares or imports one — a
/// `struct bool` with `Drop` gives `needs Drop` — and the builtin, with `no Drop`, where not.
fn analyzer(raw: &str, text: &str) -> serde_json::Value {
    let ty = raw.split_once(':').map_or("", |(_, t)| t);
    let declared = ty
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .any(|word| !word.is_empty() && text.contains(&format!("struct {word}(")));
    let glue = if declared { "needs Drop" } else { "no Drop" };
    answers::hover(&format!("```rust\n{raw}\n```\n\n---\n\n{glue}"))
}

/// No hover at all: a workspace the analyzer has not loaded.
fn silent(_: &str, _: &str) -> serde_json::Value {
    serde_json::Value::Null
}

fn crashed(_: &str, _: &str) -> serde_json::Value {
    answers::failure("rust-analyzer panicked")
}

/// What rust-analyzer says of a type it cannot resolve: `{unknown}`, without drop glue.
fn unresolved(raw: &str, _: &str) -> serde_json::Value {
    let name = raw.split(':').next().unwrap_or(raw);
    answers::hover(&format!(
        "```rust\n{name}: {{unknown}}\n```\n\n---\n\nno Drop"
    ))
}

/// A type parameter that has the primitive's name (`fn gap<u32>(…)`).
fn generic(raw: &str, _: &str) -> serde_json::Value {
    answers::hover(&format!(
        "```rust\n{raw}\n```\n\n---\n\ntype param may need Drop"
    ))
}

/// A hover that does not say, as from an analyzer configured without drop glue.
fn wordless(raw: &str, _: &str) -> serde_json::Value {
    answers::hover(&format!("```rust\n{raw}\n```"))
}

async fn gateway(file: &Path, text: &str, hover: Hover) -> SocketAddr {
    let (file, text) = (file.to_path_buf(), text.to_string());
    let (table, declared) = tables(&text);
    ScriptedGateway::start(move |method, params| {
        let at = |p: &str| params.pointer(p).and_then(|v| v.as_u64()).unwrap_or(0) as u32 + 1;
        let asked = (at("/position/line"), at("/position/character"));
        match method {
            "textDocument/references" => table
                .get(&asked)
                .map_or_else(|| serde_json::json!([]), |s| answers::locations(&file, s)),
            "textDocument/hover" => declared
                .get(&asked)
                .map_or(serde_json::Value::Null, |raw| hover(raw, &text)),
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => serde_json::Value::Null,
        }
    })
    .await
    .addr()
}

const CARGO_2021: &str = "[package]\nname = \"drops\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

fn workspace(edition: &str) -> Workspace {
    manifest(&format!(
        "[package]\nname = \"drops\"\nversion = \"0.1.0\"\nedition = \"{edition}\"\n"
    ))
}

/// A crate of `DROPS` whose manifest is `cargo_toml`.
fn manifest(cargo_toml: &str) -> Workspace {
    Workspace::new(&[("Cargo.toml", cargo_toml), ("src/main.rs", DROPS)])
}

/// A program at `rel` in a workspace, and how the scripted analyzer answers a hover in it.
struct Source<'a> {
    rel: &'a str,
    text: &'a str,
    hover: Hover,
}

const MAIN: Source<'static> = Source {
    rel: "src/main.rs",
    text: DROPS,
    hover: analyzer,
};

/// Bundles `params` of `function` in `src/main.rs` into `name`, bound as `binding`.
async fn bundle(
    ws: &Workspace,
    function: &str,
    params: &[&str],
    name: &str,
    apply: bool,
) -> anyhow::Result<prod_code_mcp::parameter_object::ParameterObject> {
    bundle_source(ws, &MAIN, function, params, name, apply).await
}

/// Bundles `params` of `function` in `rel`, a copy of `DROPS`, into `name`.
async fn bundle_in(
    ws: &Workspace,
    rel: &str,
    function: &str,
    params: &[&str],
    name: &str,
    apply: bool,
) -> anyhow::Result<prod_code_mcp::parameter_object::ParameterObject> {
    bundle_source(ws, &Source { rel, ..MAIN }, function, params, name, apply).await
}

/// Bundles `params` of `function` in `source` into `name`.
async fn bundle_source(
    ws: &Workspace,
    source: &Source<'_>,
    function: &str,
    params: &[&str],
    name: &str,
    apply: bool,
) -> anyhow::Result<prod_code_mcp::parameter_object::ParameterObject> {
    let file = ws.path(source.rel);
    let remote = gateway(&file, source.text, source.hover).await;
    let at = source
        .text
        .find(&format!("fn {function}("))
        .unwrap_or_else(|| panic!("`{function}` is declared"))
        + 3;
    let (line, col) = spot(source.text, at);
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
/// field, a borrow and a scalar bundled around an owned value, and a value bundled with a scalar
/// in an `async` function: each program prints what the
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
        // An `async` function's value and a scalar the analyzer finds no drop glue in: the
        // future drops one value either way.
        (
            "counted",
            &["a", "n"],
            "Pair",
            "drop(counted(Pair { a: Guard(\"A\"), n: 3 }));",
        ),
        // `async` only in comments: a function that runs at once, bundled like `take`.
        (
            "plain",
            &["a", "b"],
            "Pair",
            "pub(crate) // async once\n/* not async */ fn plain(pair: Pair) {",
        ),
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

    // `async` a line above `fn`, or apart from it by a comment, is as `async` as `later`.
    for function in ["noted", "spread"] {
        let err = bundle(&ws, function, &["a", "b"], "Pair", true)
            .await
            .expect_err("an `async` function written over several lines");
        let err = format!("{err:#}");
        assert!(
            err.contains(&format!(
                "`{function}` is `async`: a future dropped before it is polled drops the \
                 parameters that may have a destructor in the order `a`, `b`"
            )),
            "{function}: {err}"
        );
        assert!(err.contains("nothing was rewritten"), "{err}");
    }
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

/// The edition is the one Cargo reads from the manifest: text that only looks like an `edition`
/// key does not make a crate 2021, a workspace named by `package.workspace` outside the crate's
/// directories is where an inherited edition comes from, and a manifest Cargo could not read
/// refuses a closure rather than guess.
#[tokio::test]
async fn the_edition_is_the_one_cargo_reads_from_the_manifest() {
    // No `edition` key: Cargo builds it as 2015, whatever the description says.
    let quoted = manifest(
        "[package]\nname = \"drops\"\nversion = \"0.1.0\"\ndescription = \"\"\"\nedition = \
         \"2021\"\n[workspace.package]\nedition = \"2024\"\n\"\"\"\n\n[package.metadata.docs]\n\
         edition = \"2021\"\n",
    );
    let err = bundle(&quoted, "hold", &["a", "b"], "Pair", true)
        .await
        .expect_err("a crate of edition 2015 captures all of `pair`");
    let err = format!("{err:#}");
    assert!(
        err.contains("`hold` is in a crate of edition 2015, where a closure"),
        "{err}"
    );
    assert_eq!(quoted.read("src/main.rs"), DROPS, "nothing was written");

    // Inherited from a workspace that is not an ancestor, spelled as an inline table.
    let apart = Workspace::new(&[
        (
            "ws/Cargo.toml",
            "[workspace]\nmembers = [\"../drops\"]\n\n[workspace.package] # the members'\n\
             'edition' = \"2021\"\n",
        ),
        (
            "drops/Cargo.toml",
            "[package]\nname = \"drops\"\nversion = \"0.1.0\"\nworkspace = \"../ws\"\n\
             edition = { workspace = true } # from ../ws\n",
        ),
        ("drops/src/main.rs", DROPS),
    ]);
    let done = bundle_in(
        &apart,
        "drops/src/main.rs",
        "hold",
        &["a", "b"],
        "Pair",
        false,
    )
    .await
    .expect("the workspace's edition 2021 captures only `pair.a`");
    let text = main_rs(&done);
    assert!(text.contains("let g = pair.a;"), "{text}");
    assert_eq!(run(&text, "2021"), PRINTED, "{text}");

    // A manifest Cargo rejects: a closure is refused, a body without one is still bundled.
    let broken = manifest("[package]\nname = \"drops\"\nedition = \"2021\"\nedition = \"2021\"\n");
    let err = bundle(&broken, "hold", &["a", "b"], "Pair", true)
        .await
        .expect_err("an unreadable manifest has no edition to rely on");
    let err = format!("{err:#}");
    assert!(
        err.contains("cannot tell which edition the crate of `hold` is in"),
        "{err}"
    );
    assert!(err.contains("nothing was rewritten"), "{err}");
    assert_eq!(broken.read("src/main.rs"), DROPS, "nothing was written");
    let done = bundle(&broken, "take", &["a", "b"], "Pair", false)
        .await
        .expect("a body without a closure does not depend on the edition");
    let text = main_rs(&done);
    assert_eq!(run(&text, "2021"), PRINTED, "{text}");
}

/// A program may name its own type `bool` and give it `Drop` (#441), declared in the file or
/// imported: the spelling proves nothing, and the analyzer resolves the name. Two such
/// parameters are reversed in the struct like any owned pair, a builtin scalar between two of
/// them still bundles, and one of them between the bundled ones is refused.
#[tokio::test]
async fn a_type_of_the_program_named_bool_keeps_its_drop_order() {
    let declared = DROPS.replace("Guard", "bool");
    let imported = format!(
        "mod shadow {{\n    pub struct bool(pub &'static str);\n\n    impl Drop for bool {{\n        \
         fn drop(&mut self) {{\n            println!(\"drop {{}}\", self.0);\n        }}\n    \
         }}\n}}\n\nuse shadow::bool;\n\n{}",
        &declared[declared.find("fn take(").expect("a take")..]
    );
    for (how, text) in [
        ("declared", declared.as_str()),
        ("imported", imported.as_str()),
    ] {
        let ws = Workspace::new(&[("Cargo.toml", CARGO_2021), ("src/main.rs", text)]);
        let source = Source { text, ..MAIN };
        assert_eq!(run(text, "2021"), PRINTED, "{how}");

        let done = bundle_source(&ws, &source, "take", &["a", "b"], "Pair", false)
            .await
            .unwrap_or_else(|e| panic!("{how}: adjacent parameters are bundled: {e:#}"));
        let rewritten = main_rs(&done);
        assert_eq!(run(&rewritten, "2021"), PRINTED, "{how}: {rewritten}");
        assert!(
            rewritten.contains("pub struct Pair {\n    pub b: bool,\n    pub a: bool,\n}"),
            "{how}: {rewritten}"
        );

        let done = bundle_source(&ws, &source, "gap", &["a", "b"], "Pair", false)
            .await
            .unwrap_or_else(|e| panic!("{how}: the builtin `u32` has no destructor: {e:#}"));
        let rewritten = main_rs(&done);
        assert!(
            rewritten.contains("gap(Pair { a: bool(\"A\"), b: bool(\"B\") }, 1);"),
            "{how}: {rewritten}"
        );
        assert_eq!(run(&rewritten, "2021"), PRINTED, "{how}: {rewritten}");

        let err = bundle_source(&ws, &source, "apart", &["a", "b"], "Pair", true)
            .await
            .expect_err("`x` is dropped between `b` and `a`");
        let err = format!("{err:#}");
        assert!(
            err.contains(
                "`apart` drops the parameters that may have a destructor in the order `b`, `x`, \
                 `a`; with `a`, `b` in `Pair` it would drop them in the order `x`, `b`, `a`"
            ),
            "{how}: {err}"
        );
        assert!(
            err.contains("`x` is spelled `bool`, but the analyzer reports `needs Drop` for it"),
            "{how}: {err}"
        );
        assert!(err.contains("nothing was rewritten"), "{how}: {err}");
        assert_eq!(ws.read("src/main.rs"), text, "{how}: nothing was written");
    }
}

/// A primitive's name is taken for the builtin only on the analyzer's word. No hover, a failed
/// query, a type it cannot resolve (which it reports without drop glue), a type parameter of
/// that name, and a hover that does not say all leave the parameter as one that may have a
/// destructor: `n: u32` between two bundled values is refused, and so is bundling it with the
/// value of an `async` function — both of which bundle with the analyzer's `no Drop` above.
#[tokio::test]
async fn a_primitive_the_analyzer_does_not_vouch_for_may_have_a_destructor() {
    let ws = workspace("2021");
    let cases: &[(Hover, &str)] = &[
        (silent, "the analyzer gave no hover for it"),
        (crashed, "the hover query failed"),
        (
            unresolved,
            "the analyzer does not resolve its type (`{unknown}`)",
        ),
        (
            generic,
            "the analyzer reports `type param may need Drop` for it",
        ),
        (
            wordless,
            "the analyzer's hover does not say whether its type has drop glue",
        ),
    ];
    for (hover, why) in cases {
        let source = Source {
            hover: *hover,
            ..MAIN
        };
        let err = bundle_source(&ws, &source, "gap", &["a", "b"], "Pair", true)
            .await
            .expect_err(why);
        let err = format!("{err:#}");
        assert!(
            err.contains(
                "`gap` drops the parameters that may have a destructor in the order `b`, `n`, `a`"
            ),
            "{why}: {err}"
        );
        assert!(
            err.contains(&format!("`n` is spelled `u32`, but {why}")),
            "{err}"
        );

        let err = bundle_source(&ws, &source, "counted", &["a", "n"], "Pair", true)
            .await
            .expect_err(why);
        let err = format!("{err:#}");
        assert!(
            err.contains(
                "`counted` is `async`: a future dropped before it is polled drops the parameters \
                 that may have a destructor in the order `a`, `n`"
            ),
            "{why}: {err}"
        );
        assert!(err.contains("nothing was rewritten"), "{err}");
    }
    assert_eq!(ws.read("src/main.rs"), DROPS, "nothing was written");
}

/// One file where `bool` is the program's type with `Drop` at the top and in `imported`, and
/// the builtin in `builtin`, which only the analyzer can tell apart.
const LIVE: &str = r#"struct Guard(&'static str);

impl Drop for Guard {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

#[allow(non_camel_case_types)]
struct bool(&'static str);

impl Drop for bool {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn take(a: bool, b: bool) {
    println!("take {} {}", a.0, b.0);
}

fn apart(a: Guard, x: bool, b: Guard) {
    println!("apart {} {} {}", a.0, x.0, b.0);
}

mod imported {
    use super::bool;

    fn pass(a: bool, b: bool) {
        println!("pass {} {}", a.0, b.0);
    }

    pub fn run() {
        pass(bool("A"), bool("B"));
    }
}

mod builtin {
    use super::Guard;

    fn gap(a: Guard, on: bool, b: Guard) {
        println!("gap {} {} {}", a.0, on, b.0);
    }

    pub fn run() {
        gap(Guard("A"), true, Guard("B"));
    }
}

fn main() {
    take(bool("A"), bool("B"));
    apart(Guard("A"), bool("X"), Guard("B"));
    imported::run();
    builtin::run();
}
"#;

const LIVE_PRINTED: &str = "take A B
drop B
drop A
apart A X B
drop B
drop X
drop A
pass A B
drop B
drop A
gap A true B
drop B
drop A
";

/// Bundles `a` and `b` of `function` in `LIVE` against the gateway at `addr`, without writing.
async fn live_bundle(
    addr: SocketAddr,
    root: &Path,
    function: &str,
) -> anyhow::Result<prod_code_mcp::parameter_object::ParameterObject> {
    let at = LIVE.find(&format!("fn {function}(")).expect("declared") + 3;
    let (line, col) = spot(LIVE, at);
    prod_code_mcp::parameter_object::introduce(
        addr,
        root,
        &root.join("src/main.rs"),
        line,
        col,
        &["a".to_string(), "b".to_string()],
        "Pair",
        "pair",
        false,
        false,
    )
    .await
}

/// `LIVE` against a real gateway and its rust-analyzer: the same spelling `bool` is the
/// program's type with `Drop` in two places and the builtin in a third, and each bundle compiles
/// and prints what the original did. It runs when `PROD_CODE_LIVE_GATEWAY` holds the address of
/// a gateway built from this checkout, and is skipped, saying so, everywhere else.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_real_rust_analyzer_tells_the_programs_bool_from_the_builtin() {
    let Some(addr) = std::env::var("PROD_CODE_LIVE_GATEWAY")
        .ok()
        .and_then(|a| a.parse::<SocketAddr>().ok())
    else {
        eprintln!("skipping: PROD_CODE_LIVE_GATEWAY names no gateway to run against");
        return;
    };
    assert_eq!(run(LIVE, "2021"), LIVE_PRINTED);
    // Not a dot-directory: some tools pass over hidden ones.
    let dir = tempfile::Builder::new()
        .prefix("po-drop-live-")
        .tempdir()
        .expect("checkout dir");
    let root = std::fs::canonicalize(dir.path()).expect("canonical root");
    for (rel, text) in [("Cargo.toml", CARGO_2021), ("src/main.rs", LIVE)] {
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

    // Until the analyzer has loaded the crate it gives no hover, and `on: bool` is refused as a
    // type that may have a destructor; a preview costs nothing, so it is repeated until then.
    let mut gap = Err(anyhow::anyhow!("not asked"));
    for _ in 0..60 {
        gap = live_bundle(addr, &root, "gap").await;
        if gap
            .as_ref()
            .is_ok_and(|d| d.call_sites == 1 && d.diagnostics.is_empty())
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
    }
    let gap = gap.unwrap_or_else(|e| panic!("the builtin `bool` has no destructor: {e:#}"));
    let text = main_rs(&gap);
    eprintln!("{text}");
    assert!(
        text.contains("pub struct Pair {\n    pub b: Guard,\n    pub a: Guard,\n}"),
        "{text}"
    );
    assert!(
        text.contains("gap(Pair { a: Guard(\"A\"), b: Guard(\"B\") }, true);"),
        "{text}"
    );
    assert_eq!(run(&text, "2021"), LIVE_PRINTED, "{text}");

    for function in ["take", "pass"] {
        let done = live_bundle(addr, &root, function)
            .await
            .unwrap_or_else(|e| panic!("`{function}` is bundled: {e:#}"));
        assert_eq!(done.call_sites, 1, "{function}");
        let text = main_rs(&done);
        eprintln!("{text}");
        assert!(
            text.contains("pub struct Pair {\n    pub b: bool,\n    pub a: bool,\n}"),
            "{function}: {text}"
        );
        assert_eq!(run(&text, "2021"), LIVE_PRINTED, "{function}: {text}");
    }

    let err = live_bundle(addr, &root, "apart")
        .await
        .expect_err("the program's `bool` is dropped between `b` and `a`");
    let err = format!("{err:#}");
    eprintln!("{err}");
    assert!(
        err.contains("`x` is spelled `bool`, but the analyzer reports `needs Drop` for it"),
        "{err}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("src/main.rs")).expect("read"),
        LIVE,
        "nothing was written"
    );
}
