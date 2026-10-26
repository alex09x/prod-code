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
async fn receiver_move_preserves_literals_comments_and_nested_owner_bindings() {
    let source = r####"struct Tax(u32);
struct Order(u32);
impl Order {
    fn f(&self, tax: &Tax) -> u32 {
        // self Self tax
        /* Self /* self */ tax */
        println!("self Self tax");
        println!(r##"self Self tax"##);
        println!(r##"self "Self" tax"##);
        println!("{}", std::str::from_utf8(b"self Self tax").unwrap());
        println!("{}", std::str::from_utf8(br##"self Self tax"##).unwrap());
        println!("{}", std::str::from_utf8(br##"self "Self" tax"##).unwrap());
        struct Inner(u32);
        impl Inner {
            fn read(&self) -> u32 { let _: &Self = self; self.0 }
        }
        let outer = tax.0;
        let nested = { let tax = Tax(3); tax.0 };
        self.0 + outer + nested + Inner(4).read()
    }
}
fn main() { let o = Order(1); let t = Tax(2); println!("{}", o.f(&t)); }
"####;
    let ws = workspace(source);
    let remote = gateway(
        &ws,
        source,
        &[
            ("tax: &Tax", &["tax.0;\n        let nested"]),
            ("f(&self", &["f(&t)"]),
        ],
    )
    .await;
    let expected = run(source, "receiver original");
    let plan = move_receiver(&ws, remote, false, false).await.unwrap();
    untouched(&ws, source);
    assert_eq!(run(&plan.rewritten[0].1, "receiver preview"), expected);
    assert!(plan.rewritten[0].1.contains("// self Self tax"));
    let moved = move_receiver(&ws, remote, true, false).await.unwrap();
    assert!(moved.applied, "{}", moved.render(12000));
    assert_eq!(run(&ws.read("src/main.rs"), "receiver applied"), expected);
}

#[tokio::test]
async fn selected_binding_cursor_and_explicit_reference_lifetime_work() {
    let source = "struct Tax(u32);\nmod tax { pub struct Helper; }\nstruct Order(u32);\nimpl Order {\n    fn f<'a>(&self, helper: tax::Helper, tax: &'a Tax) -> u32 {\n        let _ = helper; self.0 + tax.0\n    }\n}\nfn main() { let o = Order(1); let t = Tax(2); println!(\"{}\", o.f(tax::Helper, &t)); }\n";
    let ws = workspace(source);
    let remote = gateway(
        &ws,
        source,
        &[("tax: &'a", &["tax.0"]), ("f<'a>", &["f(tax::Helper"])],
    )
    .await;
    assert_eq!(run(source, "lifetime cursor original"), "3\n");
    // Generic method names need the declaration's cursor, not the first call's f(.
    let (l, c) = position(source, "f<'a>");
    let moved = prod_code_mcp::move_method::move_method(
        remote,
        &ws.root(),
        &ws.path("src/main.rs"),
        l,
        c,
        "tax",
        true,
        false,
    )
    .await
    .unwrap();
    assert!(moved.applied, "{}", moved.render(12000));
    assert_eq!(
        run(&ws.read("src/main.rs"), "lifetime cursor applied"),
        "3\n"
    );
}

#[tokio::test]
async fn module_word_before_plain_destination_binding_does_not_steal_cursor() {
    let source = "struct Tax(u32);\nmod tax { pub struct Helper; }\nstruct Order(u32);\nimpl Order {\n    fn f(&self, helper: tax::Helper, tax: &Tax) -> u32 {\n        let _ = helper; self.0 + tax.0\n    }\n}\nfn main() { let o = Order(1); let t = Tax(2); println!(\"{}\", o.f(tax::Helper, &t)); }\n";
    let ws = workspace(source);
    let remote = gateway(
        &ws,
        source,
        &[("tax: &Tax", &["tax.0"]), ("f(&self", &["f(tax::Helper"])],
    )
    .await;
    let result = move_receiver(&ws, remote, true, false).await.unwrap();
    assert!(result.applied);
    assert_eq!(run(&ws.read("src/main.rs"), "cursor applied"), "3\n");
}
