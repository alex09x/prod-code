/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::cases::{Case, case};

pub fn uncertain() -> Vec<Case> {
    let mut raw = case(
        "raw-implicit-capture",
        "use std::f32::consts::*;\nfn run(w: u32) {\n    let a = w + 1;\n    let b = w + 1;\n    println!(r#\"{a}:{b}:{PI}\"#);\n}\nfn main() { run(2); }\n",
        "w + 1",
    );
    raw.name = "PI";
    let mut escaped = case(
        "escaped-implicit-capture",
        "use std::f32::consts::*;\nfn run(w: u32) {\n    let a = w + 1;\n    let b = w + 1;\n    println!(\"{a}:{b}:{\\u{50}I}\");\n}\nfn main() { run(2); }\n",
        "w + 1",
    );
    escaped.name = "PI";
    vec![
        raw,
        escaped,
        case(
            "unknown-macro-capture",
            "macro_rules! output { ($($t:tt)*) => { println!($($t)*); } }\nfn run(w: u32) {\n    let a = w + 1;\n    let b = w + 1;\n    output!(\"{a}:{b}\");\n}\nfn main() { run(2); }\n",
            "w + 1",
        ),
        case(
            "shadowed-format-macro-capture",
            "macro_rules! println { ($($t:tt)*) => { () }; }\nfn run(w: u32) -> (u32, u32) {\n    let a = w + 1;\n    let b = w + 1;\n    println!(\"unrelated\");\n    (a, b)\n}\nfn main() { std::println!(\"{:?}\", run(2)); }\n",
            "w + 1",
        ),
        case(
            "nested-format-macro-capture",
            "fn run(w: u32) {\n    let a = w + 1;\n    let b = w + 1;\n    println!(concat!(\"{a}:\", \"{}\"), b, a=a);\n}\nfn main() { run(2); }\n",
            "w + 1",
        ),
    ]
}

pub fn positive() -> Vec<Case> {
    let mut out = vec![
        case(
            "raw-formatting",
            "fn run(w: u32) {\n    let a = w + 1;\n    let b = w + 1;\n    println!(r#\"{a}:{b}\"#);\n}\nfn main() { run(2); }\n",
            "w + 1",
        ),
        case(
            "escaped-formatting",
            "fn run(w: u32) {\n    let a = w + 1;\n    let b = w + 1;\n    println!(\"{a}:{b}\\n\");\n}\nfn main() { run(2); }\n",
            "w + 1",
        ),
        case(
            "conditional-copy-pure-prefix",
            "fn run(flag: bool, x: u32, d: u32) {\n    let a = if flag { x / d } else { 0 };\n    let b = x / d;\n    println!(\"{a}:{b}\");\n}\nfn main() { run(false, 8, 2); run(true, 8, 2); }\n",
            "x / d",
        ),
        case(
            "allocated-formatting",
            "fn run(w: u32) {\n    let a = w + 1;\n    let b = w + 1;\n    let label = format!(\"{a}:{b}\");\n    println!(\"{label}\");\n}\nfn main() { run(2); }\n",
            "w + 1",
        ),
    ];
    for c in &mut out {
        c.refuse = false;
    }
    out
}
