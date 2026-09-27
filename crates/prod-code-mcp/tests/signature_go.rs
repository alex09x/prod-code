//! Reordering Go parameters through a real gopls (#448): the adapter directly, and the public
//! MCP tool that reaches it through `signature::change_with`.
//!
//! The gateway here is the scripted one, but nothing it answers is scripted: every language
//! server request goes to a real `gopls` started on the fixture, and diagnostics come from the
//! real Go compiler (see `prod_code_testkit::gopls`). The programs are run before and after each
//! change, and what they print — including the order in which their arguments' effects
//! happened — must not change. `go` and `gopls` are required: without them these tests fail.

use futures_util::{SinkExt, StreamExt};
use prod_code_mcp::protocol::McpContentItem;
use prod_code_mcp::signature::{Modifiers, Param, SignatureChange};
use prod_code_protocol::{
    ProdCodeCodec, ShadowHypothesisResult, ShadowRunRequest, ShadowRunResponse, WireMessage,
};
use prod_code_testkit::ScriptedGateway;
use prod_code_testkit::gopls::{GoModule, GoplsBridge, require_go_toolchain, uri};
use serde_json::json;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio_util::codec::Framed;

/// Adds the compiler-shadow part of the protocol to the LSP-only test gateway. The response is
/// deliberately strict about the command and complete proposals, and models the one compiler
/// failure fixture; the source-built gateway test supplies the real remote compiler proof.
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
        "-c",
        "-mod=readonly",
        "-o",
        ".prod-code-testbins/",
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

fn keep(names: &[&str]) -> Vec<Param> {
    names.iter().map(|n| Param::Keep(n.to_string())).collect()
}

fn add(name: &str, ty: &str, value: &str) -> Param {
    Param::Add {
        name: name.to_string(),
        ty: ty.to_string(),
        value: value.to_string(),
    }
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

const ADD_LIB: &str = r#"package main

import "fmt"

var addTrace []string

func markAdd(tag string, value int) int {
	addTrace = append(addTrace, tag)
	return value
}

func wordAdd(tag string, value string) string {
	addTrace = append(addTrace, tag)
	return value
}

// Blend keeps the old argument effects in their original order.
func Blend(amount int, label string) string {
	addTrace = append(addTrace, fmt.Sprintf("Blend(%d,%s)", amount, label))
	return fmt.Sprintf("%d:%s", amount, label)
}
"#;

const ADD_MAIN: &str = r#"package main

import "fmt"

func main() {
	fmt.Println(Blend(markAdd("amount", 7), wordAdd("label", "pear")))
	fmt.Println(addTrace)
}
"#;

const ADD_OTHER: &str = r#"package main

func fromOther() string {
	return Blend(2, "other")
}
"#;

const ADD_TEST: &str = r#"package main

import "testing"

func TestBlend(t *testing.T) {
	if got := Blend(3, "test"); got != "3:test" {
		t.Fatalf("got %q", got)
	}
}
"#;

/// Additions at the beginning, middle and end are insertion-only across source and test callers.
/// Delimiters inside quoted literals do not split the request, preview writes nothing, and the
/// old side-effecting arguments run in the same order after the applied change.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn typed_literal_parameters_are_added_without_reordering_old_argument_effects() {
    require_go_toolchain();
    let fixture = GoModule::new(&[
        ("go.mod", "module example.com/add\n\ngo 1.22\n"),
        ("lib.go", ADD_LIB),
        ("main.go", ADD_MAIN),
        ("other.go", ADD_OTHER),
        ("main_test.go", ADD_TEST),
    ]);
    let before = fixture.run();
    let tests_before = go_tests(&fixture);
    assert!(before.contains("[amount label Blend(7,pear)]"), "{before}");
    let untouched = fixture.snapshot();
    let bridge = GoplsBridge::start(&fixture).await;
    let remote = with_compiler_shadow(bridge.addr()).await;
    let request = vec![
        add("prefix", "string", "\"p,)\""),
        Param::Keep("amount".into()),
        add("separator", "rune", "','"),
        Param::Keep("label".into()),
        add("scale", "int", "0x2A"),
        add("suffix", "string", "`s,)`"),
    ];
    let preview = change_with(
        remote,
        &fixture,
        "lib.go",
        "Blend(amount",
        &request,
        &Modifiers::default(),
        false,
    )
    .await
    .unwrap_or_else(|e| panic!("addition preview: {e:#}"));
    assert!(!preview.applied);
    assert_eq!(fixture.snapshot(), untouched, "preview wrote");
    assert_eq!(
        preview.new_signature,
        "prefix string, amount int, separator rune, label string, scale int, suffix string"
    );
    let main = rewritten(&preview, "main.go");
    assert!(
        main.contains(
            "Blend(\"p,)\", markAdd(\"amount\", 7), ',', wordAdd(\"label\", \"pear\"), 0x2A, `s,)`)"
        ),
        "{main}"
    );
    assert!(
        rewritten(&preview, "main_test.go")
            .contains("Blend(\"p,)\", 3, ',', \"test\", 0x2A, `s,)`)")
    );
    assert!(
        bridge.renames().is_empty(),
        "addition asked gopls to rename"
    );

    let applied = change_with(
        remote,
        &fixture,
        "lib.go",
        "Blend(amount",
        &request,
        &Modifiers::default(),
        true,
    )
    .await
    .unwrap_or_else(|e| panic!("addition applies: {e:#}"));
    assert!(applied.applied);
    let lib = fixture.read("lib.go");
    assert!(lib.contains(
        "func Blend(prefix string, amount int, separator rune, label string, scale int, suffix string) string {"
    ));
    assert_eq!(fixture.run(), before, "old argument effects changed order");
    assert_eq!(go_tests(&fixture), tests_before, "tests ran differently");
}

/// Malformed, stale and outside-checkout function locations are unknown evidence, not an empty
/// or partial success, and `force` cannot turn any of them into a write.
#[tokio::test]
async fn additions_refuse_untrusted_function_reference_evidence() {
    let fixture = GoModule::new(&[
        ("go.mod", "module example.com/evidence\n\ngo 1.22\n"),
        (
            "lib.go",
            "package evidence\n\nfunc AddMe(value int) int { return value }\n",
        ),
        (
            "use.go",
            "package evidence\n\nfunc use() int { return AddMe(1) }\n",
        ),
    ]);
    let location = |rel: &str, needle: &str| {
        let text = fixture.read(rel);
        let offset = text.find(needle).unwrap();
        let before = &text[..offset];
        let line = before.matches('\n').count() as u32;
        let character = before.rsplit('\n').next().unwrap().encode_utf16().count() as u32;
        json!({
            "uri": uri(&fixture.path(rel)),
            "range": {
                "start": { "line": line, "character": character },
                "end": { "line": line, "character": character + "AddMe".len() as u32 }
            }
        })
    };
    let declaration = location("lib.go", "AddMe");
    let call = location("use.go", "AddMe");
    let outside = tempfile::tempdir().unwrap();
    let outside_file = outside.path().join("outside.go");
    std::fs::write(&outside_file, "package outside\nfunc AddMe() {}\n").unwrap();
    let outside_location = json!({
        "uri": uri(&outside_file),
        "range": {
            "start": { "line": 1, "character": 5 },
            "end": { "line": 1, "character": 10 }
        }
    });
    let mut stale = call.clone();
    stale["range"]["start"]["character"] =
        json!(call["range"]["start"]["character"].as_u64().unwrap() + 1);
    stale["range"]["end"]["character"] =
        json!(call["range"]["end"]["character"].as_u64().unwrap() + 1);
    let cases = [
        (
            json!([declaration.clone(), { "uri": uri(&fixture.path("use.go")) }]),
            "malformed",
        ),
        (json!([declaration.clone(), stale]), "stale"),
        (
            json!([declaration.clone(), outside_location]),
            "outside the checkout",
        ),
    ];
    let untouched = fixture.snapshot();
    for (answer, said) in cases {
        let gateway = ScriptedGateway::start(move |method, _| {
            if method == "textDocument/references" {
                answer.clone()
            } else {
                serde_json::Value::Null
            }
        })
        .await;
        let error = change_with(
            gateway.addr(),
            &fixture,
            "lib.go",
            "AddMe(value",
            &[Param::Keep("value".into()), add("extra", "int", "0")],
            &Modifiers::default(),
            true,
        )
        .await
        .unwrap_err();
        let error = format!("{error:#}");
        assert!(error.contains(said), "{said}: {error}");
        assert_eq!(fixture.snapshot(), untouched, "{said} wrote");
    }
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

func UsesLen(a int) int { return a + len("x") }

func Spread(a int, rest ...int) int { return a + len(rest) }

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
        (
            "external.go",
            "//go:build ignore\n\npackage main\n\nfunc External(a int) int\n",
        ),
    ]);
    let before = fixture.run();
    eprintln!("original program:\n{before}");
    let untouched = fixture.snapshot();
    let bridge = GoplsBridge::start(&fixture).await;
    let remote = with_compiler_shadow(bridge.addr()).await;
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
    // Removing, malformed/combined additions, changing results, and a position that is no
    // declaration.
    let removed = err(change(remote, &fixture, "lib.go", "Keep(a", &["a"], true).await);
    assert!(
        removed.contains("removing `b`") && removed.contains(open),
        "{removed}"
    );
    let mut added = keep(&["a", "b"]);
    added.push(add("c", "int", "make(chan int)"));
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
    assert!(
        added.contains("must be one numeric, string or rune literal"),
        "{added}"
    );
    for (rel, needle, request, said) in [
        (
            "lib.go",
            "Keep(a",
            vec![
                Param::Keep("a".into()),
                Param::Keep("b".into()),
                add("c", "int", "0x1e+2"),
            ],
            "must be one numeric, string or rune literal",
        ),
        (
            "lib.go",
            "Keep(a",
            vec![
                Param::Keep("b".into()),
                Param::Keep("a".into()),
                add("c", "int", "0"),
            ],
            "retain every old parameter exactly once",
        ),
        (
            "lib.go",
            "UsesLen(a",
            vec![add("len", "int", "0"), Param::Keep("a".into())],
            "shadow existing references",
        ),
        (
            "lib.go",
            ") Add(x",
            vec![
                Param::Keep("x".into()),
                Param::Keep("y".into()),
                add("z", "int", "0"),
            ],
            "receiver method",
        ),
        (
            "lib.go",
            "Pair[T",
            vec![
                Param::Keep("t".into()),
                Param::Keep("u".into()),
                add("z", "int", "0"),
            ],
            "generic function",
        ),
        (
            "lib.go",
            "Spread(a",
            vec![
                Param::Keep("a".into()),
                Param::Keep("rest".into()),
                add("z", "int", "0"),
            ],
            "variadic function",
        ),
        (
            "external.go",
            "External(a",
            vec![Param::Keep("a".into()), add("z", "int", "0")],
            "no body",
        ),
    ] {
        let refused = err(change_with(
            remote,
            &fixture,
            rel,
            needle,
            &request,
            &Modifiers::default(),
            true,
        )
        .await);
        assert!(refused.contains(said), "{needle}: {refused}");
    }
    let compiler = err(change_with(
        remote,
        &fixture,
        "lib.go",
        "Keep(a",
        &[
            Param::Keep("a".into()),
            Param::Keep("b".into()),
            add("bad", "string", "1"),
        ],
        &Modifiers::default(),
        true,
    )
    .await);
    assert!(
        compiler.contains("does not compile") && compiler.contains("force` does not override"),
        "{compiler}"
    );
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
        "reorders named parameters and removes provably unused ones",
        "Grouped parameters",
        "variadic parameter stays last",
        "Go additions, parameter/result type changes",
        "empty array to remove all",
        "generic functions with calls",
        "function values and unreconciled calls",
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
    let remote = with_compiler_shadow(bridge.addr()).await;
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

    // Refusals through the tool, `force` or not: a removal, a result change, a non-literal
    // addition, and the declared order itself.
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
            json!({ "symbol": "Price", "params": ["qty", "unit", "discount", "label", "extra: int = qty + 1"], "apply": true }),
            "must be one numeric, string or rune literal",
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

    // The same public tool adds a literal-backed parameter after the native gopls reorder.
    let added = run(json!({
        "symbol": "Price",
        "params": ["label", "unit", "qty", "discount", "currency: string = \"USD,)\""],
        "apply": true,
        "force": true
    }))
    .await
    .unwrap_or_else(|e| panic!("the addition applies: {e:#}"));
    let text = text_of(&added);
    assert!(!added.is_error, "{text}");
    assert!(text.contains("[applied to 4 file(s)]"), "{text}");
    assert!(
        fixture
            .read("lib.go")
            .contains("discount float64, currency string) (total int, err error)")
    );
    assert!(
        fixture
            .read("main.go")
            .contains("Price(\"first\", u, q, 0.1, \"USD,)\")")
    );
    assert_eq!(fixture.run(), before, "the added literal changed behaviour");
    let (tested, output) = fixture.go(&["test", "-count=1", "./..."]);
    assert!(tested, "go test after the change: {output}");
}

/// The version of the `gopls` on `PATH`, for the log of a real-server run.
fn gopls_version() -> String {
    let out = std::process::Command::new("gopls")
        .arg("version")
        .output()
        .expect("gopls version runs");
    assert!(out.status.success(), "gopls version: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// `go test -v` of the module, which must pass, without its timings: the tests that ran, what
/// they logged and their verdicts.
fn go_tests(fixture: &GoModule) -> String {
    let (ok, output) = fixture.go(&["test", "-v", "-count=1", "./..."]);
    assert!(ok, "go test fails: {output}");
    output
        .lines()
        .map(|l| match l.find(" (") {
            Some(cut) if l.starts_with("--- ") => &l[..cut],
            _ if l.starts_with("ok ") => "ok",
            _ => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every comment of a Go text, in order.
fn comments_of(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| l.find("//").map(|at| l[at..].to_string()))
        .collect()
}

const REMOVE_LIB: &str = r#"package main

import "fmt"

var trace []string

// mark records an effect and passes its value on.
func mark(tag string, v int) int {
	trace = append(trace, tag)
	return v
}

// Label brackets a name; the width it was once padded to is no longer read.
func Label(name string, width int) string {
	return "[" + name + "]"
}

// Ship sends qty items to dest; the note and the priority are no longer read.
func Ship(qty int, note string, priority int, dest string) string {
	trace = append(trace, fmt.Sprintf("Ship(%d,%s)", qty, dest))
	return fmt.Sprint(qty, "->", dest)
}

var ticks int

// Tick counts, whatever it is told.
func Tick(reason string, n int) {
	ticks++
}

// Total adds xs to base; the label is not read.
func Total(label string, base int, xs ...int) int {
	for _, x := range xs {
		base += x
	}
	return base
}

// Pad returns s; the widths are not read.
func Pad(s string, widths ...int) string {
	return s
}

type Counter struct{ n int }

// Bump adds by; why is not read.
func (c *Counter) Bump(by int, why string) {
	c.n += by
}
"#;

const REMOVE_MAIN: &str = r#"package main

import "fmt"

func main() {
	w, note, prio := 8, "fragile", 2
	s := "pear"
	nums := []int{4, 5}
	fmt.Println(Label("a,b", 10), Label(s, w)) // Label("x", 1) stays a comment
	fmt.Println(Ship(3, "glass", 1, "LA"), Ship(mark("m", 2), note, prio, "NY"))
	Tick("start", 1)
	Tick(s, w)
	fmt.Println(Total("t", 1, 2, 3), Total(s, 0, nums...), Total("none", 7))
	fmt.Println(Pad("x", 1, 2), Pad("y", nums...), Pad("z"))
	var c Counter
	c.Bump(mark("b", 4), "because")
	fmt.Println(w, note, prio, ticks, c.n, "Ship(1, \"s\", 2, \"d\")")
	fmt.Println(trace)
}
"#;

const REMOVE_TEST: &str = r#"package main

import "testing"

func TestShip(t *testing.T) {
	if got := Ship(2, "n", 0, "SF"); got != "2->SF" {
		t.Fatalf("got %q", got)
	}
	t.Log(Label("t", 3), Total("u", 1, 2), Pad("p", 9))
}

func TestBump(t *testing.T) {
	var c Counter
	c.Bump(5, "test")
	Tick("test", 0)
	if c.n != 5 {
		t.Fatalf("got %d", c.n)
	}
}
"#;

/// Parameters the body provably never reads are removed through gopls, alone, with a reorder,
/// all of them, beside a kept or a removed variadic parameter, and from a method; the arguments
/// dropped are literals and plain variables, whose evaluation does nothing. The program and its
/// tests run the same before and after, and comments and strings stay as they were.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unused_parameters_are_removed_by_gopls_and_the_program_runs_the_same() {
    require_go_toolchain();
    eprintln!("gopls on PATH: {}", gopls_version());
    let fixture = GoModule::new(&[
        ("go.mod", "module example.com/remove\n\ngo 1.22\n"),
        ("lib.go", REMOVE_LIB),
        ("main.go", REMOVE_MAIN),
        ("main_test.go", REMOVE_TEST),
    ]);
    let before = fixture.run();
    let tests_before = go_tests(&fixture);
    eprintln!("original program:\n{before}\noriginal tests:\n{tests_before}");
    assert!(
        before.contains("[a,b] [pear]") && before.contains("[Ship(3,LA) m Ship(2,NY) b]"),
        "{before}"
    );
    assert!(
        tests_before.contains("--- PASS: TestShip"),
        "{tests_before}"
    );
    let comments = (comments_of(REMOVE_LIB), comments_of(REMOVE_MAIN));
    let bridge = GoplsBridge::start(&fixture).await;
    let remote = bridge.addr();

    // One unused parameter, previewed: gopls is asked for the shorter signature, and nothing is
    // written.
    let untouched = fixture.snapshot();
    let preview = change(remote, &fixture, "lib.go", "Label(name", &["name"], false)
        .await
        .unwrap_or_else(|e| panic!("the removal previews: {e:#}"));
    assert!(!preview.applied);
    assert_eq!(fixture.snapshot(), untouched, "a preview wrote");
    assert!(preview.unmatched.is_empty(), "{:?}", preview.unmatched);
    assert!(preview.unexpected.is_empty(), "{:?}", preview.unexpected);
    assert!(preview.diagnostics.is_empty(), "{:?}", preview.diagnostics);
    assert_eq!(preview.old_signature, "name string, width int");
    assert_eq!(preview.new_signature, "name string");
    assert_eq!(
        bridge.renames().last().unwrap()["newName"],
        "func(name string) string"
    );
    assert!(
        rewritten(&preview, "main.go")
            .contains("fmt.Println(Label(\"a,b\"), Label(s)) // Label(\"x\", 1) stays a comment"),
        "{}",
        rewritten(&preview, "main.go")
    );
    assert!(rewritten(&preview, "main_test.go").contains("Label(\"t\")"));
    eprintln!("{}", preview.render(4000));
    let label = change(remote, &fixture, "lib.go", "Label(name", &["name"], true)
        .await
        .unwrap_or_else(|e| panic!("the removal applies: {e:#}"));
    assert!(label.applied);
    for (path, text) in &label.rewritten {
        assert_eq!(&std::fs::read_to_string(path).unwrap(), text, "{path}");
    }
    assert!(
        fixture
            .read("lib.go")
            .contains("func Label(name string) string {")
    );

    // Two removed and the rest reordered: a call beside a literal keeps its effect, in order.
    let ship = change(
        remote,
        &fixture,
        "lib.go",
        "Ship(qty",
        &["dest", "qty"],
        true,
    )
    .await
    .unwrap_or_else(|e| panic!("the removal and reorder apply: {e:#}"));
    assert!(ship.applied);
    assert_eq!(
        ship.old_signature,
        "qty int, note string, priority int, dest string"
    );
    assert_eq!(ship.new_signature, "dest string, qty int");
    let main = fixture.read("main.go");
    assert!(
        main.contains("fmt.Println(Ship(\"LA\", 3), Ship(\"NY\", mark(\"m\", 2)))"),
        "{main}"
    );
    assert!(fixture.read("main_test.go").contains("Ship(\"SF\", 2)"));

    // Every parameter removed.
    let tick = change(remote, &fixture, "lib.go", "Tick(reason", &[], true)
        .await
        .unwrap_or_else(|e| panic!("removing every parameter applies: {e:#}"));
    assert!(tick.applied);
    assert_eq!(tick.new_signature, "");
    let main = fixture.read("main.go");
    assert!(main.contains("\tTick()\n\tTick()\n"), "{main}");
    assert!(fixture.read("lib.go").contains("func Tick() {"));

    // A fixed parameter removed before a kept variadic one: the tail, spread or not, stays.
    let total = change(
        remote,
        &fixture,
        "lib.go",
        "Total(label",
        &["base", "xs"],
        true,
    )
    .await
    .unwrap_or_else(|e| panic!("removing before a variadic parameter applies: {e:#}"));
    assert!(total.applied);
    let main = fixture.read("main.go");
    assert!(
        main.contains("Total(1, 2, 3), Total(0, nums...), Total(7)"),
        "{main}"
    );

    // The variadic parameter removed: every argument of the tail goes, a spread slice too.
    let pad = change(remote, &fixture, "lib.go", "Pad(s", &["s"], true)
        .await
        .unwrap_or_else(|e| panic!("removing a variadic parameter applies: {e:#}"));
    assert!(pad.applied);
    let main = fixture.read("main.go");
    assert!(
        main.contains("Pad(\"x\"), Pad(\"y\"), Pad(\"z\")"),
        "{main}"
    );
    assert!(fixture.read("main_test.go").contains("Pad(\"p\")"));

    // A method: the receiver stays exactly as written, the effectful argument that stays too.
    let bump = change(remote, &fixture, "lib.go", "Bump(by", &["by"], true)
        .await
        .unwrap_or_else(|e| panic!("removing a method parameter applies: {e:#}"));
    assert!(bump.applied);
    assert!(
        fixture
            .read("lib.go")
            .contains("func (c *Counter) Bump(by int) {")
    );
    assert!(fixture.read("main.go").contains("c.Bump(mark(\"b\", 4))"));
    assert!(fixture.read("main_test.go").contains("c.Bump(5)"));

    let lib = fixture.read("lib.go");
    let main = fixture.read("main.go");
    eprintln!("transformed lib.go:\n{lib}\ntransformed main.go:\n{main}");
    assert_eq!((comments_of(&lib), comments_of(&main)), comments);
    assert!(
        main.contains("\"Ship(1, \\\"s\\\", 2, \\\"d\\\")\""),
        "{main}"
    );
    let after = fixture.run();
    let tests_after = go_tests(&fixture);
    eprintln!("transformed program:\n{after}\ntransformed tests:\n{tests_after}");
    assert_eq!(after, before, "the program prints something else");
    assert_eq!(tests_after, tests_before, "the tests ran differently");
    let (vetted, output) = fixture.go(&["vet", "./..."]);
    assert!(vetted, "go vet after the change: {output}");
}

const REFUSE_REMOVE_LIB: &str = r#"package main

var trace []string

func mark(tag string, v int) int {
	trace = append(trace, tag)
	return v
}

// Keep reads both.
func Keep(a, b int) int { return a + b }

// Later reads b only in a closure.
func Later(a, b int) func() int {
	return func() int { return a + b }
}

// Drop never reads b; its callers pass effects for it.
func Drop(a, b int) int { return a }

type box struct{ n int }

func two() (int, int) { return 1, 2 }

// Spare never reads b, but is used as a value.
func Spare(a, b int) int { return a }

// Both never reads b, and is called with a pair.
func Both(a, b int) int { return a }

// Gen never reads n.
func Gen[T any](t T, n int) T { return t }
"#;

const REFUSE_REMOVE_MAIN: &str = r#"package main

import "fmt"

func main() {
	ch := make(chan int, 1)
	ch <- 7
	xs := []int{1, 2}
	p := &box{n: 3}
	i := 1
	fmt.Println(Keep(1, 2), Later(1, 2)())
	fmt.Println(Drop(1, mark("call", 2)), Drop(2, <-ch), Drop(3, xs[i]), Drop(4, p.n), Drop(5, i+1))
	f := Spare
	fmt.Println(f(1, 2), Both(two()), Gen(1, 2), trace)
}
"#;

/// A removal is refused, with `force`, before gopls is asked for an edit, whenever it is not
/// proven harmless: the body reads the parameter (directly or in a closure), a dropped argument
/// is a call, a receive, an index, a selector (which can dereference nil) or an operator, the
/// function is used as a value, a call passes a pair, or the function is generic. A reference
/// list that cannot be had stops it too. Not a byte changes, and the program runs the same.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn removals_that_are_used_effectful_or_unproven_are_refused_and_write_nothing() {
    require_go_toolchain();
    eprintln!("gopls on PATH: {}", gopls_version());
    let fixture = GoModule::new(&[
        ("go.mod", "module example.com/refuseremove\n\ngo 1.22\n"),
        ("lib.go", REFUSE_REMOVE_LIB),
        ("main.go", REFUSE_REMOVE_MAIN),
    ]);
    let before = fixture.run();
    eprintln!("original program:\n{before}");
    let untouched = fixture.snapshot();
    let bridge = GoplsBridge::start(&fixture).await;
    let remote = bridge.addr();
    let open = "remain open requirements";
    let refused = |needle: &'static str, order: &'static [&'static str]| {
        let fixture = &fixture;
        async move {
            let (line, col) = at(fixture, "lib.go", needle);
            match prod_code_mcp::signature_go::change_with(
                remote,
                fixture.root(),
                &fixture.path("lib.go"),
                line,
                col,
                &keep(order),
                &Modifiers::default(),
                true,
                true,
            )
            .await
            {
                Ok(c) => panic!("expected a refusal, got {}", c.render(3000)),
                Err(e) => format!("{e:#}"),
            }
        }
    };

    // What gopls itself does with the removal: the call, the receive and the index are dropped
    // without a trace, and the program no longer runs them.
    let (line, _) = at(&fixture, "lib.go", "func Drop");
    let native = bridge.native(
        "textDocument/rename",
        json!({
            "textDocument": { "uri": uri(&fixture.path("lib.go")) },
            "position": { "line": line - 1, "character": 0 },
            "newName": "func(a int) int"
        }),
    );
    eprintln!("gopls's own answer to removing Drop's b: {native:?}");

    let body = refused("Keep(a", &["a"]).await;
    assert!(
        body.contains("removing `b`") && body.contains("still uses it at lib.go:11:"),
        "{body}"
    );
    assert!(body.contains(open), "{body}");
    let closure = refused("Later(a", &["a"]).await;
    assert!(
        closure.contains("removing `b`") && closure.contains("still uses it at lib.go:15:"),
        "{closure}"
    );
    let effects = refused("Drop(a", &["a"]).await;
    for arg in ["mark(\"call\", 2)", "<-ch", "xs[i]", "p.n", "i+1"] {
        assert!(
            effects.contains(&format!(
                "`{arg}` is passed for the removed `b` and would no longer be evaluated"
            )),
            "{arg}: {effects}"
        );
    }
    assert!(effects.contains("`force` does not override"), "{effects}");
    let value = refused("Spare(a", &["a"]).await;
    assert!(
        value.contains("used as a value") && value.contains("main.go:13:7"),
        "{value}"
    );
    let pair = refused("Both(a", &["a"]).await;
    assert!(
        pair.contains("passes 1 argument(s) and `Both` declares 2"),
        "{pair}"
    );
    let generic = refused("Gen[T", &["t"]).await;
    assert!(
        generic.contains("removing `n`") && generic.contains("generic"),
        "{generic}"
    );
    assert!(generic.contains(open), "{generic}");

    // A reference list that cannot be had: nothing is guessed.
    bridge.fail("textDocument/references", "no package metadata");
    let unknown = refused("Drop(a", &["a"]).await;
    bridge.heal();
    assert!(unknown.contains("cannot list the references"), "{unknown}");

    assert!(
        bridge.renames().is_empty(),
        "gopls was asked for an edit: {:?}",
        bridge.renames()
    );
    assert_eq!(
        fixture.snapshot(),
        untouched,
        "a refused removal wrote to the checkout"
    );
    assert_eq!(fixture.run(), before);
}
