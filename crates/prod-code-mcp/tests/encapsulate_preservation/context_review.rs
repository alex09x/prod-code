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
use serde_json::json;

fn record_source(name: &str, phase: &str, file: &str, source: &str) {
    emit(format!(
        "CONTEXT_SOURCE {}\n",
        json!({"case": name, "phase": phase, "file": file, "source": source})
    ));
}

fn refs_for(owner_file: &str, owner: &str, file: &str, body: &str) -> Vec<(String, (u32, u32))> {
    let mut out = Vec::new();
    for (path, source) in [(owner_file, owner), (file, body)] {
        for (at, _) in source.match_indices("count") {
            if path == owner_file
                && (source[..at].ends_with("int ")
                    || source[..at].ends_with("Payload ")
                    || source[..at].ends_with("pub "))
            {
                continue;
            }
            let prefix = &source[..at];
            let pos = (
                prefix.bytes().filter(|b| *b == b'\n').count() as u32 + 1,
                prefix.rsplit('\n').next().unwrap().encode_utf16().count() as u32 + 1,
            );
            out.push((path.to_string(), pos));
        }
    }
    out
}

fn compiler_available(program: &str, args: &[&str]) -> bool {
    std::process::Command::new(program)
        .args(args)
        .output()
        .is_ok_and(|output| output.status.success())
}

fn check_runtime(ws: &Workspace, file: &str) -> bool {
    match file {
        "main.py" => run(ws, "python3", &[file]).status.success(),
        "main.cpp" => {
            let mut passed = true;
            for (program, prefix) in [("zig", Some("c++")), ("g++", None)] {
                let probe: Vec<_> = prefix.into_iter().chain(["--version"]).collect();
                if !compiler_available(program, &probe) {
                    eprintln!("skipping unavailable C++ compiler: {program}");
                    continue;
                }
                let args: Vec<_> = prefix
                    .into_iter()
                    .chain(["-std=c++17", "-pedantic-errors", file, "-o", "probe_cpp"])
                    .collect();
                let compiled = run(ws, program, &args).status.success();
                let executed = compiled && run(ws, "./probe_cpp", &[]).status.success();
                if !executed {
                    eprintln!("C++ compiler or runtime failed: {program}");
                }
                passed &= executed;
            }
            passed
        }
        _ => {
            run(ws, "rustc", &["--edition=2024", file, "-o", "probe"])
                .status
                .success()
                && run(ws, "./probe", &[]).status.success()
        }
    }
}

async fn verify_aggregate_review_repairs(ws: &Workspace, failures: &mut Vec<String>) {
    let cpp_available = compiler_available("zig", &["c++", "--version"])
        || compiler_available("g++", &["--version"]);
    let py = "class Box:\n def __init__(self):\n  self.count = 0\n";
    let cpp = "class Box {\npublic:\n int count;\n};\n";
    let cpp_payload = "struct Payload {\n int value = 0;\n Payload() = default;\n Payload(int v) : value(v) {}\n Payload(const Payload&) = delete;\n Payload& operator=(const Payload&) = default;\n};\nclass Box {\npublic:\n Payload count;\n};\n";
    let rust_map = "pub struct Box {\n pub count: std::collections::HashMap<String, u32>,\n}\nimpl Box {\n pub fn new() -> Self { Self { count: std::collections::HashMap::new() } }\n}\n";
    let rust_fn = "pub struct Box {\n pub count: fn(i32, i32) -> Result<i32, u32>,\n}\nimpl Box {\n pub fn new() -> Self { Self { count: |a, b| Ok(a + b) } }\n}\n";
    let rust_int = "pub struct Box {\n pub count: i32,\n}\nimpl Box {\n pub fn new() -> Self { Self { count: 0 } }\n}\n";
    let cases = [
        (
            "py-indent-bracket-module",
            py,
            "main.py",
            "from model import Box\nb = Box()\nx = (1,\n     2); b.count = x[1]\nassert b.count == 2\n",
        ),
        (
            "py-indent-bracket-function",
            py,
            "main.py",
            "from model import Box\ndef update(b):\n    x = [1,\n2]; b.count = x[1]\nb = Box()\nupdate(b)\nassert b.count == 2\n",
        ),
        (
            "py-indent-string-function",
            py,
            "main.py",
            "from model import Box\ndef update(b):\n    note = \"\"\"first\nsecond\"\"\"; b.count = len(note.splitlines())\nb = Box()\nupdate(b)\nassert b.count == 2\n",
        ),
        (
            "py-indent-fstring-function",
            py,
            "main.py",
            "from model import Box\ndef update(b):\n    note = f\"\"\"{1\n+ 1}\"\"\"; b.count = int(note)\nb = Box()\nupdate(b)\nassert b.count == 2\n",
        ),
        (
            "py-indent-backslash-module",
            py,
            "main.py",
            "from model import Box\nb = Box()\nx = 1 + \\\n    1; b.count = x\nassert b.count == 2\n",
        ),
        (
            "py-indent-backslash-function",
            py,
            "main.py",
            "from model import Box\ndef update(b):\n    x = 1 + \\\n1; b.count = x\nb = Box()\nupdate(b)\nassert b.count == 2\n",
        ),
        (
            "py-rhs-crlf-continuation",
            py,
            "main.py",
            "from model import Box\r\nb = Box()\r\nb.count = 1 + \\\r\n    1\r\nassert b.count == 2\r\n",
        ),
        (
            "py-rhs-crlf-nested-read",
            py,
            "main.py",
            "from model import Box\r\nb = Box()\r\nb.count = b.count + \\\r\n    2\r\nassert b.count == 2\r\n",
        ),
        (
            "py-rhs-multiline-fstring",
            py,
            "main.py",
            "from model import Box\nb = Box()\nb.count = f\"\"\"{1\n+ 1}\"\"\"; assert b.count == \"2\"\n",
        ),
        (
            "py-rhs-multiline-nested-fstring-read",
            py,
            "main.py",
            "from model import Box\nb = Box()\nb.count = f\"\"\"{int(f'''{b.count\n+ 1}''')\n+ 1}\"\"\"\nassert b.count == \"2\"\n",
        ),
        (
            "cpp-structured-binding-loop",
            cpp,
            "main.cpp",
            "#include \"box.hpp\"\n#include <utility>\nint main() { std::pair<int, Box> boxes[1]; boxes[0].first = 2; boxes[0].second.count = 0; for (auto& [key, box] : boxes) box.count = key; return boxes[0].second.count == 2 ? 0 : 1; }\n",
        ),
        (
            "cpp-structured-binding-rhs-before-target",
            cpp,
            "main.cpp",
            "#include \"box.hpp\"\n#include <utility>\nstruct Source { Box* volatile* slot; Box* next; operator int() & { *slot = next; return 7; } };\nint main() { Box a, z; a.count = 0; z.count = 0; std::pair<int, Box* volatile> entry{0, &a}; auto& [key, target] = entry; Source source{&entry.second, &z}; target->count = source; return a.count == 0 && z.count == 7 && target->count == 7 ? 0 : 1; }\n",
        ),
        (
            "cpp-structured-binding-temporary-lifetime",
            cpp,
            "main.cpp",
            "#include \"box.hpp\"\n#include <utility>\nstruct Token { Box* volatile* slot; Box* next; int value() { return 7; } ~Token() { *slot = next; } };\nint main() { Box a, z; a.count = 0; z.count = 0; std::pair<int, Box* volatile> entry{0, &a}; auto& [key, target] = entry; target->count = Token{&entry.second, &z}.value(); return a.count == 7 && z.count == 0 && target == &z ? 0 : 1; }\n",
        ),
        (
            "cpp-structured-binding-noncopy-braced-and-lambda",
            cpp_payload,
            "main.cpp",
            "#include \"box.hpp\"\n#include <utility>\nint main() { std::pair<int, Box> boxes[1]; for (auto& [key, box] : boxes) box.count = {7}; if (boxes[0].second.count.value != 7) return 1; int n = 0; ([&]() -> Box& { ++n; return boxes[0].second; })().count = {9}; return boxes[0].second.count.value == 9 && n == 1 ? 0 : 1; }\n",
        ),
        (
            "rust-turbofish-rhs-and-nested-read",
            rust_map,
            "main.rs",
            "mod model;\nuse std::collections::HashMap;\nfn main() {\n let mut b = model::Box::new();\n b.count = HashMap::<String, u32>::new();\n assert!(b.count.is_empty());\n b.count = HashMap::<String, u32>::from([(\"k\".to_string(), b.count.len() as u32 + 7)]);\n assert_eq!(b.count[\"k\"], 7);\n}\n",
        ),
        (
            "rust-closure-params-rhs-and-nested-read",
            rust_fn,
            "main.rs",
            "mod model;\nfn base() -> model::Box { model::Box::new() }\nfn main() {\n let mut b = model::Box::new();\n b.count = |x, y| Ok(x + y);\n assert_eq!((b.count)(2, 3), Ok(5));\n b.count = |x: i32, y: i32| -> Result<i32, u32> { Ok((base().count)(x, y)? + 2) };\n assert_eq!((b.count)(3, 4), Ok(9));\n}\n",
        ),
        (
            "rust-match-bitwise-or-and-turbofish",
            rust_int,
            "main.rs",
            "mod model;\nfn main() {\n let mut b = model::Box::new();\n for flag in [true, false] {\n  match flag {\n   true => b.count = 1 | 2,\n   false => b.count = std::convert::identity::<Result<i32, u32>>(Ok(b.count + 4)).unwrap(),\n  }\n }\n assert_eq!(b.count, 7);\n}\n",
        ),
    ];
    for (shape, owner, file, body) in cases {
        if file == "main.cpp" && !cpp_available {
            eprintln!("skipping {shape}: no C++ compiler available");
            continue;
        }
        let (owner_file, anchor) = match file {
            "main.rs" => ("model.rs", "pub count:"),
            "main.cpp" => ("box.hpp", " count;"),
            _ => ("model.py", "self.count ="),
        };
        for force in [false, true] {
            let name = format!("{shape}-force-{force}");
            ws.write(owner_file, owner);
            ws.write(file, body);
            for (path, source) in [(owner_file, owner), (file, body)] {
                record_source(&name, "input", path, source);
            }
            assert!(
                check_runtime(ws, file),
                "original {name} must compile and run"
            );
            let decl = spot(owner, anchor, "count");
            let owned_refs = refs_for(owner_file, owner, file, body);
            let refs: Vec<(&str, (u32, u32))> = owned_refs
                .iter()
                .map(|(p, pos)| (p.as_str(), *pos))
                .collect();
            emit(format!(
                "REVIEW_REFERENCES {}\n",
                json!({"case": name, "declaration": [owner_file, decl], "references": refs})
            ));
            let gw = gateway(ws, "Box", "count", (owner_file, decl), &refs).await;
            let (addr, root, target) = (gw.addr(), ws.root(), ws.path(owner_file));
            for apply in [false, true] {
                let res = if file == "main.rs" {
                    encapsulate(addr, &root, &target, decl.0, decl.1, None, apply, force).await
                } else {
                    encapsulate_polyglot(
                        addr,
                        &root,
                        &target,
                        Some("Box"),
                        "count",
                        None,
                        apply,
                        force,
                    )
                    .await
                };
                match res {
                    Ok(plan) => {
                        let returned: Vec<_> = plan
                            .rewritten
                            .iter()
                            .map(|(p, s)| {
                                let f = std::path::Path::new(p).file_name().unwrap();
                                json!({"file": f.to_str().unwrap(), "source": s})
                            })
                            .collect();
                        emit(format!(
                            "REVIEW_PLAN {}\n",
                            json!({
                                "case": name, "apply": apply, "applied": plan.applied,
                                "unmatched": plan.unmatched, "returned": returned
                            })
                        ));
                        if plan.applied != apply || !plan.unmatched.is_empty() {
                            failures.push(format!("{name} apply={apply}"));
                        }
                    }
                    Err(err) => {
                        let msg = err.to_string();
                        emit(format!(
                            "REVIEW_PLAN {}\n",
                            json!({"case": name, "apply": apply, "applied": false, "error": msg})
                        ));
                        failures.push(format!("{name} apply={apply} err={err}"));
                    }
                }
                for (path, orig) in [(owner_file, owner), (file, body)] {
                    let current = ws.read(path);
                    let phase = if apply { "applied" } else { "preview" };
                    record_source(&name, phase, path, &current);
                    if !apply && current.as_bytes() != orig.as_bytes() {
                        failures.push(format!("{name} {path} preview mutated"));
                    }
                }
            }
            if !check_runtime(ws, file) {
                failures.push(name);
            }
        }
    }
}

#[tokio::test]
async fn rust_consuming_receiver_keeps_unconditional_rhs() {
    let owner = "pub struct Box {\n pub count: i32,\n}\nimpl Box { pub fn new() -> Self { Self { count: 0 } } }\nimpl Drop for Box { fn drop(&mut self) { assert_eq!(self.count, 7); println!(\"drop=7\"); } }\n";
    let ws = Workspace::new(&[("model.rs", owner), ("main.rs", "")]);
    let mut failures = Vec::new();
    for (shape, body) in [
        (
            "owned",
            "mod model;\nfn main() { let target = Some(model::Box::new()); let answer; target.unwrap().count = { answer = 7; answer }; assert_eq!(answer, 7); println!(\"answer=7\"); }\n",
        ),
        (
            "borrowed",
            "mod model;\nfn main() { let mut b = model::Box::new(); let target = Some(&mut b); let answer; target.unwrap().count = { answer = 7; answer }; assert_eq!(answer, 7); println!(\"answer=7\"); }\n",
        ),
    ] {
        for force in [false, true] {
            let name = format!("rust-consuming-{shape}-force-{force}");
            ws.write("model.rs", owner);
            ws.write("main.rs", body);
            for (file, source) in [("model.rs", owner), ("main.rs", body)] {
                record_source(&name, "input", file, source);
            }
            assert!(check_runtime(&ws, "main.rs"));
            let refs = [
                ("model.rs", spot(owner, "Self { count:", "count")),
                ("model.rs", spot(owner, "self.count, 7", "count")),
                ("main.rs", spot(body, ".count =", "count")),
            ];
            emit(format!("CONTEXT_REFERENCES {name} {refs:?}\n"));
            let decl = spot(owner, "pub count:", "count");
            let gw = gateway(&ws, "Box", "count", ("model.rs", decl), &refs).await;
            let (addr, root, target) = (gw.addr(), ws.root(), ws.path("model.rs"));
            for apply in [false, true] {
                let plan = encapsulate(addr, &root, &target, decl.0, decl.1, None, apply, force)
                    .await
                    .unwrap();
                emit(format!(
                    "CONTEXT_PLAN {name} apply={apply} applied={} unmatched={:?}\n",
                    plan.applied, plan.unmatched
                ));
                if plan.applied != apply || !plan.unmatched.is_empty() {
                    failures.push(format!("{name} apply={apply}"));
                }
                if !apply {
                    assert_eq!(ws.read("model.rs"), owner);
                    assert_eq!(ws.read("main.rs"), body);
                }
            }
            for file in ["model.rs", "main.rs"] {
                record_source(&name, "emitted", file, &ws.read(file));
            }
            if !check_runtime(&ws, "main.rs") {
                failures.push(name);
            }
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
async fn aggregate_review_repairs() {
    let ws = Workspace::new(&[]);
    let mut failures = Vec::new();
    verify_aggregate_review_repairs(&ws, &mut failures).await;
    assert!(failures.is_empty(), "{failures:?}");
}
