/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */
use super::*;

#[test]
fn coverage_caller_propagation_matrix() {
    use Language::*;
    use Wrapper::*;
    for (lang, ext, declaration, caller, wrapper, now, expected_count, expected_blocked) in [
        (
            Go,
            "go",
            "func selected() int {}",
            "func run() *int {\nselected()\n}\n",
            Pointer,
            "*int",
            1,
            vec![],
        ),
        (
            Go,
            "go",
            "func selected() int {}",
            "func run() int {\nselected()\n}\n",
            Pointer,
            "*int",
            0,
            vec!["input.go:3:1 the caller returns `int`: `selected()`"],
        ),
        (
            Cpp,
            "cpp",
            "int selected() {}",
            "int* run() {\nselected();\n}\n",
            Pointer,
            "int*",
            1,
            vec![],
        ),
        (
            TypeScript,
            "ts",
            "function selected(): number {}",
            "function run(): Box<number> {\nselected();\n}\n",
            Custom("pkg.Box<T>".into()),
            "Box<number>",
            1,
            vec![],
        ),
        (
            TypeScript,
            "ts",
            "function selected(): number {}",
            "function run() {\nselected();\n}\n",
            Custom("Box".into()),
            "Box<number>",
            0,
            vec!["input.ts:3:1 the caller returns `none`: `selected();`"],
        ),
        (
            Python,
            "py",
            "def selected() -> int:\n    pass",
            "def run() -> Optional[int]:\n    selected()\n",
            Option,
            "Optional[int]",
            1,
            vec![],
        ),
        (
            Cpp,
            "cpp",
            "int selected() {}",
            "std::optional<int> run() {\nselected();\n}\n",
            Option,
            "std::optional<int>",
            1,
            vec![],
        ),
        (
            Swift,
            "swift",
            "func selected() -> Int {}",
            "func run() -> Int? {\nselected()\n}\n",
            Option,
            "Int?",
            1,
            vec![],
        ),
        (
            Go,
            "go",
            "func selected() int {}",
            "func run() *int {\nselected()\n}\n",
            Option,
            "*int",
            1,
            vec![],
        ),
        (
            Python,
            "py",
            "def selected() -> int:\n    pass",
            "def run() -> Result[int, Error]:\n    selected()\n",
            Result,
            "Result[int, Error]",
            1,
            vec![],
        ),
        (
            Cpp,
            "cpp",
            "int selected() {}",
            "std::expected<int, Error> run() {\nselected();\n}\n",
            Result,
            "std::expected<int, Error>",
            1,
            vec![],
        ),
        (
            Swift,
            "swift",
            "func selected() -> Int {}",
            "func run() -> Result<Int, Error> {\nselected()\n}\n",
            Result,
            "Result<Int, Error>",
            1,
            vec![],
        ),
        (
            Go,
            "go",
            "func selected() int {}",
            "func run() error {\nselected()\n}\n",
            Result,
            "(int, error)",
            1,
            vec![],
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(format!("input.{ext}"));
        let source = format!("{declaration}\n{caller}");
        std::fs::write(&file, &source).unwrap();
        let decl = find_polyglot_decl(&source, lang, Some("selected"), None).unwrap();
        let mut rewritten = BTreeMap::new();
        let mut count = 0;
        let mut blocked = vec![];
        let mut unmatched = vec![];
        collect_and_rewrite_callers(
            dir.path(),
            &file,
            &source,
            lang,
            &decl,
            &wrapper,
            now,
            &decl.was,
            None,
            None,
            &mut HashSet::new(),
            &mut rewritten,
            &mut count,
            &mut blocked,
            &mut unmatched,
        )
        .unwrap();
        assert_eq!(count, expected_count, "{source}");
        assert_eq!(blocked, expected_blocked, "{source}");
        assert_eq!(unmatched, Vec::<String>::new(), "{source}");
        assert_eq!(rewritten, BTreeMap::new(), "{source}");
    }
}

#[test]
fn coverage_promise_callers_and_unmatched() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("value.ts");
    let source = "export function selected(): number {\nreturn selected();\n}\n";
    std::fs::write(&file, source).unwrap();
    let client = dir.path().join("client.ts");
    let input = "import * as api from './value';\nasync function run() {\napi.selected()[0];\nawait api.selected();\n}\nfunction sync() {\napi.selected();\n}\nconst value = api.selected;\n";
    std::fs::write(&client, input).unwrap();
    let decl = find_polyglot_decl(source, Language::TypeScript, Some("selected"), None).unwrap();
    let mut rewritten = BTreeMap::new();
    let mut count = 0;
    let mut blocked = vec![];
    let mut unmatched = vec![];
    collect_and_rewrite_callers(
        dir.path(),
        &file,
        source,
        Language::TypeScript,
        &decl,
        &Wrapper::Promise,
        "Promise<number>",
        "number",
        None,
        None,
        &mut HashSet::new(),
        &mut rewritten,
        &mut count,
        &mut blocked,
        &mut unmatched,
    )
    .unwrap();
    assert_eq!(count, 2);
    assert_eq!(
        rewritten.get(&client).unwrap(),
        "import * as api from './value';\nasync function run() {\n(await api.selected())[0];\nawait api.selected();\n}\nfunction sync() {\napi.selected();\n}\nconst value = api.selected;\n"
    );
    assert_eq!(
        blocked,
        vec![
            "client.ts:7:5 the caller is not async and cannot await `selected`: `api.selected();`"
        ]
    );
    unmatched.sort();
    assert_eq!(
        unmatched,
        vec![
            "client.ts:9:19 `selected` used as a value",
            "value.ts:2:8 (a call inside `selected` itself)"
        ]
    );
}
