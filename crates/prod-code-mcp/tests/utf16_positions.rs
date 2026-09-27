//! Refactorings at analyzer positions in a file with CRLF line breaks and a supplementary
//! character (😀, four bytes and two UTF-16 units) before the target on its line (#456).
//!
//! An LSP column counts UTF-16 units, and a CRLF break is two bytes of the file. The planners
//! share one conversion between positions and byte offsets; the base revision counted Unicode
//! scalars and one byte per break, so past the first CRLF every reference landed a byte early per
//! line, and past 😀 a column landed one character late. Each scenario compiles and runs the
//! program before and after the rewrite, and every refusal leaves each file byte for byte as it
//! was.

use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::Value;
use std::net::SocketAddr;
use std::path::Path;
use std::process::Command;

const CARGO: &str = "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

const U32: &str = "```rust\nu32\n```\n\n---\n\nThe 32-bit unsigned integer type.";

/// `text` with every `\n` a CRLF break.
fn crlf(text: &str) -> String {
    text.replace('\n', "\r\n")
}

/// `scale`, declared and called after 😀 on the same line, with CRLF breaks.
fn scale_program() -> String {
    crlf(
        "/* 😀 */ pub fn scale(x: u32, factor: u32) -> u32 {\n    x * 10 + factor\n}\n\nfn main() {\n    let e = \"😀\"; let direct = scale(3, 4);\n    println!(\"{e} {direct}\");\n}\n",
    )
}

/// `render`, declared and called after 😀 on the same line, with CRLF breaks.
fn render_program() -> String {
    crlf(
        "/* 😀 */ fn render(text: &str) -> String {\n    let width = 80;\n    format!(\"{text}:{width}\")\n}\n\nfn main() {\n    let e = \"😀\"; println!(\"{e} {}\", render(\"x\"));\n}\n",
    )
}

/// The 1-based line and UTF-16 column of the `nth` occurrence of `needle`, plus `skip` bytes,
/// counted here rather than by the code under test.
fn spot(text: &str, needle: &str, nth: usize, skip: usize) -> (u32, u32) {
    let at = text.match_indices(needle).nth(nth).expect(needle).0 + skip;
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let line = text[..at].matches('\n').count() as u32 + 1;
    (line, text[start..at].encode_utf16().count() as u32 + 1)
}

/// `text` with each `from` replaced by `to`, exactly once.
fn edited(text: &str, pairs: &[(&str, &str)]) -> String {
    let mut out = text.to_string();
    for (from, to) in pairs {
        assert_eq!(out.matches(from).count(), 1, "{from}");
        out = out.replace(from, to);
    }
    out
}

/// What `program` prints, compiled with the toolchain the tests run on. A compiler that cannot
/// be started fails the test: a runtime claim that was not run is not evidence.
fn run(program: &str) -> String {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("main.rs");
    let bin = dir.path().join("fixture");
    std::fs::write(&src, program).unwrap();
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let built = Command::new(&rustc)
        .args(["--edition", "2021", "-A", "warnings", "-o"])
        .arg(&bin)
        .arg(&src)
        .output()
        .unwrap_or_else(|e| panic!("cannot start {rustc:?}: {e}"));
    assert!(
        built.status.success(),
        "the program does not compile:\n{}\n{program}",
        String::from_utf8_lossy(&built.stderr)
    );
    let ran = Command::new(&bin).output().unwrap();
    assert!(ran.status.success(), "the program failed: {ran:?}");
    String::from_utf8(ran.stdout).unwrap()
}

fn workspace(main: &str) -> Workspace {
    Workspace::new(&[("Cargo.toml", CARGO), ("src/main.rs", main)])
}

fn assert_untouched(ws: &Workspace, main: &str, why: &str) {
    assert_eq!(
        ws.read("Cargo.toml"),
        CARGO,
        "Cargo.toml was written: {why}"
    );
    assert_eq!(
        ws.read("src/main.rs"),
        main,
        "src/main.rs was written: {why}"
    );
}

fn swap() -> Vec<prod_code_mcp::signature::Param> {
    ["factor", "x"]
        .iter()
        .map(|n| prod_code_mcp::signature::parse_param(n).unwrap())
        .collect()
}

/// A gateway for `scale`: its references at `refs`, the call rewrite answering with
/// `rewritten`, `u32` confirmed as the built-in type, and no errors.
async fn scale_gateway(
    main: &Path,
    program: &str,
    refs: &[(u32, u32)],
    rewritten: String,
) -> (ScriptedGateway, SocketAddr) {
    let (main, program, refs) = (main.to_path_buf(), program.to_string(), refs.to_vec());
    let g = ScriptedGateway::start(move |method, _| match method {
        "textDocument/references" => answers::locations(&main, &refs),
        "prodCode/structuralReplace" => answers::whole_file(&main, &program, &rewritten),
        "textDocument/hover" => answers::hover(U32),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;
    let addr = g.addr();
    (g, addr)
}

/// A reorder of `scale` whose declaration and call each follow 😀 on a CRLF line: the analyzer's
/// UTF-16 positions find both, the dry run leaves nothing unmatched, and the written program
/// keeps every CRLF and prints what the original printed. The base revision read the call five
/// bytes early and the declaration one character late, and refused.
#[tokio::test]
async fn a_reorder_after_supplementary_text_on_crlf_lines_is_written_and_runs_the_same() {
    let program = scale_program();
    let ws = workspace(&program);
    let (root, main) = (ws.root(), ws.path("src/main.rs"));
    let call = ("scale(3, 4)", "scale(4, 3)");
    let (line, col) = spot(&program, "scale(x", 0, 0);
    assert_eq!((line, col), (1, 17), "😀 is two columns");
    let refs = [spot(&program, call.0, 0, 0)];
    assert_eq!(refs[0], (6, 32));
    let (_g, remote) = scale_gateway(&main, &program, &refs, edited(&program, &[call])).await;

    let dry =
        prod_code_mcp::signature::change(remote, &root, &main, line, col, &swap(), false, false)
            .await
            .unwrap_or_else(|e| panic!("the dry run reports: {e:#}"));
    assert!(dry.unmatched.is_empty(), "{:?}", dry.unmatched);
    assert!(dry.unexpected.is_empty(), "{:?}", dry.unexpected);
    assert_eq!(dry.symbol, "scale");
    assert_untouched(&ws, &program, "a dry run");

    let done =
        prod_code_mcp::signature::change(remote, &root, &main, line, col, &swap(), true, false)
            .await
            .unwrap_or_else(|e| panic!("the reorder is written: {e:#}"));
    assert!(done.applied, "{}", done.render(4000));
    let expected = edited(
        &program,
        &[
            call,
            ("scale(x: u32, factor: u32)", "scale(factor: u32, x: u32)"),
        ],
    );
    let now = ws.read("src/main.rs");
    assert_eq!(now, expected);
    assert_eq!(now.matches("\r\n").count(), program.matches("\r\n").count());
    assert_eq!(run(&program), "😀 34\n");
    assert_eq!(run(&now), run(&program), "{now}");
}

/// A parameter extracted from `render`, whose declaration and call follow 😀 on CRLF lines: a
/// second planner over the same conversion. The selection, the declaration the outline names and
/// the call the references place are each where the file has them, and the program prints the
/// same.
#[tokio::test]
async fn an_extracted_parameter_after_supplementary_text_on_crlf_lines_runs_the_same() {
    let program = render_program();
    let ws = workspace(&program);
    let (root, main) = (ws.root(), ws.path("src/main.rs"));
    let call = spot(&program, "render(\"x\")", 0, 0);
    assert_eq!(call, (7, 38), "😀 is two columns");
    let symbols = serde_json::json!([
        answers::document_symbol("render", 12, 1, 4, spot(&program, "render(text", 0, 0).1),
        answers::document_symbol("main", 12, 6, 8, 4),
    ]);
    let (m, s) = (main.clone(), symbols.clone());
    let g = ScriptedGateway::start(move |method, _| match method {
        "textDocument/references" => answers::locations(&m, &[call]),
        "textDocument/documentSymbol" => s.clone(),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;
    let (from, to) = (spot(&program, "80", 0, 0), spot(&program, "80", 0, 2));
    let done = prod_code_mcp::extract_parameter::extract(
        g.addr(),
        &root,
        &main,
        from,
        to,
        "width_limit",
        Some("usize"),
        false,
        true,
        false,
    )
    .await
    .unwrap_or_else(|e| panic!("the parameter is extracted: {e:#}"));
    assert!(
        done.applied && done.unmatched.is_empty(),
        "{:?}",
        done.unmatched
    );
    assert_eq!(done.symbol, "render");
    let expected = edited(
        &program,
        &[
            (
                "render(text: &str)",
                "render(text: &str, width_limit: usize)",
            ),
            ("let width = 80;", "let width = width_limit;"),
            ("render(\"x\")", "render(\"x\", 80)"),
        ],
    );
    let now = ws.read("src/main.rs");
    assert_eq!(now, expected);
    assert_eq!(run(&program), "😀 x:80\n");
    assert_eq!(run(&now), run(&program), "{now}");
}

/// Positions on no character are refused, never moved to a nearby one: a declaration asked for
/// between the two halves of 😀 or past the end of its line, and an analyzer reference that
/// splits 😀 or runs past its line. Every write, forced or not, leaves each file as it was.
#[tokio::test]
async fn positions_on_no_character_are_refused_and_nothing_is_written() {
    let program = scale_program();
    let ws = workspace(&program);
    let (root, main) = (ws.root(), ws.path("src/main.rs"));
    let call = ("scale(3, 4)", "scale(4, 3)");
    let good = spot(&program, call.0, 0, 0);
    let (_g, remote) = scale_gateway(&main, &program, &[good], edited(&program, &[call])).await;
    // Column 5 is the second half of 😀, and 999 is past the end of the line.
    for (line, col) in [(1, 5), (1, 999), (0, 17), (1, 0), (99, 1)] {
        for force in [false, true] {
            let err = prod_code_mcp::signature::change(
                remote,
                &root,
                &main,
                line,
                col,
                &swap(),
                true,
                force,
            )
            .await
            .expect_err(&format!("{line}:{col} is on no character (force {force})"));
            assert!(
                format!("{err:#}").contains("not at the resolved position"),
                "{line}:{col}: {err:#}"
            );
            assert_untouched(
                &ws,
                &program,
                &format!("declaration {line}:{col}, force {force}"),
            );
        }
    }

    // On the call's line 😀 takes columns 14 and 15, and the line ends at column 44. A reference
    // the file does not hold stops the plan, the dry run included.
    let e = spot(&program, "😀\"; let direct", 0, 0);
    assert_eq!(e, (6, 14));
    for stale in [(6, 15), (6, 45), (6, 999)] {
        let (_g, remote) =
            scale_gateway(&main, &program, &[good, stale], edited(&program, &[call])).await;
        let place = format!("src/main.rs:{}:{}", stale.0, stale.1);
        for (apply, force) in [(false, false), (true, false), (true, true)] {
            let err = prod_code_mcp::signature::change(
                remote,
                &root,
                &main,
                1,
                17,
                &swap(),
                apply,
                force,
            )
            .await
            .expect_err(&format!(
                "planned past {place} (apply {apply}, force {force})"
            ));
            assert!(format!("{err:#}").contains(&place), "{err:#}");
            assert_untouched(&ws, &program, &format!("reference {place}, force {force}"));
        }
    }
}

/// The same program against a real gateway and its rust-analyzer: the references, the positions
/// and the structural rewrite are the analyzer's own, over a file with CRLF breaks and 😀 before
/// the declaration and the call on their lines. The reorder is written and prints the same; a
/// declaration asked for inside 😀 or past its line is refused with the file as it was. It runs
/// when `PROD_CODE_LIVE_GATEWAY` holds the address of a running gateway on this machine, built
/// from this checkout; invoke this ignored integration test explicitly with that prerequisite.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires PROD_CODE_LIVE_GATEWAY pointing to a running gateway"]
async fn a_real_rust_analyzer_reorder_after_supplementary_text_on_crlf_lines() {
    let addr = std::env::var("PROD_CODE_LIVE_GATEWAY")
        .expect("set PROD_CODE_LIVE_GATEWAY to run this integration test")
        .parse::<SocketAddr>()
        .expect("PROD_CODE_LIVE_GATEWAY must be a socket address");
    let program = scale_program();
    let printed = run(&program);
    assert_eq!(printed, "😀 34\n");
    // Not a dot-directory: some tools pass over hidden ones.
    let dir = tempfile::Builder::new()
        .prefix("utf16-live-")
        .tempdir()
        .expect("checkout dir");
    let root = std::fs::canonicalize(dir.path()).expect("canonical root");
    for (rel, text) in [("Cargo.toml", CARGO), ("src/main.rs", program.as_str())] {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(path, text).expect("write");
    }
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(&root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("git runs")
    };
    assert!(git(&["init", "-q"]).success());
    assert!(git(&["-c", "core.autocrlf=false", "add", "-A"]).success());
    git(&[
        "-c",
        "user.email=test@example.invalid",
        "-c",
        "user.name=test",
        "commit",
        "-qm",
        "fixture",
    ]);
    let main = root.join("src/main.rs");
    let (line, col) = spot(&program, "scale(x", 0, 0);
    let call = ("scale(3, 4)", "scale(4, 3)");

    // Until the analyzer has loaded the crate it lists no references; a dry run costs nothing,
    // so it is repeated until the call comes back rewritten.
    let mut dry = Err(anyhow::anyhow!("not asked"));
    for _ in 0..60 {
        dry =
            prod_code_mcp::signature::change(addr, &root, &main, line, col, &swap(), false, false)
                .await;
        if dry.as_ref().is_ok_and(|d| {
            d.unmatched.is_empty()
                && d.unexpected.is_empty()
                && d.rewritten
                    .iter()
                    .any(|(_, t)| t.contains(call.1) && !t.contains(call.0))
        }) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
    }
    let dry = dry.unwrap_or_else(|e| panic!("the dry run reports: {e:#}"));
    eprintln!("{}", dry.render(4000));
    assert!(dry.unmatched.is_empty(), "{:?}", dry.unmatched);
    assert!(dry.unexpected.is_empty(), "{:?}", dry.unexpected);
    assert_eq!(std::fs::read_to_string(&main).expect("read"), program);

    // With the crate loaded, a declaration inside 😀, past its line or on no line is refused,
    // forced or not, and the file stays as it was.
    for (l, c) in [(1, 5), (1, 999), (99, 1)] {
        for force in [false, true] {
            let err =
                prod_code_mcp::signature::change(addr, &root, &main, l, c, &swap(), true, force)
                    .await
                    .expect_err(&format!("{l}:{c} is on no character (force {force})"));
            assert!(
                format!("{err:#}").contains("not at the resolved position"),
                "{l}:{c}: {err:#}"
            );
            assert_eq!(std::fs::read_to_string(&main).expect("read"), program);
        }
    }

    let done =
        prod_code_mcp::signature::change(addr, &root, &main, line, col, &swap(), true, false)
            .await
            .unwrap_or_else(|e| panic!("the reorder is written: {e:#}"));
    assert!(done.applied, "{}", done.render(4000));
    let now = std::fs::read_to_string(&main).expect("read");
    let expected = edited(
        &program,
        &[
            call,
            ("scale(x: u32, factor: u32)", "scale(factor: u32, x: u32)"),
        ],
    );
    assert_eq!(now, expected);
    assert_eq!(run(&now), printed, "{now}");
    eprintln!(
        "prints {printed:?} as written and {:?} as reordered",
        run(&now)
    );
}
