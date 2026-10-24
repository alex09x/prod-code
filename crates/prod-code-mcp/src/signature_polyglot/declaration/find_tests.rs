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
fn coverage_b2_declaration_positions_and_modifiers() {
    for (text, lang, name, asyncness, visibility) in [
        ("export async function f<T>(x: T): T { return x; }", Language::TypeScript, "f", true, Some("export ")),
        ("private f(x: number) { return x; }", Language::TypeScript, "f", false, Some("private ")),
        ("public int f(int x) { return x; }", Language::Java, "f", false, Some("public ")),
        ("async def f(x):\n    return x\n", Language::Python, "f", true, None),
        ("func f(x: Int) async -> Int { return x }", Language::Swift, "f", true, None),
        ("func f(x int) int { return x }", Language::Go, "f", false, None),
        ("int f (int x) { return x; }", Language::C, "f", false, None),
    ] {
        let result = find_polyglot_declaration(text, lang, 1, 1).unwrap();
        assert_eq!(result.fn_name, name);
        assert_eq!(result.open_paren, text.find('(').unwrap());
        assert_eq!(result.close_paren, text.find(')').unwrap());
        assert_eq!(result.is_async, asyncness);
        assert_eq!(result.visibility_span.map(|(s,e)| &text[s..e]), visibility);
        assert_eq!(result.params.len(), 1);
    }
    let text = "// before\n\nfunction f() {}\n";
    assert_eq!(find_polyglot_declaration(text, Language::TypeScript, 1, 1).unwrap().fn_name, "f");
    assert_eq!(find_polyglot_declaration(text, Language::TypeScript, 4, 1).unwrap().fn_name, "f");
    assert_eq!(find_polyglot_declaration("no declaration", Language::Python, 1, 1).unwrap_err().to_string(), "could not determine function declaration name");
    assert_eq!(find_polyglot_declaration("def f(", Language::Python, 1, 1).unwrap_err().to_string(), "could not locate declaration for function at line 1");
    assert_eq!(find_polyglot_declaration("", Language::Python, 1, 1).unwrap_err().to_string(), "could not determine function declaration name");
}
