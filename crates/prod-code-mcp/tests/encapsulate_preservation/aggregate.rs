/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::*;
use prod_code_mcp::encapsulate_field::{encapsulate, encapsulate_polyglot};
use prod_code_testkit::Workspace;

fn execute(ws: &Workspace, file: &str) -> bool {
    match file {
        "main.py" => run(ws, "python3", &[file]).status.success(),
        "main.js" => run(ws, "node", &[file]).status.success(),
        "main.cpp" => {
            run(ws, "c++", &["-std=c++17", file, "-o", "probe"])
                .status
                .success()
                && run(ws, "./probe", &[]).status.success()
        }
        _ => {
            run(ws, "rustc", &["--edition=2024", file, "-o", "probe"])
                .status
                .success()
                && run(ws, "./probe", &[]).status.success()
        }
    }
}

#[tokio::test]
async fn aggregate_public_api_runtime_and_refusal() {
    let cpp = "class Box {\npublic:\n int count;\n};\n";
    let py = "class Box:\n def __init__(self):\n  self.count = 0\n";
    let js = "class Box {\n count = 0;\n}\nmodule.exports = Box;\n";
    let rust = "pub struct Box {\n pub count: i32,\n}\nimpl Box { pub fn new() -> Self { Self { count: 0 } } }\n";
    let coercion = "pub struct Box {\n pub count: Option<std::boxed::Box<dyn std::fmt::Display>>,\n}\nimpl Box { pub fn new() -> Self { Self { count: None } } }\n";
    let imported_coercion = "use std::fmt::Display as Label;\npub struct Box {\n pub count: Option<std::boxed::Box<dyn Label>>,\n}\nimpl Box { pub fn new() -> Self { Self { count: None } } }\n";
    let cases = [
        (
            "cpp-copy-assignment-only",
            "box.hpp",
            "struct Payload { int value = 0; Payload() = default; Payload(const Payload&) = delete; Payload& operator=(const Payload&) = default; };\nclass Box {\npublic:\n Payload count;\n};\n",
            "main.cpp",
            "#include \"box.hpp\"\nint main() { Box b; Payload source; source.value = 7; b.count = source; return b.count.value == 7 ? 0 : 1; }\n",
            true,
        ),
        (
            "cpp-lambda-receiver",
            "box.hpp",
            cpp,
            "main.cpp",
            "#include \"box.hpp\"\nint main() { Box b; int n = 0; ([&]() -> Box& { ++n; return b; })().count = 2; return b.count == 2 && n == 1 ? 0 : 1; }\n",
            true,
        ),
        (
            "rust-match-arms",
            "model.rs",
            rust,
            "main.rs",
            "mod model;\nfn main() { let mut b = model::Box::new(); for flag in [true, false] { match flag { true => b.count = 1, false => b.count = 2 } assert_eq!(b.count, if flag { 1 } else { 2 }); } }\n",
            true,
        ),
        (
            "rust-rhs-temporary-drop",
            "model.rs",
            rust,
            "main.rs",
            "mod model;\nstruct Token<'a>(&'a std::cell::Cell<usize>);\nimpl Token<'_> { fn value(&self) -> i32 { 7 } }\nimpl Drop for Token<'_> { fn drop(&mut self) { self.0.set(1); } }\nfn main() { let mut boxes = [model::Box::new(), model::Box::new()]; let index = std::cell::Cell::new(0); boxes[index.get()].count = Token(&index).value(); assert_eq!((boxes[0].count, boxes[1].count, index.get()), (7, 0, 1)); println!(\"rust temporary drops after target=first\"); }\n",
            true,
        ),
        (
            "cpp-rhs-temporary-drop",
            "box.hpp",
            cpp,
            "main.cpp",
            "#include \"box.hpp\"\nBox* volatile p; struct Token { Box* next; int value() { return 7; } ~Token() { p = next; } };\nint main() { Box a, z; a.count = 0; z.count = 0; p = &a; p->count = Token{&z}.value(); return a.count == 7 && z.count == 0 && p == &z ? 0 : 1; }\n",
            true,
        ),
        (
            "rust-raw-receiver",
            "model.rs",
            rust,
            "main.rs",
            "mod model;\nfn main() { let mut r#type = model::Box::new(); r#type.count = 2; let mut boxes = [model::Box::new()]; let r#index = 0; boxes[r#index].count = r#type.count; assert_eq!((r#type.count, boxes[0].count), (2, 2)); }\n",
            true,
        ),
        (
            "rust-macro-receiver",
            "model.rs",
            rust,
            "main.rs",
            "mod model;\nfn main() { let mut b = model::Box::new(); macro_rules! target { () => { &mut b }; } target!().count = 2; assert_eq!(b.count, 2); }\n",
            false,
        ),
        (
            "cpp-braced-values",
            "box.hpp",
            cpp,
            "main.cpp",
            "#include \"box.hpp\"\nint main() { Box b; b.count = {}; if (b.count != 0) return 1; b.count = {7}; return b.count == 7 ? 0 : 1; }\n",
            true,
        ),
        (
            "cpp-lvalue-conversion",
            "box.hpp",
            cpp,
            "main.cpp",
            "#include \"box.hpp\"\nstruct Source { operator int() & { return 1; } operator int() && { return 2; } };\nint main() { Box b; Source source; b.count = source; return b.count == 1 ? 0 : 1; }\n",
            true,
        ),
        (
            "cpp-noncopy-conversion",
            "box.hpp",
            cpp,
            "main.cpp",
            "#include \"box.hpp\"\nstruct Source { Source() = default; Source(const Source&) = delete; operator int() & { return 7; } };\nint main() { Box b; Source source; b.count = source; return b.count == 7 ? 0 : 1; }\n",
            true,
        ),
        (
            "cpp-conversion-rebind",
            "box.hpp",
            cpp,
            "main.cpp",
            "#include \"box.hpp\"\nBox* volatile p; struct Source { Box* next; operator int() & { p = next; return 7; } };\nint main() { Box a, z; a.count = 0; z.count = 0; p = &a; Source source{&z}; p->count = source; return a.count == 0 && z.count == 7 ? 0 : 1; }\n",
            true,
        ),
        (
            "python-suite",
            "owner.py",
            py,
            "main.py",
            "from owner import Box\ndef update(b):\n b.count = 2\nb = Box()\nupdate(b)\nif True:\n b.count = 3\nfor i in range(1):\n b.count = 4\nwhile b.count < 5:\n b.count = 5\nassert b.count == 5\n",
            true,
        ),
        (
            "python-value-lifetime",
            "owner.py",
            py,
            "main.py",
            "from owner import Box\nimport weakref, gc\nclass Token: pass\nb = Box()\nb.count = Token(); ref = weakref.ref(b.count); del b; gc.collect(); assert ref() is None\nprint('python staging released')\n",
            true,
        ),
        (
            "python-exception-lifetime",
            "owner.py",
            py,
            "main.py",
            "from owner import Box\nimport weakref, gc\nclass Token: pass\nref = None\ndef rhs():\n global ref\n t = Token()\n ref = weakref.ref(t)\n return t\ndef target():\n raise RuntimeError('target')\ntry:\n target().count = rhs()\nexcept RuntimeError as err:\n saved = err\ngc.collect()\nassert ref() is None\nprint('python exceptional staging released with traceback retained')\n",
            true,
        ),
        (
            "rust-nested-coercion",
            "model.rs",
            coercion,
            "main.rs",
            "mod model;\nfn main() { let mut b = model::Box::new(); b.count = Some(std::boxed::Box::new(1_i32)); assert_eq!(b.count.as_ref().unwrap().to_string(), \"1\"); println!(\"rust nested coercion preserved\"); }\n",
            true,
        ),
        (
            "rust-imported-coercion",
            "model.rs",
            imported_coercion,
            "main.rs",
            "mod model;\nfn main() { let mut b = model::Box::new(); b.count = Some(std::boxed::Box::new(1_i32)); assert_eq!(b.count.as_ref().unwrap().to_string(), \"1\"); println!(\"rust imported coercion preserved\"); }\n",
            true,
        ),
        (
            "python-rebind",
            "owner.py",
            py,
            "main.py",
            "from owner import Box\na = Box()\nz = Box()\nb = a\ndef rhs():\n global b\n b = z\n return 7\nb.count = rhs()\nassert a.count == 0 and z.count == 7\nprint('python target=second')\n",
            true,
        ),
        (
            "cpp-rebind",
            "box.hpp",
            cpp,
            "main.cpp",
            "#include \"box.hpp\"\nint main() { Box a, z; a.count = 0; z.count = 0; Box* volatile p = &a; auto rhs = [&]() { p = &z; return 7; }; p->count = rhs(); return a.count == 0 && z.count == 7 ? 0 : 1; }\n",
            true,
        ),
        (
            "rust-index",
            "model.rs",
            rust,
            "main.rs",
            "mod model;\nfn main() { let mut boxes = [model::Box::new(), model::Box::new()]; let flag = std::cell::Cell::new(0); boxes[flag.get()].count = { flag.set(1); 7 }; assert_eq!((boxes[0].count, boxes[1].count), (0, 7)); println!(\"rust target=second\"); }\n",
            true,
        ),
        (
            "cpp-header",
            "box.hpp",
            cpp,
            "main.cpp",
            "#include \"box.hpp\"\nint main() { Box b; b.count = 0; for (int i=0; b.count = 2; ++i) break; return b.count == 2 ? 0 : 1; }\n",
            false,
        ),
        (
            "js-header",
            "owner.js",
            js,
            "main.js",
            "const Box = require('./owner'); const b = new Box(); let n=0; for (let i=0; b.count = 2; ++i) { n++; break; } if(n !== 1 || b.count !== 2) throw Error('consumed condition'); console.log('JS loop ran');\n",
            false,
        ),
        (
            "js-bodies",
            "owner.js",
            js,
            "main.js",
            "const Box = require('./owner'); const b = new Box(); function rhs() { return 2; } if (true) b.count = rhs(); for (let i=0; i<1; ++i) b.count = 3; if(b.count !== 3) throw Error('unbraced bodies');\n",
            true,
        ),
        (
            "js-compound-trivia",
            "owner.js",
            js,
            "main.js",
            "const Box = require('./owner'); const b = new Box(); b.count = 2; b.count /* keep power */ **= 2; if(b.count !== 4) throw Error('compound');\n",
            false,
        ),
        (
            "python-continuation",
            "owner.py",
            py,
            "main.py",
            "from owner import Box\nb = Box()\nother = \\\nb.count = 2\nassert other == b.count == 2\n",
            false,
        ),
        (
            "cpp-trivia",
            "box.hpp",
            cpp,
            "main.cpp",
            "#include \"box.hpp\"\nint main() { Box b; b.count /* keep note */ = 2; return b.count == 2 ? 0 : 1; }\n",
            true,
        ),
        (
            "cpp-trivia-newline",
            "box.hpp",
            cpp,
            "main.cpp",
            "#include \"box.hpp\"\nint main() { Box b; b.count // keep operator newline\n = 2; return b.count == 2 ? 0 : 1; }\n",
            true,
        ),
        (
            "cpp-compound-trivia",
            "box.hpp",
            cpp,
            "main.cpp",
            "#include \"box.hpp\"\nint main() { Box b; b.count = 0; b.count /* keep note */ += 2; return b.count == 2 ? 0 : 1; }\n",
            false,
        ),
        (
            "cpp-conditional-comma",
            "box.hpp",
            cpp,
            "main.cpp",
            "#include \"box.hpp\"\nint main() { Box b; bool enabled=true; b.count = enabled ? 1, 2 : 3; int n=0; b.count = enabled ? false ? 4, 5 : 6, 7 : 8, ++n; return b.count == 7 && n == 1 ? 0 : 1; }\n",
            true,
        ),
        (
            "cpp-line-comment",
            "box.hpp",
            cpp,
            "main.cpp",
            "#include \"box.hpp\"\nint main() { Box b; b.count = 2 // keep newline\n; return b.count == 2 ? 0 : 1; }\n",
            true,
        ),
        (
            "rust-line-comment",
            "model.rs",
            rust,
            "main.rs",
            "mod model;\nfn main() { let mut b = model::Box::new(); b.count = 2 // keep newline\n; assert_eq!(b.count, 2); }\n",
            true,
        ),
    ];
    // Reuse one root: client workspace watchers outlive individual runtimes.
    let ws = Workspace::new(&[("model.rs", ""), ("main.rs", "")]);
    let mut failures = Vec::new();
    for (name, owner_file, owner, file, body, supported) in cases {
        for force in [false, true] {
            ws.write(owner_file, owner);
            ws.write(file, body);
            emit(format!(
                "CASE {name} force={force}\nINPUT {owner_file}:\n{owner}\nINPUT {file}:\n{body}\n"
            ));
            assert!(execute(&ws, file), "original {name}");
            // Enumerate exact fixture occurrences, including internal constructor fields.
            let mut refs = Vec::new();
            for (path, source) in [(owner_file, owner), (file, body)] {
                for (at, _) in source.match_indices("count") {
                    if path == owner_file
                        && (source[..at].ends_with("int ")
                            || source[..at].ends_with("Payload ")
                            || source[..at].ends_with("pub ")
                            || (file == "main.js" && source[..at].ends_with(' ')))
                    {
                        continue;
                    }
                    let prefix = &source[..at];
                    refs.push((
                        path,
                        (
                            prefix.bytes().filter(|c| *c == b'\n').count() as u32 + 1,
                            prefix.rsplit('\n').next().unwrap().encode_utf16().count() as u32 + 1,
                        ),
                    ));
                }
            }
            let anchor = if file == "main.rs" {
                "pub count:"
            } else if file == "main.cpp" {
                " count;"
            } else if file == "main.py" {
                "self.count ="
            } else {
                "count ="
            };
            let decl = spot(owner, anchor, "count");
            let gw = gateway(&ws, "Box", "count", (owner_file, decl), &refs).await;
            let result = if file == "main.rs" {
                encapsulate(
                    gw.addr(),
                    &ws.root(),
                    &ws.path(owner_file),
                    decl.0,
                    decl.1,
                    None,
                    true,
                    force,
                )
                .await
            } else {
                encapsulate_polyglot(
                    gw.addr(),
                    &ws.root(),
                    &ws.path(owner_file),
                    Some("Box"),
                    "count",
                    None,
                    true,
                    force,
                )
                .await
            };
            emit(format!(
                "RESULT {name}: {result:?}\nEMITTED {owner_file}:\n{}\nEMITTED {file}:\n{}\n",
                ws.read(owner_file),
                ws.read(file)
            ));
            let actual = result.as_ref().is_ok_and(|p| p.applied);
            let runtime = execute(&ws, file);
            let untouched = ws.read(owner_file) == owner && ws.read(file) == body;
            if actual != supported || !runtime || (!supported && !untouched) {
                failures.push(format!("{name} force={force} applied={actual} expected={supported} runtime={runtime} untouched={untouched}"));
            }
            if name.contains("trivia") || name.contains("line-comment") {
                assert!(ws.read(file).contains("keep "));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
