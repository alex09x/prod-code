//! Bundling parameters in C, C++ and Swift, driven end to end against a scripted gateway.
//!
//! Every scripted answer here is one the real server gave on the same files: clangd on a Linux
//! build node for a CMake C project and a CMake C++ project, and sourcekit-lsp on a macOS node
//! for a SwiftPM package, asked over the language server protocol in scratch repositories
//! holding exactly these sources. Three of those answers shaped the code. clangd leaves a
//! header's prototype and the definition out of the references unless it is asked for
//! declarations as well, which is how the declarations are told from the calls. clangd takes a
//! C++20 designated initialiser in a C++17 project without a word — it is an extension there —
//! so the choice between designators and a plain aggregate is made from the standard the build
//! declares, not from the analyzer's verdict. And sourcekit-lsp still lists a file its index
//! knew and the checkout no longer has, a reference that has to be passed over quietly.

use prod_code_testkit::{Answer, ScriptedGateway, Workspace, answers};
use serde_json::Value;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

async fn scripted_gateway(answer: Answer) -> SocketAddr {
    ScriptedGateway::start_arc(answer).await.addr()
}

/// The file, 0-based line and 0-based character a request asks about, and whether a references
/// request asks for the declarations too.
fn position(params: &Value) -> (String, u64, u64, bool) {
    let uri = params
        .pointer("/textDocument/uri")
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();
    let at = |p: &str| params.pointer(p).and_then(|v| v.as_u64()).unwrap_or(0);
    let declarations = params
        .pointer("/context/includeDeclaration")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    (
        uri,
        at("/position/line"),
        at("/position/character"),
        declarations,
    )
}

/// References at 1-based positions spread over several files, with the range shape of `answer`.
fn spread(answer: fn(&Path, &[(u32, u32)]) -> Value, spots: &[(&Path, u32, u32)]) -> Value {
    Value::Array(
        spots
            .iter()
            .flat_map(|(path, line, col)| {
                answer(path, &[(*line, *col)])
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

const C_HOME_H: &str = "#ifndef HOME_H
#define HOME_H

/* Build describes a shape. */
int build(const char *name, int width, int height);

int twice(const char *name);

#endif
";

const C_HOME_C: &str = "#include \"home.h\"

#include <stdio.h>

int build(const char *name, int width, int height) {
    int area = width * height;
    return printf(\"%s %d\\n\", name, area);
}

static int scale(int value, int num, int den) {
    return value * num / den;
}

int twice(const char *name) {
    return build(name, 1, 2) + scale(3, 4, 5);
}
";

const C_OTHER: &str = "#include \"home.h\"

int call_it(void) {
    return build(\"a\", 3, 4);
}

int (*as_value)(const char *, int, int) = build;
";

const C_MAIN: &str = "#include \"home.h\"

int call_it(void);

int main(void) {
    return build(\"m\", 5, 6) + call_it() + twice(\"t\");
}
";

struct Project {
    ws: Workspace,
    remote: SocketAddr,
}

async fn c_project() -> Project {
    let ws = Workspace::new(&[
        (
            "CMakeLists.txt",
            "cmake_minimum_required(VERSION 3.16)\nproject(po_c C)\nset(CMAKE_C_STANDARD 11)\nset(CMAKE_EXPORT_COMPILE_COMMANDS ON)\nadd_executable(po src/home.c src/other.c src/main.c)\n",
        ),
        ("src/home.h", C_HOME_H),
        ("src/home.c", C_HOME_C),
        ("src/other.c", C_OTHER),
        ("src/main.c", C_MAIN),
    ]);
    let (h, c) = (ws.path("src/home.h"), ws.path("src/home.c"));
    let (o, m) = (ws.path("src/other.c"), ws.path("src/main.c"));
    let remote = scripted_gateway(Arc::new(move |method, params| {
        let (uri, line, ch, declarations) = position(params);
        let at = answers::locations;
        match method {
            // `build`, asked from the header's prototype or from the definition: the two
            // declarations are listed only with `includeDeclaration`.
            "textDocument/references" if (line, ch) == (4, 4) => {
                let mut spots: Vec<(&Path, u32, u32)> = Vec::new();
                if declarations {
                    spots.push((h.as_path(), 5, 5));
                    spots.push((c.as_path(), 5, 5));
                }
                spots.push((c.as_path(), 15, 12));
                spots.push((m.as_path(), 6, 12));
                spots.push((o.as_path(), 4, 12));
                spots.push((o.as_path(), 7, 43));
                spread(at, &spots)
            }
            "textDocument/references" if uri.ends_with("home.c") => match (line, ch) {
                // `width` and `height` of the definition, asked where each name starts.
                (4, 32) => at(&c, &[(6, 16)]),
                (4, 43) => at(&c, &[(6, 24)]),
                // `scale`, which only its own file declares, then its `num` and `den`.
                (9, 11) if declarations => at(&c, &[(10, 12), (15, 32)]),
                (9, 11) => at(&c, &[(15, 32)]),
                (9, 32) => at(&c, &[(11, 20)]),
                (9, 41) => at(&c, &[(11, 26)]),
                _ => serde_json::json!([]),
            },
            "textDocument/references" => serde_json::json!([]),
            "textDocument/documentSymbol" if uri.ends_with("home.h") => serde_json::json!([
                answers::document_symbol("build", 12, 5, 5, 5),
                answers::document_symbol("twice", 12, 7, 7, 5),
            ]),
            "textDocument/documentSymbol" if uri.ends_with("home.c") => serde_json::json!([
                answers::document_symbol("build", 12, 5, 8, 5),
                answers::document_symbol("scale", 12, 10, 12, 12),
                answers::document_symbol("twice", 12, 14, 16, 5),
            ]),
            "textDocument/documentSymbol" if uri.ends_with("other.c") => serde_json::json!([
                answers::document_symbol("call_it", 12, 3, 5, 5),
                answers::document_symbol("as_value", 13, 7, 7, 7),
            ]),
            "textDocument/documentSymbol" => serde_json::json!([
                answers::document_symbol("call_it", 12, 3, 3, 5),
                answers::document_symbol("main", 12, 5, 7, 5),
            ]),
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => Value::Null,
        }
    }))
    .await;
    Project { ws, remote }
}

/// A C function asked for at its prototype in the header: the struct goes into the header above
/// the prototype and its comment, the prototype and the definition both take `struct Size`, the
/// definition's body reads the fields, and the calls in the three files that make one pass a
/// compound literal with designators. The function used as a value is reported.
#[tokio::test]
async fn a_c_function_declared_in_a_header_takes_a_struct_in_both_declarations() {
    let p = c_project().await;
    let root = p.ws.root();
    let done = bundle(
        p.remote,
        &root,
        &p.ws.path("src/home.h"),
        5,
        5,
        &["width", "height"],
        "Size",
        "size",
    )
    .await;

    assert_eq!(done.symbol, "build");
    assert_eq!(done.now, "const char *name, struct Size size");
    assert_eq!(done.call_sites, 3);
    assert_eq!(done.body_uses, 2);
    assert_eq!(done.unmatched.len(), 1, "{:?}", done.unmatched);
    assert!(
        done.unmatched[0].ends_with("src/other.c:7:43"),
        "{:?}",
        done.unmatched
    );
    assert_eq!(
        rewritten(&done, "home.h"),
        "#ifndef HOME_H
#define HOME_H

/* The parameters `build` takes together. */
struct Size {
    int width;
    int height;
};

/* Build describes a shape. */
int build(const char *name, struct Size size);

int twice(const char *name);

#endif
"
    );
    assert_eq!(
        rewritten(&done, "home.c"),
        "#include \"home.h\"

#include <stdio.h>

int build(const char *name, struct Size size) {
    int area = size.width * size.height;
    return printf(\"%s %d\\n\", name, area);
}

static int scale(int value, int num, int den) {
    return value * num / den;
}

int twice(const char *name) {
    return build(name, (struct Size){.width = 1, .height = 2}) + scale(3, 4, 5);
}
"
    );
    assert!(
        rewritten(&done, "other.c")
            .contains("    return build(\"a\", (struct Size){.width = 3, .height = 4});\n")
    );
    assert!(rewritten(&done, "main.c").contains(
        "    return build(\"m\", (struct Size){.width = 5, .height = 6}) + call_it() + twice(\"t\");\n"
    ));
    let report = done.render(8000);
    assert!(
        report.contains("```c\n/* The parameters `build` takes together. */"),
        "{report}"
    );
    assert_eq!(p.ws.read("src/home.h"), C_HOME_H, "nothing was written");
}

/// A `static` C function has no header: the struct goes above it in its own file.
#[tokio::test]
async fn a_static_c_function_gets_its_struct_in_its_own_file() {
    let p = c_project().await;
    let root = p.ws.root();
    let done = bundle(
        p.remote,
        &root,
        &p.ws.path("src/home.c"),
        10,
        12,
        &["den", "num"],
        "Ratio",
        "ratio",
    )
    .await;

    assert_eq!(done.symbol, "scale");
    assert_eq!(done.now, "int value, struct Ratio ratio");
    assert_eq!(done.call_sites, 1);
    assert_eq!(done.body_uses, 2);
    assert_eq!(done.rewritten.len(), 1, "only home.c changes");
    let home = rewritten(&done, "home.c");
    assert!(
        home.contains(
            "}\n\n/* The parameters `scale` takes together. */\nstruct Ratio {\n    int num;\n    int den;\n};\n\nstatic int scale(int value, struct Ratio ratio) {\n    return value * ratio.num / ratio.den;\n}\n"
        ),
        "{home}"
    );
    assert!(
        home.contains("scale(3, (struct Ratio){.num = 4, .den = 5});"),
        "{home}"
    );
}

const CPP_HOME_HPP: &str = "#pragma once

#include <string>

namespace shapes {

// Build describes a shape.
std::string build(const std::string &name, int width, int height = 2);

class Canvas {
public:
    std::string draw(const std::string &label, int x, int y) const;

private:
    int scale_ = 1;
};

} // namespace shapes
";

const CPP_HOME_CPP: &str = "#include \"home.hpp\"

namespace shapes {

std::string build(const std::string &name, int width, int height) {
    int area = width * height;
    return name + \" \" + std::to_string(area);
}

std::string Canvas::draw(const std::string &label, int x, int y) const {
    return label + \" \" + std::to_string(x * scale_) + \" \" + std::to_string(y);
}

std::string twice(const std::string &name) {
    return build(name, 1, 2);
}

} // namespace shapes
";

const CPP_OTHER: &str = "#include \"home.hpp\"

std::string call_it() {
    shapes::Canvas c;
    return shapes::build(\"a\", 3, 4) + c.draw(\"b\", 5, 6);
}

auto as_value = &shapes::build;
";

const CPP_MAIN: &str = "#include \"home.hpp\"

#include <iostream>

std::string call_it();

int main() {
    std::cout << shapes::build(\"m\", 7) << call_it() << '\\n';
}
";

/// The C++ project, building as the given standard.
async fn cpp_project(standard: u32) -> Project {
    let cmake = format!(
        "cmake_minimum_required(VERSION 3.16)\nproject(po_cpp CXX)\nset(CMAKE_CXX_STANDARD {standard})\nset(CMAKE_EXPORT_COMPILE_COMMANDS ON)\nadd_executable(po src/home.cpp src/other.cpp src/main.cpp)\n"
    );
    let ws = Workspace::new(&[
        ("CMakeLists.txt", cmake.as_str()),
        ("src/home.hpp", CPP_HOME_HPP),
        ("src/home.cpp", CPP_HOME_CPP),
        ("src/other.cpp", CPP_OTHER),
        ("src/main.cpp", CPP_MAIN),
    ]);
    let (h, c) = (ws.path("src/home.hpp"), ws.path("src/home.cpp"));
    let (o, m) = (ws.path("src/other.cpp"), ws.path("src/main.cpp"));
    let remote = scripted_gateway(Arc::new(move |method, params| {
        let (uri, line, ch, declarations) = position(params);
        let at = answers::locations;
        match method {
            "textDocument/references" if uri.ends_with("home.cpp") => match (line, ch) {
                // `build`: the declarations only with `includeDeclaration`.
                (4, 12) => {
                    let mut spots: Vec<(&Path, u32, u32)> = Vec::new();
                    if declarations {
                        spots.push((c.as_path(), 5, 13));
                    }
                    spots.push((c.as_path(), 15, 12));
                    if declarations {
                        spots.push((h.as_path(), 8, 13));
                    }
                    spots.push((m.as_path(), 8, 26));
                    spots.push((o.as_path(), 5, 20));
                    spots.push((o.as_path(), 8, 26));
                    spread(at, &spots)
                }
                (4, 47) => at(&c, &[(6, 16)]),
                (4, 58) => at(&c, &[(6, 24)]),
                // `Canvas::draw`, then its `x` and `y`.
                (9, 20) if declarations => spread(at, &[(&c, 10, 21), (&h, 12, 17), (&o, 5, 41)]),
                (9, 20) => at(&o, &[(5, 41)]),
                (9, 55) => at(&c, &[(11, 41)]),
                (9, 62) => at(&c, &[(11, 76)]),
                _ => serde_json::json!([]),
            },
            "textDocument/references" => serde_json::json!([]),
            // clangd nests a class's members in the class and everything in its namespace.
            "textDocument/documentSymbol" if uri.ends_with("home.hpp") => {
                serde_json::json!([answers::nested(
                    answers::document_symbol("shapes", 3, 5, 18, 11),
                    vec![
                        answers::document_symbol("build", 12, 8, 8, 13),
                        answers::nested(
                            answers::document_symbol("Canvas", 5, 10, 16, 7),
                            vec![
                                answers::document_symbol("draw", 6, 12, 12, 17),
                                answers::document_symbol("scale_", 8, 15, 15, 9),
                            ]
                        ),
                    ]
                )])
            }
            "textDocument/documentSymbol" if uri.ends_with("home.cpp") => {
                serde_json::json!([answers::nested(
                    answers::document_symbol("shapes", 3, 3, 18, 11),
                    vec![
                        answers::document_symbol("build", 12, 5, 8, 13),
                        answers::document_symbol("Canvas::draw", 6, 10, 12, 21),
                        answers::document_symbol("twice", 12, 14, 16, 13),
                    ]
                )])
            }
            "textDocument/documentSymbol" if uri.ends_with("other.cpp") => serde_json::json!([
                answers::document_symbol("call_it", 12, 3, 6, 13),
                answers::document_symbol("as_value", 13, 8, 8, 6),
            ]),
            "textDocument/documentSymbol" => serde_json::json!([
                answers::document_symbol("call_it", 12, 5, 5, 13),
                answers::document_symbol("main", 12, 7, 9, 5),
            ]),
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => Value::Null,
        }
    }))
    .await;
    Project { ws, remote }
}

/// A C++ function in a namespace with a prototype in a header that gives a default: the struct
/// goes into the header inside the namespace, the default becomes the member's initialiser,
/// both declarations take `Size`, and in a C++20 project the calls in three files pass
/// designated initialisers — the one that relied on the default leaving that field out.
#[tokio::test]
async fn a_cpp20_function_takes_a_struct_and_its_callers_pass_designated_initialisers() {
    let p = cpp_project(20).await;
    let root = p.ws.root();
    let done = bundle(
        p.remote,
        &root,
        &p.ws.path("src/home.cpp"),
        5,
        13,
        &["width", "height"],
        "Size",
        "size",
    )
    .await;

    assert_eq!(done.symbol, "build");
    assert_eq!(done.now, "const std::string &name, Size size");
    assert_eq!(done.call_sites, 3);
    assert_eq!(done.body_uses, 2);
    assert_eq!(done.unmatched.len(), 1, "{:?}", done.unmatched);
    assert!(
        done.unmatched[0].ends_with("src/other.cpp:8:26"),
        "{:?}",
        done.unmatched
    );
    let header = rewritten(&done, "home.hpp");
    assert!(
        header.starts_with(
            "#pragma once\n\n#include <string>\n\nnamespace shapes {\n\n// The parameters `build` takes together.\nstruct Size {\n    int width;\n    int height = 2;\n};\n\n// Build describes a shape.\nstd::string build(const std::string &name, Size size);\n\nclass Canvas {\n"
        ),
        "{header}"
    );
    let home = rewritten(&done, "home.cpp");
    assert!(
        home.contains(
            "std::string build(const std::string &name, Size size) {\n    int area = size.width * size.height;\n"
        ),
        "{home}"
    );
    assert!(
        home.contains("    return build(name, {.width = 1, .height = 2});\n"),
        "{home}"
    );
    assert!(
        rewritten(&done, "other.cpp").contains(
            "return shapes::build(\"a\", {.width = 3, .height = 4}) + c.draw(\"b\", 5, 6);"
        )
    );
    assert!(
        rewritten(&done, "main.cpp")
            .contains("std::cout << shapes::build(\"m\", {.width = 7}) << call_it() << '\\n';")
    );
    assert!(done.render(8000).contains("```cpp\n// The parameters"));
}

/// Before C++20 a braced list names no fields: the same bundling in a C++17 project passes the
/// values in field order, and the call that relied on the default passes only the first.
#[tokio::test]
async fn a_cpp17_project_passes_a_plain_aggregate() {
    let p = cpp_project(17).await;
    let root = p.ws.root();
    let done = bundle(
        p.remote,
        &root,
        &p.ws.path("src/home.cpp"),
        5,
        13,
        &["width", "height"],
        "Size",
        "size",
    )
    .await;

    assert_eq!(done.call_sites, 3);
    assert!(rewritten(&done, "home.cpp").contains("    return build(name, {1, 2});\n"));
    assert!(rewritten(&done, "other.cpp").contains("return shapes::build(\"a\", {3, 4}) + "));
    assert!(rewritten(&done, "main.cpp").contains("shapes::build(\"m\", {7})"));
}

/// A C++ method declared in its class in the header and defined outside it: the struct goes
/// above the class — not above the `public:` written at the class's own indentation — and the
/// declaration in the class, the definition and the call through an instance all change.
#[tokio::test]
async fn a_cpp_method_gets_its_struct_above_the_class() {
    let p = cpp_project(20).await;
    let root = p.ws.root();
    let done = bundle(
        p.remote,
        &root,
        &p.ws.path("src/home.cpp"),
        10,
        21,
        &["y", "x"],
        "Point",
        "point",
    )
    .await;

    assert_eq!(done.symbol, "draw");
    assert_eq!(done.now, "const std::string &label, Point point");
    assert_eq!(done.call_sites, 1);
    assert_eq!(done.body_uses, 2);
    assert!(done.unmatched.is_empty(), "{:?}", done.unmatched);
    let header = rewritten(&done, "home.hpp");
    assert!(
        header.contains(
            "int height = 2);\n\n// The parameters `draw` takes together.\nstruct Point {\n    int x;\n    int y;\n};\n\nclass Canvas {\npublic:\n    std::string draw(const std::string &label, Point point) const;\n"
        ),
        "{header}"
    );
    assert!(
        rewritten(&done, "home.cpp").contains(
            "std::string Canvas::draw(const std::string &label, Point point) const {\n    return label + \" \" + std::to_string(point.x * scale_) + \" \" + std::to_string(point.y);\n"
        )
    );
    assert!(rewritten(&done, "other.cpp").contains("c.draw(\"b\", {.x = 5, .y = 6})"));
}

const SWIFT_HOME: &str = "/// Describes a shape.
public func build(name: String, width: Int, height: Int) -> String {
    let area = width * height
    return \"\\(name) \\(area)\"
}

public struct Canvas {
    var scale = 1

    public init() {}

    public func draw(_ label: String, x: Int, y: Int = 0) -> String {
        return \"\\(label) \\(x * scale) \\(y)\"
    }
}

func twice(_ name: String) -> String {
    return build(name: name, width: 1, height: 2)
}
";

const SWIFT_OTHER: &str = "func callIt() -> String {
    let c = Canvas()
    return build(name: \"a\", width: 3, height: 4) + c.draw(\"b\", x: 5, y: 6)
}

let asValue = build
";

const SWIFT_REPORT: &str = "func report() -> String {
    return build(name: \"m\", width: 7, height: 8) + Canvas().draw(\"c\", x: 9)
}
";

struct Swift {
    ws: Workspace,
    home: PathBuf,
    remote: SocketAddr,
}

async fn swift() -> Swift {
    let ws = Workspace::new(&[
        (
            "Package.swift",
            "// swift-tools-version:5.9\nimport PackageDescription\n\nlet package = Package(\n    name: \"Shapes\",\n    targets: [\n        .target(name: \"Shapes\"),\n    ]\n)\n",
        ),
        ("Sources/Shapes/Home.swift", SWIFT_HOME),
        ("Sources/Shapes/Other.swift", SWIFT_OTHER),
        ("Sources/Shapes/Report.swift", SWIFT_REPORT),
    ]);
    let home = ws.path("Sources/Shapes/Home.swift");
    let (h, o) = (home.clone(), ws.path("Sources/Shapes/Other.swift"));
    let r = ws.path("Sources/Shapes/Report.swift");
    // The index still knew `Main.swift` after it had been renamed to `Report.swift`.
    let stale = ws.path("Sources/Shapes/Main.swift");
    let remote = scripted_gateway(Arc::new(move |method, params| {
        let (uri, line, ch, _) = position(params);
        let at = answers::points;
        match method {
            "textDocument/references" if uri.ends_with("Home.swift") => match (line, ch) {
                (1, 12) => spread(
                    at,
                    &[
                        (&h, 18, 12),
                        (&r, 2, 12),
                        (&stale, 2, 12),
                        (&o, 3, 12),
                        (&o, 6, 15),
                    ],
                ),
                (1, 32) => at(&h, &[(3, 16)]),
                (1, 44) => at(&h, &[(3, 24)]),
                // `draw`, then its `x` and `y`, used inside string interpolations.
                (11, 16) => spread(at, &[(&r, 2, 61), (&stale, 2, 61), (&o, 3, 54)]),
                (11, 38) => at(&h, &[(13, 28)]),
                (11, 46) => at(&h, &[(13, 41)]),
                _ => serde_json::json!([]),
            },
            "textDocument/references" => serde_json::json!([]),
            "textDocument/documentSymbol" if uri.ends_with("Home.swift") => serde_json::json!([
                answers::nested(
                    answers::document_symbol("build(name:width:height:)", 12, 2, 5, 13),
                    vec![answers::document_symbol("area", 13, 3, 3, 9)]
                ),
                answers::nested(
                    answers::document_symbol("Canvas", 23, 7, 15, 15),
                    vec![
                        answers::document_symbol("scale", 7, 8, 8, 9),
                        answers::document_symbol("init()", 6, 10, 10, 12),
                        answers::document_symbol("draw(_:x:y:)", 6, 12, 14, 17),
                    ]
                ),
                answers::document_symbol("twice(_:)", 12, 17, 19, 6),
            ]),
            "textDocument/documentSymbol" if uri.ends_with("Other.swift") => serde_json::json!([
                answers::nested(
                    answers::document_symbol("callIt()", 12, 1, 4, 6),
                    vec![answers::document_symbol("c", 13, 2, 2, 9)]
                ),
                answers::document_symbol("asValue", 13, 6, 6, 5),
            ]),
            "textDocument/documentSymbol" => {
                serde_json::json!([answers::document_symbol("report()", 12, 1, 3, 6)])
            }
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => Value::Null,
        }
    }))
    .await;
    Swift { ws, home, remote }
}

/// A public Swift function: a public struct of `let` properties, a declaration that takes it
/// under the label the parameters had, the body reading its fields, the memberwise initialiser
/// passed under that label at the calls in three files, the function used as a value reported,
/// and the index's stale entry for a file the checkout no longer has passed over.
#[tokio::test]
async fn a_swift_function_takes_a_struct_and_its_callers_pass_its_initialiser() {
    let s = swift().await;
    let root = s.ws.root();
    let done = bundle(
        s.remote,
        &root,
        &s.home,
        2,
        13,
        &["width", "height"],
        "Size",
        "size",
    )
    .await;

    assert_eq!(done.symbol, "build");
    assert_eq!(done.now, "name: String, size: Size");
    assert_eq!(done.call_sites, 3);
    assert_eq!(done.body_uses, 2);
    assert_eq!(done.unmatched.len(), 1, "{:?}", done.unmatched);
    assert!(
        done.unmatched[0].ends_with("Sources/Shapes/Other.swift:6:15"),
        "{:?}",
        done.unmatched
    );
    let home = rewritten(&done, "Home.swift");
    assert!(
        home.starts_with(
            "/// The parameters `build` takes together.\npublic struct Size {\n    let width: Int\n    let height: Int\n}\n\n/// Describes a shape.\npublic func build(name: String, size: Size) -> String {\n    let area = size.width * size.height\n"
        ),
        "{home}"
    );
    assert!(
        home.contains("    return build(name: name, size: Size(width: 1, height: 2))\n"),
        "{home}"
    );
    assert!(rewritten(&done, "Other.swift").contains(
        "return build(name: \"a\", size: Size(width: 3, height: 4)) + c.draw(\"b\", x: 5, y: 6)"
    ));
    assert!(
        rewritten(&done, "Report.swift")
            .contains("return build(name: \"m\", size: Size(width: 7, height: 8)) + ")
    );
    assert!(done.render(8000).contains("```swift\n/// The parameters"));
}

/// A Swift method with an unlabelled first parameter and a defaulted last one: the struct goes
/// above the type, the defaulted field is a `var` with its default so the initialiser may leave
/// it out, and the call that relied on the default leaves it out.
#[tokio::test]
async fn a_swift_method_gets_its_struct_above_the_type() {
    let s = swift().await;
    let root = s.ws.root();
    let done = bundle(
        s.remote,
        &root,
        &s.home,
        12,
        17,
        &["x", "y"],
        "Point",
        "point",
    )
    .await;

    assert_eq!(done.symbol, "draw");
    assert_eq!(done.now, "_ label: String, point: Point");
    assert_eq!(done.call_sites, 2);
    assert_eq!(done.body_uses, 2);
    assert!(done.unmatched.is_empty(), "{:?}", done.unmatched);
    let home = rewritten(&done, "Home.swift");
    assert!(
        home.contains(
            "}\n\n/// The parameters `draw` takes together.\npublic struct Point {\n    let x: Int\n    var y: Int = 0\n}\n\npublic struct Canvas {\n    var scale = 1\n\n    public init() {}\n\n    public func draw(_ label: String, point: Point) -> String {\n        return \"\\(label) \\(point.x * scale) \\(point.y)\"\n    }\n}\n"
        ),
        "{home}"
    );
    assert!(rewritten(&done, "Other.swift").contains("c.draw(\"b\", point: Point(x: 5, y: 6))"));
    assert!(rewritten(&done, "Report.swift").contains("Canvas().draw(\"c\", point: Point(x: 9))"));
}
