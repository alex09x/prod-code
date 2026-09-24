//! `extract_parameter` on TypeScript, JavaScript, Python and Go, against a scripted gateway.
//!
//! Every answer here is what the real server said about the same file: the TypeScript server
//! (`tsc --lsp`), basedpyright and gopls were asked for `documentSymbol`, `hover`, `references`
//! and `diagnostic` on these fixtures, and the answers were copied, positions included. They
//! differ in exactly the places the extraction has to care about: gopls names a method
//! `(*Store).Limit` while its callers write `Limit`, basedpyright ends a function's range on its
//! last statement rather than on a closing brace and lists an import among the references, and
//! every server answers a hover on a literal with nothing.

use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// One `documentSymbol` node in the server's own 0-based coordinates: the full range as
/// (start line, start character, end line, end character) and where the name starts.
fn node(name: &str, kind: u32, range: [u32; 4], name_at: [u32; 2], children: Vec<Value>) -> Value {
    json!({
        "name": name,
        "kind": kind,
        "range": {
            "start": { "line": range[0], "character": range[1] },
            "end": { "line": range[2], "character": range[3] }
        },
        "selectionRange": {
            "start": { "line": name_at[0], "character": name_at[1] },
            "end": { "line": name_at[0], "character": name_at[1] + name.rsplit('.').next().unwrap_or(name).len() as u32 }
        },
        "children": children
    })
}

/// A local binding, which is never a candidate for the parameter.
fn local(name: &str, line: u32, character: u32) -> Value {
    node(
        name,
        13,
        [line, character, line, character + name.len() as u32],
        [line, character],
        Vec::new(),
    )
}

/// References in several files, 1-based, as one answer.
fn locations_in(files: &[(&PathBuf, &[(u32, u32)])]) -> Value {
    Value::Array(
        files
            .iter()
            .flat_map(|(path, spots)| {
                answers::locations(path, spots)
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            })
            .collect(),
    )
}

/// A hover whose range is the `len` characters from the asked position, as every server
/// answers for an identifier.
fn hover_over(params: &Value, len: u32, markdown: &str) -> Value {
    let line = params
        .pointer("/position/line")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let character = params
        .pointer("/position/character")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    json!({
        "contents": { "kind": "markdown", "value": markdown },
        "range": {
            "start": { "line": line, "character": character },
            "end": { "line": line, "character": character + len as u64 }
        }
    })
}

/// Error diagnostics at 1-based (line, column, message), with the server's code and source.
fn errors(items: &[(u32, u32, String)], code: Value, source: &str) -> Value {
    let items: Vec<Value> = items
        .iter()
        .map(|(line, col, message)| {
            json!({
                "code": code,
                "message": message,
                "severity": 1,
                "source": source,
                "range": {
                    "start": { "line": line - 1, "character": col - 1 },
                    "end": { "line": line - 1, "character": col }
                }
            })
        })
        .collect();
    json!({ "kind": "full", "items": items })
}

fn uri_ends_with(params: &Value, suffix: &str) -> bool {
    params
        .pointer("/textDocument/uri")
        .and_then(Value::as_str)
        .is_some_and(|u| u.ends_with(suffix))
}

fn rewritten(done: &prod_code_mcp::extract_parameter::ExtractedParameter, file: &str) -> String {
    done.rewritten
        .iter()
        .find(|(p, _)| Path::new(p).ends_with(file))
        .map(|(_, t)| t.clone())
        .unwrap_or_else(|| panic!("{file} was not rewritten: {:?}", done.rewritten))
}

// ---------------------------------------------------------------------------------------------
// TypeScript

const TS_HOME: &str = "export function render(text: string): string {\n  const width = 80;\n  const n = text.length;\n  return text.padEnd(width + n);\n}\n\nexport class Store {\n  entries: number[] = [];\n\n  limit(): number {\n    const cap = 64 * 1024;\n    return Math.min(cap, this.entries.length);\n  }\n}\n\nexport function caller(): string {\n  return render(\"x\");\n}\n";
const TS_OTHER: &str = "import { render, Store } from \"./home\";\n\nexport function use(): number {\n  const s = new Store();\n  return render(\"y\").length + s.limit();\n}\n";

/// The TypeScript server's outline of `TS_HOME`: the method is a child of the class.
fn ts_home_symbols() -> Value {
    json!([
        node(
            "render",
            12,
            [0, 0, 4, 1],
            [0, 16],
            vec![local("width", 1, 8), local("n", 2, 8)]
        ),
        node(
            "Store",
            5,
            [6, 0, 13, 1],
            [6, 13],
            vec![
                node("entries", 7, [7, 2, 7, 25], [7, 2], Vec::new()),
                node(
                    "limit",
                    6,
                    [9, 2, 12, 3],
                    [9, 2],
                    vec![local("cap", 10, 10)]
                ),
            ]
        ),
        node("caller", 12, [15, 0, 17, 1], [15, 16], Vec::new()),
    ])
}

fn ts_workspace() -> (Workspace, PathBuf, PathBuf) {
    let ws = Workspace::new(&[
        (
            "package.json",
            "{ \"name\": \"t\", \"version\": \"1.0.0\", \"private\": true }\n",
        ),
        (
            "tsconfig.json",
            "{ \"compilerOptions\": { \"strict\": true }, \"include\": [\"src\"] }\n",
        ),
        ("src/home.ts", TS_HOME),
        ("src/other.ts", TS_OTHER),
    ]);
    let (home, other) = (ws.path("src/home.ts"), ws.path("src/other.ts"));
    (ws, home, other)
}

/// A literal in a TypeScript function becomes a `number` parameter, and the callers in both
/// files pass it. No hover is needed: the server answers nothing on a literal.
#[tokio::test]
async fn typescript_a_literal_becomes_a_typed_parameter_every_caller_passes() {
    let (ws, home, other) = ts_workspace();
    let (h, o) = (home.clone(), other.clone());
    let gateway = ScriptedGateway::start(move |method, _params| match method {
        "textDocument/documentSymbol" => ts_home_symbols(),
        "textDocument/references" => locations_in(&[(&h, &[(17, 10)]), (&o, &[(5, 10)])]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;

    let done = prod_code_mcp::extract_parameter::extract(
        gateway.addr(),
        &ws.root(),
        &home,
        (2, 17),
        (2, 19),
        "pad",
        None,
        false,
        false,
        false,
    )
    .await
    .unwrap_or_else(|e| panic!("the extraction runs: {e:#}"));

    assert_eq!(done.symbol, "render");
    assert_eq!(done.ty, "number");
    assert_eq!(done.call_sites, 2);
    assert!(done.unmatched.is_empty(), "{:?}", done.unmatched);
    let home_text = rewritten(&done, "src/home.ts");
    assert!(
        home_text.contains("export function render(text: string, pad: number): string {"),
        "{home_text}"
    );
    assert!(home_text.contains("  const width = pad;"), "{home_text}");
    assert!(
        home_text.contains("return render(\"x\", 80);"),
        "{home_text}"
    );
    let other_text = rewritten(&done, "src/other.ts");
    assert!(
        other_text.contains("return render(\"y\", 80).length + s.limit();"),
        "{other_text}"
    );
    assert!(
        other_text.starts_with("import { render, Store } from \"./home\";"),
        "{other_text}"
    );
    assert!(
        done.render(4000).contains("new parameter: `pad: number`"),
        "{}",
        done.render(4000)
    );
    assert_eq!(
        ws.read("src/home.ts"),
        TS_HOME,
        "nothing written without apply"
    );
}

const TS_FRAME: &str = "const WIDTH = 80;\n\nexport function frame(text: string): string {\n  const left = WIDTH;\n  return text.padStart(left).padEnd(WIDTH);\n}\n\nexport function caller(): string {\n  return frame(\"x\");\n}\n";

/// `replace_all` and `apply`: both reads of a module constant become the parameter, its type
/// comes from the hover (`const WIDTH: 80`, widened to `number`), and the file changes on disk.
#[tokio::test]
async fn typescript_replace_all_takes_the_type_from_the_hover_and_apply_writes_it() {
    let ws = Workspace::new(&[("src/frame.ts", TS_FRAME)]);
    let frame = ws.path("src/frame.ts");
    let f = frame.clone();
    let gateway = ScriptedGateway::start(move |method, params| match method {
        "textDocument/documentSymbol" => json!([
            node("WIDTH", 14, [0, 6, 0, 16], [0, 6], Vec::new()),
            node(
                "frame",
                12,
                [2, 0, 5, 1],
                [2, 16],
                vec![local("left", 3, 8)]
            ),
            node("caller", 12, [7, 0, 9, 1], [7, 16], Vec::new()),
        ]),
        "textDocument/hover" => hover_over(params, 5, "```typescript\nconst WIDTH: 80\n```\n"),
        "textDocument/references" => answers::locations(&f, &[(9, 10)]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;

    let done = prod_code_mcp::extract_parameter::extract(
        gateway.addr(),
        &ws.root(),
        &frame,
        (4, 16),
        (4, 21),
        "width",
        None,
        true,
        true,
        false,
    )
    .await
    .unwrap_or_else(|e| panic!("the extraction runs: {e:#}"));

    assert_eq!(done.ty, "number", "the literal type is widened");
    assert_eq!(done.replaced, 2);
    assert!(done.applied);
    let on_disk = ws.read("src/frame.ts");
    assert!(
        on_disk.contains("export function frame(text: string, width: number): string {"),
        "{on_disk}"
    );
    assert!(on_disk.contains("  const left = width;"), "{on_disk}");
    assert!(on_disk.contains(".padEnd(width);"), "{on_disk}");
    assert!(on_disk.contains("return frame(\"x\", WIDTH);"), "{on_disk}");
    assert!(on_disk.starts_with("const WIDTH = 80;"), "{on_disk}");
}

/// `width + n` names two locals of `render`; at the call sites neither exists, the server says
/// so, and the extraction is reported and refused as in Rust.
#[tokio::test]
async fn typescript_an_expression_naming_a_local_is_refused_with_the_reason() {
    let (ws, home, other) = ts_workspace();
    let (h, o) = (home.clone(), other.clone());
    let gateway = ScriptedGateway::start(move |method, params| match method {
        "textDocument/documentSymbol" => ts_home_symbols(),
        "textDocument/references" => locations_in(&[(&h, &[(17, 10)]), (&o, &[(5, 10)])]),
        "textDocument/diagnostic" => {
            let (line, col) = if uri_ends_with(params, "home.ts") {
                (17, 22)
            } else {
                (5, 22)
            };
            errors(
                &[
                    (line, col, "Cannot find name 'width'.".to_string()),
                    (line, col + 8, "Cannot find name 'n'.".to_string()),
                ],
                json!(2304),
                "ts",
            )
        }
        _ => Value::Null,
    })
    .await;

    let root = ws.root();
    let run = |apply| {
        prod_code_mcp::extract_parameter::extract(
            gateway.addr(),
            &root,
            &home,
            (4, 22),
            (4, 31),
            "pad",
            Some("number"),
            false,
            apply,
            false,
        )
    };
    let done = run(false).await.expect("the extraction runs");
    assert_eq!(done.expression, "width + n");
    assert!(!done.diagnostics.is_empty(), "{:?}", done.diagnostics);
    assert!(
        done.diagnostics
            .iter()
            .any(|d| d.contains("Cannot find name 'n'.")),
        "{:?}",
        done.diagnostics
    );
    let report = done.render(4000);
    assert!(report.contains("if it names a local"), "{report}");

    let err = run(true)
        .await
        .expect_err("apply refuses what does not compile");
    assert!(
        format!("{err:#}").contains("nothing was written"),
        "{err:#}"
    );
    assert_eq!(ws.read("src/home.ts"), TS_HOME);
}

/// A class method: the parameter goes after the method's own list, and `s.limit()` in the other
/// file passes the argument. The server answers nothing on the literal `64`, so without a type
/// the extraction asks for one rather than writing an untyped TypeScript parameter.
#[tokio::test]
async fn typescript_a_class_method_takes_the_parameter_and_its_callers_pass_it() {
    let (ws, home, other) = ts_workspace();
    let o = other.clone();
    let gateway = ScriptedGateway::start(move |method, _params| match method {
        "textDocument/documentSymbol" => ts_home_symbols(),
        "textDocument/references" => answers::locations(&o, &[(5, 33)]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;

    let root = ws.root();
    let run = |ty| {
        prod_code_mcp::extract_parameter::extract(
            gateway.addr(),
            &root,
            &home,
            (11, 17),
            (11, 26),
            "cap_bytes",
            ty,
            false,
            false,
            false,
        )
    };
    let err = run(None).await.expect_err("no type, and none to be had");
    assert!(
        format!("{err:#}").contains("pass the type explicitly"),
        "{err:#}"
    );

    let done = run(Some("number")).await.expect("the extraction runs");
    assert_eq!(done.symbol, "limit");
    assert_eq!(done.call_sites, 1);
    let home_text = rewritten(&done, "src/home.ts");
    assert!(
        home_text.contains("  limit(cap_bytes: number): number {"),
        "{home_text}"
    );
    assert!(
        home_text.contains("    const cap = cap_bytes;"),
        "{home_text}"
    );
    let other_text = rewritten(&done, "src/other.ts");
    assert!(other_text.contains("s.limit(64 * 1024);"), "{other_text}");
}

// ---------------------------------------------------------------------------------------------
// JavaScript

/// JavaScript has no annotations: the parameter is a bare name even when a type is given, and
/// the callers pass the literal as in every other language.
#[tokio::test]
async fn javascript_the_parameter_has_no_type() {
    let ws = Workspace::new(&[
        (
            "src/home.js",
            "export function render(text) {\n  const width = 80;\n  return text.padEnd(width);\n}\n\nexport function caller() {\n  return render(\"x\");\n}\n",
        ),
        (
            "src/other.js",
            "import { render } from \"./home.js\";\n\nexport const y = render(\"y\");\n",
        ),
    ]);
    let (home, other) = (ws.path("src/home.js"), ws.path("src/other.js"));
    let (h, o) = (home.clone(), other.clone());
    let gateway = ScriptedGateway::start(move |method, _params| match method {
        "textDocument/documentSymbol" => json!([
            node(
                "render",
                12,
                [0, 0, 3, 1],
                [0, 16],
                vec![local("width", 1, 8)]
            ),
            node("caller", 12, [5, 0, 7, 1], [5, 16], Vec::new()),
        ]),
        "textDocument/references" => locations_in(&[(&h, &[(7, 10)]), (&o, &[(3, 18)])]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;

    let done = prod_code_mcp::extract_parameter::extract(
        gateway.addr(),
        &ws.root(),
        &home,
        (2, 17),
        (2, 19),
        "pad",
        Some("number"),
        false,
        false,
        false,
    )
    .await
    .expect("the extraction runs");

    assert_eq!(done.ty, "");
    assert_eq!(done.call_sites, 2);
    let home_text = rewritten(&done, "src/home.js");
    assert!(
        home_text.contains("export function render(text, pad) {"),
        "{home_text}"
    );
    assert!(
        home_text.contains("return render(\"x\", 80);"),
        "{home_text}"
    );
    let other_text = rewritten(&done, "src/other.js");
    assert!(
        other_text.contains("export const y = render(\"y\", 80);"),
        "{other_text}"
    );
    assert!(done.render(4000).contains("new parameter: `pad`"));
}

// ---------------------------------------------------------------------------------------------
// Python

const PY_HOME: &str = "def render(text: str) -> str:\n    width = 80\n    n = len(text)\n    return text.ljust(width + n)\n\n\nclass Store:\n    def __init__(self) -> None:\n        self.entries: list[int] = []\n\n    def limit(self) -> int:\n        cap = 64 * 1024\n        return min(cap, len(self.entries))\n\n\ndef caller() -> str:\n    return render(\"x\")\n";
const PY_OTHER: &str = "from pkg.home import Store, render\n\n\ndef use() -> int:\n    s = Store()\n    return len(render(\"y\")) + s.limit()\n";

/// basedpyright's outline of `PY_HOME`: a function's range ends at the end of its last
/// statement, and a method is a child of its class.
fn py_home_symbols() -> Value {
    json!([
        node(
            "render",
            12,
            [0, 0, 3, 32],
            [0, 4],
            vec![
                node("text", 13, [0, 11, 0, 20], [0, 11], Vec::new()),
                local("width", 1, 4),
                local("n", 2, 4),
            ]
        ),
        node(
            "Store",
            5,
            [6, 0, 12, 42],
            [6, 6],
            vec![
                node("__init__", 6, [7, 4, 8, 36], [7, 8], Vec::new()),
                node(
                    "limit",
                    6,
                    [10, 4, 12, 42],
                    [10, 8],
                    vec![local("cap", 11, 8)]
                ),
                local("entries", 8, 13),
            ]
        ),
        node("caller", 12, [15, 0, 16, 22], [15, 4], Vec::new()),
    ])
}

fn py_workspace() -> (Workspace, PathBuf, PathBuf) {
    let ws = Workspace::new(&[
        (
            "pyproject.toml",
            "[project]\nname = \"t\"\nversion = \"0.1.0\"\n",
        ),
        ("pkg/__init__.py", ""),
        ("pkg/home.py", PY_HOME),
        ("pkg/other.py", PY_OTHER),
    ]);
    let (home, other) = (ws.path("pkg/home.py"), ws.path("pkg/other.py"));
    (ws, home, other)
}

/// A Python function with callers in two files. basedpyright lists the `from … import` line
/// among the references; it is not a call and needs no argument, so it is neither rewritten
/// nor reported as one that was missed.
#[tokio::test]
async fn python_a_literal_becomes_a_parameter_and_the_import_is_left_alone() {
    let (ws, home, other) = py_workspace();
    let (h, o) = (home.clone(), other.clone());
    let gateway = ScriptedGateway::start(move |method, _params| match method {
        "textDocument/documentSymbol" => py_home_symbols(),
        "textDocument/references" => locations_in(&[(&h, &[(17, 12)]), (&o, &[(1, 29), (6, 16)])]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;

    let done = prod_code_mcp::extract_parameter::extract(
        gateway.addr(),
        &ws.root(),
        &home,
        (2, 13),
        (2, 15),
        "pad",
        None,
        false,
        false,
        false,
    )
    .await
    .unwrap_or_else(|e| panic!("the extraction runs: {e:#}"));

    assert_eq!(done.ty, "int");
    assert_eq!(done.call_sites, 2);
    assert!(
        done.unmatched.is_empty(),
        "the import is not a missed call: {:?}",
        done.unmatched
    );
    let home_text = rewritten(&done, "pkg/home.py");
    assert!(
        home_text.contains("def render(text: str, pad: int) -> str:"),
        "{home_text}"
    );
    assert!(home_text.contains("    width = pad\n"), "{home_text}");
    assert!(
        home_text.contains("return render(\"x\", 80)"),
        "{home_text}"
    );
    let other_text = rewritten(&done, "pkg/other.py");
    assert!(
        other_text.starts_with("from pkg.home import Store, render\n"),
        "{other_text}"
    );
    assert!(
        other_text.contains("len(render(\"y\", 80))"),
        "{other_text}"
    );
}

const PY_FRAME: &str = "WIDTH = 80\n\n\ndef frame(text: str) -> str:\n    left = WIDTH\n    return text.rjust(left).ljust(WIDTH)\n\n\ndef caller() -> str:\n    return frame(\"x\")\n";

/// `replace_all` in Python reaches the function's last line: basedpyright ends the range on the
/// last statement, which is body, not a closing brace. The type is the hover's
/// `Literal[80]`, widened to `int`.
#[tokio::test]
async fn python_replace_all_reaches_the_last_statement_and_apply_writes_it() {
    let ws = Workspace::new(&[("pkg/frame.py", PY_FRAME)]);
    let frame = ws.path("pkg/frame.py");
    let f = frame.clone();
    let gateway = ScriptedGateway::start(move |method, params| match method {
        "textDocument/documentSymbol" => json!([
            node("WIDTH", 14, [0, 0, 0, 5], [0, 0], Vec::new()),
            node(
                "frame",
                12,
                [3, 0, 5, 40],
                [3, 4],
                vec![
                    node("text", 13, [3, 10, 3, 19], [3, 10], Vec::new()),
                    local("left", 4, 4)
                ]
            ),
            node("caller", 12, [8, 0, 9, 21], [8, 4], Vec::new()),
        ]),
        "textDocument/hover" => {
            hover_over(params, 5, "```python\n(constant) WIDTH: Literal[80]\n```")
        }
        "textDocument/references" => answers::locations(&f, &[(10, 12)]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;

    let done = prod_code_mcp::extract_parameter::extract(
        gateway.addr(),
        &ws.root(),
        &frame,
        (5, 12),
        (5, 17),
        "width",
        None,
        true,
        true,
        false,
    )
    .await
    .unwrap_or_else(|e| panic!("the extraction runs: {e:#}"));

    assert_eq!(done.ty, "int");
    assert_eq!(done.replaced, 2, "the read on the last line is body too");
    let on_disk = ws.read("pkg/frame.py");
    assert!(
        on_disk.contains("def frame(text: str, width: int) -> str:"),
        "{on_disk}"
    );
    assert!(on_disk.contains("    left = width\n"), "{on_disk}");
    assert!(on_disk.contains(".ljust(width)\n"), "{on_disk}");
    assert!(on_disk.contains("return frame(\"x\", WIDTH)"), "{on_disk}");
}

/// `width + n` names two locals. The hover on `width` covers only `width`, so its type is not
/// the expression's and the parameter is written untyped; the checker then finds neither name
/// at the call sites, and the extraction is refused with the reason.
#[tokio::test]
async fn python_an_expression_naming_a_local_is_refused_with_the_reason() {
    let (ws, home, other) = py_workspace();
    let (h, o) = (home.clone(), other.clone());
    let gateway = ScriptedGateway::start(move |method, params| match method {
        "textDocument/documentSymbol" => py_home_symbols(),
        "textDocument/hover" => {
            hover_over(params, 5, "```python\n(variable) width: Literal[80]\n```")
        }
        "textDocument/references" => locations_in(&[(&h, &[(17, 12)]), (&o, &[(1, 29), (6, 16)])]),
        "textDocument/diagnostic" => {
            let (line, col) = if uri_ends_with(params, "home.py") {
                (17, 24)
            } else {
                (6, 28)
            };
            errors(
                &[
                    (line, col, "\"width\" is not defined".to_string()),
                    (line, col + 8, "\"n\" is not defined".to_string()),
                ],
                json!("reportUndefinedVariable"),
                "basedpyright",
            )
        }
        _ => Value::Null,
    })
    .await;

    let root = ws.root();
    let run = |apply| {
        prod_code_mcp::extract_parameter::extract(
            gateway.addr(),
            &root,
            &home,
            (4, 23),
            (4, 32),
            "pad",
            None,
            false,
            apply,
            false,
        )
    };
    let done = run(false).await.expect("the extraction runs");
    assert_eq!(done.expression, "width + n");
    assert_eq!(
        done.parameter, "pad",
        "the hover on `width` is not the sum's type"
    );
    assert!(
        done.diagnostics
            .iter()
            .any(|d| d.contains("\"n\" is not defined")),
        "{:?}",
        done.diagnostics
    );
    assert!(done.render(4000).contains("if it names a local"));

    let err = run(true)
        .await
        .expect_err("apply refuses what does not compile");
    assert!(
        format!("{err:#}").contains("nothing was written"),
        "{err:#}"
    );
    assert_eq!(ws.read("pkg/home.py"), PY_HOME);
}

/// A method with `self`: the parameter goes after `self`, the caller keeps its receiver and
/// passes the expression. No hover types a literal product, so the parameter is untyped.
#[tokio::test]
async fn python_a_method_with_self_takes_the_parameter_last() {
    let (ws, home, other) = py_workspace();
    let o = other.clone();
    let gateway = ScriptedGateway::start(move |method, _params| match method {
        "textDocument/documentSymbol" => py_home_symbols(),
        "textDocument/references" => answers::locations(&o, &[(6, 33)]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;

    let done = prod_code_mcp::extract_parameter::extract(
        gateway.addr(),
        &ws.root(),
        &home,
        (12, 15),
        (12, 24),
        "cap_bytes",
        None,
        false,
        false,
        false,
    )
    .await
    .unwrap_or_else(|e| panic!("the extraction runs: {e:#}"));

    assert_eq!(done.symbol, "limit");
    assert_eq!(done.call_sites, 1);
    let home_text = rewritten(&done, "pkg/home.py");
    assert!(
        home_text.contains("    def limit(self, cap_bytes) -> int:"),
        "{home_text}"
    );
    assert!(
        home_text.contains("        cap = cap_bytes\n"),
        "{home_text}"
    );
    let other_text = rewritten(&done, "pkg/other.py");
    assert!(other_text.contains("s.limit(64 * 1024)"), "{other_text}");
}

// ---------------------------------------------------------------------------------------------
// Go

const GO_HOME: &str = "package shop\n\nimport \"strings\"\n\nfunc Render(text string) string {\n\twidth := 80\n\tn := len(text)\n\treturn text + strings.Repeat(\" \", width+n)\n}\n\ntype Store struct {\n\tentries []int\n}\n\nfunc (s *Store) Limit() int {\n\tlimit := 64 * 1024\n\treturn min(limit, len(s.entries))\n}\n\nfunc Caller() string {\n\treturn Render(\"x\")\n}\n";
const GO_OTHER: &str = "package shop\n\nfunc Use() int {\n\ts := &Store{}\n\treturn len(Render(\"y\")) + s.Limit()\n}\n";

/// gopls's outline of `GO_HOME`: flat, with the method named after its receiver.
fn go_home_symbols() -> Value {
    json!([
        node("Render", 12, [4, 0, 8, 1], [4, 5], Vec::new()),
        node(
            "Store",
            23,
            [10, 5, 12, 1],
            [10, 5],
            vec![node("entries", 8, [11, 1, 11, 14], [11, 1], Vec::new())]
        ),
        node("(*Store).Limit", 6, [14, 0, 17, 1], [14, 16], Vec::new()),
        node("Caller", 12, [19, 0, 21, 1], [19, 5], Vec::new()),
    ])
}

fn go_workspace() -> (Workspace, PathBuf, PathBuf) {
    let ws = Workspace::new(&[
        ("go.mod", "module example.com/t\n\ngo 1.22\n"),
        ("shop/home.go", GO_HOME),
        ("shop/other.go", GO_OTHER),
    ]);
    let (home, other) = (ws.path("shop/home.go"), ws.path("shop/other.go"));
    (ws, home, other)
}

/// A Go function with callers in two files: the parameter is spelled `name type`.
#[tokio::test]
async fn go_a_literal_becomes_a_parameter_every_caller_passes() {
    let (ws, home, other) = go_workspace();
    let (h, o) = (home.clone(), other.clone());
    let gateway = ScriptedGateway::start(move |method, _params| match method {
        "textDocument/documentSymbol" => go_home_symbols(),
        "textDocument/references" => locations_in(&[(&h, &[(21, 9)]), (&o, &[(5, 13)])]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;

    let done = prod_code_mcp::extract_parameter::extract(
        gateway.addr(),
        &ws.root(),
        &home,
        (6, 11),
        (6, 13),
        "pad",
        None,
        false,
        false,
        false,
    )
    .await
    .unwrap_or_else(|e| panic!("the extraction runs: {e:#}"));

    assert_eq!(done.ty, "int");
    assert_eq!(done.call_sites, 2);
    let home_text = rewritten(&done, "shop/home.go");
    assert!(
        home_text.contains("func Render(text string, pad int) string {"),
        "{home_text}"
    );
    assert!(home_text.contains("\twidth := pad\n"), "{home_text}");
    assert!(
        home_text.contains("return Render(\"x\", 80)"),
        "{home_text}"
    );
    let other_text = rewritten(&done, "shop/other.go");
    assert!(
        other_text.contains("len(Render(\"y\", 80))"),
        "{other_text}"
    );
    assert!(done.render(4000).contains("new parameter: `pad int`"));
}

const GO_FRAME: &str = "package shop\n\nconst Base = 80\n\nfunc Frame(text string) int {\n\tleft := Base\n\treturn left + Base + len(text)\n}\n\nfunc UseFrame() int {\n\treturn Frame(\"x\")\n}\n";

/// `replace_all` in Go, typed by gopls's hover on an untyped constant: `untyped int` is `int`.
#[tokio::test]
async fn go_replace_all_takes_an_untyped_constants_default_type() {
    let ws = Workspace::new(&[
        ("go.mod", "module example.com/t\n\ngo 1.22\n"),
        ("shop/frame.go", GO_FRAME),
    ]);
    let frame = ws.path("shop/frame.go");
    let f = frame.clone();
    let gateway = ScriptedGateway::start(move |method, params| match method {
        "textDocument/documentSymbol" => json!([
            node("Base", 14, [2, 6, 2, 15], [2, 6], Vec::new()),
            node("Frame", 12, [4, 0, 7, 1], [4, 5], Vec::new()),
            node("UseFrame", 12, [9, 0, 11, 1], [9, 5], Vec::new()),
        ]),
        "textDocument/hover" => hover_over(
            params,
            4,
            "```go\nconst Base untyped int = 80\n```\n\n---\n\n[`shop.Base` on pkg.go.dev](https://pkg.go.dev/example.com/t/shop#Base)",
        ),
        "textDocument/references" => answers::locations(&f, &[(11, 9)]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;

    let done = prod_code_mcp::extract_parameter::extract(
        gateway.addr(),
        &ws.root(),
        &frame,
        (6, 10),
        (6, 14),
        "base",
        None,
        true,
        true,
        false,
    )
    .await
    .unwrap_or_else(|e| panic!("the extraction runs: {e:#}"));

    assert_eq!(done.ty, "int");
    assert_eq!(done.replaced, 2);
    let on_disk = ws.read("shop/frame.go");
    assert!(
        on_disk.contains("func Frame(text string, base int) int {"),
        "{on_disk}"
    );
    assert!(on_disk.contains("\tleft := base\n"), "{on_disk}");
    assert!(
        on_disk.contains("return left + base + len(text)"),
        "{on_disk}"
    );
    assert!(on_disk.contains("return Frame(\"x\", Base)"), "{on_disk}");
}

/// `width+n` names two locals; gopls reports them undefined at the call sites.
#[tokio::test]
async fn go_an_expression_naming_a_local_is_refused_with_the_reason() {
    let (ws, home, other) = go_workspace();
    let (h, o) = (home.clone(), other.clone());
    let gateway = ScriptedGateway::start(move |method, params| match method {
        "textDocument/documentSymbol" => go_home_symbols(),
        "textDocument/references" => locations_in(&[(&h, &[(21, 9)]), (&o, &[(5, 13)])]),
        "textDocument/diagnostic" => {
            let (line, col) = if uri_ends_with(params, "home.go") {
                (21, 21)
            } else {
                (5, 25)
            };
            errors(
                &[
                    (line, col, "undefined: width".to_string()),
                    (line, col + 6, "undefined: n".to_string()),
                ],
                json!("UndeclaredName"),
                "compiler",
            )
        }
        _ => Value::Null,
    })
    .await;

    let root = ws.root();
    let run = |apply| {
        prod_code_mcp::extract_parameter::extract(
            gateway.addr(),
            &root,
            &home,
            (8, 36),
            (8, 43),
            "pad",
            Some("int"),
            false,
            apply,
            false,
        )
    };
    let done = run(false).await.expect("the extraction runs");
    assert_eq!(done.expression, "width+n");
    assert!(
        done.diagnostics.iter().any(|d| d.contains("undefined: n")),
        "{:?}",
        done.diagnostics
    );
    assert!(done.render(4000).contains("if it names a local"));

    let err = run(true)
        .await
        .expect_err("apply refuses what does not compile");
    assert!(
        format!("{err:#}").contains("nothing was written"),
        "{err:#}"
    );
    assert_eq!(ws.read("shop/home.go"), GO_HOME);
}

/// A method with a receiver. gopls calls it `(*Store).Limit` and selects only `Limit`; the
/// declaration is found there, not in the receiver, and the call `s.Limit()` is matched by the
/// bare name.
#[tokio::test]
async fn go_a_method_with_a_receiver_takes_the_parameter() {
    let (ws, home, other) = go_workspace();
    let o = other.clone();
    let gateway = ScriptedGateway::start(move |method, _params| match method {
        "textDocument/documentSymbol" => go_home_symbols(),
        "textDocument/references" => answers::locations(&o, &[(5, 30)]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;

    let done = prod_code_mcp::extract_parameter::extract(
        gateway.addr(),
        &ws.root(),
        &home,
        (16, 11),
        (16, 20),
        "capacity",
        Some("int"),
        false,
        false,
        false,
    )
    .await
    .unwrap_or_else(|e| panic!("the extraction runs: {e:#}"));

    assert_eq!(done.symbol, "(*Store).Limit");
    assert_eq!(done.call_sites, 1);
    assert!(done.unmatched.is_empty(), "{:?}", done.unmatched);
    let home_text = rewritten(&done, "shop/home.go");
    assert!(
        home_text.contains("func (s *Store) Limit(capacity int) int {"),
        "{home_text}"
    );
    assert!(home_text.contains("\tlimit := capacity\n"), "{home_text}");
    let other_text = rewritten(&done, "shop/other.go");
    assert!(other_text.contains("s.Limit(64 * 1024)"), "{other_text}");
}
