/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::case::field_at_line_col;
use super::super::rust::{
    access_at, accessors, field_at, inherent_impl, is_generic, owner_at, returns_by_value,
};
use super::super::types::{Access, EncapsulatedField};

const STRUCT: &str = "pub struct Config {\n    /// How long.\n    pub timeout: u64,\n    \
                      pub(crate) name: String, // who\n    pub map: HashMap<String, Vec<u8>>\n}\n";

#[test]
fn field_at_line_col_uses_lsp_utf16_columns() {
    let text = "😀 const name: string = '';\n";
    assert_eq!(field_at_line_col(text, 1, 10).as_deref(), Some("name"));
    assert_eq!(field_at_line_col(text, 1, 2), None);
}

#[test]
fn a_field_declaration_gives_its_name_visibility_and_type() {
    let at = STRUCT.find("timeout").unwrap() + 3;
    let decl = field_at(STRUCT, at).unwrap();
    assert_eq!(decl.name, "timeout");
    assert_eq!(decl.vis, "pub ");
    assert_eq!(decl.ty, "u64");
    assert_eq!(&STRUCT[decl.vis_at..decl.name_at], "pub ");

    let decl = field_at(STRUCT, STRUCT.find("name:").unwrap()).unwrap();
    assert_eq!(decl.vis, "pub(crate) ");
    assert_eq!(decl.ty, "String");

    let decl = field_at(STRUCT, STRUCT.find("map").unwrap()).unwrap();
    assert_eq!(decl.ty, "HashMap<String, Vec<u8>>");

    let err = field_at("let x: u32 = 1;", 4).unwrap_err().to_string();
    assert!(err.contains("`let` comes before it"), "{err}");
    let err = field_at("use a::b;", 4).unwrap_err().to_string();
    assert!(err.contains("not a field declaration"), "{err}");
    assert!(field_at("   ", 1).is_err());
}

#[test]
fn the_owner_is_the_struct_whose_braces_hold_the_field() {
    let text = format!("struct Unit;\nstruct Pair(u8, u8);\n{STRUCT}");
    let (name, at, close) = owner_at(&text, text.find("timeout").unwrap()).unwrap();
    assert_eq!(name, "Config");
    assert!(text[at..].starts_with("struct Config"));
    assert_eq!(&text[close..=close], "}");
    assert!(owner_at(&text, 3).is_none());
    assert!(!is_generic(&text, at, "Config"));
    let generic = "pub struct Page<'a, T> {\n    pub rows: &'a [T],\n}\n";
    assert!(is_generic(generic, generic.find("struct").unwrap(), "Page"));
}

#[test]
fn only_an_inherent_impl_takes_the_accessors() {
    let text = "impl Default for Config {}\nimpl ConfigBuilder {}\nimpl<T> Config<T> where T: \
                Clone {\n}\nimpl Config {}\n";
    let open = inherent_impl(text, "Config").unwrap();
    assert!(text[..open].ends_with("impl<T> Config<T> where T: Clone "));
    assert!(inherent_impl("impl Other {}\n", "Config").is_none());
    assert!(inherent_impl("implement Config {}\n", "Config").is_none());
}

#[test]
fn copy_types_are_returned_by_value_and_everything_else_by_reference() {
    for ty in ["u64", "bool", "&str", "Option<u32>", "Option<&'static str>"] {
        assert!(returns_by_value(ty), "{ty}");
    }
    for ty in ["String", "Vec<u8>", "&mut u8", "Option<String>", "Duration"] {
        assert!(!returns_by_value(ty), "{ty}");
    }
}

fn access(text: &str) -> Access {
    let at = text.find("timeout").unwrap();
    access_at(text, at, "timeout".len())
}

#[test]
fn each_use_of_a_field_is_read_for_what_it_does() {
    assert_eq!(
        access("let t = cfg.timeout;"),
        Access::Read { chained: false }
    );
    assert_eq!(
        access("if a.b().timeout == 3 {}"),
        Access::Read { chained: false }
    );
    assert_eq!(access("cfg.timeout.max(1)"), Access::Read { chained: true });
    let text = "cfg.timeout = f(a, \"; ,\", ';');\n";
    let Access::Write { rhs } = access(text) else {
        panic!("{:?}", access(text));
    };
    assert_eq!(text[rhs.0..rhs.1].trim(), "f(a, \"; ,\", ';')");
    let text = "match x { _ => cfg.timeout = 2, }";
    let Access::Write { rhs } = access(text) else {
        panic!();
    };
    assert_eq!(text[rhs.0..rhs.1].trim(), "2");
    assert!(matches!(access("cfg.timeout += 1;"), Access::Blocked(_)));
    assert!(matches!(access("cfg.timeout <<= 1;"), Access::Blocked(_)));
    assert!(matches!(
        access("cfg.timeout <= 1"),
        Access::Read { chained: false }
    ));
    assert!(matches!(
        access("Config { timeout: 1 }"),
        Access::Blocked(_)
    ));
    assert!(matches!(
        access("let Config { timeout, .. } = c;"),
        Access::Blocked(_)
    ));
    assert!(matches!(access("0..timeout"), Access::Blocked(_)));
    assert_eq!(
        access("bump(&mut self.cfgs[0].timeout);"),
        Access::Blocked("a mutable borrow of the field")
    );
    assert_eq!(
        access("f(&mut_ref.timeout)"),
        Access::Read { chained: false }
    );
    assert_eq!(
        access("f(&mut x.get()?.timeout)"),
        Access::Blocked("a mutable borrow of the field")
    );
    assert_eq!(access("cfg.timeout()"), Access::NotAccess);
    assert_eq!(access("a::b.timeout"), Access::Read { chained: false });
}

#[test]
fn the_accessors_are_what_rustfmt_would_write() {
    assert_eq!(
        accessors("    ", "pub ", "timeout", "u64", true, true),
        "    pub fn timeout(&self) -> u64 {\n        self.timeout\n    }\n\n    pub fn \
         set_timeout(&mut self, timeout: u64) {\n        self.timeout = timeout;\n    }\n"
    );
    assert_eq!(
        accessors("", "pub(crate) ", "name", "String", false, false),
        "pub(crate) fn name(&self) -> &String {\n    &self.name\n}\n"
    );
}

fn report() -> EncapsulatedField {
    EncapsulatedField {
        owner: "Config".into(),
        root: "/root".into(),
        file: "src/config.rs".into(),
        field: "name".into(),
        ty: "String".into(),
        by_value: false,
        reads: 2,
        writes: 1,
        chained_reads: 1,
        left_in_file: 3,
        blocked: vec!["src/main.rs:4:14 a struct literal or pattern names the field".into()],
        unmatched: vec!["src/main.rs:9:1 (a call, not a field access)".into()],
        rewritten: vec![("/root/src/main.rs".into(), "fn main() {}\n".into())],
        diagnostics: vec!["mismatched types (src/main.rs:5:9)".into()],
        applied: false,
    }
}

#[test]
fn the_report_names_the_accessors_and_everything_left_over() {
    let text = report().render(10_000);
    assert!(
        text.contains("getter: `fn name(&self) -> &String`"),
        "{text}"
    );
    assert!(
        text.contains("setter: `fn set_name(&mut self, name: String)`"),
        "{text}"
    );
    assert!(text.contains("2 read(s) and 1 write(s)"), "{text}");
    assert!(text.contains("3 reference(s) inside it left"), "{text}");
    assert!(text.contains("cannot become a method call"), "{text}");
    assert!(text.contains("a call, not a field access"), "{text}");
    assert!(text.contains("the analyzer rejects the result"), "{text}");
    assert!(text.contains("verify: \"compile\""), "{text}");
    assert!(text.contains("nothing was written"), "{text}");

    let mut done = report();
    done.by_value = true;
    done.writes = 0;
    done.blocked.clear();
    done.unmatched.clear();
    done.diagnostics.clear();
    done.applied = true;
    let text = done.render(10);
    assert!(text.contains("-> String`"), "{text}");
    assert!(!text.contains("setter"), "{text}");
    assert!(!text.contains("verify"), "{text}");
    assert!(text.contains("0 errors"), "{text}");
    assert!(text.contains("diff truncated"), "{text}");
    assert!(text.contains("[applied to 1 file(s)]"), "{text}");
}
