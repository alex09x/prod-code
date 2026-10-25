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
fn coverage_w2_semantic_callers_only_rewrite_resolved_references() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("value.ts");
    let source = "export function selected(): number { return 1; }\nasync function run() {\nselected();\nother.selected();\n}\n";
    std::fs::write(&file, source).unwrap();
    let client = dir.path().join("client.ts");
    let imported = "import { selected } from './value';\nasync function next() {\nselected();\n}\n";
    std::fs::write(&client, imported).unwrap();
    let decl = find_polyglot_decl(source, Language::TypeScript, Some("selected"), None).unwrap();
    let mut references = HashSet::from([
        (file.canonicalize().unwrap(), 3, 1),
        (client.canonicalize().unwrap(), 1, 10),
    ]);
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
        &mut references,
        &mut rewritten,
        &mut count,
        &mut blocked,
        &mut unmatched,
    )
    .unwrap();
    assert_eq!(count, 1);
    assert_eq!(rewritten, BTreeMap::from([(file, "export async function selected(): Promise<number> { return 1; }\nasync function run() {\nawait selected();\nother.selected();\n}\n".into())]));
    assert!(references.is_empty());
    assert!(blocked.is_empty());
    assert!(unmatched.is_empty());
    assert_eq!(std::fs::read_to_string(client).unwrap(), imported);
}

#[test]
fn coverage_w2_caller_refusals_name_the_exact_incompatible_return() {
    use Language::*;
    use Wrapper::*;
    for (lang, ext, declaration, caller, wrapper, now, expected_ret) in [
        (
            TypeScript,
            "ts",
            "function selected(): number {}",
            "function run(): number {\nselected();\n}",
            Option,
            "number | null",
            "number",
        ),
        (
            JavaScript,
            "js",
            "function selected(): number {}",
            "function run(): number {\nselected();\n}",
            Result,
            "Result<number, Error>",
            "number",
        ),
        (
            Python,
            "py",
            "def selected() -> int:\n    pass",
            "def run() -> int:\n    selected()",
            Option,
            "Optional[int]",
            "int",
        ),
        (
            Python,
            "py",
            "def selected() -> int:\n    pass",
            "def run():\n    selected()",
            Result,
            "Result[int, Error]",
            "None",
        ),
        (
            Cpp,
            "cpp",
            "int selected() {}",
            "int run() {\nselected();\n}",
            Option,
            "std::optional<int>",
            "int",
        ),
        (
            Cpp,
            "cpp",
            "int selected() {}",
            "int run() {\nselected();\n}",
            Result,
            "std::expected<int, Error>",
            "int",
        ),
        (
            Swift,
            "swift",
            "func selected() -> Int {}",
            "func run() -> Int {\nselected()\n}",
            Option,
            "Int?",
            "Int",
        ),
        (
            Swift,
            "swift",
            "func selected() -> Int {}",
            "func run() -> Int {\nselected()\n}",
            Result,
            "Result<Int, Error>",
            "Int",
        ),
        (
            Go,
            "go",
            "func selected() int {}",
            "func run() int {\nselected()\n}",
            Option,
            "*int",
            "int",
        ),
        (
            Go,
            "go",
            "func selected() int {}",
            "func run() int {\nselected()\n}",
            Result,
            "(int, error)",
            "int",
        ),
        (
            Cpp,
            "cpp",
            "int selected() {}",
            "void run() {\nselected();\n}",
            Pointer,
            "int*",
            "void",
        ),
        (
            TypeScript,
            "ts",
            "function selected(): number {}",
            "function run(): number {\nselected();\n}",
            Custom("pkg::Box<T>".into()),
            "Box<number>",
            "number",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(format!("input.{ext}"));
        let source = format!("{declaration}\n{caller}\n");
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
        let at = source.rfind("selected()").unwrap();
        let (line, col) = one_based_lsp_position(&source, at);
        let line_text = source.lines().nth(line as usize - 1).unwrap().trim();
        assert_eq!(
            blocked,
            [format!(
                "input.{ext}:{line}:{col} the caller returns `{expected_ret}`: `{line_text}`"
            )],
            "{source}"
        );
        assert_eq!(count, 0);
        assert!(unmatched.is_empty());
        assert!(rewritten.is_empty());
    }
}

#[test]
fn coverage_w2_cpp_prototype_is_rewritten_without_counting_as_a_caller() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("value.cpp");
    let header = dir.path().join("value.h");
    let source = "int selected() { return 1; }\n";
    std::fs::write(&file, source).unwrap();
    std::fs::write(&header, "int selected();\n").unwrap();
    let decl = find_polyglot_decl(source, Language::Cpp, Some("selected"), None).unwrap();
    let mut rewritten = BTreeMap::new();
    let mut count = 0;
    let mut blocked = vec![];
    let mut unmatched = vec![];
    let mut references = HashSet::from([(header.canonicalize().unwrap(), 1, 5)]);
    collect_and_rewrite_callers(
        dir.path(),
        &file,
        source,
        Language::Cpp,
        &decl,
        &Wrapper::Option,
        "std::optional<int>",
        "int",
        None,
        None,
        &mut references,
        &mut rewritten,
        &mut count,
        &mut blocked,
        &mut unmatched,
    )
    .unwrap();
    assert_eq!(
        rewritten,
        BTreeMap::from([(header, "std::optional<int> selected();\n".into())])
    );
    assert_eq!(count, 0);
    assert!(references.is_empty());
    assert!(blocked.is_empty());
    assert!(unmatched.is_empty());
}
