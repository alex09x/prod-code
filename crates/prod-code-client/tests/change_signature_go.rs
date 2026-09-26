//! `prod-code change-signature` on a Go module, through a real gopls (#448).
//!
//! The binary is spawned against a scripted gateway that manufactures no answer: every
//! language request goes to a real `gopls` and diagnostics are the real Go compiler's (see
//! `prod_code_testkit::gopls`). `go` and `gopls` are required: without them this test fails.

use prod_code_testkit::gopls::{GoModule, GoplsBridge, require_go_toolchain};
use std::net::SocketAddr;
use std::path::Path;
use std::process::Output;

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

/// Help names the Go scope; a preview writes nothing; `--verify`, a removal, a result change
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
    let remote = bridge.addr();
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
        "adding or removing a parameter",
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

    // Refusals that `--force` does not lift: a removal, a result change, and a reorder that
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
}
