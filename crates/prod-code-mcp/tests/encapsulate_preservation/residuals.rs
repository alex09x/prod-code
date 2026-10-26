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

fn record(case: &str, phase: &str, file: &str, source: &str) {
    emit(format!(
        "RESIDUAL_SOURCE {}\n",
        json!({"case": case, "phase": phase, "file": file, "source": source})
    ));
}

fn execute(ws: &Workspace, rust: bool, case: &str, phase: &str) -> Option<Vec<u8>> {
    if rust {
        let compile = run(ws, "rustc", &["--edition=2024", "main.rs", "-o", "probe"]);
        emit(format!(
            "RESIDUAL_EXECUTION {}\n",
            json!({"case": case, "phase": phase, "program": "rustc", "status": compile.status.code()})
        ));
        if !compile.status.success() {
            return None;
        }
    }
    let out = if rust {
        run(ws, "./probe", &[])
    } else {
        run(ws, "python3", &["main.py"])
    };
    emit(format!(
        "RESIDUAL_EXECUTION {}\n",
        json!({"case": case, "phase": phase, "program": if rust { "./probe" } else { "python3" }, "status": out.status.code(), "stdout": String::from_utf8_lossy(&out.stdout)})
    ));
    out.status.success().then_some(out.stdout)
}

// Semantic locations are scripted; compiler/runtime results are native observations.
async fn verify(ws: &Workspace, shape: &str, owner: &str, body: &str, rust: bool) -> Vec<String> {
    let mut failures = Vec::new();
    let (owner_file, file, anchor) = if rust {
        ("model.rs", "main.rs", "pub count:")
    } else {
        ("model.py", "main.py", "self.count =")
    };
    for force in [false, true] {
        let case = format!("{shape}-force-{force}");
        ws.write(owner_file, owner);
        ws.write(file, body);
        for (path, source) in [(owner_file, owner), (file, body)] {
            record(&case, "input", path, source);
        }
        let original = execute(ws, rust, &case, "original")
            .expect("complete original program must compile and run");
        let decl = spot(owner, anchor, "count");
        let mut refs = Vec::new();
        for (path, source) in [(owner_file, owner), (file, body)] {
            for (at, _) in source.match_indices("count") {
                let prefix = &source[..at];
                let pos = (
                    prefix.bytes().filter(|b| *b == b'\n').count() as u32 + 1,
                    prefix.rsplit('\n').next().unwrap().encode_utf16().count() as u32 + 1,
                );
                if path != owner_file || pos != decl {
                    refs.push((path, pos));
                }
            }
        }
        emit(format!(
            "RESIDUAL_REFERENCES {}\n",
            json!({"case": case, "declaration": [owner_file, decl], "references": refs})
        ));
        let gw = gateway(ws, "Box", "count", (owner_file, decl), &refs).await;
        let mut proposed = None;
        for apply in [false, true] {
            let plan = if rust {
                encapsulate(
                    gw.addr(),
                    &ws.root(),
                    &ws.path(owner_file),
                    decl.0,
                    decl.1,
                    None,
                    apply,
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
                    apply,
                    force,
                )
                .await
            };
            match plan {
                Ok(plan) => {
                    let returned: Vec<_> = plan
                        .rewritten
                        .iter()
                        .map(|(path, source)| {
                            let file = std::path::Path::new(path)
                                .file_name()
                                .unwrap()
                                .to_str()
                                .unwrap();
                            record(
                                &case,
                                if apply {
                                    "returned-applied"
                                } else {
                                    "returned-preview"
                                },
                                file,
                                source,
                            );
                            (file.to_string(), source.clone())
                        })
                        .collect();
                    emit(format!(
                        "RESIDUAL_PLAN {}\n",
                        json!({"case": case, "apply": apply, "applied": plan.applied, "unmatched": plan.unmatched, "returned": returned})
                    ));
                    if plan.applied != apply || !plan.unmatched.is_empty() {
                        failures.push(format!("{case}: incomplete plan apply={apply}"));
                    }
                    if apply {
                        if proposed.as_ref() != Some(&returned) {
                            failures.push(format!("{case}: returned preview/application differ"));
                        }
                        for (path, source) in returned {
                            if ws.read(&path) != source {
                                failures.push(format!("{case}: returned/disk differ {path}"));
                            }
                        }
                    } else {
                        proposed = Some(returned);
                    }
                }
                Err(err) => {
                    emit(format!(
                        "RESIDUAL_PLAN {}\n",
                        json!({"case": case, "apply": apply, "error": err.to_string()})
                    ));
                    failures.push(format!("{case}: apply={apply}: {err}"));
                }
            }
            for (path, source) in [(owner_file, owner), (file, body)] {
                let actual = ws.read(path);
                record(
                    &case,
                    if apply { "applied" } else { "preview" },
                    path,
                    &actual,
                );
                if !apply && actual != source {
                    failures.push(format!("{case}: preview mutated {path}"));
                }
            }
        }
        let generated = execute(ws, rust, &case, "generated");
        if generated.as_ref() != Some(&original) {
            failures.push(format!(
                "{case}: generated runtime/observable output differs"
            ));
        }
        for comment in body
            .lines()
            .filter_map(|line| line.find("# KEEP").map(|at| &line[at..]))
        {
            if !ws.read(file).contains(comment) {
                failures.push(format!("{case}: comment changed {comment}"));
            }
        }
    }
    failures
}

#[tokio::test]
async fn adjacent_python_string_comments() {
    let owner = "class Box:\n def __init__(self):\n  self.count = None\n";
    let ws = Workspace::new(&[("model.py", owner), ("main.py", "")]);
    let mut failures = Vec::new();
    for (shape, rhs) in [
        ("string", "\"ok\""),
        ("hash-string", "\"# opaque\""),
        ("bytes", "b\"# opaque\""),
        ("raw", "r\"# opaque\\tail\""),
        ("fstring", "f\"{value}# opaque\""),
        ("triple", "\"\"\"# opaque\nsecond line\"\"\""),
        ("ftriple", "f\"\"\"{value}\n# opaque\"\"\""),
        ("concat", "\"one\" '# opaque'"),
        ("bracket-comment", "(\"ok\"# KEEP inside\n )"),
    ] {
        let body = format!(
            "from model import Box\nb = Box()\nvalue = 7\nb.count = {rhs}# KEEP adjacent {shape}\nprint(repr(b.count))\n"
        );
        failures.extend(verify(&ws, shape, owner, &body, false).await);
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn form_feed_python_suite_nesting() {
    let owner = "class Box:\n def __init__(self):\n  self.count = 0\n";
    let ws = Workspace::new(&[("model.py", owner), ("main.py", "")]);
    let mut failures = Vec::new();
    for (shape, indent, prefix) in [
        ("form-feed", "\x0c    ", ""),
        ("reset", "  \x0c    ", ""),
        ("tab-reset", "\t\x0c    ", ""),
        ("semicolon", "\x0c    ", "value = 1; "),
        ("bracket", "\x0c    ", "value = (\n0); "),
        ("triple", "\x0c    ", "value = '''a\nb'''; "),
        ("backslash", "\x0c    ", "value = 1 + \\\n0; "),
    ] {
        for (ending, newline, bom) in [("lf", "\n", ""), ("bom-crlf", "\r\n", "\u{feff}")] {
            let body = format!("{bom}from model import Box\ndef update(b):\n{indent}{prefix}b.count = 2# KEEP suite\n{indent}assert b.count == 2\nb = Box()\nupdate(b)\nprint(b.count)\n").replace('\n', newline);
            failures.extend(verify(&ws, &format!("{shape}-{ending}"), owner, &body, false).await);
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn qualified_rust_paths_and_outer_boundaries() {
    let owner = "pub struct Box {\n pub count: i32,\n}\nimpl Box { pub fn new() -> Self { Self { count: 0 } } }\n";
    let map_owner = "pub struct Box {\n pub count: std::collections::HashMap<String, u32>,\n}\nimpl Box { pub fn new() -> Self { Self { count: std::collections::HashMap::new() } } }\n";
    let ws = Workspace::new(&[("model.rs", owner), ("main.rs", "")]);
    let mut failures = Vec::new();
    let prelude = "mod model;\ntrait Trait { fn method() -> i32 { 7 } }\nstruct Type<A, B>(A, B);\nimpl<A, B> Trait for Type<A, B> {}\ntrait Carrier { type Assoc; }\nimpl<A, B> Carrier for Type<A, B> { type Assoc = Type<A, B>; }\n";
    for (shape, expression, expected) in [
        ("qualified-trait", "<Type<i32, u32> as Trait>::method()", 7),
        (
            "nested-qualified",
            "<Type<Result<i32, u32>, (i32, u32)> as Trait>::method()",
            7,
        ),
        (
            "comment-qualified",
            "<Type<i32 /* , > */, u32> as Trait>::method()",
            7,
        ),
        ("comparison", "i32::from(1 < 2)", 1),
        ("shift", "1 << 2 | 1", 5),
        (
            "binary-qualified",
            "1 + <Type<i32, u32> as Trait>::method()",
            8,
        ),
        (
            "double-qualified",
            "<<Type<i32, u32> as Carrier>::Assoc as Trait>::method()",
            7,
        ),
        (
            "comment-before-qualified",
            "/* \" /* nested */ */ <Type<i32, u32> as Trait>::method()",
            7,
        ),
        (
            "const-generic",
            "<Type<[u8; { 1 << 1 }], u32> as Trait>::method()",
            7,
        ),
        (
            "turbofish",
            "std::convert::identity::<Result<i32, u32>>(Ok(7)).unwrap()",
            7,
        ),
        ("closure", "(|x, y| x | y)(3, 4)", 7),
        (
            "mixed-comparison",
            "if 1 < 2 { <Type<i32, u32> as Trait>::method() } else { 0 }",
            7,
        ),
    ] {
        let body = format!(
            "{prelude}fn main() {{ let mut b = model::Box::new(); match true {{ true => b.count = {expression}, false => b.count = 99, }} assert_eq!(b.count, {expected}); println!(\"{{}}\", b.count); }}\n"
        );
        failures.extend(verify(&ws, shape, owner, &body, true).await);
    }
    let body = "mod model;\nfn main() { let mut b = model::Box::new(); b.count = <std::collections::HashMap<String, u32>>::new(); assert!(b.count.is_empty()); b.count = <std::collections::HashMap<String, u32>>::from([(\"k\".into(), 7)]); assert_eq!(b.count[\"k\"], 7); println!(\"{}\", b.count[\"k\"]); }\n";
    failures.extend(verify(&ws, "qualified-inherent", map_owner, body, true).await);
    let bool_owner = "pub struct Box {\n pub count: bool,\n}\nimpl Box { pub fn new() -> Self { Self { count: false } } }\n";
    for (shape, expression) in [
        ("outer-comparison", "1 < 2"),
        ("outer-less-equal", "1 <= 2"),
        ("outer-shift-comparison", "1 << 2 < 9"),
        (
            "outer-qualified-comparison",
            "1 < <Type<i32, u32> as Trait>::method()",
        ),
        ("outer-string-comparison", "\"a\" < \"b\""),
    ] {
        let body = format!(
            "{prelude}fn main() {{ let mut b = model::Box::new(); match true {{ true => b.count = {expression}, false => b.count = false, }} assert!(b.count); println!(\"{{}}\", b.count); }}\n"
        );
        failures.extend(verify(&ws, shape, bool_owner, &body, true).await);
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
