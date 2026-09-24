//! Bundling parameters in TypeScript, Python and Go, driven end to end against a scripted
//! gateway.
//!
//! Every scripted answer here is one the real server gave on the same files: the TypeScript
//! server (`tsc --lsp`), basedpyright and gopls on a Linux build node, asked with
//! `prod-code refs`, `prod-code hover` and `prod-code outline` in scratch repositories holding
//! exactly these sources. Two of those answers shaped the code. basedpyright lists the import
//! `from app.home import build` among the references to `build`, which is neither a call nor a
//! use to report. It also lists the keyword of `build(name, 1, height=2)` among the references
//! to the parameter `height`, which is why only references inside the body become field reads.

use prod_code_testkit::{Answer, ScriptedGateway, Workspace, answers};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
async fn bundle(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    params: &[&str],
    name: &str,
    binding: &str,
) -> prod_code_mcp::parameter_object::ParameterObject {
    let params: Vec<String> = params.iter().map(|p| p.to_string()).collect();
    match prod_code_mcp::parameter_object::introduce(
        remote, root, file, line, col, &params, name, binding, false, false,
    )
    .await
    {
        Ok(done) => done,
        Err(err) => panic!("bundling runs: {err:#}"),
    }
}

const TS_HOME: &str = "export function build(name: string, width: number, height: number): string {
  const area = width * height;
  return `${name} ${area}`;
}

export class Canvas {
  scale = 1;

  draw(label: string, x: number, y: number): string {
    return `${label} ${x * this.scale} ${y}`;
  }
}

export function twice(name: string): string {
  return build(name, 1, 2);
}
";

const TS_OTHER: &str = "import { build, Canvas } from \"./home\";

export function callIt(): string {
  const c = new Canvas();
  return build(\"a\", 3, 4) + c.draw(\"b\", 5, 6);
}

export const asValue = build;
";

struct TypeScript {
    ws: Workspace,
    home: PathBuf,
    remote: SocketAddr,
}

async fn typescript() -> TypeScript {
    let ws = Workspace::new(&[
        (
            "package.json",
            "{ \"name\": \"po-ts\", \"version\": \"1.0.0\", \"private\": true }\n",
        ),
        (
            "tsconfig.json",
            "{ \"compilerOptions\": { \"target\": \"es2020\", \"module\": \"commonjs\", \"strict\": true }, \"include\": [\"src\"] }\n",
        ),
        ("src/home.ts", TS_HOME),
        ("src/other.ts", TS_OTHER),
    ]);
    let (home, other) = (ws.path("src/home.ts"), ws.path("src/other.ts"));
    let (h, o) = (home.clone(), other.clone());
    let remote = scripted_gateway(Arc::new(move |method, params| {
        let (uri, line, ch) = position(params);
        match method {
            "textDocument/references" if uri.ends_with("home.ts") => match (line, ch) {
                // `build`: the TypeScript server does not list the import of it.
                (0, 16) => spread(&[(&h, 15, 10), (&o, 5, 10), (&o, 8, 24)]),
                (0, 36) => answers::locations(&h, &[(2, 16)]),
                (0, 51) => answers::locations(&h, &[(2, 24)]),
                // `draw`, then its `x` and `y`.
                (8, 2) => answers::locations(&o, &[(5, 31)]),
                (8, 22) => answers::locations(&h, &[(10, 24)]),
                (8, 33) => answers::locations(&h, &[(10, 42)]),
                _ => serde_json::json!([]),
            },
            "textDocument/references" => serde_json::json!([]),
            "textDocument/documentSymbol" if uri.ends_with("home.ts") => serde_json::json!([
                answers::document_symbol("build", 12, 1, 4, 17),
                answers::document_symbol("Canvas", 5, 6, 12, 14),
                answers::document_symbol("twice", 12, 14, 16, 17),
            ]),
            "textDocument/documentSymbol" => serde_json::json!([
                answers::document_symbol("callIt", 12, 3, 6, 17),
                answers::document_symbol("asValue", 13, 8, 8, 14),
            ]),
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => serde_json::Value::Null,
        }
    }))
    .await;
    TypeScript { ws, home, remote }
}

/// A TypeScript function: an interface with the declared types, exported because the function
/// is, a declaration that takes it, the body reading its fields, an object literal at the call
/// in the same file and at the one in another file, and the function used as a value reported.
#[tokio::test]
async fn a_typescript_function_takes_an_interface_and_its_callers_pass_an_object_literal() {
    let t = typescript().await;
    let root = t.ws.root();
    let done = bundle(
        t.remote,
        &root,
        &t.home,
        1,
        17,
        &["width", "height"],
        "Size",
        "size",
    )
    .await;

    assert_eq!(done.symbol, "build");
    assert_eq!(done.now, "name: string, size: Size");
    assert_eq!(done.call_sites, 2);
    assert_eq!(done.body_uses, 2);
    assert_eq!(
        done.unmatched.len(),
        1,
        "only the use as a value: {:?}",
        done.unmatched
    );
    assert!(
        done.unmatched[0].ends_with("src/other.ts:8:24"),
        "{:?}",
        done.unmatched
    );
    assert_eq!(
        rewritten(&done, "home.ts"),
        "/** The parameters `build` takes together. */
export interface Size {
  width: number;
  height: number;
}

export function build(name: string, size: Size): string {
  const area = size.width * size.height;
  return `${name} ${area}`;
}

export class Canvas {
  scale = 1;

  draw(label: string, x: number, y: number): string {
    return `${label} ${x * this.scale} ${y}`;
  }
}

export function twice(name: string): string {
  return build(name, { width: 1, height: 2 });
}
"
    );
    let other = rewritten(&done, "other.ts");
    assert!(
        other.contains("return build(\"a\", { width: 3, height: 4 }) + c.draw(\"b\", 5, 6);"),
        "{other}"
    );
    assert!(
        other.starts_with("import { build, Canvas } from \"./home\";\n"),
        "{other}"
    );
    assert!(
        done.imports.is_empty(),
        "a structural type needs no import: {:?}",
        done.imports
    );
    let report = done.render(8000);
    assert!(
        report.contains("```typescript\n/** The parameters"),
        "{report}"
    );
    assert!(
        report.contains("the function passed as a value"),
        "{report}"
    );
    assert_eq!(t.ws.read("src/home.ts"), TS_HOME, "nothing was written");
}

/// A TypeScript class method: the interface goes above the class, not inside it, and the call
/// through an instance passes the literal.
#[tokio::test]
async fn a_typescript_method_gets_its_interface_above_the_class() {
    let t = typescript().await;
    let root = t.ws.root();
    let done = bundle(
        t.remote,
        &root,
        &t.home,
        9,
        3,
        &["y", "x"],
        "Point",
        "point",
    )
    .await;

    assert_eq!(done.symbol, "draw");
    assert_eq!(done.now, "label: string, point: Point");
    assert_eq!(done.call_sites, 1);
    assert_eq!(done.body_uses, 2);
    assert!(done.unmatched.is_empty(), "{:?}", done.unmatched);
    let home = rewritten(&done, "home.ts");
    assert!(
        home.contains(
            "}\n\n/** The parameters `draw` takes together. */\nexport interface Point {\n  x: number;\n  y: number;\n}\n\nexport class Canvas {\n  scale = 1;\n\n  draw(label: string, point: Point): string {\n    return `${label} ${point.x * this.scale} ${point.y}`;\n  }\n}\n"
        ),
        "{home}"
    );
    let other = rewritten(&done, "other.ts");
    assert!(other.contains("c.draw(\"b\", { x: 5, y: 6 })"), "{other}");
}

const PY_HOME: &str = "\"\"\"Shapes.\"\"\"

import math


def build(name: str, width: int, height: int = 2) -> str:
    area = width * height
    return f\"{name} {area} {math.pi}\"


class Canvas:
    def draw(self, label: str, x: float, y: float) -> str:
        return f\"{label} {x} {y}\"


def loose(a, b):
    return a + b


def twice(name: str) -> str:
    return build(name, 1, height=2)
";

const PY_OTHER: &str = "from app.home import build, Canvas


def call_it() -> str:
    c = Canvas()
    return build(\"a\", 3, 4) + c.draw(\"b\", 5.0, 6.0)


as_value = build
";

struct Python {
    ws: Workspace,
    home: PathBuf,
    remote: SocketAddr,
}

async fn python() -> Python {
    let ws = Workspace::new(&[
        (
            "pyproject.toml",
            "[project]\nname = \"po-py\"\nversion = \"0.1.0\"\n",
        ),
        ("app/__init__.py", ""),
        ("app/home.py", PY_HOME),
        ("app/other.py", PY_OTHER),
    ]);
    let (home, other) = (ws.path("app/home.py"), ws.path("app/other.py"));
    let (h, o) = (home.clone(), other.clone());
    let remote = scripted_gateway(Arc::new(move |method, params| {
        let (uri, line, ch) = position(params);
        match method {
            "textDocument/references" if uri.ends_with("home.py") => match (line, ch) {
                // `build`: basedpyright lists the import of it too.
                (5, 4) => spread(&[(&h, 21, 12), (&o, 1, 22), (&o, 6, 12), (&o, 9, 12)]),
                (5, 21) => answers::locations(&h, &[(7, 12)]),
                // `height` is also the keyword of the call in `twice`.
                (5, 33) => answers::locations(&h, &[(7, 20), (21, 27)]),
                // `draw`, then its `x` and `y`.
                (11, 8) => answers::locations(&o, &[(6, 33)]),
                (11, 31) => answers::locations(&h, &[(13, 27)]),
                (11, 41) => answers::locations(&h, &[(13, 31)]),
                // `loose`, then `a` and `b`.
                (15, 4) => serde_json::json!([]),
                (15, 10) => answers::locations(&h, &[(17, 12)]),
                (15, 13) => answers::locations(&h, &[(17, 16)]),
                _ => serde_json::json!([]),
            },
            "textDocument/references" => serde_json::json!([]),
            // An unannotated parameter nothing calls is `Unknown` to basedpyright.
            "textDocument/hover" if uri.ends_with("home.py") && line == 15 => {
                let name = if ch == 10 { "a" } else { "b" };
                answers::hover(&format!("```python\n(parameter) {name}: Unknown\n```"))
            }
            "textDocument/documentSymbol" if uri.ends_with("home.py") => serde_json::json!([
                answers::document_symbol("build", 12, 6, 8, 5),
                answers::document_symbol("Canvas", 5, 11, 13, 7),
                answers::document_symbol("loose", 12, 16, 17, 5),
                answers::document_symbol("twice", 12, 20, 21, 5),
            ]),
            "textDocument/documentSymbol" => serde_json::json!([
                answers::document_symbol("call_it", 12, 4, 6, 5),
                answers::document_symbol("as_value", 13, 9, 9, 1),
            ]),
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => serde_json::Value::Null,
        }
    }))
    .await;
    Python { ws, home, remote }
}

/// A Python function: a dataclass with the annotations and the default, the import of
/// `dataclass` added once, keyword arguments matched by name, the caller in another module
/// importing the new type next to the function, the import of the function itself left alone,
/// and the function used as a value reported.
#[tokio::test]
async fn a_python_function_takes_a_dataclass_and_its_callers_construct_it() {
    let p = python().await;
    let root = p.ws.root();
    let done = bundle(
        p.remote,
        &root,
        &p.home,
        6,
        5,
        &["width", "height"],
        "Size",
        "size",
    )
    .await;

    assert_eq!(done.symbol, "build");
    assert_eq!(done.now, "name: str, size: Size");
    assert_eq!(done.call_sites, 2);
    assert_eq!(
        done.body_uses, 2,
        "the keyword `height=2` in `twice` is not a use in the body"
    );
    assert_eq!(done.unmatched.len(), 1, "{:?}", done.unmatched);
    assert!(
        done.unmatched[0].ends_with("app/other.py:9:12"),
        "{:?}",
        done.unmatched
    );
    assert_eq!(
        rewritten(&done, "home.py"),
        "\"\"\"Shapes.\"\"\"

import math
from dataclasses import dataclass


@dataclass
class Size:
    \"\"\"The parameters `build` takes together.\"\"\"

    width: int
    height: int = 2


def build(name: str, size: Size) -> str:
    area = size.width * size.height
    return f\"{name} {area} {math.pi}\"


class Canvas:
    def draw(self, label: str, x: float, y: float) -> str:
        return f\"{label} {x} {y}\"


def loose(a, b):
    return a + b


def twice(name: str) -> str:
    return build(name, Size(width=1, height=2))
"
    );
    assert_eq!(
        rewritten(&done, "other.py"),
        "from app.home import build, Canvas, Size


def call_it() -> str:
    c = Canvas()
    return build(\"a\", Size(width=3, height=4)) + c.draw(\"b\", 5.0, 6.0)


as_value = build
"
    );
    assert_eq!(done.imports.len(), 2, "{:?}", done.imports);
    assert!(
        done.render(8000)
            .contains("```python\n@dataclass\nclass Size:")
    );
}

/// A Python method: `self` stays first, the dataclass goes above the class, and the caller
/// that imports only the class gets the new type added to that import.
#[tokio::test]
async fn a_python_method_keeps_self_and_gets_its_dataclass_above_the_class() {
    let p = python().await;
    let root = p.ws.root();
    let done = bundle(
        p.remote,
        &root,
        &p.home,
        12,
        9,
        &["x", "y"],
        "Point",
        "point",
    )
    .await;

    assert_eq!(done.symbol, "draw");
    assert_eq!(done.now, "self, label: str, point: Point");
    assert_eq!(done.call_sites, 1);
    assert_eq!(done.body_uses, 2);
    assert!(done.unmatched.is_empty(), "{:?}", done.unmatched);
    let home = rewritten(&done, "home.py");
    assert!(
        home.contains(
            "    return f\"{name} {area} {math.pi}\"\n\n\n@dataclass\nclass Point:\n    \"\"\"The parameters `draw` takes together.\"\"\"\n\n    x: float\n    y: float\n\n\nclass Canvas:\n    def draw(self, label: str, point: Point) -> str:\n        return f\"{label} {point.x} {point.y}\"\n"
        ),
        "{home}"
    );
    assert!(
        home.contains("import math\nfrom dataclasses import dataclass\n"),
        "{home}"
    );
    let other = rewritten(&done, "other.py");
    assert!(
        other.contains("c.draw(\"b\", Point(x=5.0, y=6.0))"),
        "{other}"
    );
    assert!(
        other.starts_with("from app.home import build, Canvas, Point\n"),
        "{other}"
    );
}

/// Parameters without annotations that basedpyright cannot type either: no type is invented, so
/// the new type is a plain class, and no `dataclass` import is added.
#[tokio::test]
async fn untyped_python_parameters_make_a_plain_class() {
    let p = python().await;
    let root = p.ws.root();
    let done = bundle(p.remote, &root, &p.home, 16, 5, &["a", "b"], "Pair", "pair").await;

    assert_eq!(
        done.struct_text,
        "class Pair:\n    \"\"\"The parameters `loose` takes together.\"\"\"\n\n    def __init__(self, a, b):\n        self.a = a\n        self.b = b\n"
    );
    assert_eq!(done.now, "pair: Pair");
    let home = rewritten(&done, "home.py");
    assert!(home.contains("    return pair.a + pair.b\n"), "{home}");
    assert!(!home.contains("dataclass"), "{home}");
    assert!(done.imports.is_empty(), "{:?}", done.imports);
}

const GO_HOME: &str = "package shapes

import \"fmt\"

// Build describes a shape.
func Build(name string, width, height int) string {
\tarea := width * height
\treturn fmt.Sprintf(\"%s %d\", name, area)
}

// Canvas draws.
type Canvas struct {
\tscale int
}

// Draw draws a label.
func (c *Canvas) Draw(label string, x int, y int) string {
\treturn fmt.Sprintf(\"%s %d %d\", label, x*c.scale, y)
}
";

const GO_OTHER: &str = "package shapes

func CallIt() string {
\tc := &Canvas{scale: 1}
\treturn Build(\"a\", 3, 4) + c.Draw(\"b\", 5, 6)
}

var AsValue = Build
";

struct Go {
    ws: Workspace,
    home: PathBuf,
    remote: SocketAddr,
}

async fn go() -> Go {
    let ws = Workspace::new(&[
        ("go.mod", "module example.com/po\n\ngo 1.22\n"),
        ("shapes/home.go", GO_HOME),
        ("shapes/other.go", GO_OTHER),
    ]);
    let (home, other) = (ws.path("shapes/home.go"), ws.path("shapes/other.go"));
    let (h, o) = (home.clone(), other.clone());
    let remote = scripted_gateway(Arc::new(move |method, params| {
        let (uri, line, ch) = position(params);
        match method {
            "textDocument/references" if uri.ends_with("home.go") => match (line, ch) {
                (5, 5) => spread(&[(&o, 5, 9), (&o, 8, 15)]),
                (5, 24) => answers::locations(&h, &[(7, 10)]),
                (5, 31) => answers::locations(&h, &[(7, 18)]),
                // `Draw`, then its `x` and `y`.
                (16, 17) => answers::locations(&o, &[(5, 30)]),
                (16, 36) => answers::locations(&h, &[(18, 40)]),
                (16, 43) => answers::locations(&h, &[(18, 51)]),
                _ => serde_json::json!([]),
            },
            "textDocument/references" => serde_json::json!([]),
            // gopls names a method after its receiver type.
            "textDocument/documentSymbol" if uri.ends_with("home.go") => serde_json::json!([
                answers::document_symbol("Build", 12, 6, 9, 6),
                answers::document_symbol("Canvas", 23, 12, 14, 6),
                answers::document_symbol("(*Canvas).Draw", 6, 17, 19, 18),
            ]),
            "textDocument/documentSymbol" => serde_json::json!([
                answers::document_symbol("CallIt", 12, 3, 6, 6),
                answers::document_symbol("AsValue", 13, 8, 8, 5),
            ]),
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => serde_json::Value::Null,
        }
    }))
    .await;
    Go { ws, home, remote }
}

/// A Go function whose two bundled parameters share one type (`width, height int`): a struct
/// above the function's doc comment with the fields gofmt would align, the field names kept as
/// the parameters' (unexported), a composite literal at the call in the other file, and the
/// function used as a value reported.
#[tokio::test]
async fn a_go_function_takes_a_struct_and_its_callers_pass_a_composite_literal() {
    let g = go().await;
    let root = g.ws.root();
    let done = bundle(
        g.remote,
        &root,
        &g.home,
        6,
        6,
        &["width", "height"],
        "Size",
        "size",
    )
    .await;

    assert_eq!(done.symbol, "Build");
    assert_eq!(done.now, "name string, size Size");
    assert_eq!(done.call_sites, 1);
    assert_eq!(done.body_uses, 2);
    assert_eq!(done.unmatched.len(), 1, "{:?}", done.unmatched);
    assert!(
        done.unmatched[0].ends_with("shapes/other.go:8:15"),
        "{:?}",
        done.unmatched
    );
    let home = rewritten(&done, "home.go");
    assert!(
        home.starts_with(
            "package shapes\n\nimport \"fmt\"\n\n// Size holds the parameters Build takes together.\ntype Size struct {\n\twidth  int\n\theight int\n}\n\n// Build describes a shape.\nfunc Build(name string, size Size) string {\n\tarea := size.width * size.height\n"
        ),
        "{home}"
    );
    assert_eq!(
        rewritten(&done, "other.go"),
        "package shapes\n\nfunc CallIt() string {\n\tc := &Canvas{scale: 1}\n\treturn Build(\"a\", Size{width: 3, height: 4}) + c.Draw(\"b\", 5, 6)\n}\n\nvar AsValue = Build\n"
    );
    assert!(done.render(8000).contains("```go\n// Size holds"));
}

/// A Go method: the receiver stays where Go writes it, the struct goes above the method's doc
/// comment, and the call through a value is not given the value as a package qualifier.
#[tokio::test]
async fn a_go_method_keeps_its_receiver_and_its_callers_pass_the_bare_type() {
    let g = go().await;
    let root = g.ws.root();
    let done = bundle(
        g.remote,
        &root,
        &g.home,
        17,
        18,
        &["x", "y"],
        "Point",
        "point",
    )
    .await;

    assert_eq!(done.symbol, "Draw");
    assert_eq!(done.now, "label string, point Point");
    assert_eq!(done.call_sites, 1);
    assert_eq!(done.body_uses, 2);
    let home = rewritten(&done, "home.go");
    assert!(
        home.ends_with(
            "}\n\n// Point holds the parameters Draw takes together.\ntype Point struct {\n\tx int\n\ty int\n}\n\n// Draw draws a label.\nfunc (c *Canvas) Draw(label string, point Point) string {\n\treturn fmt.Sprintf(\"%s %d %d\", label, point.x*c.scale, point.y)\n}\n"
        ),
        "{home}"
    );
    let other = rewritten(&done, "other.go");
    assert!(
        other.contains("c.Draw(\"b\", Point{x: 5, y: 6})"),
        "{other}"
    );
}
