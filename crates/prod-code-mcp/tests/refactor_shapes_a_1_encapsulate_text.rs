/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Text-level pieces of the polyglot encapsulate-field refactoring: Go and Python
//! rewriting, the line replacers and the analyzer symbol lookup.

use prod_code_mcp::encapsulate_field::{
    encapsulate_field_go, encapsulate_field_py, field_position_in_symbols, lsp_symbol_position,
    replace_cpp_unqualified, replace_line_self, replace_line_this, replace_line_this_private,
    rewrite_external_go, rewrite_external_py,
};
use serde_json::json;

#[test]
fn external_go_accesses_become_accessor_calls() {
    let cases: &[(&str, &str, &str, usize, usize)] = &[
        ("x := c.Count\n", "count", "x := c.Count()\n", 1, 0),
        ("c.count = 5\n", "count", "c.SetCount(5)\n", 0, 1),
        (
            "if c.Count == 3 {\n",
            "count",
            "if c.Count() == 3 {\n",
            1,
            0,
        ),
        ("c.Count()\n", "count", "c.Count()\n", 0, 0),
        (
            "c.CountMax\nc.count_x\n",
            "count",
            "c.CountMax\nc.count_x\n",
            0,
            0,
        ),
        (
            "// c.Count\n  * c.Count\n",
            "count",
            "// c.Count\n  * c.Count\n",
            0,
            0,
        ),
        ("c.Count", "count", "c.Count()", 1, 0),
        (
            "a.Count + b.Count\n",
            "count",
            "a.Count() + b.Count()\n",
            2,
            0,
        ),
        ("c.count\n", "count", "c.Count()\n", 1, 0),
        ("c.Size = n\n", "size", "c.SetSize(n)\n", 0, 1),
        ("plain line\n", "count", "plain line\n", 0, 0),
    ];
    for (idx, (code, field, expected, reads, writes)) in cases.iter().enumerate() {
        let got = rewrite_external_go(code, field);
        assert_eq!(got, (expected.to_string(), *reads, *writes), "case {idx}");
    }
}

#[test]
fn go_struct_field_is_hidden_behind_accessors() {
    let src = "package p\n\ntype Box struct {\n\t// the count\n\tcount int\n\tname string\n}\n";
    let (owner, ty, code, reads, writes, left) = encapsulate_field_go(src, None, "count").unwrap();
    assert_eq!(
        (owner.as_str(), ty.as_str(), reads, writes, left),
        ("Box", "int", 0, 0, 0)
    );
    assert_eq!(
        code,
        "package p\n\ntype Box struct {\n\t// the count\n\tcount int\n\tname string\n}\n\nfunc (b *Box) Count() int {\n\treturn b.count\n}\n\nfunc (b *Box) SetCount(count int) {\n\tb.count = count\n}\n"
    );

    let two = "type A struct {\n\tx int\n}\n\ntype B struct {\n\tx string\n}\n";
    let (owner, ty, code, ..) = encapsulate_field_go(two, Some("B"), "x").unwrap();
    assert_eq!((owner.as_str(), ty.as_str()), ("B", "string"));
    assert_eq!(
        code,
        "type A struct {\n\tx int\n}\n\ntype B struct {\n\tx string\n}\n\nfunc (b *B) X() string {\n\treturn b.x\n}\n\nfunc (b *B) SetX(x string) {\n\tb.x = x\n}\n"
    );

    // an upper-case name is lowered as a whole, and receivers follow Unicode names
    let acronym = "type C struct {\n\tuRL string\n}\n";
    let (_, _, code, ..) = encapsulate_field_go(acronym, None, "URL").unwrap();
    assert_eq!(
        code,
        "type C struct {\n\turl string\n}\n\nfunc (c *C) URL() string {\n\treturn c.url\n}\n\nfunc (c *C) SetURL(url string) {\n\tc.url = url\n}\n"
    );
    let unicode = "type Ωmega struct {\n\tn int\n}\n";
    let (_, _, code, ..) = encapsulate_field_go(unicode, None, "n").unwrap();
    assert!(code.ends_with("func (ω *Ωmega) N() int {\n\treturn ω.n\n}\n\nfunc (ω *Ωmega) SetN(n int) {\n\tω.n = n\n}\n"));
}

#[test]
fn go_encapsulation_refusals() {
    let cases: &[(&str, Option<&str>, &str, &str)] = &[
        ("package p\n", None, "x", "Could not find struct in Go file"),
        (
            "type Box struct {\n\tx int\n}\n",
            Some("Other"),
            "x",
            "Could not find struct in Go file",
        ),
        (
            "type Box struct {\n\tx int\n",
            None,
            "x",
            "Could not find closing brace of Go struct",
        ),
        (
            "type Box struct {\n\tx int\n}\n",
            None,
            "missing",
            "Field `missing` not found in struct `Box`",
        ),
        (
            "type Box struct {\n\tCount int\n}\n",
            None,
            "count",
            "cannot encapsulate exported Go field `Count` without changing its public API and serialization behavior",
        ),
        (
            "type Box struct {\n\tcount int `json:\"count\"`\n}\n",
            None,
            "count",
            "cannot encapsulate tagged Go field `count` without preserving reflection behavior",
        ),
    ];
    for (idx, (src, class, field, message)) in cases.iter().enumerate() {
        let err = encapsulate_field_go(src, *class, field).expect_err("must refuse");
        assert_eq!(err.to_string(), *message, "case {idx}");
    }
}

#[test]
fn external_python_accesses_become_accessor_calls() {
    let cases: &[(&str, &str, &str, usize, usize)] = &[
        ("x = obj.count\n", "count", "x = obj.get_count()\n", 1, 0),
        ("obj.count = 5\n", "count", "obj.set_count(5)\n", 0, 1),
        ("self.count = 1\n", "count", "self.count = 1\n", 0, 0),
        (
            "obj.count()\nobj.counter\nobj.count_x\n",
            "count",
            "obj.count()\nobj.counter\nobj.count_x\n",
            0,
            0,
        ),
        (
            "# obj.count\n    # obj.count\n",
            "count",
            "# obj.count\n    # obj.count\n",
            0,
            0,
        ),
        (
            "if obj.count == 3:\n",
            "count",
            "if obj.get_count() == 3:\n",
            1,
            0,
        ),
        ("obj.count", "count", "obj.get_count()", 1, 0),
        (
            "a.count + b.count\n",
            "count",
            "a.get_count() + b.get_count()\n",
            2,
            0,
        ),
        ("obj.fooBar\n", "fooBar", "obj.get_foo_bar()\n", 1, 0),
        ("obj.other\n", "count", "obj.other\n", 0, 0),
    ];
    for (idx, (code, field, expected, reads, writes)) in cases.iter().enumerate() {
        let got = rewrite_external_py(code, field);
        assert_eq!(got, (expected.to_string(), *reads, *writes), "case {idx}");
    }
}

#[test]
fn python_class_field_is_hidden_behind_accessors() {
    let src = "class Box:\n    def __init__(self, count: int = 0):\n        self.count = count\n\n    def bump(self):\n        self.count += 1\n\ndef use(b):\n    return b.count";
    let (owner, ty, code, reads, writes, left) = encapsulate_field_py(src, None, "count").unwrap();
    assert_eq!(
        (owner.as_str(), ty.as_str(), reads, writes, left),
        ("Box", "int", 1, 0, 2)
    );
    assert_eq!(
        code,
        "class Box:\n    def __init__(self, count: int = 0):\n        self._count = count\n\n    def bump(self):\n        self._count += 1\n\n\n    def get_count(self) -> int:\n        return self._count\n\n    def set_count(self, count: int) -> None:\n        self._count = count\n\ndef use(b):\n    return b.get_count()"
    );

    let annotated = "class Pt:\n    x: int = 0\n    y: int = 0\n";
    let (owner, ty, code, _, _, left) = encapsulate_field_py(annotated, Some("Pt"), "x").unwrap();
    assert_eq!((owner.as_str(), ty.as_str(), left), ("Pt", "int", 1));
    assert_eq!(
        code,
        "class Pt:\n    _x: int = 0\n    y: int = 0\n\n    def get_x(self) -> int:\n        return self._x\n\n    def set_x(self, x: int) -> None:\n        self._x = x\n"
    );

    let untyped = "class Pt:\n    x = 5\n";
    let (_, ty, code, ..) = encapsulate_field_py(untyped, None, "x").unwrap();
    assert_eq!(ty, "");
    assert_eq!(
        code,
        "class Pt:\n    _x = 5\n\n    def get_x(self):\n        return self._x\n\n    def set_x(self, x) -> None:\n        self._x = x\n"
    );

    let in_method = "class Box:\n    def __init__(self):\n        self.count: int = 0\n";
    let (_, ty, code, ..) = encapsulate_field_py(in_method, None, "count").unwrap();
    assert_eq!(ty, "int");
    assert_eq!(
        code,
        "class Box:\n    def __init__(self):\n        self._count: int = 0\n\n    def get_count(self) -> int:\n        return self._count\n\n    def set_count(self, count: int) -> None:\n        self._count = count\n"
    );

    let second = "class A:\n    pass\nclass B:\n    y = 1\n";
    let (owner, _, code, ..) = encapsulate_field_py(second, Some("B"), "y").unwrap();
    assert_eq!(owner, "B");
    assert_eq!(
        code,
        "class A:\n    pass\nclass B:\n    _y = 1\n\n    def get_y(self):\n        return self._y\n\n    def set_y(self, y) -> None:\n        self._y = y\n"
    );
    for (text, class) in [("x = 1\n", None), ("class A:\n    pass\n", Some("Z"))] {
        let err = encapsulate_field_py(text, class, "x").expect_err("no class");
        assert_eq!(err.to_string(), "Could not find class in Python file");
    }
}

#[test]
fn line_replacers_keep_longer_names_and_report_counts() {
    assert_eq!(
        replace_line_this("this.x = this.xy + this.x;", "x"),
        ("this._x = this.xy + this._x;".to_string(), 2)
    );
    assert_eq!(replace_line_this("this.x", "x"), ("this._x".to_string(), 1));
    assert_eq!(
        replace_line_this("nothing", "x"),
        ("nothing".to_string(), 0)
    );
    assert_eq!(
        replace_line_this_private("return this.x; this.x2", "x"),
        ("return this.#x; this.x2".to_string(), 1)
    );
    assert_eq!(
        replace_line_self("self.x, self.x_y", "x"),
        ("self._x, self.x_y".to_string(), 1)
    );
}

#[test]
fn cpp_unqualified_replacement_table() {
    // (line, in block comment before, expected line, count, shadowed, in block comment after)
    let cases: &[(&str, bool, &str, usize, bool, bool)] = &[
        ("return count;", false, "return count_;", 1, false, false),
        ("int count = 5;", false, "int count_ = 5;", 1, true, false),
        ("foo(count);", false, "foo(count_);", 1, false, false),
        (
            "obj.count + p->count",
            false,
            "obj.count + p->count",
            0,
            false,
            false,
        ),
        (
            "log(\"count\", 'count');",
            false,
            "log(\"count\", 'count');",
            0,
            false,
            false,
        ),
        (
            "s = \"a\\\"count\";",
            false,
            "s = \"a\\\"count\";",
            0,
            false,
            false,
        ),
        ("count // count", false, "count_ // count", 1, false, false),
        (
            "a /* count */ + count",
            false,
            "a /* count */ + count_",
            1,
            false,
            false,
        ),
        (
            "a /* count */ count;",
            false,
            "a /* count */ count_;",
            1,
            true,
            false,
        ),
        ("count */ count", true, "count */ count_", 1, false, false),
        (
            "still inside count",
            true,
            "still inside count",
            0,
            false,
            true,
        ),
        ("count(1)", false, "count(1)", 0, false, false),
        ("é + count", false, "é + count_", 1, false, false),
    ];
    for (idx, (line, before, expected, count, shadowed, after)) in cases.iter().enumerate() {
        let mut in_block = *before;
        let got = replace_cpp_unqualified(line, "count", &mut in_block);
        assert_eq!(got, (expected.to_string(), *count, *shadowed), "case {idx}");
        assert_eq!(in_block, *after, "case {idx} block state");
    }
}

#[test]
fn analyzer_symbol_positions() {
    assert_eq!(
        lsp_symbol_position(&json!({"selectionRange": {"start": {"line": 2, "character": 4}}})),
        Some((3, 5))
    );
    assert_eq!(
        lsp_symbol_position(
            &json!({"location": {"range": {"start": {"line": 0, "character": 0}}}})
        ),
        Some((1, 1))
    );
    assert_eq!(
        lsp_symbol_position(&json!({"range": {"start": {"line": 7, "character": 1}}})),
        Some((8, 2))
    );
    assert_eq!(lsp_symbol_position(&json!({"name": "x"})), None);
    assert_eq!(
        lsp_symbol_position(&json!({"range": {"start": {"line": 1}}})),
        None
    );
    assert_eq!(
        lsp_symbol_position(&json!({"range": {"start": {"line": 4294967295u64, "character": 0}}})),
        None
    );
    assert_eq!(
        lsp_symbol_position(&json!({"range": {"start": {"line": 1, "character": 99999999999u64}}})),
        None
    );

    let at = |line: u32| json!({"selectionRange": {"start": {"line": line, "character": 1}}});
    let symbols = json!([
        {"name": "Other", "children": [{"name": "size", "selectionRange": at(9)["selectionRange"]}]},
        {"name": "Box", "children": [
            {"name": "name", "selectionRange": at(1)["selectionRange"]},
            {"name": "#size", "selectionRange": at(2)["selectionRange"]}
        ]},
        {"name": "size", "containerName": "Flat", "selectionRange": at(5)["selectionRange"]},
        {"name": "size", "containerName": "Box"}
    ]);
    assert_eq!(
        field_position_in_symbols(&symbols, "Box", "size"),
        Some((3, 2))
    );
    assert_eq!(
        field_position_in_symbols(&symbols, "Flat", "size"),
        Some((6, 2))
    );
    assert_eq!(field_position_in_symbols(&symbols, "Box", "missing"), None);
    assert_eq!(
        field_position_in_symbols(&json!({"not": "an array"}), "Box", "size"),
        None
    );
}
