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
async fn raw_identifier_header_leak_preserves_independent_owner() {
    owner_after_expression(
        "raw identifier",
        "struct Check { r#fn: bool }\n        let check = Check { r#fn: 1 < 2 };\n        assert!(check.r#fn);",
    )
    .await;
}

#[tokio::test]
async fn array_length_header_leak_preserves_independent_owner() {
    for declaration in [
        "fn helper() -> [u8; 1 << 2] { [0; 4] }\n        assert_eq!(helper().len(), 4);",
        "fn helper() -> Option<([u8; 1 << 2], impl Iterator<Item = u8>)> { Some(([0; 4], std::iter::once(0))) }\n        assert_eq!(helper().unwrap().0.len(), 4);",
    ] {
        owner_after_expression("array length", declaration).await;
    }
}

async fn owner_after_expression(label: &str, expression: &str) {
    let source = format!(
        "struct Tax;\nstruct Order(u64);\nimpl Order {{\n    fn f() -> usize {{\n        {expression}\n        struct Inner;\n        impl Inner {{ fn size() -> usize {{ std::mem::size_of::<Self>() }} }}\n        std::mem::size_of::<Self>() + Inner::size()\n    }}\n}}\nfn main() {{ println!(\"{{}}\", Order::f()); }}\n"
    );
    assert_eq!(run(&source, &format!("{label} original")), "8\n");
    let mut failures = Vec::new();
    let ws = workspace(&source);
    let remote = gateway(&ws, &source, &[("f() ->", &["f());"])]).await;
    for force in [false, true] {
        for apply in [false, true] {
            ws.write("src/main.rs", &source);
            let result = move_associated(&ws, remote, apply, force).await;
            println!("{label}, apply={apply}, force={force}: {result:?}");
            match result {
                Ok(plan) => {
                    assert_eq!(plan.applied, apply);
                    let transformed = if apply {
                        assert_eq!(ws.read("Cargo.toml"), CARGO);
                        assert_eq!(ws.read("untouched.txt").as_bytes(), b"sentinel\r\n");
                        ws.read("src/main.rs")
                    } else {
                        untouched(&ws, &source);
                        plan.rewritten[0].1.clone()
                    };
                    let output = run(
                        &transformed,
                        &format!("{label}, apply={apply}, force={force}"),
                    );
                    if output != "8\n" {
                        failures.push(format!("apply={apply}, force={force}: {output:?}"));
                    }
                }
                Err(error) => {
                    untouched(&ws, &source);
                    failures.push(format!("apply={apply}, force={force}: refused: {error}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{label}: {failures:?}");
}

#[tokio::test]
async fn const_expression_operators_keep_independent_owner_self() {
    let mut mismatches = Vec::new();
    for expression in ["2 > 1", "1 < 2", "(8 >> 1) == 4", "{ 2 > 1 }"] {
        let source = format!(
            "struct Tax;\nstruct Order(u64);\nimpl Order {{\n    fn f() -> usize {{\n        struct Inner<const B: bool>;\n        impl Inner<{{ {expression} }}> {{ fn size() -> usize {{ std::mem::size_of::<Self>() }} }}\n        std::mem::size_of::<Self>() + Inner::<true>::size()\n    }}\n}}\nfn main() {{ println!(\"{{}}\", Order::f()); }}\n"
        );
        assert_eq!(run(&source, "const owner original"), "8\n");
        let ws = workspace(&source);
        let remote = gateway(&ws, &source, &[("f() ->", &["f());"])]).await;
        for force in [false, true] {
            ws.write("src/main.rs", &source);
            let plan = move_associated(&ws, remote, false, force).await.unwrap();
            untouched(&ws, &source);
            let preview = run(&plan.rewritten[0].1, "const owner preview");
            let moved = move_associated(&ws, remote, true, force).await.unwrap();
            assert!(moved.applied);
            let applied = run(&ws.read("src/main.rs"), "const owner applied");
            if preview != "8\n" || applied != "8\n" {
                mismatches.push(format!(
                    "{expression}, force={force}: preview={preview:?}, applied={applied:?}"
                ));
            }
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:?}");
}

#[tokio::test]
async fn macro_owner_tokens_do_not_acquire_rust_header_context() {
    let mut permitted = Vec::new();
    for prefix in ["tag:", "tag ->", "fn tag:"] {
        let source = format!(
            "macro_rules! make {{ ({prefix} impl $t:ty {{ $($body:tt)* }}) => {{ impl $t {{ $($body)* }} }}; }}\nstruct Tax;\nstruct Order(u64);\nimpl Order {{\n    fn f() -> usize {{\n        struct Inner;\n        make!({prefix} impl Inner {{ fn size() -> usize {{ std::mem::size_of::<Self>() }} }});\n        Inner::size()\n    }}\n}}\nfn main() {{ println!(\"{{}}\", Order::f()); }}\n"
        );
        assert_eq!(run(&source, "macro owner original"), "0\n");
        let ws = workspace(&source);
        let remote = gateway(&ws, &source, &[("f() ->", &["f());"])]).await;
        for force in [false, true] {
            for apply in [false, true] {
                ws.write("src/main.rs", &source);
                let result = move_associated(&ws, remote, apply, force).await;
                println!("macro owner prefix={prefix:?}, apply={apply}, force={force}: {result:?}");
                match result {
                    Err(error) => {
                        assert!(error.to_string().contains("ownership"), "{error}");
                        untouched(&ws, &source);
                    }
                    Ok(plan) => {
                        if !apply {
                            untouched(&ws, &source);
                        }
                        let transformed = if apply {
                            ws.read("src/main.rs")
                        } else {
                            plan.rewritten[0].1.clone()
                        };
                        let output = run(&transformed, "macro owner incorrectly permitted");
                        permitted.push(format!(
                            "{prefix}, apply={apply}, force={force}: {output:?}"
                        ));
                    }
                }
            }
        }
    }
    assert!(
        permitted.is_empty(),
        "ambiguous macro owners must refuse: {permitted:?}"
    );
}
