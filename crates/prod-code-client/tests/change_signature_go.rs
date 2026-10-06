/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `prod-code change-signature` on a Go module, through a real gopls (#448).
//!
//! The binary is spawned against a scripted gateway that manufactures no answer: every
//! language request goes to a real `gopls` and diagnostics are the real Go compiler's (see
//! `prod_code_testkit::gopls`). `go` and `gopls` are required: without them this test fails.

use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    ProdCodeCodec, ShadowHypothesisResult, ShadowRunRequest, ShadowRunResponse, WireMessage,
};
use prod_code_testkit::gopls::{GoModule, GoplsBridge, require_go_toolchain};
use std::net::SocketAddr;
use std::path::Path;
use std::process::Output;
use tokio::net::TcpListener;
use tokio_util::codec::Framed;

async fn with_compiler_shadow(upstream: SocketAddr) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind proxy");
    let addr = listener.local_addr().expect("proxy address");
    tokio::spawn(async move {
        loop {
            let Ok((client, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let server = tokio::net::TcpStream::connect(upstream)
                    .await
                    .expect("connect scripted gateway");
                let mut client = Framed::new(client, ProdCodeCodec::new());
                let mut server = Framed::new(server, ProdCodeCodec::new());
                loop {
                    tokio::select! {
                        incoming = client.next() => match incoming {
                            Some(Ok(WireMessage::ShadowRunRequest(req))) => {
                                client.send(WireMessage::ShadowRunResponse(mock_compile(req)))
                                    .await.expect("send compiler response");
                            }
                            Some(Ok(message)) => {
                                server.send(message).await.expect("forward to scripted gateway");
                            }
                            _ => return,
                        },
                        outgoing = server.next() => match outgoing {
                            Some(Ok(message)) => {
                                client.send(message).await.expect("forward to client");
                            }
                            _ => return,
                        },
                    }
                }
            });
        }
    });
    addr
}

fn mock_compile(req: ShadowRunRequest) -> ShadowRunResponse {
    let expected = [
        "go",
        "test",
        "-exec=true",
        "-run=^$",
        "-mod=readonly",
        "./...",
    ];
    let malformed = req.command.iter().map(String::as_str).ne(expected)
        || req.timeout_secs != 120
        || req.parallel != 1
        || req.tail_bytes != 16 * 1024
        || req.env != [("GOTOOLCHAIN".into(), "local".into())]
        || req.hypotheses.len() != 1
        || req.hypotheses[0].name != "go-compiler-verification"
        || req.hypotheses[0].files.is_empty()
        || req.hypotheses[0].files.iter().any(|file| {
            !file.relative_path.ends_with(".go") || file.content.as_ref().is_none_or(Vec::is_empty)
        });
    if malformed {
        return ShadowRunResponse {
            server_workspace_root: req.client_workspace_root,
            mode: String::new(),
            results: Vec::new(),
            error: Some("malformed compiler shadow request".into()),
        };
    }
    let proposal = req.hypotheses[0]
        .files
        .iter()
        .filter_map(|file| file.content.as_deref())
        .flatten()
        .copied()
        .collect::<Vec<_>>();
    let proposal = String::from_utf8_lossy(&proposal);
    let fails = proposal.contains("bad string") && proposal.contains("Keep(1, 2, 1)");
    let output = fails.then(|| b"cannot use 1 as string value in argument to Keep\n".to_vec());
    ShadowRunResponse {
        server_workspace_root: req.client_workspace_root,
        mode: "overlay".into(),
        results: vec![ShadowHypothesisResult {
            name: "go-compiler-verification".into(),
            exit_code: Some(if fails { 1 } else { 0 }),
            duration_ms: 1,
            timed_out: false,
            error: None,
            output_len: output.as_ref().map_or(0, |text| text.len() as u64),
            output_tail: output,
        }],
        error: None,
    }
}

const LIB: &str = r#"package main

import "fmt"

var trace []string

func mark(tag string, v int) int {
	trace = append(trace, tag)
	return v
}

// Price charges qty items of unit cents less a discount.
func Price(qty, unit int, discount float64, label string) (total int, err error) {
	trace = append(trace, fmt.Sprintf("Price(%d,%d,%.2f,%s)", qty, unit, discount, label))
	total = int(float64(qty*unit) * (1 - discount))
	return total, nil
}

func Diff(a, b int) int { return a - b }
"#;

const MAIN: &str = r#"package main

import "fmt"

func main() {
	q := 3
	t, _ := Price(q, 250, 0.1, "first")
	d := Diff(mark("a", 5), mark("b", 2))
	fmt.Println(t, d, trace)
}
"#;

async fn cli(root: &Path, home: &Path, remote: SocketAddr, args: &[&str]) -> Output {
    tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"))
        .arg("--remote")
        .arg(remote.to_string())
        .args(args)
        .env("PROD_CODE_REMOTE", remote.to_string())
        .env("HOME", home)
        .current_dir(root)
        .output()
        .await
        .expect("run prod-code")
}

fn said(out: &Output) -> String {
    format!(
        "status {:?}\n--- stdout\n{}\n--- stderr\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// Help names the Go scope; a preview writes nothing; `--verify`, removal of a used parameter, a result change
/// and an effect-order hazard are refused with every file kept; `--apply` writes a program
/// that prints what it printed before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn change_signature_reorders_go_parameters_from_the_command_line() {
    require_go_toolchain();
    let module = GoModule::new(&[
        ("go.mod", "module example.com/cli\n\ngo 1.22\n"),
        ("lib.go", LIB),
        ("main.go", MAIN),
    ]);
    let home = tempfile::Builder::new()
        .prefix("gosighome")
        .tempdir()
        .expect("home");
    let before = module.run();
    let untouched = module.snapshot();
    let bridge = GoplsBridge::start(&module).await;
    let remote = with_compiler_shadow(bridge.addr()).await;
    let root = module.root();
    let reorder = [
        "change-signature",
        "Price",
        "--param",
        "label",
        "--param",
        "unit",
        "--param",
        "qty",
        "--param",
        "discount",
    ];

    let help = cli(root, home.path(), remote, &["change-signature", "--help"]).await;
    assert!(help.status.success(), "{}", said(&help));
    // Whatever width the help is wrapped at, the words stay in order.
    let text = said(&help).split_whitespace().collect::<Vec<_>>().join(" ");
    for promised in [
        "Change what a function takes",
        "Go (a `.go` file, through gopls v0.23.0)",
        "permutation of the named",
        "variadic parameter stays last",
        "adds typed literal parameters",
        "compile on the node before preview or apply",
        "removes provably unused ones",
        "--remove-all",
        "generic function that has calls",
        "function used as a value",
        "`true`, `false` and `nil` count as variables",
        "Refused for Go before anything is written",
    ] {
        assert!(text.contains(promised), "{promised}: {text}");
    }

    // A preview: the report on stdout, exit 0, and no file touched.
    let preview = cli(root, home.path(), remote, &reorder).await;
    let text = said(&preview);
    assert!(preview.status.success(), "{text}");
    assert!(
        text.contains("- now: (label string, unit, qty int, discount float64)"),
        "{text}"
    );
    assert!(text.contains("Price(\"first\", 250, q, 0.1)"), "{text}");
    assert!(text.contains("nothing was written"), "{text}");
    assert_eq!(
        module.snapshot(),
        untouched,
        "a preview wrote to the checkout"
    );
    let renames = bridge.renames().len();
    assert_eq!(renames, 1, "{:?}", bridge.renames());

    // `--verify compile` is refused before gopls is asked for an edit, with or without
    // `--apply` and `--force`.
    for extra in [
        &["--verify", "compile"][..],
        &["--verify", "compile", "--apply", "--force"],
    ] {
        let args: Vec<&str> = reorder.iter().chain(extra).copied().collect();
        let refused = cli(root, home.path(), remote, &args).await;
        let text = said(&refused);
        assert!(!refused.status.success(), "{text}");
        assert!(
            text.contains("not supported for Go") && text.contains("cargo check"),
            "{text}"
        );
    }
    assert_eq!(
        bridge.renames().len(),
        renames,
        "verify asked gopls for an edit"
    );

    // Refusals that `--force` does not lift: removal of a used parameter, a result change, and a reorder that
    // would run `mark("b", 2)` before `mark("a", 5)`.
    let refusals: [(&[&str], &str); 3] = [
        (
            &[
                "change-signature",
                "Price",
                "--param",
                "label",
                "--param",
                "unit",
                "--param",
                "qty",
                "--apply",
                "--force",
            ],
            "removing `discount`",
        ),
        (
            &[
                "change-signature",
                "Price",
                "--param",
                "label",
                "--param",
                "unit",
                "--param",
                "qty",
                "--param",
                "discount",
                "--returns",
                "int",
                "--apply",
            ],
            "results",
        ),
        (
            &[
                "change-signature",
                "Diff",
                "--param",
                "b",
                "--param",
                "a",
                "--apply",
                "--force",
            ],
            "`mark(\"a\", 5)` and `mark(\"b\", 2)` would be evaluated in the opposite order",
        ),
    ];
    for (args, why) in refusals {
        let refused = cli(root, home.path(), remote, args).await;
        let text = said(&refused);
        assert!(!refused.status.success(), "{args:?}: {text}");
        assert!(text.contains(why), "{args:?}: {text}");
    }
    assert_eq!(
        module.snapshot(),
        untouched,
        "a refused change wrote to the checkout"
    );
    assert_eq!(module.run(), before);

    // Applied: both files rewritten on disk, and the program prints the same.
    let args: Vec<&str> = reorder.iter().copied().chain(["--apply"]).collect();
    let applied = cli(root, home.path(), remote, &args).await;
    let text = said(&applied);
    assert!(applied.status.success(), "{text}");
    assert!(text.contains("[applied to 2 file(s)]"), "{text}");
    assert!(
        module
            .read("main.go")
            .contains("Price(\"first\", 250, q, 0.1)")
    );
    assert!(module.read("lib.go").contains(
        "func Price(label string, unit, qty int, discount float64) (total int, err error) {"
    ));
    assert_eq!(
        module.run(),
        before,
        "the reordered program prints something else"
    );

    // The existing CLI syntax for typed additions reaches the Go adapter too. The quoted value
    // contains delimiters, and `force` cannot bypass any safety check (this one is valid).
    let added = cli(
        root,
        home.path(),
        remote,
        &[
            "change-signature",
            "Price",
            "--param",
            "label",
            "--param",
            "unit",
            "--param",
            "qty",
            "--param",
            "discount",
            "--param",
            "currency: string = \"USD,)\"",
            "--apply",
            "--force",
        ],
    )
    .await;
    let text = said(&added);
    assert!(added.status.success(), "{text}");
    assert!(text.contains("[applied to 2 file(s)]"), "{text}");
    assert!(
        module
            .read("lib.go")
            .contains("discount float64, currency string) (total int, err error)")
    );
    assert!(
        module
            .read("main.go")
            .contains("Price(\"first\", 250, q, 0.1, \"USD,)\")")
    );
    assert_eq!(module.run(), before, "the added literal changed behaviour");
}

/// Real gopls edits for both partial removal and an explicitly empty list through the CLI.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn change_signature_removes_unused_go_parameters_including_all_of_them() {
    require_go_toolchain();
    let module = GoModule::new(&[
        ("go.mod", "module example.com/removecli\n\ngo 1.22\n"),
        (
            "lib.go",
            "package main\n\nfunc Keep(keep int, discard string) int {\n\treturn keep\n}\n\nfunc Empty(first int, second string) int {\n\treturn 41\n}\n",
        ),
        (
            "main.go",
            "package main\n\nimport \"fmt\"\n\nfunc main() {\n\tfmt.Println(Keep(9, \"unused\"), Empty(8, \"noop\"))\n}\n",
        ),
    ]);
    let home = tempfile::tempdir().unwrap();
    let before = module.run();
    let untouched = module.snapshot();
    let bridge = GoplsBridge::start(&module).await;
    let remote = bridge.addr();
    for args in [
        vec!["change-signature", "Empty"],
        vec![
            "change-signature",
            "Empty",
            "--remove-all",
            "--param",
            "first",
        ],
    ] {
        let out = cli(module.root(), home.path(), remote, &args).await;
        assert_eq!(out.status.code(), Some(2), "{}", said(&out));
    }
    assert!(bridge.renames().is_empty(), "invalid flags reached gopls");
    for args in [
        vec!["change-signature", "Keep", "--param", "keep"],
        vec!["change-signature", "Empty", "--remove-all"],
    ] {
        let out = cli(module.root(), home.path(), remote, &args).await;
        assert!(out.status.success(), "{}", said(&out));
        assert!(said(&out).contains("nothing was written"), "{}", said(&out));
        assert_eq!(module.snapshot(), untouched);
    }
    for args in [
        vec!["change-signature", "Keep", "--param", "keep", "--apply"],
        vec!["change-signature", "Empty", "--remove-all", "--apply"],
    ] {
        let out = cli(module.root(), home.path(), remote, &args).await;
        assert!(out.status.success(), "{}", said(&out));
    }
    assert!(module.read("lib.go").contains("func Keep(keep int) int"));
    assert!(module.read("lib.go").contains("func Empty() int"));
    assert!(module.read("main.go").contains("Keep(9)"));
    assert!(module.read("main.go").contains("Empty()"));
    assert_eq!(module.run(), before);
}
