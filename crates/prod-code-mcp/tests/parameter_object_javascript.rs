//! Bundling parameters in JavaScript and JSX, driven end to end against a scripted gateway.
//!
//! Every scripted answer here is one the TypeScript server gave on the same files (a
//! `jsconfig.json` project of `.js` and `.jsx` sources), asked with `prod-code refs` through a
//! Linux build node. Three of its answers shaped the code. It lists the call `make(…)` of a
//! function imported as `build as make` among the references to `build`, at `make`, so a call
//! may spell the function by another name. It lists the shorthand property of `{ name, width }`
//! as a use of `width`, where a bare `size.width` would not parse — and on JavaScript it reports
//! exactly that syntax error (`',' expected.`), which is what the check before writing catches.
//! And it hovers a JavaScript parameter as `any`, which is why no hover is asked for.
//!
//! The last test runs the same fixture against a real gateway when `PROD_CODE_LIVE_GATEWAY`
//! names one, and says it was skipped when it does not.

use prod_code_mcp::protocol::McpContentItem;
use prod_code_testkit::{Answer, ScriptedGateway, Workspace, answers};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

async fn scripted_gateway(answer: Answer) -> SocketAddr {
    ScriptedGateway::start_arc(answer).await.addr()
}

/// The file, 0-based line and 0-based character a request asks about.
fn position(params: &serde_json::Value) -> (String, u64, u64) {
    let uri = params
        .pointer("/textDocument/uri")
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();
    let at = |p: &str| params.pointer(p).and_then(|v| v.as_u64()).unwrap_or(0);
    (uri, at("/position/line"), at("/position/character"))
}

/// References at 1-based positions spread over several files.
fn spread(spots: &[(&Path, u32, u32)]) -> serde_json::Value {
    serde_json::Value::Array(
        spots
            .iter()
            .flat_map(|(path, line, col)| {
                answers::locations(path, &[(*line, *col)])
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            })
            .collect(),
    )
}

fn rewritten(done: &prod_code_mcp::parameter_object::ParameterObject, rel: &str) -> String {
    done.rewritten
        .iter()
        .find(|(p, _)| p.ends_with(rel))
        .map(|(_, t)| t.clone())
        .unwrap_or_else(|| panic!("{rel} was not rewritten"))
}

#[allow(clippy::too_many_arguments)]
async fn introduce(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    params: &[&str],
    binding: &str,
    apply: bool,
) -> anyhow::Result<prod_code_mcp::parameter_object::ParameterObject> {
    let params: Vec<String> = params.iter().map(|p| p.to_string()).collect();
    prod_code_mcp::parameter_object::introduce(
        remote, root, file, line, col, &params, "Size", binding, apply, false,
    )
    .await
}

fn text_of(result: &prod_code_mcp::protocol::McpToolCallResult) -> String {
    result
        .content
        .iter()
        .map(|c| match c {
            McpContentItem::Text { text } => text.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

const HOME: &str = "function describe(shape) {
  return `${shape.name} ${shape.area}`;
}

export function build(name, width, height = 2) {
  const area = width * height;
  return describe({ name, area, width });
}

export class Canvas {
  constructor() {
    this.scale = 1;
  }

  draw(label, x, y) {
    return `${label} ${x * this.scale} ${y}`;
  }
}

export function twice(name) {
  return build(name, 1) + build.call(null, name, 3, 4);
}
";

const OTHER: &str = "import { build as make, Canvas } from \"./home.js\";

export function callIt() {
  const c = new Canvas();
  return make(\"a\", 3, 4) + c.draw(\"b\", 5, 6);
}

export const asValue = make;
";

const VIEW: &str = "import { build } from \"./home.js\";

export function Label({ title }) {
  return <p title={title}>{build(title, 7)}</p>;
}
";

/// `build` with `width` and `height` bundled: the declaration, the body with a shorthand
/// property, and every caller — in the same file, through `call`, under an import alias, and
/// inside a JSX expression.
const HOME_BUNDLED: &str = "function describe(shape) {
  return `${shape.name} ${shape.area}`;
}

export function build(name, size) {
  const area = size.width * size.height;
  return describe({ name, area, width: size.width });
}

export class Canvas {
  constructor() {
    this.scale = 1;
  }

  draw(label, x, y) {
    return `${label} ${x * this.scale} ${y}`;
  }
}

export function twice(name) {
  return build(name, { width: 1, height: 2 }) + build.call(null, name, { width: 3, height: 4 });
}
";

const FILES: [(&str, &str); 5] = [
    (
        "package.json",
        "{ \"name\": \"po-js\", \"version\": \"1.0.0\", \"private\": true, \"type\": \"module\" }\n",
    ),
    (
        "jsconfig.json",
        "{ \"compilerOptions\": { \"target\": \"es2020\", \"module\": \"esnext\", \"jsx\": \"preserve\" }, \"include\": [\"src\"] }\n",
    ),
    ("src/home.js", HOME),
    ("src/other.js", OTHER),
    ("src/view.jsx", VIEW),
];

struct Fixture {
    ws: Workspace,
    home: PathBuf,
    remote: SocketAddr,
    /// Whether the server reports an error in the rewritten `home.js`.
    broken: Arc<AtomicBool>,
}

async fn fixture() -> Fixture {
    let ws = Workspace::new(&FILES);
    let (home, other, view) = (
        ws.path("src/home.js"),
        ws.path("src/other.js"),
        ws.path("src/view.jsx"),
    );
    let broken = Arc::new(AtomicBool::new(false));
    let (h, o, v, b) = (home.clone(), other, view, broken.clone());
    let remote = scripted_gateway(Arc::new(move |method, params| {
        let (uri, line, ch) = position(params);
        match method {
            "textDocument/references" if uri.ends_with("home.js") => match (line, ch) {
                // `build`: the renamed import, two calls here, the aliased call and the alias
                // as a value in other.js, and the call in the JSX. Not the plain import.
                (4, 16) => spread(&[
                    (&o, 1, 10),
                    (&h, 21, 10),
                    (&h, 21, 27),
                    (&o, 5, 10),
                    (&o, 8, 24),
                    (&v, 4, 28),
                ]),
                (4, 28) => answers::locations(&h, &[(6, 16), (7, 33)]),
                (4, 35) => answers::locations(&h, &[(6, 24)]),
                // `draw`, then its `x` and `y`.
                (14, 2) => answers::locations(&o, &[(5, 30)]),
                (14, 14) => answers::locations(&h, &[(16, 24)]),
                (14, 17) => answers::locations(&h, &[(16, 42)]),
                _ => serde_json::json!([]),
            },
            "textDocument/references" => serde_json::json!([]),
            "textDocument/diagnostic" if uri.ends_with("home.js") && b.load(Ordering::SeqCst) => {
                answers::error_at(7, 37, "1005", "',' expected.")
            }
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => serde_json::Value::Null,
        }
    }))
    .await;
    Fixture {
        ws,
        home,
        remote,
        broken,
    }
}

/// A JavaScript function takes a plain object: no interface, no annotation, no class and no
/// import anywhere; the body reads its fields, a shorthand property keeps its key, a caller
/// that left out the defaulted parameter passes the default, `call` keeps its receiver, the
/// aliased call and the one in JSX are rewritten, and the alias used as a value is reported.
#[tokio::test]
async fn a_javascript_function_takes_a_plain_object_and_every_caller_passes_a_literal() {
    let f = fixture().await;
    let root = f.ws.root();
    let done = introduce(f.remote, &root, &f.home, 5, 17, &["width", "height"], "size", false)
        .await
        .expect("bundling runs");

    assert_eq!(done.symbol, "build");
    assert_eq!(done.was, "name, width, height = 2");
    assert_eq!(done.now, "name, size");
    assert_eq!(done.call_sites, 4);
    assert_eq!(done.body_uses, 3);
    assert_eq!(done.unmatched.len(), 1, "{:?}", done.unmatched);
    assert!(
        done.unmatched[0].ends_with("src/other.js:8:24"),
        "only the alias as a value: {:?}",
        done.unmatched
    );
    assert!(done.unreported.is_empty(), "{:?}", done.unreported);
    assert_eq!(rewritten(&done, "home.js"), HOME_BUNDLED);
    let other = rewritten(&done, "other.js");
    assert_eq!(
        other,
        OTHER.replace("make(\"a\", 3, 4)", "make(\"a\", { width: 3, height: 4 })"),
        "the aliased call is rewritten and the import is left as it was"
    );
    assert_eq!(
        rewritten(&done, "view.jsx"),
        VIEW.replace("build(title, 7)", "build(title, { width: 7, height: 2 })")
    );
    for (path, text) in &done.rewritten {
        for typed in ["interface", "Size", ": number", "@typedef", "import type"] {
            assert!(!text.contains(typed), "{path} has `{typed}`:\n{text}");
        }
    }
    assert!(done.imports.is_empty(), "{:?}", done.imports);
    assert_eq!(
        done.struct_text,
        "// Size: the plain object `build` takes; nothing is declared for it\n{ width, height = 2 }\n"
    );
    let report = done.render(8000);
    assert!(report.contains("```javascript\n// Size"), "{report}");
    assert!(report.contains("the function passed as a value"), "{report}");
    assert!(
        report.contains("0 errors (in JavaScript that is the syntax"),
        "{report}"
    );
    assert!(!done.applied);
    assert_eq!(f.ws.read("src/home.js"), HOME, "nothing was written");
}

/// A class method keeps `this`: its body still reads `this.scale`, the fields come through the
/// binding inside template literals, the call through an instance passes the literal, and no
/// declaration appears above the class.
#[tokio::test]
async fn a_javascript_method_keeps_its_receiver_and_gets_no_declaration() {
    let f = fixture().await;
    let root = f.ws.root();
    let done = introduce(f.remote, &root, &f.home, 15, 3, &["y", "x"], "point", false)
        .await
        .expect("bundling runs");

    assert_eq!(done.symbol, "draw");
    assert_eq!(done.now, "label, point");
    assert_eq!(done.call_sites, 1);
    assert_eq!(done.body_uses, 2);
    assert!(done.unmatched.is_empty(), "{:?}", done.unmatched);
    let home = rewritten(&done, "home.js");
    assert_eq!(
        home,
        HOME.replace(
            "  draw(label, x, y) {\n    return `${label} ${x * this.scale} ${y}`;",
            "  draw(label, point) {\n    return `${label} ${point.x * this.scale} ${point.y}`;"
        )
    );
    assert!(
        rewritten(&done, "other.js").contains("c.draw(\"b\", { x: 5, y: 6 })"),
        "{done:?}"
    );
}

/// Nothing is written until the server has seen every rewritten file together: an error in one
/// of them refuses the whole change and leaves the checkout as it was, and a clean one is
/// written to every file at once.
#[tokio::test]
async fn a_javascript_change_is_written_only_when_the_server_accepts_all_of_it() {
    let f = fixture().await;
    let root = f.ws.root();
    f.broken.store(true, Ordering::SeqCst);
    let err = introduce(f.remote, &root, &f.home, 5, 17, &["width", "height"], "size", true)
        .await
        .expect_err("apply refuses a result the server rejects");
    let err = format!("{err:#}");
    assert!(err.contains("nothing was written"), "{err}");
    assert!(err.contains("',' expected."), "{err}");
    for (rel, text) in FILES {
        assert_eq!(f.ws.read(rel), text, "{rel} was left alone");
    }

    f.broken.store(false, Ordering::SeqCst);
    let done = introduce(f.remote, &root, &f.home, 5, 17, &["width", "height"], "size", true)
        .await
        .expect("a clean result is applied");
    assert!(done.applied);
    assert_eq!(f.ws.read("src/home.js"), HOME_BUNDLED);
    assert!(
        f.ws.read("src/other.js")
            .contains("make(\"a\", { width: 3, height: 4 })")
    );
    assert!(
        f.ws.read("src/view.jsx")
            .contains("{build(title, { width: 7, height: 2 })}")
    );
}

/// The MCP tool, as an agent calls it: the binding defaults to the name in lowerCamelCase, and
/// the report says what the server's verdict means for JavaScript.
#[tokio::test]
async fn the_tool_bundles_javascript_with_a_lower_camel_binding() {
    let f = fixture().await;
    let root = f.ws.root();
    let result = prod_code_mcp::tools::execute_tool(
        f.remote,
        &root,
        "code_introduce_parameter_object",
        serde_json::json!({
            "path": "src/home.js",
            "line": 5,
            "character": 17,
            "params": ["width", "height"],
            "name": "Size",
        }),
    )
    .await
    .expect("the tool runs");
    let text = text_of(&result);
    assert!(text.contains("- now: (name, size)"), "{text}");
    assert!(text.contains("4 call site(s) rewritten, 3 use(s)"), "{text}");
    assert!(text.contains("```javascript"), "{text}");
    assert!(text.contains("nothing was written"), "{text}");
    assert_eq!(f.ws.read("src/home.js"), HOME);
}

const REFUSALS: &str = "export function gather(first, ...more) {
  return [first, ...more];
}

export function place({ x, y }, z) {
  return x + y + z;
}

export function stamp(label, at = Date.now(), zone = \"utc\") {
  return `${label} ${at} ${zone}`;
}

export function sum(a, b) {
  return a + b + arguments.length;
}

export function scale(value, num, den) {
  const ratio = num / den;
  return value * ratio;
}

export function span(start, step, end = start + 1) {
  return [start, step, end];
}

export function pair(a, b) {
  return a + b;
}

export function join(a, b) {
  return `${a}${b}`;
}

export function pad(text, width, fill = \" \") {
  return text.padStart(width, fill);
}

export function callers(xs, s, n, ch) {
  return [pair(...xs), join.apply(null, xs), pad(s, n, ch), pad(s, 4)];
}
";

/// Each case that cannot be shown to mean the same afterwards is refused with its reason, and
/// nothing is written: a rest parameter, a name a destructuring pattern binds, a default that is
/// not a constant, `arguments`, a binding the function already uses, a parameter another one's
/// default reads, a spread argument, `apply`, and a variable passed where a default applied.
#[tokio::test]
async fn what_javascript_cannot_prove_the_same_is_refused_with_its_reason() {
    let ws = Workspace::new(&[
        FILES[0],
        FILES[1],
        ("src/refusals.js", REFUSALS),
    ]);
    let file = ws.path("src/refusals.js");
    let r = file.clone();
    let remote = scripted_gateway(Arc::new(move |method, params| {
        let (_, line, ch) = position(params);
        match method {
            "textDocument/references" => match (line, ch) {
                (12, 20) => answers::locations(&r, &[(14, 10)]),
                (12, 23) => answers::locations(&r, &[(14, 14)]),
                (16, 29) => answers::locations(&r, &[(18, 17)]),
                (16, 34) => answers::locations(&r, &[(18, 23)]),
                (21, 21) => answers::locations(&r, &[(22, 41), (23, 11)]),
                (21, 28) => answers::locations(&r, &[(23, 18)]),
                (25, 16) => answers::locations(&r, &[(39, 11)]),
                (25, 21) => answers::locations(&r, &[(27, 10)]),
                (25, 24) => answers::locations(&r, &[(27, 14)]),
                (29, 16) => answers::locations(&r, &[(39, 24)]),
                (29, 21) => answers::locations(&r, &[(31, 13)]),
                (29, 24) => answers::locations(&r, &[(31, 17)]),
                (33, 16) => answers::locations(&r, &[(39, 46), (39, 61)]),
                (33, 26) => answers::locations(&r, &[(35, 24)]),
                (33, 33) => answers::locations(&r, &[(35, 31)]),
                _ => serde_json::json!([]),
            },
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => serde_json::Value::Null,
        }
    }))
    .await;
    let root = ws.root();
    let cases: [(u32, &[&str], &str, &str); 9] = [
        (
            1,
            &["first", "more"],
            "opts",
            "`more` takes a variable number of arguments",
        ),
        (
            5,
            &["x", "z"],
            "opts",
            "`x` is bound by the destructuring pattern `{ x, y }` of `place`",
        ),
        (
            9,
            &["at", "zone"],
            "opts",
            "`at` defaults to `Date.now()`, which is evaluated in `stamp` on every call",
        ),
        (13, &["a", "b"], "opts", "`sum` reads `arguments`"),
        (
            17,
            &["num", "den"],
            "ratio",
            "`ratio` is already a name in `scale`",
        ),
        (
            22,
            &["start", "step"],
            "opts",
            "`start` is read at src/refusals.js:22:41, outside the body of `span`",
        ),
        (
            26,
            &["a", "b"],
            "opts",
            "`pair` is called with `...xs` at src/refusals.js:39:11",
        ),
        (
            30,
            &["a", "b"],
            "opts",
            "`join` is called through `apply` at src/refusals.js:39:24",
        ),
        (
            34,
            &["width", "fill"],
            "opts",
            "`pad` at src/refusals.js:39:46 passes `ch` as `fill`, which defaults to `\" \"`",
        ),
    ];
    for (line, params, binding, reason) in cases {
        let err = match introduce(remote, &root, &file, line, 17, params, binding, true).await {
            Ok(done) => panic!("line {line} was bundled: {}", done.render(4000)),
            Err(err) => format!("{err:#}"),
        };
        assert!(err.contains(reason), "line {line}: {err}");
    }
    assert_eq!(ws.read("src/refusals.js"), REFUSALS, "nothing was written");
}

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

/// A gateway that answers a reference request at a 1-based position from `table`, and finds
/// no error in any file.
async fn table_gateway(
    table: impl Fn((u32, u32)) -> Option<serde_json::Value> + Send + Sync + 'static,
) -> SocketAddr {
    scripted_gateway(Arc::new(move |method, params| {
        let (_, line, ch) = position(params);
        match method {
            "textDocument/references" => table((line as u32 + 1, ch as u32 + 1))
                .unwrap_or_else(|| serde_json::json!([])),
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => serde_json::Value::Null,
        }
    }))
    .await
}

/// Runs `text` as a script with `program`, and returns what it printed; `None` when the program
/// is not installed where the test runs.
fn run_script(program: &str, name: &str, text: &str) -> Option<String> {
    let dir = tempfile::tempdir().expect("script dir");
    let path = dir.path().join(name);
    std::fs::write(&path, text).expect("write the script");
    let out = std::process::Command::new(program).arg(&path).output().ok()?;
    assert!(
        out.status.success(),
        "{program} {name} failed: {}\n{text}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

const ORDER: &str = "const seen = [];
function mark(v) {
  seen.push(v);
  return v;
}

export function place(a, b, c) {
  return [a, b, c].join(\"\");
}

export function run() {
  return place(mark(\"a\"), mark(\"b\"), mark(\"c\"));
}
";

/// What would make the rewrite a different program, or one written from a partial answer, stops
/// it before anything is written (#436): bundled arguments that would be evaluated in another
/// order, a reference the file no longer has at the analyzer's position — for the function and
/// for a parameter, since no type checker would notice either in JavaScript — and a reference
/// query that fails, for the function and for a parameter.
#[tokio::test]
async fn javascript_refuses_reordering_stale_references_and_failed_queries() {
    let ws = Workspace::new(&[FILES[0], FILES[1], ("src/order.js", ORDER)]);
    let file = ws.path("src/order.js");
    let mode = Arc::new(std::sync::Mutex::new(""));
    let (f, m) = (file.clone(), mode.clone());
    let decl = spot(ORDER, "place(a", 0);
    let param = |skip: usize| spot(ORDER, "place(a", skip);
    let remote = table_gateway(move |at| {
        let mode = *m.lock().expect("mode");
        let refs = |spots: &[(u32, u32)]| Some(answers::locations(&f, spots));
        match at {
            _ if at == decl && mode == "callee fails" => Some(answers::failure("server crashed")),
            _ if at == decl && mode == "callee stale" => refs(&[spot(ORDER, "seen", 0)]),
            _ if at == decl => refs(&[spot(ORDER, "place(mark", 0)]),
            _ if at == param(6) && mode == "param fails" => {
                Some(answers::failure("server crashed"))
            }
            _ if at == param(6) && mode == "param stale" => refs(&[spot(ORDER, "[a, b", 0)]),
            _ if at == param(6) => refs(&[spot(ORDER, "a, b, c]", 0)]),
            _ if at == param(9) => refs(&[spot(ORDER, "b, c]", 0)]),
            _ if at == param(12) => refs(&[spot(ORDER, "c]", 0)]),
            _ => None,
        }
    })
    .await;
    let root = ws.root();
    let cases: [(&str, &[&str], &str); 5] = [
        (
            "",
            &["a", "c"],
            "`place` at src/order.js:12:10 passes `mark(\"b\")` between the bundled arguments; \
             in the object `mark(\"c\")` would be evaluated before it",
        ),
        (
            "callee stale",
            &["a", "b"],
            "the analyzer places `place` at src/order.js:1:7, but the file says otherwise",
        ),
        (
            "param stale",
            &["a", "b"],
            "the analyzer places a use of `a` at src/order.js:8:10, but the file says otherwise",
        ),
        (
            "callee fails",
            &["a", "b"],
            "the references to `place` at src/order.js:7:17 could not be listed, so nothing was \
             rewritten",
        ),
        (
            "param fails",
            &["a", "b"],
            "the references to `a` at src/order.js:7:23 could not be listed",
        ),
    ];
    for (case, params, reason) in cases {
        *mode.lock().expect("mode") = case;
        let err = match introduce(remote, &root, &file, decl.0, decl.1, params, "opts", true).await
        {
            Ok(done) => panic!("{case:?} was bundled: {}", done.render(4000)),
            Err(err) => format!("{err:#}"),
        };
        assert!(err.contains(reason), "{case:?}: {err}");
    }
    assert_eq!(ws.read("src/order.js"), ORDER, "nothing was written");

    // Adjacent, the same arguments are evaluated in the same order, and the change is made.
    *mode.lock().expect("mode") = "";
    let done = introduce(remote, &root, &file, decl.0, decl.1, &["b", "c"], "opts", false)
        .await
        .expect("adjacent arguments are bundled");
    assert!(
        rewritten(&done, "order.js")
            .contains("place(mark(\"a\"), { b: mark(\"b\"), c: mark(\"c\") })"),
        "{done:?}"
    );
}

const RUN: &str = "\"use strict\";
const seen = [];
function mark(v) {
  seen.push(v);
  return v;
}

function place(a, b, c) {
  return [a, b, c].join(\"\");
}

function tag(__proto__, toString, fill = 0) {
  const own = { __proto__ };
  return [Object.getPrototypeOf(own) === Object.prototype, own.__proto__, typeof toString, fill].join(\" \");
}

const x = \"x\";
const y = \"y\";
console.log(place(1, mark(\"b\"), \"c\"), place(x, y, x));
console.log(tag(\"p\"), tag(\"q\", void 0, 5), tag(\"r\", 1, void 0));
console.log(seen.join(\",\"));
";

/// What JavaScript does with the result, run with Node where the test runs: arguments that trade
/// places only where nothing can tell, a `__proto__` field that stays a field (a computed key in
/// the literal and in the body's shorthand), a left-out `toString` that is `undefined` and not
/// the one every object inherits, and `void 0` standing for a default. The rewritten script
/// prints what the original printed.
#[tokio::test]
async fn rewritten_javascript_prints_what_the_original_printed() {
    let ws = Workspace::new(&[FILES[0], FILES[1], ("src/run.js", RUN)]);
    let file = ws.path("src/run.js");
    let f = file.clone();
    let place = spot(RUN, "place(a", 0);
    let tag = spot(RUN, "tag(__proto__", 0);
    let decl = |at: (u32, u32), skip: u32| (at.0, at.1 + skip);
    let remote = table_gateway(move |at| {
        let spots: Vec<(u32, u32)> = match at {
            _ if at == place => vec![spot(RUN, "place(1", 0), spot(RUN, "place(x", 0)],
            _ if at == decl(place, 6) => vec![spot(RUN, "a, b, c]", 0)],
            _ if at == decl(place, 9) => vec![spot(RUN, "b, c]", 0)],
            _ if at == decl(place, 12) => vec![spot(RUN, "c]", 0)],
            _ if at == tag => vec![
                spot(RUN, "tag(\"p\"", 0),
                spot(RUN, "tag(\"q\"", 0),
                spot(RUN, "tag(\"r\"", 0),
            ],
            _ if at == decl(tag, 4) => vec![spot(RUN, "__proto__ }", 0)],
            _ if at == decl(tag, 15) => vec![spot(RUN, "toString, fill]", 0)],
            _ if at == decl(tag, 25) => vec![spot(RUN, "fill].", 0)],
            _ => return None,
        };
        Some(answers::locations(&f, &spots))
    })
    .await;
    let root = ws.root();

    let placed = introduce(remote, &root, &file, place.0, place.1, &["a", "c"], "opts", false)
        .await
        .expect("literals and plain names may trade places");
    let placed = rewritten(&placed, "run.js");
    assert!(
        placed.contains("function place(opts, b) {\n  return [opts.a, b, opts.c].join(\"\");"),
        "{placed}"
    );
    assert!(
        placed.contains("place({ a: 1, c: \"c\" }, mark(\"b\")), place({ a: x, c: x }, y)"),
        "{placed}"
    );

    let tagged = introduce(
        remote,
        &root,
        &file,
        tag.0,
        tag.1,
        &["__proto__", "toString", "fill"],
        "opts",
        false,
    )
    .await
    .expect("a `__proto__` parameter is bundled");
    let tagged = rewritten(&tagged, "run.js");
    assert!(
        tagged.contains("const own = { [\"__proto__\"]: opts.__proto__ };"),
        "{tagged}"
    );
    assert!(tagged.contains("typeof opts.toString, opts.fill]"), "{tagged}");
    assert!(
        tagged.contains(
            "tag({ [\"__proto__\"]: \"p\", toString: void 0, fill: 0 }), \
             tag({ [\"__proto__\"]: \"q\", toString: void 0, fill: 5 }), \
             tag({ [\"__proto__\"]: \"r\", toString: 1, fill: 0 })"
        ),
        "{tagged}"
    );
    assert_eq!(ws.read("src/run.js"), RUN, "a preview writes nothing");

    let Some(before) = run_script("node", "run.js", RUN) else {
        eprintln!("skipping the run: `node` is not installed here");
        return;
    };
    assert_eq!(
        before,
        "1bc xyx\ntrue p undefined 0 true q undefined 5 true r number 0\nb\n"
    );
    for text in [&placed, &tagged] {
        assert_eq!(
            run_script("node", "run.js", text).as_deref(),
            Some(before.as_str()),
            "{text}"
        );
    }
}

/// The same fixture against a real gateway and its TypeScript server, applied to disk. It runs
/// when `PROD_CODE_LIVE_GATEWAY` holds the gateway's address — a build node, with the server
/// built from this checkout — and is skipped, saying so, everywhere else.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_real_typescript_server_bundles_the_javascript_fixture() {
    let Some(addr) = std::env::var("PROD_CODE_LIVE_GATEWAY")
        .ok()
        .and_then(|a| a.parse::<SocketAddr>().ok())
    else {
        eprintln!("skipping: PROD_CODE_LIVE_GATEWAY names no gateway to run against");
        return;
    };
    // Not a dot-directory: some tools pass over hidden ones.
    let dir = tempfile::Builder::new()
        .prefix("po-js-live-")
        .tempdir()
        .expect("checkout dir");
    let root = std::fs::canonicalize(dir.path()).expect("canonical root");
    for (rel, text) in FILES {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(path, text).expect("write");
    }
    let git = |args: &[&str]| {
        std::process::Command::new("git")
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

    let args = |apply: bool| {
        serde_json::json!({
            "path": "src/home.js",
            "line": 5,
            "character": 17,
            "params": ["width", "height"],
            "name": "Size",
            "apply": apply,
        })
    };
    // A server still reading a new project answers with fewer references; a preview costs
    // nothing, so it is repeated until the answer settles.
    let mut preview = String::new();
    for _ in 0..20 {
        let result = prod_code_mcp::tools::execute_tool(
            addr,
            &root,
            "code_introduce_parameter_object",
            args(false),
        )
        .await
        .expect("the preview runs");
        preview = text_of(&result);
        if preview.contains("4 call site(s)") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    eprintln!("{preview}");
    assert!(preview.contains("4 call site(s) rewritten, 3 use(s)"), "{preview}");
    assert!(preview.contains("src/other.js:8:24"), "{preview}");
    assert!(preview.contains("0 errors"), "{preview}");

    let applied = text_of(
        &prod_code_mcp::tools::execute_tool(
            addr,
            &root,
            "code_introduce_parameter_object",
            args(true),
        )
        .await
        .expect("the change applies"),
    );
    assert!(applied.contains("[applied to 3 file(s)]"), "{applied}");
    let read = |rel: &str| std::fs::read_to_string(root.join(rel)).expect("read");
    assert_eq!(read("src/home.js"), HOME_BUNDLED);
    assert_eq!(
        read("src/other.js"),
        OTHER.replace("make(\"a\", 3, 4)", "make(\"a\", { width: 3, height: 4 })")
    );
    assert_eq!(
        read("src/view.jsx"),
        VIEW.replace("build(title, 7)", "build(title, { width: 7, height: 2 })")
    );
}
