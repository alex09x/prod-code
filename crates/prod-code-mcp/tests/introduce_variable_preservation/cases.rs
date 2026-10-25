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
    pub expression: &'static str,
    pub name: &'static str,
    pub refuse: bool,
}

pub(super) fn case(id: &'static str, body: &str, expression: &'static str) -> Case {
    Case {
        id,
        source: body.to_owned(),
        expression,
        name: "cached",
        refuse: true,
    }
}

pub fn regressions() -> Vec<Case> {
    let mut quoted = case(
        "4081600649-quoted-comment",
        r##"fn run(w: u32) {
    let a = (w + 1);
    let label = "(w + 1)";
    let raw = r#"(w + 1)"#;
    // (w + 1)
    /* outer (w + 1) /* nested (w + 1) */ */
    let b = (w + 1);
    println!("{a}:{b}:{label}:{raw}");
}
fn main() { run(2); }
"##,
        "(w + 1)",
    );
    quoted.refuse = false;
    let mut collision = case(
        "4081600655-name-collision",
        "fn run(w: u32) {\n    let a = (w + 1);\n    let cached = 100;\n    let b = (w + 1);\n    println!(\"{a}:{b}:{cached}\");\n}\nfn main() { run(2); }\n",
        "(w + 1)",
    );
    collision.name = "cached";
    let mut out = vec![quoted, collision];
    for (id, inside) in [
        (
            "4081600661-for",
            "for w in [10, 20] {\n        let b = w + 1;\n        println!(\"loop:{b}\");\n    }",
        ),
        (
            "4081600661-closure",
            "let f = |w: u32| {\n        let b = w + 1;\n        b\n    };\n    println!(\"closure:{}\", f(10));",
        ),
        (
            "4081600661-match",
            "match Some(10u32) {\n        Some(w) => {\n            let b = w + 1;\n            println!(\"match:{b}\");\n        }\n        None => {}\n    }",
        ),
    ] {
        out.push(case(id, &format!("fn run(w: u32) {{\n    let a = w + 1;\n    println!(\"outer:{{a}}\");\n    {inside}\n}}\nfn main() {{ run(2); }}\n"), "w + 1"));
    }
    out.push(case(
        "4081600664-overloaded-add",
        r#"use std::sync::atomic::{AtomicU32, Ordering};
static COUNT: AtomicU32 = AtomicU32::new(0);
#[derive(Clone, Copy)] struct Tick;
impl std::ops::Add for Tick {
    type Output = u32;
    fn add(self, _: Self) -> u32 {
        let n = COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        println!("add:{n}");
        n
    }
}
fn run(a: Tick, b: Tick) {
    let x = a + b;
    let y = a + b;
    println!("values:{x}:{y}");
}
fn main() { run(Tick, Tick); }
"#,
        "a + b",
    ));
    out.push(case(
        "4082049715-nested-macro",
        r#"macro_rules! m { (($e:expr)) => { 10 }; ($e:expr) => { 20 }; }
fn run(x: u32) {
    let a = m!((x == 0));
    let b = m!((x == 0));
    println!("arms:{a}:{b}");
}
fn main() { run(0); }
"#,
        "x == 0",
    ));
    out.push(case(
        "4082049720-generic-mutation",
        r#"struct S { x: u32 }
impl S { fn bump<T>(&mut self) { self.x += 10; println!("bump:{}", self.x); } }
fn run(s: &mut S) {
    let a = s.x + 1;
    s.bump::<u32>();
    let b = s.x + 1;
    println!("fields:{a}:{b}");
}
fn main() { run(&mut S { x: 2 }); }
"#,
        "s.x + 1",
    ));
    out.push(case(
        "4082049729-guarded-shift",
        r#"fn run(x: u32, n: u32) {
    if n < 32 {
        let a = x >> n;
        println!("first:{a}");
    }
    if n < 32 {
        let b = x >> n;
        println!("second:{b}");
    }
    println!("done");
}
fn main() { run(8, std::env::args().nth(1).unwrap().parse().unwrap()); }
"#,
        "x >> n",
    ));
    out
}

pub fn positive() -> Vec<Case> {
    let mut out = vec![
        case(
            "conditional-primitive-product",
            "fn run(w: u32, h: u32) -> u32 {\n    let a = if h > 2 {\n        (w + 1) * h\n    } else {\n        0\n    };\n    a + (w + 1)\n}\nfn main() { println!(\"{} {}\", run(2, 3), run(2, 0)); }\n",
            "w + 1",
        ),
        case(
            "primitive-grouping",
            "fn run(w: u32) {\n    let a = (w + 1) * 2;\n    let b = (w + 1) * 3;\n    println!(\"{a}:{b}\");\n}\nfn main() { run(2); }\n",
            "w + 1",
        ),
        case(
            "pure-fields",
            "struct S { x: u32 }\nfn run(s: &S) {\n    let a = s.x + 1;\n    let b = (s.x + 1) * 2;\n    println!(\"{a}:{b}\");\n}\nfn main() { run(&S { x: 2 }); }\n",
            "s.x + 1",
        ),
        case(
            "commented-grouping",
            "fn run(w: u32) {\n    let a = (/* keep this */ w + 1);\n    let b = (w + 1);\n    println!(\"{a}:{b}\");\n}\nfn main() { run(2); }\n",
            "w + 1",
        ),
        case(
            "same-line",
            "fn run(w: u32) { let a = w + 1; let b = w + 1; println!(\"{a}:{b}\"); }\nfn main() { run(2); }\n",
            "w + 1",
        ),
        case(
            "direct-field",
            "struct S { x: u32 }\nfn run(s: &S) {\n    let a = s.x;\n    let b = s.x;\n    println!(\"{a}:{b}\");\n}\nfn main() { run(&S { x: 2 }); }\n",
            "s.x",
        ),
        case(
            "primitive-read",
            "fn run(w: u32) {\n    let a = w;\n    let b = w;\n    println!(\"{a}:{b}\");\n}\nfn main() { run(2); }\n",
            "w",
        ),
    ];
    for c in &mut out {
        c.refuse = false;
    }
    out
}

pub fn boundaries() -> Vec<Case> {
    vec![
        case(
            "macro-copy-after-ordinary",
            "macro_rules! m { (($e:expr)) => { 10 }; ($e:expr) => { 20 }; }\nfn run(x: u32) {\n    let a = x == 0;\n    let b = m!((x == 0));\n    println!(\"{a}:{b}\");\n}\nfn main() { run(0); }\n",
            "x == 0",
        ),
        case(
            "earlier-event",
            "fn event() -> u32 { println!(\"before\"); 0 }\nfn run(w: u32) {\n    let a = event() + (w + 1);\n    let b = w + 1;\n    println!(\"{a}:{b}\");\n}\nfn main() { run(2); }\n",
            "w + 1",
        ),
        case(
            "earlier-panic",
            "fn run(w: u32, n: u32) {\n    let a = (10 / n) + (w + 1);\n    let b = w + 1;\n    println!(\"{a}:{b}\");\n}\nfn main() { run(2, 1); }\n",
            "w + 1",
        ),
        case(
            "unknown-generic-operator",
            "fn run<T: std::ops::Add<Output=u32> + Copy>(a: T, b: T) {\n    let x = a + b;\n    let y = a + b;\n    println!(\"{x}:{y}\");\n}\nfn main() { run(2u32, 3u32); }\n",
            "a + b",
        ),
        case(
            "containing-method-mutation",
            "struct S { x: u32 }\nimpl S { fn take(&mut self, n: u32) { self.x += 10; println!(\"take:{n}\"); } }\nfn run(s: &mut S) {\n    s.take(s.x + 1);\n    let b = s.x + 1;\n    println!(\"later:{b}\");\n}\nfn main() { run(&mut S { x: 2 }); }\n",
            "s.x + 1",
        ),
        case(
            "containing-assignment-mutation",
            "struct S { x: u32 }\nfn run(s: &mut S) {\n    s.x = s.x + 1;\n    let b = s.x + 1;\n    println!(\"later:{b}\");\n}\nfn main() { run(&mut S { x: 2 }); }\n",
            "s.x + 1",
        ),
    ]
}

pub fn followups() -> Vec<Case> {
    let mut capture = case(
        "implicit-format-capture",
        "use std::f32::consts::*;\nfn run(w: u32) {\n    let a = w + 1;\n    let b = w + 1;\n    println!(\"{a}:{b}:{PI}\");\n}\nfn main() { run(2); }\n",
        "w + 1",
    );
    capture.name = "PI";
    vec![
        capture,
        case(
            "branch-panic-order",
            r#"fn run(flag: bool, x: u32, d: u32, n: u32) -> (u32, u32) {
    let a = if flag { x / d } else { x % n };
    let b = x / d;
    (a, b)
}
fn main() {
    std::panic::set_hook(Box::new(|info| println!("panic:{}", info.payload().downcast_ref::<&str>().unwrap())));
    let _ = std::panic::catch_unwind(|| run(false, 1, 0, 0));
}
"#,
            "x / d",
        ),
    ]
}
