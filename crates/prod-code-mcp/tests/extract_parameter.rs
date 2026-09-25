//! `extract_parameter` on TypeScript, JavaScript, Python, Go, C, C++ and Swift, against a
//! scripted gateway.
//!
//! Every answer here is what the real server said about the same file: the TypeScript server
//! (`tsc --lsp`), basedpyright, gopls, clangd and sourcekit-lsp were asked for `documentSymbol`,
//! `hover`, `references` (and clangd for `declaration`) on these fixtures, and the answers were
//! copied, positions included. They differ in exactly the places the extraction has to care
//! about: gopls names a method `(*Store).Limit` while its callers write `Limit`, clangd names an
//! out-of-line method `Store::limit` and leaves the header's declaration out of the references,
//! sourcekit-lsp names a function `render(text:)` and answers a hover without a range,
//! basedpyright ends a function's range on its last statement rather than on a closing brace
//! and lists an import among the references, and every server answers a hover on a number with
//! nothing.

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

/// A `documentSymbol` node whose `selectionRange` is given whole, 0-based. clangd selects only
/// `limit` in `Store::limit`, and sourcekit-lsp selects `render(text: String)` for the symbol it
/// names `render(text:)`, so the name's length says nothing about either.
fn span_node(
    name: &str,
    kind: u32,
    range: [u32; 4],
    selection: [u32; 4],
    children: Vec<Value>,
) -> Value {
    let span = |r: [u32; 4]| {
        json!({
            "start": { "line": r[0], "character": r[1] },
            "end": { "line": r[2], "character": r[3] }
        })
    };
    json!({
        "name": name,
        "kind": kind,
        "range": span(range),
        "selectionRange": span(selection),
        "children": children
    })
}

// ---------------------------------------------------------------------------------------------
// C

const C_HOME_H: &str = "#ifndef HOME_H\n#define HOME_H\n\nint render(const char *text);\ndouble scale(int n);\nint caller(void);\n\n#endif\n";
const C_HOME: &str = "#include \"home.h\"\n\n#include <string.h>\n\nstatic const int WIDTH = 80;\n\nint render(const char *text) {\n    int width = 80;\n    int n = (int)strlen(text);\n    return width + n + WIDTH;\n}\n\ndouble scale(int n) {\n    const char *label = \"x\";\n    return n * 0.5 + (double)strlen(label);\n}\n\nint caller(void) {\n    return render(\"x\");\n}\n";
const C_OTHER: &str = "#include \"home.h\"\n\nint use(void) {\n    return render(\"y\") + (int)scale(2) + caller();\n}\n";

/// clangd's outline of `C_HOME`: flat, with no locals.
fn c_home_symbols() -> Value {
    json!([
        span_node("WIDTH", 13, [4, 0, 4, 27], [4, 17, 4, 22], Vec::new()),
        span_node("render", 12, [6, 0, 10, 1], [6, 4, 6, 10], Vec::new()),
        span_node("scale", 12, [12, 0, 15, 1], [12, 7, 12, 12], Vec::new()),
        span_node("caller", 12, [17, 0, 19, 1], [17, 4, 17, 10], Vec::new()),
    ])
}

fn c_workspace() -> (Workspace, PathBuf, PathBuf, PathBuf) {
    let ws = Workspace::new(&[
        (
            "CMakeLists.txt",
            "cmake_minimum_required(VERSION 3.16)\nproject(xp C)\nset(CMAKE_EXPORT_COMPILE_COMMANDS ON)\nadd_library(shop src/home.c src/other.c)\ntarget_include_directories(shop PUBLIC src)\n",
        ),
        ("src/home.h", C_HOME_H),
        ("src/home.c", C_HOME),
        ("src/other.c", C_OTHER),
    ]);
    let (header, home, other) = (
        ws.path("src/home.h"),
        ws.path("src/home.c"),
        ws.path("src/other.c"),
    );
    (ws, header, home, other)
}

/// A literal in a C function becomes an `int` parameter, the callers in both files pass it,
/// and the declaration in the header takes the parameter too. clangd leaves that declaration
/// out of `references`; `declaration` is what finds it.
#[tokio::test]
async fn c_a_literal_becomes_an_int_parameter_and_the_header_declares_it() {
    let (ws, header, home, other) = c_workspace();
    let (hd, h, o) = (header.clone(), home.clone(), other.clone());
    let gateway = ScriptedGateway::start(move |method, _params| match method {
        "textDocument/documentSymbol" => c_home_symbols(),
        "textDocument/references" => locations_in(&[(&h, &[(19, 12)]), (&o, &[(4, 12)])]),
        "textDocument/declaration" => answers::locations(&hd, &[(4, 5)]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;

    let done = prod_code_mcp::extract_parameter::extract(
        gateway.addr(),
        &ws.root(),
        &home,
        (8, 17),
        (8, 19),
        "pad",
        None,
        false,
        false,
        false,
    )
    .await
    .unwrap_or_else(|e| panic!("the extraction runs: {e:#}"));

    assert_eq!(done.symbol, "render");
    assert_eq!(done.ty, "int");
    assert_eq!(done.call_sites, 2);
    assert!(done.unmatched.is_empty(), "{:?}", done.unmatched);
    let home_text = rewritten(&done, "src/home.c");
    assert!(
        home_text.contains("int render(const char *text, int pad) {"),
        "{home_text}"
    );
    assert!(home_text.contains("    int width = pad;"), "{home_text}");
    assert!(
        home_text.contains("return render(\"x\", 80);"),
        "{home_text}"
    );
    assert!(
        home_text.contains("static const int WIDTH = 80;"),
        "{home_text}"
    );
    let header_text = rewritten(&done, "src/home.h");
    assert!(
        header_text.contains("int render(const char *text, int pad);"),
        "{header_text}"
    );
    assert!(
        header_text.contains("double scale(int n);"),
        "{header_text}"
    );
    let other_text = rewritten(&done, "src/other.c");
    assert!(
        other_text.contains("return render(\"y\", 80) + (int)scale(2) + caller();"),
        "{other_text}"
    );
    assert!(
        done.render(4000).contains("new parameter: `int pad`"),
        "{}",
        done.render(4000)
    );
    assert_eq!(
        ws.read("src/home.c"),
        C_HOME,
        "nothing written without apply"
    );
}

/// A string literal is a `const char *`, and the pointer is written against the name. The
/// header's declaration changes with the definition, and `apply` writes all three files.
#[tokio::test]
async fn c_a_string_literal_becomes_a_const_char_pointer_everywhere() {
    let (ws, header, home, other) = c_workspace();
    let (hd, o) = (header.clone(), other.clone());
    let gateway = ScriptedGateway::start(move |method, _params| match method {
        "textDocument/documentSymbol" => c_home_symbols(),
        "textDocument/references" => answers::locations(&o, &[(4, 31)]),
        "textDocument/declaration" => answers::locations(&hd, &[(5, 8)]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;

    let done = prod_code_mcp::extract_parameter::extract(
        gateway.addr(),
        &ws.root(),
        &home,
        (14, 25),
        (14, 28),
        "fallback",
        None,
        false,
        true,
        false,
    )
    .await
    .unwrap_or_else(|e| panic!("the extraction runs: {e:#}"));

    assert_eq!(done.ty, "const char *");
    assert!(done.applied);
    let home_text = ws.read("src/home.c");
    assert!(
        home_text.contains("double scale(int n, const char *fallback) {"),
        "{home_text}"
    );
    assert!(
        home_text.contains("    const char *label = fallback;"),
        "{home_text}"
    );
    assert!(
        ws.read("src/home.h")
            .contains("double scale(int n, const char *fallback);"),
        "{}",
        ws.read("src/home.h")
    );
    assert!(
        ws.read("src/other.c").contains("(int)scale(2, \"x\")"),
        "{}",
        ws.read("src/other.c")
    );
}

/// `int caller(void)` is C's way of saying it takes nothing: the new parameter replaces the
/// `void`, in the definition and in the header, rather than following it.
#[tokio::test]
async fn c_a_void_parameter_list_gives_way_to_the_new_parameter() {
    let (ws, header, home, other) = c_workspace();
    let (hd, o) = (header.clone(), other.clone());
    let gateway = ScriptedGateway::start(move |method, _params| match method {
        "textDocument/documentSymbol" => c_home_symbols(),
        "textDocument/references" => answers::locations(&o, &[(4, 42)]),
        "textDocument/declaration" => answers::locations(&hd, &[(6, 5)]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;

    let done = prod_code_mcp::extract_parameter::extract(
        gateway.addr(),
        &ws.root(),
        &home,
        (19, 19),
        (19, 22),
        "text",
        None,
        false,
        false,
        false,
    )
    .await
    .unwrap_or_else(|e| panic!("the extraction runs: {e:#}"));

    assert_eq!(done.call_sites, 1);
    let home_text = rewritten(&done, "src/home.c");
    assert!(
        home_text.contains("int caller(const char *text) {"),
        "{home_text}"
    );
    assert!(
        home_text.contains("    return render(text);"),
        "{home_text}"
    );
    let header_text = rewritten(&done, "src/home.h");
    assert!(
        header_text.contains("int caller(const char *text);"),
        "{header_text}"
    );
    let other_text = rewritten(&done, "src/other.c");
    assert!(other_text.contains("+ caller(\"x\");"), "{other_text}");
}

// ---------------------------------------------------------------------------------------------
// C++

const CPP_STORE_H: &str = "#pragma once\n\n#include <string>\n#include <vector>\n\nstd::string render(const std::string &text);\n\nclass Store {\npublic:\n    int limit() const;\n    int size() const {\n        int cap = 64 * 1024;\n        return cap < static_cast<int>(entries.size()) ? cap : static_cast<int>(entries.size());\n    }\n\nprivate:\n    std::vector<int> entries;\n};\n";
const CPP_STORE: &str = "#include \"store.h\"\n\nstatic const std::string PREFIX = \"> \";\n\nstd::string render(const std::string &text) {\n    std::size_t width = 80;\n    std::string label = PREFIX;\n    return label + text + std::string(width, ' ');\n}\n\nint Store::limit() const {\n    int cap = 64 * 1024;\n    return cap + static_cast<int>(entries.size());\n}\n\nstd::string caller() {\n    return render(\"x\");\n}\n";
const CPP_OTHER: &str = "#include \"store.h\"\n\nint use() {\n    Store s;\n    return static_cast<int>(render(\"y\").size()) + s.limit() + s.size();\n}\n";

/// clangd's outline of `CPP_STORE`: the out-of-line method is named `Store::limit` and only
/// `limit` is selected.
fn cpp_store_symbols() -> Value {
    json!([
        span_node("PREFIX", 13, [2, 0, 2, 38], [2, 25, 2, 31], Vec::new()),
        span_node("render", 12, [4, 0, 8, 1], [4, 12, 4, 18], Vec::new()),
        span_node(
            "Store::limit",
            6,
            [10, 0, 13, 1],
            [10, 11, 10, 16],
            Vec::new()
        ),
        span_node("caller", 12, [15, 0, 17, 1], [15, 12, 15, 18], Vec::new()),
    ])
}

/// clangd's outline of `CPP_STORE_H`: the methods are children of the class.
fn cpp_header_symbols() -> Value {
    json!([
        span_node("render", 12, [5, 0, 5, 43], [5, 12, 5, 18], Vec::new()),
        span_node(
            "Store",
            5,
            [7, 0, 17, 1],
            [7, 6, 7, 11],
            vec![
                span_node("limit", 6, [9, 4, 9, 21], [9, 8, 9, 13], Vec::new()),
                span_node("size", 6, [10, 4, 13, 5], [10, 8, 10, 12], Vec::new()),
                span_node("entries", 8, [16, 4, 16, 28], [16, 21, 16, 28], Vec::new()),
            ]
        ),
    ])
}

fn cpp_workspace() -> (Workspace, PathBuf, PathBuf, PathBuf) {
    let ws = Workspace::new(&[
        (
            "CMakeLists.txt",
            "cmake_minimum_required(VERSION 3.16)\nproject(xp CXX)\nset(CMAKE_CXX_STANDARD 17)\nset(CMAKE_EXPORT_COMPILE_COMMANDS ON)\nadd_library(shop src/store.cpp src/other.cpp)\ntarget_include_directories(shop PUBLIC src)\n",
        ),
        ("src/store.h", CPP_STORE_H),
        ("src/store.cpp", CPP_STORE),
        ("src/other.cpp", CPP_OTHER),
    ]);
    let (header, store, other) = (
        ws.path("src/store.h"),
        ws.path("src/store.cpp"),
        ws.path("src/other.cpp"),
    );
    (ws, header, store, other)
}

/// A C++ function declared in a header and defined in a `.cpp`: both take the parameter, and
/// the callers in both source files pass the literal.
#[tokio::test]
async fn cpp_a_function_declared_in_a_header_takes_the_parameter_in_both_places() {
    let (ws, header, store, other) = cpp_workspace();
    let (hd, s, o) = (header.clone(), store.clone(), other.clone());
    let gateway = ScriptedGateway::start(move |method, _params| match method {
        "textDocument/documentSymbol" => cpp_store_symbols(),
        "textDocument/references" => locations_in(&[(&s, &[(17, 12)]), (&o, &[(5, 29)])]),
        "textDocument/declaration" => answers::locations(&hd, &[(6, 13)]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;

    let done = prod_code_mcp::extract_parameter::extract(
        gateway.addr(),
        &ws.root(),
        &store,
        (6, 25),
        (6, 27),
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
    assert!(done.unmatched.is_empty(), "{:?}", done.unmatched);
    let store_text = rewritten(&done, "src/store.cpp");
    assert!(
        store_text.contains("std::string render(const std::string &text, int pad) {"),
        "{store_text}"
    );
    assert!(
        store_text.contains("    std::size_t width = pad;"),
        "{store_text}"
    );
    assert!(
        store_text.contains("return render(\"x\", 80);"),
        "{store_text}"
    );
    let header_text = rewritten(&done, "src/store.h");
    assert!(
        header_text.contains("std::string render(const std::string &text, int pad);"),
        "{header_text}"
    );
    let other_text = rewritten(&done, "src/other.cpp");
    assert!(
        other_text.contains("render(\"y\", 80).size()"),
        "{other_text}"
    );
}

/// A method declared in its class and defined outside it as `Store::limit`: the declaration in
/// the class takes the parameter as well, and `s.limit()` is matched by the bare name. clangd
/// answers nothing on the literal, so a product of two needs its type from the caller.
#[tokio::test]
async fn cpp_a_method_defined_outside_its_class_changes_its_declaration_too() {
    let (ws, header, store, other) = cpp_workspace();
    let (hd, o) = (header.clone(), other.clone());
    let gateway = ScriptedGateway::start(move |method, _params| match method {
        "textDocument/documentSymbol" => cpp_store_symbols(),
        "textDocument/references" => answers::locations(&o, &[(5, 53)]),
        "textDocument/declaration" => answers::locations(&hd, &[(10, 9)]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;

    let root = ws.root();
    let run = |ty| {
        prod_code_mcp::extract_parameter::extract(
            gateway.addr(),
            &root,
            &store,
            (12, 15),
            (12, 24),
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

    let done = run(Some("int")).await.expect("the extraction runs");
    assert_eq!(done.symbol, "Store::limit");
    assert_eq!(done.call_sites, 1);
    assert!(done.unmatched.is_empty(), "{:?}", done.unmatched);
    let store_text = rewritten(&done, "src/store.cpp");
    assert!(
        store_text.contains("int Store::limit(int cap_bytes) const {"),
        "{store_text}"
    );
    assert!(
        store_text.contains("    int cap = cap_bytes;"),
        "{store_text}"
    );
    let header_text = rewritten(&done, "src/store.h");
    assert!(
        header_text.contains("    int limit(int cap_bytes) const;"),
        "{header_text}"
    );
    assert!(
        header_text.contains("    int size() const {"),
        "{header_text}"
    );
    let other_text = rewritten(&done, "src/other.cpp");
    assert!(
        other_text.contains("s.limit(64 * 1024) + s.size()"),
        "{other_text}"
    );
}

/// A method defined inside its class is its own declaration: clangd's `declaration` points
/// back at the definition, and the parameter is added once, not twice.
#[tokio::test]
async fn cpp_a_method_defined_in_its_class_is_changed_once() {
    let (ws, header, _store, other) = cpp_workspace();
    let (hd, o) = (header.clone(), other.clone());
    let gateway = ScriptedGateway::start(move |method, _params| match method {
        "textDocument/documentSymbol" => cpp_header_symbols(),
        "textDocument/references" => answers::locations(&o, &[(5, 65)]),
        "textDocument/declaration" => answers::locations(&hd, &[(11, 9)]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;

    let done = prod_code_mcp::extract_parameter::extract(
        gateway.addr(),
        &ws.root(),
        &header,
        (12, 19),
        (12, 28),
        "cap_bytes",
        Some("int"),
        false,
        false,
        false,
    )
    .await
    .unwrap_or_else(|e| panic!("the extraction runs: {e:#}"));

    assert_eq!(done.symbol, "size");
    assert_eq!(done.call_sites, 1);
    let header_text = rewritten(&done, "src/store.h");
    assert!(
        header_text.contains("    int size(int cap_bytes) const {"),
        "{header_text}"
    );
    assert!(
        !header_text.contains("cap_bytes, int cap_bytes"),
        "{header_text}"
    );
    assert!(
        header_text.contains("        int cap = cap_bytes;"),
        "{header_text}"
    );
    assert!(
        header_text.contains("    int limit() const;"),
        "{header_text}"
    );
    let other_text = rewritten(&done, "src/other.cpp");
    assert!(other_text.contains("s.size(64 * 1024);"), "{other_text}");
    assert_eq!(done.rewritten.len(), 2, "{:?}", done.rewritten);
}

// ---------------------------------------------------------------------------------------------
// Swift

const SWIFT_HOME: &str = "let prefix = \"> \"\n\npublic func render(text: String) -> String {\n    let width = 80\n    let n = text.count\n    return prefix + text + String(repeating: \" \", count: width + n)\n}\n\npublic func pad(_ text: String, _ count: Int) -> String {\n    return text + String(repeating: \" \", count: count + 4)\n}\n\npublic struct Store {\n    var entries: [Int] = []\n\n    public func limit() -> Int {\n        let cap = 64 * 1024\n        return min(cap, entries.count)\n    }\n}\n\nfunc caller() -> String {\n    return render(text: \"x\") + pad(\"x\", 2)\n}\n";
const SWIFT_OTHER: &str = "func use() -> Int {\n    let s = Store()\n    return render(text: \"y\").count + s.limit() + pad(\"y\", 1).count\n}\n";

/// sourcekit-lsp's outline of `SWIFT_HOME`: functions are named with their labels, a range
/// starts after `public`, and the selection covers the name and its parameters.
fn swift_home_symbols() -> Value {
    json!([
        span_node("prefix", 13, [0, 0, 0, 17], [0, 4, 0, 10], Vec::new()),
        span_node(
            "render(text:)",
            12,
            [2, 7, 6, 1],
            [2, 12, 2, 32],
            vec![
                span_node("width", 13, [3, 4, 3, 18], [3, 8, 3, 13], Vec::new()),
                span_node("n", 13, [4, 4, 4, 22], [4, 8, 4, 9], Vec::new()),
            ]
        ),
        span_node("pad(_:_:)", 12, [8, 7, 10, 1], [8, 12, 8, 45], Vec::new()),
        span_node(
            "Store",
            23,
            [12, 7, 19, 1],
            [12, 14, 12, 19],
            vec![
                span_node("entries", 7, [13, 4, 13, 27], [13, 8, 13, 15], Vec::new()),
                span_node(
                    "limit()",
                    6,
                    [15, 11, 18, 5],
                    [15, 16, 15, 23],
                    vec![span_node(
                        "cap",
                        13,
                        [16, 8, 16, 27],
                        [16, 12, 16, 15],
                        Vec::new()
                    )]
                ),
            ]
        ),
        span_node("caller()", 12, [21, 0, 23, 1], [21, 5, 21, 13], Vec::new()),
    ])
}

/// sourcekit-lsp's references, 1-based: an empty range where the name starts.
fn points_in(files: &[(&PathBuf, &[(u32, u32)])]) -> Value {
    Value::Array(
        files
            .iter()
            .flat_map(|(path, spots)| {
                spots.iter().map(move |(line, col)| {
                    let at = json!({ "line": line - 1, "character": col - 1 });
                    json!({
                        "uri": format!("file://{}", path.display()),
                        "range": { "start": at, "end": at }
                    })
                })
            })
            .collect(),
    )
}

fn swift_workspace() -> (Workspace, PathBuf, PathBuf) {
    let ws = Workspace::new(&[
        (
            "Package.swift",
            "// swift-tools-version:5.9\nimport PackageDescription\n\nlet package = Package(\n    name: \"Shop\",\n    targets: [.target(name: \"Shop\")]\n)\n",
        ),
        ("Sources/Shop/Home.swift", SWIFT_HOME),
        ("Sources/Shop/Other.swift", SWIFT_OTHER),
    ]);
    let (home, other) = (
        ws.path("Sources/Shop/Home.swift"),
        ws.path("Sources/Shop/Other.swift"),
    );
    (ws, home, other)
}

/// Every parameter of `render(text:)` is labeled, so the new one is too: the declaration gains
/// `margin: Int` and the callers in both files pass `margin: 80`.
#[tokio::test]
async fn swift_a_literal_becomes_a_labeled_parameter_every_caller_names() {
    let (ws, home, other) = swift_workspace();
    let (h, o) = (home.clone(), other.clone());
    let gateway = ScriptedGateway::start(move |method, _params| match method {
        "textDocument/documentSymbol" => swift_home_symbols(),
        "textDocument/references" => points_in(&[(&h, &[(23, 12)]), (&o, &[(3, 12)])]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;

    let done = prod_code_mcp::extract_parameter::extract(
        gateway.addr(),
        &ws.root(),
        &home,
        (4, 17),
        (4, 19),
        "margin",
        None,
        false,
        false,
        false,
    )
    .await
    .unwrap_or_else(|e| panic!("the extraction runs: {e:#}"));

    assert_eq!(done.symbol, "render(text:)");
    assert_eq!(done.ty, "Int");
    assert_eq!(done.call_sites, 2);
    assert!(done.unmatched.is_empty(), "{:?}", done.unmatched);
    let home_text = rewritten(&done, "Sources/Shop/Home.swift");
    assert!(
        home_text.contains("public func render(text: String, margin: Int) -> String {"),
        "{home_text}"
    );
    assert!(
        home_text.contains("    let width = margin\n"),
        "{home_text}"
    );
    assert!(
        home_text.contains("return render(text: \"x\", margin: 80) + pad(\"x\", 2)"),
        "{home_text}"
    );
    let other_text = rewritten(&done, "Sources/Shop/Other.swift");
    assert!(
        other_text.contains("render(text: \"y\", margin: 80).count"),
        "{other_text}"
    );
    assert!(
        done.render(4000).contains("new parameter: `margin: Int`"),
        "{}",
        done.render(4000)
    );
}

/// `pad(_:_:)` takes positional arguments, so the new parameter is `_ extra: Int` and the
/// callers append the expression without a label.
#[tokio::test]
async fn swift_positional_parameters_get_a_positional_one() {
    let (ws, home, other) = swift_workspace();
    let (h, o) = (home.clone(), other.clone());
    let gateway = ScriptedGateway::start(move |method, _params| match method {
        "textDocument/documentSymbol" => swift_home_symbols(),
        "textDocument/references" => points_in(&[(&h, &[(23, 32)]), (&o, &[(3, 50)])]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;

    let done = prod_code_mcp::extract_parameter::extract(
        gateway.addr(),
        &ws.root(),
        &home,
        (10, 57),
        (10, 58),
        "extra",
        None,
        false,
        true,
        false,
    )
    .await
    .unwrap_or_else(|e| panic!("the extraction runs: {e:#}"));

    assert_eq!(done.parameter, "_ extra: Int");
    assert_eq!(done.call_sites, 2);
    assert!(done.applied);
    let home_text = ws.read("Sources/Shop/Home.swift");
    assert!(
        home_text
            .contains("public func pad(_ text: String, _ count: Int, _ extra: Int) -> String {"),
        "{home_text}"
    );
    assert!(home_text.contains("count: count + extra)"), "{home_text}");
    assert!(home_text.contains("pad(\"x\", 2, 4)"), "{home_text}");
    let other_text = ws.read("Sources/Shop/Other.swift");
    assert!(
        other_text.contains("pad(\"y\", 1, 4).count"),
        "{other_text}"
    );
}

/// A method of a struct with no parameters: none is positional, so the new one is labeled and
/// `s.limit()` becomes `s.limit(capBytes: 64 * 1024)`. sourcekit-lsp answers a hover without a
/// range, and on a number it describes `Int` the type, not the product; only a selection that
/// is a single name is typed from a hover, so here the type has to be given.
#[tokio::test]
async fn swift_a_method_takes_a_labeled_parameter_and_its_caller_names_it() {
    let (ws, home, other) = swift_workspace();
    let o = other.clone();
    let gateway = ScriptedGateway::start(move |method, _params| match method {
        "textDocument/documentSymbol" => swift_home_symbols(),
        "textDocument/hover" => answers::hover(
            "Int\n```swift\n@frozen struct Int : FixedWidthInteger, SignedInteger, _ExpressibleByBuiltinIntegerLiteral\n```\n\n---\nA signed integer value type.",
        ),
        "textDocument/references" => points_in(&[(&o, &[(3, 40)])]),
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
            (17, 19),
            (17, 28),
            "capBytes",
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

    let done = run(Some("Int")).await.expect("the extraction runs");
    assert_eq!(done.symbol, "limit()");
    assert_eq!(done.call_sites, 1);
    let home_text = rewritten(&done, "Sources/Shop/Home.swift");
    assert!(
        home_text.contains("    public func limit(capBytes: Int) -> Int {"),
        "{home_text}"
    );
    assert!(
        home_text.contains("        let cap = capBytes\n"),
        "{home_text}"
    );
    let other_text = rewritten(&done, "Sources/Shop/Other.swift");
    assert!(
        other_text.contains("s.limit(capBytes: 64 * 1024)"),
        "{other_text}"
    );
}

/// The hover on a single name gives its type even without a range; on `width + n` the same
/// answer about `width` is not the sum's type and is not used.
#[tokio::test]
async fn swift_a_hover_without_a_range_types_only_a_single_name() {
    let (ws, home, other) = swift_workspace();
    let (h, o) = (home.clone(), other.clone());
    let gateway = ScriptedGateway::start(move |method, params| match method {
        "textDocument/documentSymbol" => swift_home_symbols(),
        "textDocument/hover" => {
            match params
                .pointer("/position/character")
                .and_then(Value::as_u64)
            {
                Some(11) => answers::hover("prefix\n```swift\nlet prefix: String\n```\n\n---\n"),
                Some(57) => answers::hover("width\n```swift\nlet width: Int\n```\n\n---\n"),
                _ => Value::Null,
            }
        }
        "textDocument/references" => points_in(&[(&h, &[(23, 12)]), (&o, &[(3, 12)])]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;

    let root = ws.root();
    let done = prod_code_mcp::extract_parameter::extract(
        gateway.addr(),
        &root,
        &home,
        (6, 12),
        (6, 18),
        "lead",
        None,
        false,
        false,
        false,
    )
    .await
    .unwrap_or_else(|e| panic!("the extraction runs: {e:#}"));
    assert_eq!(done.parameter, "lead: String");
    let home_text = rewritten(&done, "Sources/Shop/Home.swift");
    assert!(
        home_text.contains("    return lead + text + String("),
        "{home_text}"
    );
    assert!(
        home_text.contains("render(text: \"x\", lead: prefix)"),
        "{home_text}"
    );

    let err = prod_code_mcp::extract_parameter::extract(
        gateway.addr(),
        &root,
        &home,
        (6, 58),
        (6, 67),
        "total",
        None,
        false,
        false,
        false,
    )
    .await
    .expect_err("the hover on `width` does not type `width + n`");
    assert!(
        format!("{err:#}").contains("pass the type explicitly"),
        "{err:#}"
    );
}
