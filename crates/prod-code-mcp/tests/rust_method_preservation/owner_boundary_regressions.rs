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

async fn preserve(source: &str, expected: &str, label: &str) {
    assert_eq!(run(source, &format!("{label} original")), expected);
    let mut mismatches = Vec::new();
    let ws = workspace(source);
    let remote = gateway(&ws, source, &[("f() ->", &["f());"])]).await;
    for force in [false, true] {
        ws.write("src/main.rs", source);
        let plan = move_associated(&ws, remote, false, force).await.unwrap();
        untouched(&ws, source);
        let preview = run(
            &plan.rewritten[0].1,
            &format!("{label} preview force={force}"),
        );
        assert!(
            move_associated(&ws, remote, true, force)
                .await
                .unwrap()
                .applied
        );
        let applied = run(
            &ws.read("src/main.rs"),
            &format!("{label} applied force={force}"),
        );
        if preview != expected || applied != expected {
            mismatches.push(format!(
                "force={force}: preview={preview:?}, applied={applied:?}"
            ));
        }
    }
    assert!(mismatches.is_empty(), "{label}: {mismatches:?}");
}

#[tokio::test]
async fn field_initializer_comparisons_do_not_hide_independent_owners() {
    for expression in ["1 < 2", "2 > 1", "(1 < 2) && (3 > 2)", "{ 1 < 2 }"] {
        let source = format!(
            "struct Tax;\nstruct Order(u128);\nimpl Order {{\n    fn f() -> usize {{\n        struct Check {{ flag: bool }}\n        let _ = Check {{ flag: {expression} }};\n        struct Inner(u64);\n        impl Inner {{ fn size() -> usize {{ std::mem::size_of::<Self>() }} }}\n        Inner::size()\n    }}\n}}\nfn main() {{ println!(\"{{}}\", Order::f()); }}\n"
        );
        preserve(&source, "8\n", "field initializer owner").await;
    }
}

#[tokio::test]
async fn type_macro_tokens_do_not_close_owner_generics() {
    for invocation in [
        "ty!(a > b)",
        "ty![a < b]",
        "ty!{a >> b}",
        "ty!((a > b) [<] {>})",
    ] {
        let source = format!(
            "macro_rules! ty {{ ($($t:tt)*) => {{ u8 }}; }}\nstruct Tax;\nstruct Order(u128);\nimpl Order {{\n    fn f() -> usize {{\n        #[repr(C, packed)] struct Inner<T>(std::marker::PhantomData<T>, u64, T);\n        impl Inner<{invocation}> {{ fn size() -> usize {{ std::mem::size_of::<Self>() }} }}\n        Inner::<u8>::size()\n    }}\n}}\nfn main() {{ println!(\"{{}}\", Order::f()); }}\n"
        );
        preserve(&source, "9\n", "type macro owner").await;
    }
}
