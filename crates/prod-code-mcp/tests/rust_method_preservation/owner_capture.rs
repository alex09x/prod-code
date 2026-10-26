/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::*;

#[tokio::test]
async fn comparisons_inside_nested_owners_keep_their_self_identity() {
    for receiver in [false, true] {
        let params = if receiver { "&self, tax: &Tax" } else { "" };
        let outer = if receiver { "let _ = tax;" } else { "" };
        let call = if receiver { "o.f(&t)" } else { "Order::f()" };
        let source = format!(
            "struct Tax;\nstruct Order(u64);\nimpl Order {{\n    fn f({params}) -> usize {{\n        {outer}\n        struct Inner;\n        impl Inner {{\n            fn size() -> usize {{\n                if 1 < 2 {{ std::mem::size_of::<Self>() }} else {{ 99 }}\n            }}\n        }}\n        Inner::size() + std::mem::size_of::<Self>()\n    }}\n}}\nfn main() {{ let o = Order(2); let t = Tax; println!(\"{{}}\", {call}); }}\n"
        );
        let ws = workspace(&source);
        let groups: Vec<(&str, &[&str])> = if receiver {
            vec![("tax: &Tax", &["tax;"]), ("f(&self", &["f(&t)"])]
        } else {
            vec![("f() ->", &["f());"])]
        };
        let remote = gateway(&ws, &source, &groups).await;
        assert_eq!(run(&source, "nested comparison original"), "8\n");
        let plan = if receiver {
            move_receiver(&ws, remote, false, false).await
        } else {
            move_associated(&ws, remote, false, false).await
        }
        .unwrap();
        untouched(&ws, &source);
        assert_eq!(
            run(&plan.rewritten[0].1, "nested comparison preview"),
            "8\n"
        );
        let moved = if receiver {
            move_receiver(&ws, remote, true, false).await
        } else {
            move_associated(&ws, remote, true, false).await
        }
        .unwrap();
        assert!(moved.applied);
        assert_eq!(
            run(&ws.read("src/main.rs"), "nested comparison applied"),
            "8\n"
        );
    }
}

#[tokio::test]
async fn implicit_receiver_captures_refuse_without_writing_even_with_force() {
    for literal in [
        "\"{self:?}\"",
        "r#\"{self:?}\"#",
        "\"\\x7bself:?}\"",
        "\"{\\u{73}elf:?}\"",
        "\"{tax:?}\"",
    ] {
        let source = format!(
            "#[derive(Debug)]\nstruct Tax(u32);\n#[derive(Debug)]\nstruct Order(u64);\nimpl Order {{\n    fn f(&self, tax: &Tax) -> String {{\n        let _ = tax; format!({literal})\n    }}\n}}\nfn main() {{ let o = Order(2); let t = Tax(3); println!(\"{{}}\", o.f(&t)); }}\n"
        );
        let expected = if literal.contains("tax") {
            "Tax(3)\n"
        } else {
            "Order(2)\n"
        };
        assert_eq!(run(&source, "implicit capture original"), expected);
        let ws = workspace(&source);
        let remote = gateway(
            &ws,
            &source,
            &[("tax: &Tax", &["tax;"]), ("f(&self", &["f(&t)"])],
        )
        .await;
        for force in [false, true] {
            ws.write("src/main.rs", &source);
            // Record the compiled counterexample even when the refusal assertion fails.
            let plan = move_receiver(&ws, remote, false, force).await;
            if let Ok(plan) = &plan {
                let observed = run(&plan.rewritten[0].1, "implicit capture unguarded preview");
                println!("expected {expected:?}, observed {observed:?}");
            }
            let err = plan
                .expect_err("implicit capture requires an identity proof")
                .to_string();
            println!("implicit capture force={force}: {err}");
            assert!(err.contains("implicit format capture"), "{err}");
            untouched(&ws, &source);
            let err = move_receiver(&ws, remote, true, force)
                .await
                .unwrap_err()
                .to_string();
            assert!(err.contains("implicit format capture"), "{err}");
            untouched(&ws, &source);
        }
    }
}

#[tokio::test]
async fn explicit_arguments_literal_data_and_nested_captures_remain_supported() {
    let source = r###"#[derive(Debug)]
struct Tax(u32);
#[derive(Debug)]
struct Order(u64);
impl Order {
    fn f(&self, tax: &Tax) -> String {
        let _ = tax;
        #[derive(Debug)]
        struct Inner(u32);
        impl Inner {
            fn label(&self) -> String {
                if 1 < 2 { format!("{self:?}") } else { unreachable!() }
            }
        }
        // These braces are data, rather than implicit captures.
        let data = "{self:?}";
        let bytes = std::str::from_utf8(b"{self:?}").unwrap();
        format!(r#"{:?}|{}|{{self:?}}|{}|{}"#, self, Inner(4).label(), data, bytes)
    }
}
fn main() { let o = Order(2); let t = Tax(3); println!("{}", o.f(&t)); }
"###;
    let expected = "Order(2)|Inner(4)|{self:?}|{self:?}|{self:?}\n";
    assert_eq!(
        run(source, "explicit and nested capture original"),
        expected
    );
    let ws = workspace(source);
    let remote = gateway(
        &ws,
        source,
        &[("tax: &Tax", &["tax;"]), ("f(&self", &["f(&t)"])],
    )
    .await;
    let plan = move_receiver(&ws, remote, false, false).await.unwrap();
    untouched(&ws, source);
    assert_eq!(
        run(&plan.rewritten[0].1, "explicit and nested capture preview"),
        expected
    );
    assert!(plan.rewritten[0].1.contains("let data = \"{self:?}\";"));
    let moved = move_receiver(&ws, remote, true, false).await.unwrap();
    assert!(moved.applied);
    assert_eq!(
        run(
            &ws.read("src/main.rs"),
            "explicit and nested capture applied"
        ),
        expected
    );
}
