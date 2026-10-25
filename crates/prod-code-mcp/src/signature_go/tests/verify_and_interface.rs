/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::signature_go::interface::{
    declares_interface_method, interface_method_file, ordinary_receiver,
    package_interface_method_file, receiver_selector_call,
};
use crate::signature_go::parse::{header, parameters};
use crate::signature_go::reorder_verify::verify_gopls_edits;
use crate::signature_go::types::{Call, Decl, GoParam, Receiver, TextEdit};

const SOURCE: &str =
    "package p\n\nfunc Add(a int, b string) {}\n\nfunc use() {\n\tAdd(1, \"x\")\n}\n";

struct Case {
    root: PathBuf,
    file: PathBuf,
    text: String,
    decl: Decl,
    call: Call,
    new_params: Vec<GoParam>,
}

fn case(text: &str) -> Case {
    let root = PathBuf::from("/w");
    let file = root.join("lib.go");
    let decl = header(text, text.find("func Add").unwrap()).unwrap();
    let open = text.find("Add(1").unwrap() + 3;
    let close = open + text[open..].find(')').unwrap();
    let call = Call {
        path: file.clone(),
        at: "lib.go:6:2".into(),
        open,
        close,
        args: vec!["1".into(), "\"x\"".into()],
    };
    let new_params = parameters("b string, a int").unwrap();
    Case {
        root,
        file,
        text: text.into(),
        decl,
        call,
        new_params,
    }
}

fn verify(
    c: &Case,
    edits: Vec<TextEdit>,
) -> (Vec<String>, Vec<String>, String, BTreeMap<PathBuf, String>) {
    let mut by_file = BTreeMap::new();
    if !edits.is_empty() {
        by_file.insert(c.file.clone(), edits);
    }
    let originals = BTreeMap::from([(c.file.clone(), c.text.clone())]);
    let mut rewritten = BTreeMap::new();
    let v = verify_gopls_edits(
        &c.root,
        &c.file,
        &c.text,
        &c.decl,
        &c.new_params,
        &[1, 0],
        2,
        false,
        std::slice::from_ref(&c.call),
        &by_file,
        &originals,
        &mut rewritten,
    );
    (v.unexpected, v.unmatched, v.new_signature, rewritten)
}

fn good_edits(c: &Case) -> Vec<TextEdit> {
    vec![
        (c.decl.open + 1, c.decl.close, "b string, a int".into()),
        (c.call.open + 1, c.call.close, "\"x\", 1".into()),
    ]
}

const UNMATCHED: &str = "lib.go:6:2: gopls wrote (1, \"x\") where the new arguments are (\"x\", 1)";

#[test]
fn edits_that_reorder_the_declaration_and_every_call_are_accepted() {
    let c = case(SOURCE);
    let (unexpected, unmatched, signature, rewritten) = verify(&c, good_edits(&c));
    assert_eq!((unexpected, unmatched), (vec![], vec![]));
    assert_eq!(signature, "b string, a int");
    assert_eq!(
        rewritten[&c.file],
        "package p\n\nfunc Add(b string, a int) {}\n\nfunc use() {\n\tAdd(\"x\", 1)\n}\n"
    );
}

#[test]
fn edits_that_stray_from_the_request_are_named() {
    let c = case(SOURCE);
    let mut stray = good_edits(&c);
    stray.insert(0, (0, 7, "PACKAGE".into()));
    assert_eq!(
        verify(&c, stray).0,
        ["lib.go:1: gopls changed text outside the argument and parameter lists"]
    );

    let (unexpected, unmatched, _, _) = verify(&c, vec![good_edits(&c).remove(0)]);
    assert_eq!(
        (unexpected, unmatched),
        (vec![], vec![UNMATCHED.to_string()])
    );

    let (unexpected, unmatched, signature, _) = verify(&c, vec![]);
    assert_eq!(
        unexpected,
        ["lib.go: gopls did not rewrite the declaration of `Add`"]
    );
    assert_eq!(unmatched, [UNMATCHED]);
    assert_eq!(signature, "a int, b string");

    let drops_a = vec![
        (c.decl.open + 1, c.decl.close, "b string".to_string()),
        good_edits(&c).remove(1),
    ];
    assert_eq!(
        verify(&c, drops_a).0,
        ["lib.go: gopls declared (b string) where (b string, a int) was asked"]
    );

    let eats_call = vec![
        good_edits(&c).remove(0),
        (c.call.open, c.call.close + 1, String::new()),
    ];
    let (_, unmatched, _, _) = verify(&c, eats_call);
    assert_eq!(
        unmatched,
        ["lib.go:6:2: gopls wrote (an unreadable call) where the new arguments are (\"x\", 1)"]
    );
}

#[test]
fn a_comment_gopls_loses_is_reported() {
    let c = case(
        "package p\n\nfunc Add(a int /* keep */, b string) {}\n\nfunc use() {\n\tAdd(1, \"x\")\n}\n",
    );
    let (unexpected, unmatched, _, _) = verify(&c, good_edits(&c));
    assert_eq!(
        unexpected,
        ["lib.go: gopls's edit drops or changes a comment"]
    );
    assert!(unmatched.is_empty());
}

#[test]
fn an_interface_member_is_found_by_name_and_call_shape_only() {
    let cases = [
        (
            "type I interface {\n // Run()\n Other(int)\n Embedded\n Run (x int)\n}",
            true,
        ),
        ("type I interface { Get[T any](x T); Run() }", true),
        ("type I interface { Run }", false),
        ("type I interface { Run()", false),
        ("type I interface Run()", false),
        ("type I interface { Other() }; func Run() {}", false),
        ("type I interface { M(f func() int, \"Run()\") }", false),
    ];
    for (text, want) in cases {
        assert_eq!(declares_interface_method(text, "Run"), want, "{text:?}");
    }
}

#[test]
fn the_checkout_scan_finds_the_first_go_file_declaring_the_member() {
    let dir = tempfile::Builder::new().prefix("sg").tempdir().unwrap();
    let root = dir.path();
    let write = |rel: &str, text: &str| {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    };
    write(".git/x.go", "type I interface { Run() }");
    write("notes.txt", "type I interface { Run() }");
    write("a/b.go", "package a\ntype I interface { Other() }\n");
    assert_eq!(interface_method_file(root, "Run").unwrap(), None);
    write("a/c/d.go", "package c\ntype I interface { Run() }\n");
    assert_eq!(
        interface_method_file(root, "Run").unwrap(),
        Some(root.join("a/c/d.go"))
    );
    let gone = root.join("missing");
    assert_eq!(
        interface_method_file(&gone, "Run").unwrap_err().to_string(),
        format!(
            "cannot read {} while checking interface obligations",
            gone.display()
        )
    );
}

#[test]
fn the_package_scan_ignores_other_packages_and_refuses_what_it_cannot_read() {
    let dir = tempfile::Builder::new().prefix("sg").tempdir().unwrap();
    let root = dir.path();
    let write = |rel: &str, text: &str| std::fs::write(root.join(rel), text).unwrap();
    write("a.go", "package p\nfunc (m M) Run() {}\n");
    write("other.go", "package q\ntype I interface { Run() }\n");
    write("notes.txt", "package p\ntype I interface { Run() }\n");
    assert_eq!(
        package_interface_method_file(&root.join("a.go"), "Run").unwrap(),
        None
    );
    write("same.go", "package p\ntype I interface { Run() }\n");
    assert_eq!(
        package_interface_method_file(&root.join("a.go"), "Run").unwrap(),
        Some(root.join("same.go"))
    );
    write("bare.go", "func f() {}\n");
    assert_eq!(
        package_interface_method_file(&root.join("bare.go"), "Run")
            .unwrap_err()
            .to_string(),
        format!(
            "cannot identify the Go package in {}",
            root.join("bare.go").display()
        )
    );
    let gone = root.join("gone.go");
    assert_eq!(
        package_interface_method_file(&gone, "Run")
            .unwrap_err()
            .to_string(),
        format!("cannot read {}", gone.display())
    );
    #[cfg(unix)]
    {
        std::fs::remove_file(root.join("same.go")).unwrap();
        std::os::unix::fs::symlink(root.join("a.go"), root.join("link.go")).unwrap();
        assert_eq!(
            package_interface_method_file(&root.join("a.go"), "Run")
                .unwrap_err()
                .to_string(),
            format!(
                "linked Go source {} cannot be inspected for package interface obligations",
                root.join("link.go").display()
            )
        );
    }
}

#[test]
fn a_method_is_a_selector_call_on_a_value_not_on_the_type() {
    let ok = [
        "m.Add(1)",
        "myMeter.Add(1)",
        "m. Add(1)",
        "x.y.Add(1)",
        "get().Add(1)",
    ];
    for text in ok {
        receiver_selector_call(text, text.find("Add").unwrap(), "*Meter")
            .unwrap_or_else(|e| panic!("{text}: {e}"));
    }
    let method_expression =
        "the selector receiver can be the receiver type `Meter`, a method expression";
    for text in [
        "Meter.Add(m, 1)",
        "(*Meter).Add(m, 1)",
        "pkg.Meter.Add(m, 1)",
        "x := Meter.Add",
    ] {
        assert_eq!(
            receiver_selector_call(text, text.find("Add").unwrap(), "*Meter")
                .unwrap_err()
                .to_string(),
            method_expression,
            "{text}"
        );
    }
    for (text, at) in [("Add(1)", 0), ("x := Add", 5), ("m ,Add", 3)] {
        assert_eq!(
            receiver_selector_call(text, at, "Meter")
                .unwrap_err()
                .to_string(),
            "the method name is not preceded by a selector",
            "{text}"
        );
    }
}

#[test]
fn a_receiver_binds_one_name_to_a_named_value_or_pointer_type() {
    assert_eq!(
        ordinary_receiver("m *Meter").unwrap(),
        Receiver {
            binding: "m".into(),
            ty: "*Meter".into()
        }
    );
    assert_eq!(
        ordinary_receiver("  m   Meter ").unwrap(),
        Receiver {
            binding: "m".into(),
            ty: "Meter".into()
        }
    );
    let refused = [
        ("1m Meter", "the receiver does not bind one ordinary name"),
        (
            "Meter",
            "the receiver type `` is not an ordinary named value or pointer type",
        ),
        (
            "m pkg.Meter",
            "the receiver type `pkg.Meter` is not an ordinary named value or pointer type",
        ),
        (
            "m []Meter",
            "the receiver type `[]Meter` is not an ordinary named value or pointer type",
        ),
        (
            "m Meter[T]",
            "the receiver type `Meter[T]` is not an ordinary named value or pointer type",
        ),
    ];
    for (receiver, want) in refused {
        assert_eq!(
            ordinary_receiver(receiver).unwrap_err().to_string(),
            want,
            "{receiver}"
        );
    }
}
