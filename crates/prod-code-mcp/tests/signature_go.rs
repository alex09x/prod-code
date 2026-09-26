//! Reordering Go parameters through a real gopls (#448): the adapter directly, and the public
//! MCP tool that reaches it through `signature::change_with`.
//!
//! The gateway here is the scripted one, but nothing it answers is scripted: every language
//! server request goes to a real `gopls` started on the fixture, and diagnostics come from the
//! real Go compiler (see `prod_code_testkit::gopls`). The programs are run before and after each
//! change, and what they print — including the order in which their arguments' effects
//! happened — must not change. `go` and `gopls` are required: without them these tests fail.

use prod_code_mcp::protocol::McpContentItem;
use prod_code_mcp::signature::{Modifiers, Param, SignatureChange};
use prod_code_testkit::gopls::{GoModule, GoplsBridge, require_go_toolchain, uri};
use serde_json::json;
use std::net::SocketAddr;

fn keep(names: &[&str]) -> Vec<Param> {
    names.iter().map(|n| Param::Keep(n.to_string())).collect()
}

/// The 1-based position of the first `needle` in the file.
fn at(fixture: &GoModule, rel: &str, needle: &str) -> (u32, u32) {
    let text = fixture.read(rel);
    let offset = text
        .find(needle)
        .unwrap_or_else(|| panic!("{needle} in {rel}"));
    let before = &text[..offset];
    (
        before.matches('\n').count() as u32 + 1,
        before.rsplit('\n').next().unwrap().chars().count() as u32 + 1,
    )
}

async fn change(
    remote: SocketAddr,
    fixture: &GoModule,
    rel: &str,
    needle: &str,
    order: &[&str],
    apply: bool,
) -> anyhow::Result<SignatureChange> {
    change_with(
        remote,
        fixture,
        rel,
        needle,
        &keep(order),
        &Modifiers::default(),
        apply,
    )
    .await
}

async fn change_with(
    remote: SocketAddr,
    fixture: &GoModule,
    rel: &str,
    needle: &str,
    request: &[Param],
    modifiers: &Modifiers,
    apply: bool,
) -> anyhow::Result<SignatureChange> {
    let (line, col) = at(fixture, rel, needle);
    prod_code_mcp::signature_go::change_with(
        remote,
        fixture.root(),
        &fixture.path(rel),
        line,
        col,
        request,
        modifiers,
        apply,
        false,
    )
    .await
}

fn rewritten<'a>(change: &'a SignatureChange, rel: &str) -> &'a str {
    change
        .rewritten
        .iter()
        .find(|(p, _)| p.ends_with(&format!("/{rel}")))
        .map(|(_, t)| t.as_str())
        .unwrap_or_else(|| panic!("{rel} was not rewritten: {:?}", change.rewritten))
}

const SHOP_LIB: &str = r#"package main

import "fmt"

var trace []string

// note records an effect and passes its value on.
func note(tag string, v int) int {
	trace = append(trace, tag)
	return v
}

// Price charges qty items of unit cents less a discount, and records what it was given.
func Price(qty, unit int, discount float64, label string) (total int, err error) {
	trace = append(trace, fmt.Sprintf("Price(%d,%d,%.2f,%s)", qty, unit, discount, label))
	total = int(float64(qty*unit) * (1 - discount))
	return total, nil
}

type Cart struct{ items []string }

// Add puts n copies of name in the cart.
func (c *Cart) Add(name string, n int) {
	for i := 0; i < n; i++ {
		c.items = append(c.items, name)
	}
	trace = append(trace, fmt.Sprintf("Add(%s,%d)", name, n))
}

// Sum adds the values to base.
func Sum(label string, base int, xs ...int) int {
	for _, x := range xs {
		base += x
	}
	trace = append(trace, fmt.Sprintf("Sum(%s,%d)", label, base))
	return base
}
"#;

const SHOP_MAIN: &str = r#"package main

import "fmt"

func main() {
	q, u := 3, 250
	t, _ := Price(q, u, 0.1, "first")
	var c Cart
	c.Add("pear", note("n", 2))
	fmt.Println(t, report(), Sum("s", 1, 2, 3), Sum("t", 0, []int{4, 5}...), len(c.items))
	fmt.Println(trace)
}
"#;

const SHOP_OTHER: &str = r#"package main

func report() int {
	t, _ := Price(2, 100, 0, "second")
	return t
}
"#;

const SHOP_TEST: &str = r#"package main

import "testing"

func TestPrice(t *testing.T) {
	if got, _ := Price(1, 100, 0.5, "test"); got != 50 {
		t.Fatalf("got %d", got)
	}
}
"#;

fn shop() -> GoModule {
    GoModule::new(&[
        ("go.mod", "module example.com/shop\n\ngo 1.22\n"),
        ("lib.go", SHOP_LIB),
        ("main.go", SHOP_MAIN),
        ("other.go", SHOP_OTHER),
        ("main_test.go", SHOP_TEST),
    ])
}

/// Grouped parameters with named results across four files, a method whose argument calls a
/// function, and a variadic function called with a spread: previewed without a write, then
/// applied, and the program prints exactly what it printed before, effects in the same order.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn functions_and_methods_are_reordered_by_gopls_and_run_the_same() {
    require_go_toolchain();
    let fixture = shop();
    let before = fixture.run();
    eprintln!("original program:\n{before}");
    assert!(
        before.contains("[Price(3,250,0.10,first) n Add(pear,2) Price(2,100,0.00,second)"),
        "{before}"
    );
    let bridge = GoplsBridge::start(&fixture).await;
    let remote = bridge.addr();

    // A preview writes nothing.
    let untouched = fixture.snapshot();
    let preview = change(
        remote,
        &fixture,
        "lib.go",
        "Price(qty",
        &["label", "unit", "qty", "discount"],
        false,
    )
    .await
    .unwrap_or_else(|e| panic!("the preview runs: {e:#}"));
    assert!(!preview.applied);
    assert_eq!(
        fixture.snapshot(),
        untouched,
        "a preview wrote to the checkout"
    );
    assert!(preview.unmatched.is_empty(), "{:?}", preview.unmatched);
    assert!(preview.unexpected.is_empty(), "{:?}", preview.unexpected);
    assert!(preview.diagnostics.is_empty(), "{:?}", preview.diagnostics);
    assert_eq!(
        preview.old_signature,
        "qty, unit int, discount float64, label string"
    );
    assert_eq!(
        preview.new_signature,
        "label string, unit, qty int, discount float64"
    );
    assert_eq!(preview.rewritten.len(), 4, "{:?}", preview.rewritten);
    assert!(rewritten(&preview, "main.go").contains("Price(\"first\", u, q, 0.1)"));
    assert!(rewritten(&preview, "other.go").contains("Price(\"second\", 100, 2, 0)"));
    assert!(rewritten(&preview, "main_test.go").contains("Price(\"test\", 100, 1, 0.5)"));
    let rename = bridge.renames().last().cloned().unwrap();
    assert_eq!(
        rename["newName"],
        "func(label string, unit int, qty int, discount float64) (total int,err error)"
    );
    assert_eq!(rename["position"], json!({ "line": 13, "character": 0 }));
    eprintln!("{}", preview.render(4000));

    let done = change(
        remote,
        &fixture,
        "lib.go",
        "Price(qty",
        &["label", "unit", "qty", "discount"],
        true,
    )
    .await
    .unwrap_or_else(|e| panic!("the change applies: {e:#}"));
    assert!(done.applied);
    for (path, text) in &done.rewritten {
        assert_eq!(&std::fs::read_to_string(path).unwrap(), text, "{path}");
    }
    assert!(fixture.read("lib.go").contains(
        "func Price(label string, unit, qty int, discount float64) (total int, err error) {"
    ));

    // A method, whose argument calls a function: the literal beside it has nothing to reorder.
    let method = change(remote, &fixture, "lib.go", "Add(name", &["n", "name"], true)
        .await
        .unwrap_or_else(|e| panic!("the method is reordered: {e:#}"));
    assert!(method.applied);
    assert!(
        fixture
            .read("main.go")
            .contains("c.Add(note(\"n\", 2), \"pear\")")
    );
    assert!(
        fixture
            .read("lib.go")
            .contains("func (c *Cart) Add(n int, name string) {")
    );

    // A variadic function: the fixed parameters move, the variadic one stays last.
    let variadic = change(
        remote,
        &fixture,
        "lib.go",
        "Sum(label",
        &["base", "label", "xs"],
        true,
    )
    .await
    .unwrap_or_else(|e| panic!("the variadic function is reordered: {e:#}"));
    assert!(variadic.applied);
    let main = fixture.read("main.go");
    assert!(main.contains("Sum(1, \"s\", 2, 3)"), "{main}");
    assert!(main.contains("Sum(0, \"t\", []int{4, 5}...)"), "{main}");

    let after = fixture.run();
    eprintln!("transformed program:\n{after}");
    assert_eq!(after, before, "the reordered program prints something else");
    let (tested, output) = fixture.go(&["test", "-count=1", "./..."]);
    assert!(tested, "go test after the change: {output}");
    let (vetted, output) = fixture.go(&["vet", "./..."]);
    assert!(vetted, "go vet after the change: {output}");
}

const REFUSE_LIB: &str = r#"package main

import "fmt"

var trace []string

func mark(tag string, v int) int {
	trace = append(trace, tag)
	return v
}

func Diff(a, b int) int { return a - b }

func Scale(x, y int) int { return x * y }

func Pair[T any, U any](t T, u U) string { return fmt.Sprint(t, u) }

type Adder interface{ Add(x int, y string) }

type Acc struct{ total int }

func (a *Acc) Add(x int, y string) { a.total += x + len(y) }

func Blank(int, string) {}

func Keep(a, b int) int { return a + b }

func Sub(a, b int) int { return a - b }

func bump(p *int) int { *p += 10; return *p }

// shadowed passes a variable that is called true: a read, not a constant.
func shadowed() int {
	true := 1
	return Sub(true, bump(&true))
}
"#;

const REFUSE_MAIN: &str = r#"package main

import "fmt"

func main() {
	d := Diff(mark("a", 5), mark("b", 2))
	f := Scale
	var acc Acc
	var ad Adder = &acc
	acc.Add(1, "x")
	ad.Add(2, "y")
	Blank(1, "z")
	fmt.Println(d, f(2, 3), Pair(1, "s"), Keep(1, 2), acc.total, shadowed(), trace)
}
"#;

/// Everything that is not a safe reorder of named parameters is refused, says why, keeps the
/// open requirement in view, and leaves every file as it was — the program still runs the same.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unsupported_or_unsafe_changes_are_refused_and_write_nothing() {
    require_go_toolchain();
    let fixture = GoModule::new(&[
        ("go.mod", "module example.com/refuse\n\ngo 1.22\n"),
        ("lib.go", REFUSE_LIB),
        ("main.go", REFUSE_MAIN),
    ]);
    let before = fixture.run();
    eprintln!("original program:\n{before}");
    let untouched = fixture.snapshot();
    let bridge = GoplsBridge::start(&fixture).await;
    let remote = bridge.addr();
    let open = "remain open requirements";
    let err = |r: anyhow::Result<SignatureChange>| match r {
        Ok(c) => panic!("expected a refusal, got {}", c.render(2000)),
        Err(e) => format!("{e:#}"),
    };

    // gopls itself would swap the two calls: its inliner ignores effects.
    let (line, _) = at(&fixture, "lib.go", "func Diff");
    let native = bridge.native(
        "textDocument/rename",
        json!({
            "textDocument": { "uri": uri(&fixture.path("lib.go")) },
            "position": { "line": line - 1, "character": 0 },
            "newName": "func(b int, a int) int"
        }),
    );
    eprintln!("gopls's own answer to reordering Diff: {native:?}");

    // Two arguments whose effects would run the other way round.
    let effects = err(change(remote, &fixture, "lib.go", "Diff(a", &["b", "a"], true).await);
    assert!(
        effects.contains(
            "`mark(\"a\", 5)` and `mark(\"b\", 2)` would be evaluated in the opposite order"
        ),
        "{effects}"
    );
    // `true` is a local variable here, not the constant: its read and the call that changes it
    // would change places.
    let shadow = err(change(remote, &fixture, "lib.go", "Sub(a", &["b", "a"], true).await);
    assert!(
        shadow.contains("`true` and `bump(&true)` would be evaluated in the opposite order"),
        "{shadow}"
    );
    // A function value keeps the old order.
    let value = err(change(remote, &fixture, "lib.go", "Scale(x", &["y", "x"], true).await);
    assert!(
        value.contains("used as a value") && value.contains("main.go:7:7"),
        "{value}"
    );
    assert!(value.contains(open), "{value}");
    // gopls refuses a generic function with calls; its reason and the requirement both show.
    let generic = err(change(remote, &fixture, "lib.go", "Pair[T", &["u", "t"], true).await);
    assert!(
        generic.contains("gopls refused") && generic.contains("generic"),
        "{generic}"
    );
    assert!(
        generic.contains("inline") && generic.contains(open),
        "{generic}"
    );
    // Unnamed parameters cannot be named in a request.
    let unnamed = err(change(remote, &fixture, "lib.go", "Blank(int", &["b", "a"], true).await);
    assert!(
        unnamed.contains("unnamed") && unnamed.contains(open),
        "{unnamed}"
    );
    // Removing, adding, changing results, and a position that is no declaration.
    let removed = err(change(remote, &fixture, "lib.go", "Keep(a", &["a"], true).await);
    assert!(
        removed.contains("removing `b`") && removed.contains(open),
        "{removed}"
    );
    let mut added = keep(&["b", "a"]);
    added.push(Param::Add {
        name: "c".into(),
        ty: "int".into(),
        value: "0".into(),
    });
    let added = err(change_with(
        remote,
        &fixture,
        "lib.go",
        "Keep(a",
        &added,
        &Modifiers::default(),
        true,
    )
    .await);
    assert!(added.contains("adding the parameter `c`"), "{added}");
    let results = Modifiers {
        returns: Some("int64".into()),
        ..Default::default()
    };
    let results = err(change_with(
        remote,
        &fixture,
        "lib.go",
        "Keep(a",
        &keep(&["b", "a"]),
        &results,
        true,
    )
    .await);
    assert!(
        results.contains("results") && results.contains(open),
        "{results}"
    );
    let nowhere = err(change(
        remote,
        &fixture,
        "main.go",
        "fmt.Println",
        &["b", "a"],
        true,
    )
    .await);
    assert!(nowhere.contains("not in the header"), "{nowhere}");

    // A method also called through an interface: gopls rewrites the direct call and not the
    // interface's, and the type no longer implements it. The preview says both; applying refuses.
    let preview = change(remote, &fixture, "lib.go", ") Add(x", &["y", "x"], false)
        .await
        .unwrap_or_else(|e| panic!("the preview runs: {e:#}"));
    assert!(
        preview.unmatched.iter().any(|u| u.contains("main.go:11:5")),
        "{:?}",
        preview.unmatched
    );
    assert!(
        !preview.diagnostics.is_empty(),
        "the broken interface is not reported"
    );
    eprintln!("{}", preview.render(3000));
    let interface = err(change(remote, &fixture, "lib.go", ") Add(x", &["y", "x"], true).await);
    assert!(
        interface.contains("not the reorder that was asked for"),
        "{interface}"
    );

    // A query that fails stops the change: references, gopls's rename, the validation. The
    // failure is injected as an error; no answer of gopls's is made up.
    let failures: [(&str, &str, &str); 3] = [
        (
            "textDocument/references",
            "no package metadata",
            "cannot list the references",
        ),
        (
            "textDocument/rename",
            "renaming is broken today",
            "renaming is broken today",
        ),
        (
            "textDocument/diagnostic",
            "no diagnostics",
            "could not be validated",
        ),
    ];
    for (method, message, said) in failures {
        bridge.fail(method, message);
        let failed = err(change(remote, &fixture, "lib.go", "Keep(a", &["b", "a"], true).await);
        bridge.heal();
        assert!(failed.contains(said), "{method}: {failed}");
    }

    assert_eq!(
        fixture.snapshot(),
        untouched,
        "a refused change wrote to the checkout"
    );
    let after = fixture.run();
    assert_eq!(after, before);

    // And the same function, asked properly, is reordered.
    let fine = change(remote, &fixture, "lib.go", "Keep(a", &["b", "a"], true)
        .await
        .unwrap_or_else(|e| panic!("a plain reorder applies: {e:#}"));
    assert!(fine.applied);
    assert!(fixture.read("main.go").contains("Keep(2, 1)"));
    assert_eq!(fixture.run(), before);
}

fn text_of(result: &prod_code_mcp::protocol::McpToolCallResult) -> String {
    result
        .content
        .iter()
        .map(|c| match c {
            McpContentItem::Text { text } => text.as_str(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The public MCP tool `code_change_signature` reaches the Go adapter through
/// `signature::change_with`, by position and by `symbol`: a preview writes nothing, a refusal
/// writes nothing, `verify: "compile"` is refused before gopls is asked for an edit (it would
/// run `cargo check` and write on its word), and `apply` writes a program that runs the same.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_mcp_tool_previews_applies_and_refuses_go_reorders() {
    require_go_toolchain();
    let tool = prod_code_mcp::tools::list_tools()
        .into_iter()
        .find(|t| t.name == "code_change_signature")
        .expect("the tool is listed");
    for promised in [
        "Go (a `.go` file, through gopls v0.23.0)",
        "permutation of the named parameters",
        "Grouped parameters",
        "variadic parameter must stay last",
        "adding (`name: Type = expression`) or removing a parameter",
        "a generic function that has calls",
        "used as a value",
        "`true`, `false` and `nil` count as variables",
    ] {
        assert!(
            tool.description.contains(promised),
            "{promised}: {}",
            tool.description
        );
    }
    assert!(
        tool.input_schema["properties"]["verify"]["description"]
            .as_str()
            .is_some_and(|d| d.contains("Refused for a Go file")),
        "{}",
        tool.input_schema
    );

    let fixture = shop();
    let before = fixture.run();
    let untouched = fixture.snapshot();
    let bridge = GoplsBridge::start(&fixture).await;
    let remote = bridge.addr();
    let root = fixture.root().to_path_buf();
    let run = |args: serde_json::Value| {
        let root = root.clone();
        async move {
            prod_code_mcp::tools::execute_tool(remote, &root, "code_change_signature", args).await
        }
    };
    let order = json!(["label", "unit", "qty", "discount"]);
    let (line, character) = at(&fixture, "lib.go", "Price(qty");

    // `verify` for Go: refused with and without `apply`, before gopls is asked to rename.
    for apply in [false, true] {
        let refused = run(json!({
            "path": "lib.go", "line": line, "character": character,
            "params": order, "verify": "compile", "apply": apply, "force": true
        }))
        .await
        .expect_err("verify is refused for Go");
        let said = format!("{refused:#}");
        assert!(
            said.contains("not supported for Go") && said.contains("cargo check"),
            "{said}"
        );
        assert!(said.contains("nothing was written"), "{said}");
    }
    assert!(
        bridge.renames().is_empty(),
        "gopls was asked for an edit: {:?}",
        bridge.renames()
    );
    assert_eq!(
        fixture.snapshot(),
        untouched,
        "a refused verify wrote to the checkout"
    );

    // A preview by name: the report, and not a byte written.
    let preview = run(json!({ "symbol": "Price", "params": order }))
        .await
        .unwrap_or_else(|e| panic!("the preview runs: {e:#}"));
    let text = text_of(&preview);
    assert!(!preview.is_error, "{text}");
    assert!(
        text.contains("- was: (qty, unit int, discount float64, label string)")
            && text.contains("- now: (label string, unit, qty int, discount float64)"),
        "{text}"
    );
    assert!(text.contains("Price(\"first\", u, q, 0.1)"), "{text}");
    assert!(text.contains("nothing was written"), "{text}");
    assert_eq!(bridge.renames().len(), 1);
    assert_eq!(
        fixture.snapshot(),
        untouched,
        "a preview wrote to the checkout"
    );

    // Refusals through the tool, `force` or not: a removal, a result change, an addition, and
    // the declared order itself.
    let refusals = [
        (
            json!({ "symbol": "Price", "params": ["label", "unit", "qty"], "apply": true, "force": true }),
            "removing `discount`",
        ),
        (
            json!({ "symbol": "Price", "params": order, "returns": "int", "apply": true }),
            "results",
        ),
        (
            json!({ "symbol": "Price", "params": ["label", "unit", "qty", "discount", "extra: int = 0"], "apply": true }),
            "adding the parameter `extra`",
        ),
        (
            json!({ "symbol": "Price", "params": ["qty", "unit", "discount", "label"], "apply": true }),
            "nothing to change",
        ),
    ];
    for (args, said) in refusals {
        let refused = run(args.clone()).await.expect_err("refused");
        let text = format!("{refused:#}");
        assert!(text.contains(said), "{args}: {text}");
    }
    assert_eq!(
        fixture.snapshot(),
        untouched,
        "a refused change wrote to the checkout"
    );

    // Applied by position: every file on disk is the reordered one, and it runs the same.
    let applied = run(json!({
        "path": "lib.go", "line": line, "character": character, "params": order, "apply": true
    }))
    .await
    .unwrap_or_else(|e| panic!("the change applies: {e:#}"));
    let text = text_of(&applied);
    assert!(!applied.is_error, "{text}");
    assert!(text.contains("[applied to 4 file(s)]"), "{text}");
    assert!(
        fixture
            .read("main.go")
            .contains("Price(\"first\", u, q, 0.1)")
    );
    assert!(
        fixture
            .read("other.go")
            .contains("Price(\"second\", 100, 2, 0)")
    );
    assert!(fixture.read("lib.go").contains(
        "func Price(label string, unit, qty int, discount float64) (total int, err error) {"
    ));
    assert_eq!(
        fixture.run(),
        before,
        "the reordered program prints something else"
    );
    let (tested, output) = fixture.go(&["test", "-count=1", "./..."]);
    assert!(tested, "go test after the change: {output}");
}
