/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::signature_go::hazards::classify;
use crate::signature_go::interface::{
    declares_interface_method, ordinary_receiver, receiver_selector_call,
};
use crate::signature_go::parse::{call_parens, declarations, parameters};
use crate::signature_go::shadowing::{
    ensure_predeclared_types_unshadowed, package_name, package_type_names,
};
use crate::signature_go::text::{comments, split_list, strip_comments};
use crate::signature_go::types::{ArgKind, Receiver};

fn params(list: &str) -> Vec<(String, String)> {
    parameters(list)
        .unwrap()
        .into_iter()
        .map(|p| (p.name, p.ty))
        .collect()
}

#[test]
fn grouped_parameters_are_flattened_with_their_types() {
    assert_eq!(
        params("a, b int, label string, fn func(x, y int) (int, error), xs ...[]map[string]int"),
        vec![
            ("a".into(), "int".into()),
            ("b".into(), "int".into()),
            ("label".into(), "string".into()),
            ("fn".into(), "func(x,y int)(int,error)".into()),
            ("xs".into(), "...[]map[string]int".into()),
        ]
    );
    assert_eq!(
        params("ch <-chan int, /* note */ c chan int,\n\tp *pkg.T,\n"),
        vec![
            ("ch".into(), "<-chan int".into()),
            ("c".into(), "chan int".into()),
            ("p".into(), "*pkg.T".into()),
        ]
    );
    assert!(parameters("int, string").unwrap_err().contains("unnamed"));
    assert!(parameters("chan int").unwrap_err().contains("unnamed"));
    assert!(parameters("_ int, b string").unwrap_err().contains("blank"));
    assert!(parameters("pkg.T").is_err());
    assert!(parameters("").unwrap().is_empty());
}

#[test]
fn declarations_are_found_and_literals_are_not() {
    let text = "package p\n\n// func Fake(a int)\nvar f = func(a, b int) int { return a }\n\
                    type F func(a int) int\n\
                    func (s *S) M(x int, y string) (n int, err error) {\n\treturn\n}\n\
                    func G[T any, U any](t T, u U) struct{ a int } { return struct{ a int }{} }\n\
                    func Asm(x, y int) int\n\
                    func main() { go func(a int) {}(1); s := \"func X(\" ; _ = s }\n";
    let decls = declarations(text);
    let names: Vec<&str> = decls.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, vec!["M", "G", "Asm", "main"]);
    let m = &decls[0];
    assert_eq!(m.receiver.as_deref(), Some("s*S"));
    assert_eq!(m.results, "(n int,err error)");
    assert_eq!(&text[m.open + 1..m.close], "x int, y string");
    assert!(decls[1].generic);
    assert_eq!(decls[1].results, "struct{a int}");
    assert_eq!(decls[2].results, "int");
    assert!(!decls[3].generic && decls[3].results.is_empty());
}

#[test]
fn package_type_shadowing_is_found_across_comments_and_groups() {
    let source = r#"// build comment
package p

// type fake string
const text = "type quoted int"
type Direct = string
type /* before group */ (
    // before name
    int64 /* after name */ = interface{}
    byte = struct {
        field int
    }
)
func local() { type string = interface{} }
"#;
    assert_eq!(package_name(source), Some("p"));
    assert_eq!(
        package_type_names(source),
        vec![
            "Direct".to_string(),
            "int64".to_string(),
            "byte".to_string()
        ]
    );

    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let file = root.join("a.go");
    std::fs::write(&file, "package p\nfunc F() int { return 1 }\n").unwrap();
    std::fs::write(root.join("shadow.go"), source).unwrap();
    std::fs::write(
        root.join("external_test.go"),
        "package p_test\ntype string = interface{}\n",
    )
    .unwrap();

    ensure_predeclared_types_unshadowed(&root, &file, &["string"]).unwrap();
    for shadowed in ["int64", "byte"] {
        let error = ensure_predeclared_types_unshadowed(&root, &file, &[shadowed])
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("declared as a package type") && error.contains("shadow.go"),
            "{shadowed}: {error}"
        );
    }
}

#[test]
fn only_named_value_or_pointer_receivers_can_be_extended() {
    assert_eq!(
        ordinary_receiver("meter Meter").unwrap(),
        Receiver {
            binding: "meter".into(),
            ty: "Meter".into()
        }
    );
    assert_eq!(
        ordinary_receiver("meter *Meter").unwrap(),
        Receiver {
            binding: "meter".into(),
            ty: "*Meter".into()
        }
    );
    for receiver in [
        "_ Meter",
        "meter Meter[T]",
        "meter pkg.Meter",
        "meter *pkg.Meter",
        "left, right Meter",
    ] {
        assert!(ordinary_receiver(receiver).is_err(), "{receiver}");
    }
    let direct = "meter.Add(1)";
    receiver_selector_call(direct, direct.find("Add").unwrap(), "Meter").unwrap();
    for source in ["Add(1)", "Meter.Add(1)", "(*Meter).Add(1)"] {
        assert!(
            receiver_selector_call(source, source.find("Add").unwrap(), "Meter").is_err(),
            "{source}"
        );
    }
    assert!(declares_interface_method(
        "type I interface { Add(x int) }",
        "Add"
    ));
    assert!(!declares_interface_method(
        "type I interface { Other(x int) }",
        "Add"
    ));
}

#[test]
fn arguments_are_split_and_classified() {
    let text =
        "x := f(a, g(b, c), \"s,)\", `r,`, '(', // c,\n\tt.u, &v, -1.5e-3, func(a int) {}, h(),\n)";
    let at = text.find("f(").unwrap();
    let (open, close) = call_parens(text, at).unwrap();
    assert_eq!(close, text.len() - 1);
    let args = split_list(&strip_comments(&text[open + 1..close]));
    assert_eq!(
        args,
        vec![
            "a",
            "g(b, c)",
            "\"s,)\"",
            "`r,`",
            "'('",
            "t.u",
            "&v",
            "-1.5e-3",
            "func(a int) {}",
            "h()"
        ]
    );
    let kinds: Vec<ArgKind> = args.iter().map(|a| classify(a)).collect();
    use ArgKind::*;
    assert_eq!(
        kinds,
        vec![
            Place, Effectful, Literal, Literal, Literal, Place, Place, Literal, Literal, Effectful
        ]
    );
    assert_eq!(classify("(x)"), Place);
    assert_eq!(classify("<-ch"), Effectful);
    assert_eq!(classify("a[i]"), Effectful);
    assert_eq!(classify("func() {}()"), Effectful);
    assert!(call_parens("f := Sub\n", 5).is_none());
    assert!(call_parens("Pair[int, string](1, \"s\")", 0).is_some());
}

#[test]
fn comments_are_listed_as_written_and_strings_are_not_comments() {
    let text = "f(a /* x */, \"// no\", '/') // tail\n/* multi\nline */ g(`/*`)\n";
    assert_eq!(
        comments(text),
        vec!["/* x */", "// tail", "/* multi\nline */"]
    );
    assert!(comments("f(a, b)").is_empty());
}
