//! Planners that follow the analyzer's references, against answers that are not a list of them
//! (#446).
//!
//! A failed request, a reply that is not a list of locations, an entry without a position, and a
//! referenced file that cannot be read are not "no references": each stops the planner with the
//! reason before anything is written, and `force`, which overrides a compiler error, does not
//! override a missing answer. A valid empty answer (`[]`, or `null`, the protocol's "none") is
//! still no references. A reference a planner read but did not rewrite blocks the write, forced
//! or not: the file it is in is not one the planner checks, and `force` overrides the analyzer's
//! verdict on a complete plan, not a plan that is incomplete.

use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::net::SocketAddr;

const CARGO: &str = "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
const LIB: &str = "pub mod count;\npub mod even;\npub mod flags;\npub mod home;\npub mod table;\npub mod total;\n";
const COUNT: &str = "pub fn count(n: u32) -> u32 {\n    n\n}\n\npub fn twice() -> Option<u32> {\n    Some(count(2) * 2)\n}\n";
const COUNT_WRAPPED: &str = "pub fn count(n: u32) -> Option<u32> {\n    Some(n)\n}\n\npub fn twice() -> Option<u32> {\n    Some(count(2) * 2)\n}\n";
/// `count` used as a value, in a file wrapping `count` does not rewrite or check.
const TABLE: &str = "pub fn table() -> fn(u32) -> u32 {\n    crate::count::count\n}\n";
const EVEN: &str = "pub fn is_even(n: u32) -> bool {\n    n % 2 == 0\n}\n\npub fn f(n: u32) -> u32 {\n    if is_even(n) { 1 } else { 0 }\n}\n";
const FLAGS: &str = "pub struct Flags {\n    pub enabled: bool,\n}\n\npub fn on(f: &Flags) -> bool {\n    f.enabled\n}\n";
const HOME: &str = "pub fn render(text: &str) -> String {\n    let width = 80;\n    format!(\"{text}{width}\")\n}\n\npub fn caller() -> String {\n    render(\"x\")\n}\n";
const TOTAL: &str = "pub fn total(v: &Vec<u32>) -> u32 {\n    v.as_ref().iter().sum()\n}\n";

const FILES: [(&str, &str); 8] = [
    ("Cargo.toml", CARGO),
    ("src/lib.rs", LIB),
    ("src/count.rs", COUNT),
    ("src/table.rs", TABLE),
    ("src/even.rs", EVEN),
    ("src/flags.rs", FLAGS),
    ("src/home.rs", HOME),
    ("src/total.rs", TOTAL),
];

fn workspace() -> Workspace {
    Workspace::new(&FILES)
}

/// Every file is as the fixture wrote it.
fn assert_untouched(ws: &Workspace, why: &str) {
    for (rel, text) in FILES {
        assert_eq!(ws.read(rel), text, "{rel} was written: {why}");
    }
}

/// Puts back what a successful apply wrote.
fn restore(ws: &Workspace) {
    for (rel, text) in FILES {
        ws.write(rel, text);
    }
}

/// A gateway that answers `references` with `refs`, finds no errors, and answers the other
/// methods a planner asks from `also`.
async fn gateway(refs: Value, also: Vec<(&'static str, Value)>) -> SocketAddr {
    ScriptedGateway::start(move |method, _params| match method {
        "textDocument/references" => refs.clone(),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => also
            .iter()
            .find(|(m, _)| *m == method)
            .map_or(Value::Null, |(_, v)| v.clone()),
    })
    .await
    .addr()
}

/// The four ways a planner is handed references, each through a different orchestration: call
/// sites rewritten after an assist (wrap), calls negated with the declaration renamed (invert),
/// calls given a new argument (extract parameter), and callers only checked (generify).
#[derive(Clone, Copy, Debug)]
enum Shape {
    Wrap,
    Invert,
    ExtractParameter,
    Generify,
}

const SHAPES: [Shape; 4] = [
    Shape::Wrap,
    Shape::Invert,
    Shape::ExtractParameter,
    Shape::Generify,
];

/// Runs `shape` against a gateway answering `references` with `refs`; `Ok(applied)`.
async fn run(
    ws: &Workspace,
    shape: Shape,
    refs: Value,
    apply: bool,
    force: bool,
) -> anyhow::Result<bool> {
    let root = ws.root();
    match shape {
        Shape::Wrap => {
            let count = ws.path("src/count.rs");
            let assist = answers::whole_file(&count, COUNT, COUNT_WRAPPED);
            let remote = gateway(refs, vec![("prodCode/applyAssist", assist)]).await;
            prod_code_mcp::wrap_return::wrap(
                remote,
                &root,
                &count,
                1,
                8,
                prod_code_mcp::wrap_return::Wrapper::Option,
                None,
                apply,
                force,
            )
            .await
            .map(|done| done.applied)
        }
        Shape::Invert => {
            let remote = gateway(refs, Vec::new()).await;
            prod_code_mcp::invert_boolean::invert(
                remote,
                &root,
                &ws.path("src/even.rs"),
                1,
                8,
                "is_odd",
                apply,
                force,
            )
            .await
            .map(|done| done.applied)
        }
        Shape::ExtractParameter => {
            let symbols = json!([
                answers::document_symbol("render", 12, 1, 4, 8),
                answers::document_symbol("caller", 12, 6, 8, 8),
            ]);
            let remote = gateway(refs, vec![("textDocument/documentSymbol", symbols)]).await;
            prod_code_mcp::extract_parameter::extract(
                remote,
                &root,
                &ws.path("src/home.rs"),
                (2, 17),
                (2, 19),
                "width_limit",
                Some("usize"),
                false,
                apply,
                force,
            )
            .await
            .map(|done| done.applied)
        }
        Shape::Generify => {
            let remote = gateway(refs, Vec::new()).await;
            prod_code_mcp::generify::generify(
                remote,
                &root,
                &ws.path("src/total.rs"),
                1,
                8,
                "v",
                "AsRef<[u32]>",
                "T",
                apply,
                force,
            )
            .await
            .map(|done| done.applied)
        }
    }
}

/// A reference in each shape's own file that it rewrites, so a planner that drops the bad
/// entry still has work to do.
fn good_reference(ws: &Workspace, shape: Shape) -> Value {
    match shape {
        Shape::Wrap => answers::locations(&ws.path("src/count.rs"), &[(6, 10)]),
        Shape::Invert => answers::locations(&ws.path("src/even.rs"), &[(6, 8)]),
        Shape::ExtractParameter => answers::locations(&ws.path("src/home.rs"), &[(7, 5)]),
        Shape::Generify => Value::Array(Vec::new()),
    }
}

/// A request the analyzer failed, a reply that is not a list, and a list with an entry that
/// has no position stop every shape before anything is written — the dry run, and the forced
/// write. The base revision took each for "no references": it reported a plan and, forced,
/// wrote it.
#[tokio::test]
async fn a_failed_or_malformed_reference_answer_stops_every_planner_even_when_forced() {
    let ws = workspace();
    let anchor = ws.path("src/lib.rs");
    let failures = [
        (
            "a failed request",
            answers::rpc_error(-32801, "content modified"),
            "content modified",
        ),
        (
            "a reply that is not a list",
            json!({ "error": "the index is not ready" }),
            "not a list",
        ),
        (
            "an entry without a position",
            json!([{ "uri": answers::uri(&anchor) }]),
            "has no file or start position",
        ),
    ];
    // Every shape is tried before the verdict, so a failure names all that let one through.
    let mut planned = Vec::new();
    for shape in SHAPES {
        for (what, refs, cause) in &failures {
            for (apply, force) in [(false, false), (true, true)] {
                let outcome = run(&ws, shape, refs.clone(), apply, force).await;
                let Err(err) = outcome else {
                    planned.push(format!(
                        "{shape:?} with {what} (apply {apply}, force {force}): {outcome:?}"
                    ));
                    restore(&ws);
                    continue;
                };
                let text = format!("{err:#}");
                assert!(
                    text.contains("nothing was planned") && text.contains(cause),
                    "{shape:?} with {what}: {text}"
                );
                assert_untouched(&ws, &format!("{shape:?} with {what}"));
            }
        }
    }
    assert!(
        planned.is_empty(),
        "planned as if there were no references:\n{}",
        planned.join("\n")
    );
}

/// A referenced file that cannot be read is an error naming it, forced or not; the base
/// revision read it as empty, reported its reference as unmatched or skipped it, and wrote the
/// rest.
#[tokio::test]
async fn an_unreadable_referenced_file_is_not_read_as_empty() {
    let ws = workspace();
    let gone = ws.root().join("src/gone.rs");
    let mut planned = Vec::new();
    for shape in [Shape::Wrap, Shape::Invert, Shape::ExtractParameter] {
        let mut refs = good_reference(&ws, shape).as_array().cloned().unwrap();
        refs.extend(
            answers::locations(&gone, &[(1, 1)])
                .as_array()
                .cloned()
                .unwrap(),
        );
        for (apply, force) in [(false, false), (true, true)] {
            let outcome = run(&ws, shape, Value::Array(refs.clone()), apply, force).await;
            let Err(err) = outcome else {
                planned.push(format!(
                    "{shape:?} (apply {apply}, force {force}): {outcome:?}"
                ));
                restore(&ws);
                continue;
            };
            let text = format!("{err:#}");
            assert!(
                text.contains("cannot read") && text.contains("gone.rs"),
                "{shape:?}: {text}"
            );
            assert_untouched(&ws, &format!("{shape:?} with an unreadable file"));
        }
    }
    assert!(
        planned.is_empty(),
        "planned without src/gone.rs:\n{}",
        planned.join("\n")
    );
}

/// The controls: `[]` and `null` are valid answers with no references, and every shape still
/// plans and writes on them. A reference each shape rewrites is written too.
#[tokio::test]
async fn a_valid_empty_answer_is_still_no_references() {
    let ws = workspace();
    for shape in SHAPES {
        for refs in [
            Value::Array(Vec::new()),
            Value::Null,
            good_reference(&ws, shape),
        ] {
            let dry = run(&ws, shape, refs.clone(), false, false).await;
            assert!(
                matches!(dry, Ok(false)),
                "{shape:?} dry run with {refs}: {dry:?}"
            );
            assert_untouched(&ws, "a dry run");
            let applied = run(&ws, shape, refs.clone(), true, false).await;
            assert!(
                matches!(applied, Ok(true)),
                "{shape:?} apply with {refs}: {applied:?}"
            );
            restore(&ws);
        }
    }
}

/// Wrapping a return type rewrites the calls it can; a reference it could not read as a call —
/// here `count` used as a value in a file it neither rewrites nor checks — is reported, and
/// stops the write, forced or not. The base revision reported it and wrote anyway.
#[tokio::test]
async fn wrapping_a_return_type_does_not_write_past_a_reference_it_did_not_rewrite() {
    let ws = workspace();
    let refs = Value::Array(
        [
            answers::locations(&ws.path("src/count.rs"), &[(6, 10)]),
            answers::locations(&ws.path("src/table.rs"), &[(2, 19)]),
        ]
        .into_iter()
        .flat_map(|v| v.as_array().cloned().unwrap())
        .collect(),
    );
    let count = ws.path("src/count.rs");
    let wrap = |apply: bool, force: bool| {
        let (root, count, refs) = (ws.root(), count.clone(), refs.clone());
        async move {
            let assist = answers::whole_file(&count, COUNT, COUNT_WRAPPED);
            let remote = gateway(refs, vec![("prodCode/applyAssist", assist)]).await;
            prod_code_mcp::wrap_return::wrap(
                remote,
                &root,
                &count,
                1,
                8,
                prod_code_mcp::wrap_return::Wrapper::Option,
                None,
                apply,
                force,
            )
            .await
        }
    };

    let done = wrap(false, false).await.expect("the dry run reports");
    assert_eq!(done.propagated, 1, "twice()");
    assert!(done.blocked.is_empty(), "{:?}", done.blocked);
    assert_eq!(done.unmatched.len(), 1, "{:?}", done.unmatched);
    assert!(
        done.unmatched[0].contains("src/table.rs:2:19") && done.unmatched[0].contains("not a call"),
        "{:?}",
        done.unmatched
    );

    let refused = wrap(true, false).await;
    let Err(err) = refused else {
        restore(&ws);
        panic!("the write went past src/table.rs:2:19: {refused:?}");
    };
    let text = format!("{err:#}");
    assert!(
        text.contains("1 reference(s) to `count` were not rewritten")
            && text.contains("nothing was written")
            && text.contains("src/table.rs:2:19"),
        "{text}"
    );
    assert_untouched(&ws, "an unmatched reference");

    // `force` overrides the analyzer; it does not complete a plan that left a reference behind.
    let forced = wrap(true, true).await;
    let Err(err) = forced else {
        restore(&ws);
        panic!("the forced write went past src/table.rs:2:19: {forced:?}");
    };
    assert!(format!("{err:#}").contains("src/table.rs:2:19"), "{err:#}");
    assert_untouched(&ws, "a forced write past an unmatched reference");
}

/// Inverting a boolean field negates every read the analyzer points at; a position where the
/// file does not say the field would keep reading the old meaning under the new name, so it
/// stops the write, forced or not. The base revision reported it and wrote anyway.
#[tokio::test]
async fn inverting_a_field_does_not_write_past_a_use_it_did_not_negate() {
    let ws = workspace();
    let flags = ws.path("src/flags.rs");
    // (6, 7) is `f.enabled`; (5, 8) is stale: `on`, not `enabled`.
    let refs = answers::locations(&flags, &[(6, 7), (5, 8)]);
    let invert = |apply: bool, force: bool| {
        let (root, flags, refs) = (ws.root(), flags.clone(), refs.clone());
        async move {
            let remote = gateway(refs, Vec::new()).await;
            prod_code_mcp::invert_boolean::invert(
                remote, &root, &flags, 2, 9, "disabled", apply, force,
            )
            .await
        }
    };

    let done = invert(false, false).await.expect("the dry run reports");
    assert_eq!(done.kind, "field");
    assert_eq!(done.negated, 1, "f.enabled");
    assert_eq!(done.unmatched.len(), 1, "{:?}", done.unmatched);
    assert!(
        done.unmatched[0].contains("src/flags.rs:5:8"),
        "{:?}",
        done.unmatched
    );

    let refused = invert(true, false).await;
    let Err(err) = refused else {
        restore(&ws);
        panic!("the write went past src/flags.rs:5:8: {refused:?}");
    };
    let text = format!("{err:#}");
    assert!(
        text.contains("would read the opposite") && text.contains("nothing was written"),
        "{text}"
    );
    assert_untouched(&ws, "an unmatched use");

    let forced = invert(true, true).await;
    let Err(err) = forced else {
        restore(&ws);
        panic!("the forced write went past src/flags.rs:5:8: {forced:?}");
    };
    assert!(
        format!("{err:#}").contains("would read the opposite"),
        "{err:#}"
    );
    assert_untouched(&ws, "a forced write past an unmatched use");
}
