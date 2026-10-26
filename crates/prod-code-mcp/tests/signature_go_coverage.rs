/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A Go reorder or removal through a real gopls proves what it changed (#1410): a caller gopls
//! did not report, such as one behind a build tag, refuses the change instead of only being
//! compiled with it, and a comment beside an argument moves or goes with that argument. `go` and
//! `gopls` are required.

use prod_code_mcp::signature::{Modifiers, Param, SignatureChange};
use prod_code_testkit::gopls::{GoModule, GoplsBridge, require_go_toolchain};
use std::net::SocketAddr;

async fn change(
    remote: SocketAddr,
    fixture: &GoModule,
    needle: &str,
    order: &[&str],
    apply: bool,
) -> anyhow::Result<SignatureChange> {
    let text = fixture.read("lib.go");
    let before = &text[..text.find(needle).expect("needle in lib.go")];
    let line = before.matches('\n').count() as u32 + 1;
    let col = before.rsplit('\n').next().unwrap().chars().count() as u32 + 1;
    let request: Vec<Param> = order.iter().map(|n| Param::Keep(n.to_string())).collect();
    prod_code_mcp::signature_go::change_with(
        remote,
        fixture.root(),
        &fixture.path("lib.go"),
        line,
        col,
        &request,
        &Modifiers::default(),
        apply,
        false,
    )
    .await
}

const TAGGED_LIB: &str = "package main\n\nfunc f(a, b int) int { return a*10 + b }\n";
const TAGGED_MAIN: &str =
    "package main\n\nimport \"fmt\"\n\nfunc main() { fmt.Println(f(1, 2), extra()) }\n";
const TAGGED_ON: &str = "//go:build extra\n\npackage main\n\nfunc extra() int { return f(1, 2) }\n";
const TAGGED_OFF: &str = "//go:build !extra\n\npackage main\n\nfunc extra() int { return 0 }\n";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_caller_behind_a_build_tag_refuses_the_reorder() {
    require_go_toolchain();
    let fixture = GoModule::new(&[
        ("go.mod", "module example.com/tagged\n\ngo 1.22\n"),
        ("lib.go", TAGGED_LIB),
        ("main.go", TAGGED_MAIN),
        ("extra_on.go", TAGGED_ON),
        ("extra_off.go", TAGGED_OFF),
    ]);
    assert_eq!(fixture.run(), "12 0\n");
    assert_eq!(fixture.go(&["run", "-tags", "extra", "."]).1, "12 12\n");
    let bridge = GoplsBridge::start(&fixture).await;
    let untouched = fixture.snapshot();
    for apply in [false, true] {
        let result = change(bridge.addr(), &fixture, "f(a, b", &["b", "a"], apply).await;
        eprintln!("tagged apply={apply}: {result:#?}");
        assert_eq!(fixture.snapshot(), untouched, "nothing is written");
        match result {
            Ok(change) => {
                assert!(!change.applied);
                assert!(
                    change.unmatched.iter().any(|u| u.contains("extra_on.go")),
                    "{:?}",
                    change.unmatched
                );
            }
            Err(error) => assert!(format!("{error:#}").contains("extra_on.go"), "{error:#}"),
        }
    }
}

const COMMENT_LIB: &str = "package main\n\nfunc g(a, b int) int { return a*10 + b }\n\nfunc h(a, b int) int { return a * 10 }\n";
const COMMENT_MAIN: &str = "package main\n\nimport \"fmt\"\n\nfunc main() {\n\tfmt.Println(g(1 /* first */, 2 /* second */))\n\tfmt.Println(h(1, 2 /* documents b */))\n}\n";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn comments_travel_with_their_arguments() {
    require_go_toolchain();
    let fixture = GoModule::new(&[
        ("go.mod", "module example.com/comments\n\ngo 1.22\n"),
        ("lib.go", COMMENT_LIB),
        ("main.go", COMMENT_MAIN),
    ]);
    assert_eq!(fixture.run(), "12\n10\n");
    let bridge = GoplsBridge::start(&fixture).await;
    let untouched = fixture.snapshot();

    // gopls prints `g(2, 1)`: the comments of both arguments are lost, so this refuses.
    let reorder = change(bridge.addr(), &fixture, "g(a, b", &["b", "a"], false)
        .await
        .expect("the preview runs");
    eprintln!("comments reorder preview: {reorder:#?}");
    assert!(
        reorder.unexpected.iter().any(|u| u.contains("comment")),
        "{:?}",
        reorder.unexpected
    );
    let refused = change(bridge.addr(), &fixture, "g(a, b", &["b", "a"], true)
        .await
        .expect_err("a lost comment refuses");
    assert!(format!("{refused:#}").contains("comment"), "{refused:#}");
    assert_eq!(fixture.snapshot(), untouched, "nothing is written");

    // gopls prints `h(1)`: the comment documented the removed argument and goes with it.
    let removal = change(bridge.addr(), &fixture, "h(a, b", &["a"], true)
        .await
        .expect("the removal applies");
    eprintln!("comments removal: {removal:#?}");
    assert!(removal.applied);
    assert!(removal.unexpected.is_empty(), "{:?}", removal.unexpected);
    let main = fixture.read("main.go");
    assert!(main.contains("h(1))"), "{main}");
    assert!(main.contains("g(1 /* first */, 2 /* second */)"), "{main}");
    assert_eq!(fixture.run(), "12\n10\n");
}
