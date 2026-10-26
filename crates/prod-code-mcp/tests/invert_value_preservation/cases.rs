/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub struct Case {
    pub id: &'static str,
    pub source: String,
    pub block: Option<&'static str>,
    pub semantic_control: bool,
    pub serde: bool,
}

pub fn default_case(attr: &str, id: &'static str) -> Case {
    Case {
        id,
        source: format!(
            "{attr}\nstruct Flags {{ enabled: bool }}\nfn main() {{\n    let f = Flags::default();\n    println!(\"{{}}\", f.enabled);\n}}\n"
        ),
        block: Some("Default"),
        semantic_control: true,
        serde: false,
    }
}

pub fn derives() -> Vec<Case> {
    vec![
        default_case("#[derive(\n    Default,\n)]", "multiline-default"),
        default_case(
            "#[derive(\n    ::core /* path */ :: default :: Default,\n)]\n/* attached ] derive(Serialize) */\n#[allow(dead_code)]\n/// documentation",
            "qualified-trivia-default",
        ),
        Case {
            id: "multiline-serde",
            source: "#[derive(\n    serde::Serialize,\n    serde /* path */ :: Deserialize,\n)]\nstruct Flags { enabled: bool }\nfn main() {\n    let f = Flags { enabled: true };\n    println!(\"{}\", f.enabled);\n}\n".into(),
            block: Some("Serialize"),
            semantic_control: false,
            serde: true,
        },
    ]
}

fn place(id: &'static str, body: &str, borrow: bool) -> Case {
    Case {
        id,
        source: format!(
            "struct Flags {{ enabled: bool }}\nfn main() {{\n    let mut f = Flags {{ enabled: false }};\n    {body}\n    println!(\"field={{}}\", f.enabled);\n}}\n"
        ),
        block: borrow.then_some("borrowed"),
        semantic_control: borrow && body.contains("*r = true"),
        serde: false,
    }
}

pub fn borrows() -> Vec<Case> {
    vec![
        place("grouped-mutable", "let r = &mut (f.enabled); *r = true;", true),
        place("grouped-receiver", "let r = &mut ((f).enabled); *r = true;", true),
        place("nested-mutable", "let r = & /* borrow */ mut (((f.enabled))); *r = true;", true),
        place("grouped-shared", "let r = &((f.enabled)); println!(\"ref={}\", *r);", true),
        Case {
            id: "local-mutable",
            source: "fn main() {\n    let mut enabled: bool = false;\n    let r = &mut ((enabled)); *r = true;\n    println!(\"{}\", enabled);\n}\n".into(),
            block: Some("borrowed"),
            semantic_control: true,
            serde: false,
        },
    ]
}

pub fn positive() -> Vec<Case> {
    vec![
        place("ordinary", "f.enabled = true;", false),
        place("bitwise", "let x = true & (f.enabled); println!(\"{}\", x);", false),
        place("logical", "let x = true && (f.enabled); println!(\"{}\", x);", false),
        place("independent-borrow", "let x = &(false | (f.enabled)); println!(\"{}\", x);", false),
        place("borrowed-expression", "let x = &(f.enabled | true); println!(\"{}\", x);", false),
        place("nested-call", "let x = &std::convert::identity(f.enabled); println!(\"{}\", x);", false),
        place("strings-comments", "let text = r#\"&mut (((f.enabled)))\"#; /* &mut ( */ let x = (f.enabled); println!(\"{}:{}\", text, x);", false),
        Case {
            id: "unrelated-derive",
            source: "#[derive(Default)]\nstruct Other { n: u32 }\n#[derive(\n    Debug, Clone,\n)]\n#[doc = \"derive(Default, Serialize)\"]\nstruct Flags { enabled: bool }\nfn main() { let f = Flags { enabled: true }; println!(\"{}\", f.enabled); }\n".into(),
            block: None,
            semantic_control: false,
            serde: false,
        },
        Case {
            id: "ordinary-local",
            source: "fn main() { let mut enabled: bool = false; enabled = true; println!(\"{}\", enabled); }\n".into(),
            block: None,
            semantic_control: false,
            serde: false,
        },
    ]
}
