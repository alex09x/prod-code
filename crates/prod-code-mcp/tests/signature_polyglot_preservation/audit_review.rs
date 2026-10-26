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
use prod_code_mcp::signature::{Modifiers, Param, change_with};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};
use tokio::sync::{Mutex as AsyncMutex, MutexGuard};

static SERIAL: AsyncMutex<()> = AsyncMutex::const_new(());

struct Fixture {
    _guard: MutexGuard<'static, ()>,
    workspace: Workspace,
    gateway: ScriptedGateway,
    references: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl Fixture {
    async fn new() -> Self {
        let _guard = SERIAL.lock().await;
        let references = Arc::new(Mutex::new(Vec::new()));
        let selected = Arc::clone(&references);
        let gateway = ScriptedGateway::start(move |method, _| match method {
            "textDocument/references" => serde_json::json!(*selected.lock().unwrap()),
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => serde_json::Value::Null,
        })
        .await;
        Self {
            _guard,
            workspace: Workspace::empty(),
            gateway,
            references,
        }
    }
}

fn try_run(root: &Path, program: &str, args: &[&str]) -> (bool, String) {
    let mut attempts = 0usize;
    let output = loop {
        match Command::new(program).args(args).current_dir(root).output() {
            Ok(output) => break output,
            Err(e) if matches!(e.raw_os_error(), Some(11 | 24)) && attempts < 40 => {
                attempts += 1;
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => panic!("required runtime {program}: {e}"),
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr);
    eprintln!(
        "{program} {args:?}: status={} stdout={stdout:?} stderr={stderr:?}",
        output.status
    );
    (output.status.success(), stdout)
}

fn snapshot(root: &Path, file: &str, label: &str) -> String {
    let source = std::fs::read_to_string(root.join(file)).unwrap();
    eprintln!("SNAPSHOT {label} {file}\n{source}END SNAPSHOT");
    assert!(try_run(root, "sha256sum", &[file]).0, "sha256sum {file}");
    source
}

fn execute_program(root: &Path, file: &str, source: &str) -> (bool, String) {
    if file.ends_with("py") {
        return try_run(root, "python3", &[file]);
    }
    if file.ends_with("js") && !source.contains("<div") {
        return try_run(root, "node", &[file]);
    }
    let mut tsc = vec![file];
    tsc.extend(
        "--jsx react --allowJs --noImplicitAny false --target es2022 --module es2022 --outDir out"
            .split(' '),
    );
    if !try_run(root, "tsc", &tsc).0 {
        let _ = std::fs::remove_dir_all(root.join("out"));
        return (false, String::new());
    }
    let js = std::fs::read_to_string(root.join("out/main.js")).unwrap();
    std::fs::remove_dir_all(root.join("out")).unwrap();
    try_run(root, "node", &["--input-type=module", "--eval", &js])
}

// Every case first runs the complete original program with a real runtime (and
// tsc for JSX, including JSX-in-.js). Record exact source pairs and zero-based
// UTF-16 reference ranges.
async fn check_case(
    fixture: &Fixture,
    label: &str,
    source: &str,
    needles: &[&str],
    asyncness: Option<bool>,
    expected: Option<&str>,
) -> Vec<String> {
    let file = if source.starts_with("def ") {
        "main.py"
    } else if source.contains("<div>{0}") {
        "main.jsx"
    } else {
        "main.js"
    };
    let selection = if label == "neighbor" {
        (1, 18)
    } else if label.starts_with("jsx-js-") {
        (2, 10)
    } else if file == "main.py" {
        (1, 5)
    } else if source.starts_with("async") {
        (1, 16)
    } else {
        (1, 10)
    };
    let params = if label.starts_with("async-") {
        vec![Param::Keep("x".into())]
    } else if label.starts_with("nested-") && expected.is_some() {
        vec![
            Param::Keep("a".into()),
            Param::Keep("b".into()),
            Param::Add {
                name: "c".into(),
                ty: String::new(),
                value: "0".into(),
            },
        ]
    } else {
        vec![Param::Keep("b".into()), Param::Keep("a".into())]
    };
    let jsx_out = "[\"div\",null,[\"p\",null,\"ready to export \",12]]\n";
    let (before_stdout, after_stdout) = match label {
        "neighbor" => ("12\n34\n", "12\n34\n"),
        "default" => ("[[1,2],3]\n", "[[1,2],3]\n"),
        "nested-arity-add" => ("2\n", "3\n"),
        l if l.starts_with("nested-") => ("123\n", "123\n"),
        l if l.starts_with("export-") => ("12\n", "12\n"),
        l if l.starts_with("jsx-js-") => (jsx_out, jsx_out),
        _ => ("1\n", "1\n"),
    };
    let mut failures = Vec::new();
    for force in [false, true] {
        // Public API sessions are pooled for the process lifetime. Reuse one
        // workspace/gateway per matrix, rather than retaining extra pools.
        let root = &fixture.workspace.root();
        for previous in ["main.js", "main.jsx", "main.py"] {
            if previous != file && root.join(previous).exists() {
                std::fs::remove_file(root.join(previous)).unwrap();
            }
        }
        fixture.workspace.write(file, source);
        let label = format!("audit-{label}-force-{force}");
        snapshot(root, file, &format!("{label}-before"));
        let (before_ok, before_out) = execute_program(root, file, source);
        assert!(
            before_ok && before_out == before_stdout,
            "original fixture {label}"
        );
        let refs = needles
            .iter()
            .map(|needle| location(root, file, source, needle))
            .collect::<Vec<_>>();
        eprintln!(
            "{label} selection={selection:?} apply=true force={force} asyncness={asyncness:?} parameters={params:?} exact references={refs:?}"
        );
        let mut attempts = 0usize;
        let result = loop {
            *fixture.references.lock().unwrap() = refs.clone();
            let res = change_with(
                fixture.gateway.addr(),
                root,
                &root.join(file),
                selection.0,
                selection.1,
                &params,
                &Modifiers {
                    asyncness,
                    ..Modifiers::default()
                },
                true,
                force,
            )
            .await;
            if res.as_ref().is_err_and(|e| {
                let msg = format!("{e:#}");
                msg.contains("Too many open files") || msg.contains("os error 24")
            }) && attempts < 40
            {
                attempts += 1;
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                continue;
            }
            break res;
        };
        eprintln!("{label} public result: {result:?}");
        let actual = snapshot(root, file, &format!("{label}-after"));
        // Run even the erroneous transformed source to retain observed behavior.
        let (after_ok, output) = execute_program(root, file, &actual);
        let valid = match expected {
            Some(expected) => {
                result.is_ok() && actual == expected && after_ok && output == after_stdout
            }
            None => {
                result.is_err_and(|e| e.to_string().contains("nothing was written"))
                    && actual.as_bytes() == source.as_bytes()
                    && after_ok
                    && output == before_stdout
            }
        };
        if !valid {
            failures.push(label);
        }
    }
    failures
}

#[tokio::test]
async fn exact_declaration_reference_preserves_neighboring_calls() {
    let f = Fixture::new().await;
    let src = "f(1,2); function f(a,b) { console.log(a*10+b); }\nf(3,4);\n";
    let exp = Some("f(2, 1); function f(b, a) { console.log(a*10+b); }\nf(4, 3);\n");
    let needles = ["f(1,2)", "f(a,b)", "f(3,4)"];
    let failures = check_case(&f, "neighbor", src, &needles, None, exp).await;
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
async fn overlapping_default_call_refuses_atomically() {
    let f = Fixture::new().await;
    let src = "function f(a=f(1,2), b=3) { return [a,b]; }\nconsole.log(JSON.stringify(f(undefined,3)));\n";
    let needles = ["f(a=", "f(1,2)", "f(undefined,3)"];
    let mut failures = check_case(&f, "default", src, &needles, None, None).await;
    for (label, asyncness, src, exp) in [
        (
            "nested-arity-add",
            None,
            "function f(a, b) { return arguments.length; }\nconsole.log(f(f(1, 2), 3));\n",
            Some(
                "function f(a, b, c = 0) { return arguments.length; }\nconsole.log(f(f(1, 2, 0), 3, 0));\n",
            ),
        ),
        (
            "nested-value-add",
            None,
            "function f(a, b) { return a * 10 + b; }\nconsole.log(f(f(1, 2), 3));\n",
            Some(
                "function f(a, b, c = 0) { return a * 10 + b; }\nconsole.log(f(f(1, 2, 0), 3, 0));\n",
            ),
        ),
        (
            "nested-reorder-refusal",
            None,
            "function f(a, b) { return a * 10 + b; }\nconsole.log(f(f(1, 2), 3));\n",
            None,
        ),
        (
            "nested-async-enable-add",
            Some(true),
            "function f(a, b) { return a * 10 + b; }\nasync function main() { console.log(f(f(1, 2), 3)); }\nmain();\n",
            Some(
                "async function f(a, b, c = 0) { return a * 10 + b; }\nasync function main() { console.log((await f((await f(1, 2, 0)), 3, 0))); }\nmain();\n",
            ),
        ),
        (
            "nested-async-disable-add",
            Some(false),
            "async function f(a, b) { return a * 10 + b; }\nasync function main() { console.log(await f(await f(1, 2), 3)); }\nmain();\n",
            Some(
                "function f(a, b, c = 0) { return a * 10 + b; }\nasync function main() { console.log(f(f(1, 2, 0), 3, 0)); }\nmain();\n",
            ),
        ),
        (
            "nested-python-add",
            None,
            "def f(a, b):\n    return a * 10 + b\nprint(f(f(1, 2), 3))\n",
            Some("def f(a, b, c = 0):\n    return a * 10 + b\nprint(f(f(1, 2, 0), 3, 0))\n"),
        ),
    ] {
        let outer = if src.contains("await f(1, 2)") {
            "f(await f(1, 2), 3)"
        } else {
            "f(f(1, 2), 3)"
        };
        let needles = ["f(a, b)", outer, "f(1, 2)"];
        failures.extend(check_case(&f, label, src, &needles, asyncness, exp).await);
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
async fn export_property_asi_block_keeps_calls_and_callbacks_visible() {
    let f = Fixture::new().await;
    let mut failures = Vec::new();
    for property in ["o.export", "o?.export"] {
        for callback in [false, true] {
            let body = if callback {
                "const callback = f; console.log(callback(1,2));"
            } else {
                "console.log(f(1,2));"
            };
            let src = format!(
                "function f(a,b) {{ return a*10+b; }}\nconst o = {{export: 0}};\n{property}\n{{ {body} }}\n"
            );
            let expected = src
                .replace("f(a,b)", "f(b, a)")
                .replace("f(1,2)", "f(2, 1)");
            let exp = (!callback).then_some(expected.as_str());
            let needles = ["f(a,b)", if callback { "f;" } else { "f(1,2)" }];
            let label = format!("export-{property}-{callback}");
            failures.extend(check_case(&f, &label, &src, &needles, None, exp).await);
        }
    }
    for (suffix, sep) in [("inline", " "), ("newline", "\n")] {
        let src = format!(
            "const React = {{ createElement: (...args) => args }};\nfunction f(a, b) {{ return a * 10 + b; }}\nconst view = <div><p>ready to{sep}export {{f(1, 2)}}</p></div>;\nconsole.log(JSON.stringify(view));\n"
        );
        let expected = src
            .replace("f(a, b)", "f(b, a)")
            .replace("f(1, 2)", "f(2, 1)");
        let label = format!("jsx-js-export-{suffix}-call");
        let needles = ["f(a, b)", "f(1, 2)"];
        failures.extend(check_case(&f, &label, &src, &needles, None, Some(&expected)).await);
    }
    let src = "const React = { createElement: (...args) => args };\nfunction f(a, b) { return a * 10 + b; }\nconst view = <div><p>ready to export {[f].map(fn => fn(1, 2))[0]}</p></div>;\nconsole.log(JSON.stringify(view));\n";
    let needles = ["f(a, b)", "f]"];
    let label = "jsx-js-export-callback";
    failures.extend(check_case(&f, label, src, &needles, None, None).await);
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
async fn labeled_regex_async_both_directions_preserve_expression_contexts() {
    let f = Fixture::new().await;
    let mut failures = Vec::new();
    for unrelated in [
        "label: {} /[)]/.test(\")\");",
        "const unused = 0\nlabel: {} /[)]/.test(\")\");",
        "function unused() { return\nlabel: {} /[)]/.test(\")\"); }",
        "function unused() { return/*\n*/label: {} /[)]/.test(\")\"); }",
        "function* unused() { yield\nlabel: {} /[)]/.test(\")\"); }",
        "function unused() { return {label: {}} / 2; }",
        "function unused() { return\n{} /[)]/.test(\")\"); }",
        "function unused() { return/*\n*/{} /[)]/.test(\")\"); }",
        "function* unused() { yield\n{} /[)]/.test(\")\"); }",
        "function unused() { return {} / 2; }",
        "switch (0) { case 1: {} /[)]/.test(\")\"); }",
        "const a = 1; switch (0) { case a: {} /[)]/.test(\")\"); }",
        "switch (0) { default: {} /[)]/.test(\")\"); }",
        "switch (0) { case 1: default: {} /[)]/.test(\")\"); }",
        "switch (0) { case 1: label: {} /[)]/.test(\")\"); }",
        "outer: inner: {} /[)]/.test(\")\");",
        "label: { nested: {} /[)]/.test(\")\"); } /[)]/.test(\")\");",
        "const unused = {key: {}} / 2; const conditional = true ? {} : {} / 2;",
        "const unused = `${0}${{key: {}} / 2}`;",
        "const React = { createElement: (...args) => args }; const unused = <div>{0}{{key: {}} / 2}</div>;",
    ] {
        for enable in [false, true] {
            let keyword = if enable { "" } else { "async " };
            let await_keyword = if enable { "" } else { "await " };
            let src = format!(
                "{keyword}function f(x) {{ return x; }}\n{unrelated}\nasync function main() {{ console.log({await_keyword}f(1)); }}\nmain();\n"
            );
            let exp = if enable {
                src.replacen("function f", "async function f", 1)
                    .replace("f(1)", "(await f(1))")
            } else {
                src.replacen("async function f", "function f", 1)
                    .replace("await f(1)", "f(1)")
            };
            let label = format!("async-{enable}-{unrelated}");
            let needles = ["f(x)", "f(1)"];
            failures.extend(check_case(&f, &label, &src, &needles, Some(enable), Some(&exp)).await);
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}
