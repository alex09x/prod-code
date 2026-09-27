//! Typed builders (#459), driven through `fixture::builder::preview` against a scripted gateway
//! and then compiled and run: the generated code has to build the struct with the setters called
//! in any order, and `build` has to name the field that was never set.
//!
//! The scripted analyzer answers from the text it was last sent, the way the real one does: a
//! definition is looked up at the position asked about in the overlay, and the deliberate error
//! verification places next to the builder is reported only where the overlay really has it. The
//! gateway integration test in `prod-code-gateway/tests/builder_live.rs` runs the same flow
//! against rust-analyzer itself.

use prod_code_mcp::fixture::builder::{self, BuilderRequest, Verification};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

const SETTINGS: &str = r#"//! Settings for the demo.

use std::collections::{BTreeMap, HashMap};

/// Not the standard `Option`: the builder must not use this one for its own slots.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub enum Option<T> {
    Nothing,
    Just(T),
}

/// Nor this `Result`.
pub type Result = u8;

pub mod network {
    /// A nested module's own type.
    #[derive(Debug, Clone, PartialEq)]
    pub struct Endpoint {
        pub host: String,
        pub port: u16,
    }
}

/// Everything at once: nested generic types, raw identifiers, shadowed `Option` and `Result`.
#[derive(Debug)]
pub struct Config {
    pub name: String,
    pub r#type: u8,
    pub r#match: Option<u32>,
    pub(crate) limits: HashMap<String, Vec<(u8, std::option::Option<Box<[u16; 4]>>)>>,
    pub endpoints: BTreeMap<
        u16,
        network::Endpoint,
    >,
    pub callback: fn(&str) -> std::result::Result<u8, String>,
    pub score: Result,
    pub tags: [&'static str; 2],
}

pub struct Pair<T> {
    pub left: T,
    pub right: T,
}

pub struct Point(pub i32, pub i32);

pub fn plain() {}
"#;

const CONFIG_FIELDS: [&str; 8] = [
    "name",
    "r#type",
    "r#match",
    "limits",
    "endpoints",
    "callback",
    "score",
    "tags",
];

/// Uses the builder the way a test would: setters in the reverse of declaration order, a field
/// left out, nothing set at all.
const USAGE: &str = r#"use crate::settings::network::Endpoint;
use crate::settings::{Config, ConfigBuilder, ConfigBuilderError};
use std::collections::HashMap;

fn length(s: &str) -> Result<u8, String> {
    Ok(s.len() as u8)
}

pub fn run() {
    let built = ConfigBuilder::new()
        .tags(["a", "b"])
        .score(7)
        .callback(length)
        .endpoints(
            [(80, Endpoint { host: "h".into(), port: 80 })]
                .into_iter()
                .collect(),
        )
        .limits(HashMap::from([(
            "k".to_string(),
            vec![(1, Some(Box::new([1, 2, 3, 4])))],
        )]))
        .r#match(crate::settings::Option::Just(5))
        .r#type(3)
        .name("n".to_string())
        .build()
        .expect("every field is set");
    println!(
        "{} {} {:?} {} {:?} {} {} {:?}",
        built.name,
        built.r#type,
        built.r#match,
        built.limits["k"][0].1.as_ref().unwrap()[3],
        built.endpoints[&80],
        (built.callback)("abc").unwrap(),
        built.score,
        built.tags
    );
    let missing: Result<Config, ConfigBuilderError> =
        ConfigBuilder::new().r#type(1).name("n".into()).build();
    let err = missing.unwrap_err();
    println!("{} | {}", err.field(), err);
    let err = ConfigBuilder::new().build().unwrap_err();
    let as_error: &dyn std::error::Error = &err;
    println!("{} | {}", err.field(), as_error);
}
"#;

const PRINTED: &str = "n 3 Just(5) 4 Endpoint { host: \"h\", port: 80 } 3 7 [\"a\", \"b\"]\n\
match | `Config` field `match` was never set\n\
name | `Config` field `name` was never set\n";

/// How the scripted analyzer behaves.
#[derive(Default, Clone)]
struct Script {
    /// Reports nothing at all, as for code in an inactive `cfg`.
    silent: bool,
    /// Also reports an error inside the builder.
    reject: bool,
    /// Resolves nothing, as an engine that is still loading.
    blind: bool,
    /// Names that resolve at the insertion point although the file does not declare them.
    in_scope: Vec<&'static str>,
    /// Names the workspace index lists, in another file.
    indexed: Vec<&'static str>,
    /// An outline that lists one field more than the source has.
    ghost_field: bool,
    /// The outline, instead of the one the text gives.
    outline: Option<Value>,
    /// The definition answer for `Config` at the insertion point, instead of its declaration.
    target_answer: Option<Value>,
    /// The definition answer for `ConfigBuilder` at the insertion point, instead of nothing.
    builder_answer: Option<Value>,
}

struct Scripted {
    gateway: ScriptedGateway,
    methods: Arc<Mutex<Vec<String>>>,
}

/// The 1-based line of the first line of `text` containing `needle`, and its 1-based column.
fn find(text: &str, needle: &str) -> (u32, u32) {
    let at = text
        .find(needle)
        .unwrap_or_else(|| panic!("`{needle}` is in the text"));
    let line = text[..at].matches('\n').count() as u32 + 1;
    let col = (at - text[..at].rfind('\n').map_or(0, |p| p + 1)) as u32 + 1;
    (line, col)
}

fn workspace() -> (Workspace, PathBuf) {
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        ("src/main.rs", "mod settings;\n\nfn main() {}\n"),
        ("src/settings.rs", SETTINGS),
        ("src/other.rs", "pub struct Elsewhere;\n"),
    ]);
    let file = ws.path("src/settings.rs");
    (ws, file)
}

async fn gateway(file: &Path, script: Script) -> Scripted {
    let latest = Arc::new(Mutex::new(String::new()));
    let methods = Arc::new(Mutex::new(Vec::new()));
    let (file, log) = (file.to_path_buf(), Arc::clone(&methods));
    let other = file.with_file_name("other.rs");
    let gateway = ScriptedGateway::start(move |method, params| {
        log.lock().unwrap().push(method.to_string());
        match method {
            "prod-code/handshake" => serde_json::json!({ "engine_age_ms": 3_600_000u64 }),
            "textDocument/didOpen" => {
                if let Some(text) = params.pointer("/textDocument/text").and_then(|t| t.as_str()) {
                    *latest.lock().unwrap() = text.to_string();
                }
                Value::Null
            }
            "textDocument/didChange" => {
                if let Some(text) = params
                    .pointer("/contentChanges/0/text")
                    .and_then(|t| t.as_str())
                {
                    *latest.lock().unwrap() = text.to_string();
                }
                Value::Null
            }
            "workspace/symbol" => {
                let query = params.get("query").and_then(|q| q.as_str()).unwrap_or("");
                if script.indexed.contains(&query) {
                    return serde_json::json!([answers::symbol(query, 23, &other, 1, 12)]);
                }
                match SETTINGS.find(&format!("pub struct {query}")) {
                    Some(_) if !query.is_empty() => {
                        let (line, col) = find(SETTINGS, &format!("pub struct {query}"));
                        serde_json::json!([answers::symbol(query, 23, &file, line, col + 11)])
                    }
                    _ => serde_json::json!([]),
                }
            }
            "textDocument/documentSymbol" => {
                if let Some(outline) = &script.outline {
                    return outline.clone();
                }
                let (from, _) = find(SETTINGS, "/// Everything at once");
                let (to, _) = find(SETTINGS, "\n}\n\npub struct Pair");
                let mut fields: Vec<Value> = CONFIG_FIELDS
                    .iter()
                    .map(|f| {
                        let (line, col) = find(SETTINGS, &format!("pub {f}:").replace("pub limits", "pub(crate) limits"));
                        answers::document_symbol(f, 8, line, line, col + 4)
                    })
                    .collect();
                if script.ghost_field {
                    fields.push(answers::document_symbol("ghost", 8, to, to, 5));
                }
                let (pair, _) = find(SETTINGS, "pub struct Pair");
                let (point, _) = find(SETTINGS, "pub struct Point");
                serde_json::json!([
                    answers::nested(answers::document_symbol("Config", 23, from, to + 1, 12), fields),
                    answers::document_symbol("Pair", 23, pair, pair + 3, 12),
                    answers::document_symbol("Point", 23, point, point, 12),
                ])
            }
            "textDocument/definition" => {
                if script.blind {
                    return Value::Null;
                }
                let text = latest.lock().unwrap().clone();
                let line = params.pointer("/position/line").and_then(|l| l.as_u64()).unwrap_or(0);
                let character = params
                    .pointer("/position/character")
                    .and_then(|c| c.as_u64())
                    .unwrap_or(0) as usize;
                let name: String = text
                    .lines()
                    .nth(line as usize)
                    .unwrap_or("")
                    .chars()
                    .skip(character)
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                let scripted = match name.as_str() {
                    "Config" => &script.target_answer,
                    "ConfigBuilder" => &script.builder_answer,
                    _ => &None,
                };
                if let Some(answer) = scripted {
                    return answer.clone();
                }
                if name == "Config" {
                    let (line, col) = find(SETTINGS, "pub struct Config");
                    answers::locations(&file, &[(line, col + 11)])
                } else if script.in_scope.contains(&name.as_str()) {
                    answers::locations(Path::new("/toolchain/lib/rustlib/src/core/src/lib.rs"), &[(9, 1)])
                } else {
                    Value::Null
                }
            }
            "textDocument/diagnostic" => {
                let text = latest.lock().unwrap().clone();
                let mut items = Vec::new();
                for (i, line) in text.lines().enumerate() {
                    let at = |needle: &str| line.find(needle).map(|c| (i as u64, c as u64));
                    if !script.silent
                        && let Some((l, c)) = at("__prod_code_missing_method")
                    {
                        items.push(serde_json::json!({
                            "severity": 1, "code": "E0599",
                            "message": "no method `__prod_code_missing_method` on type `ConfigBuilder`",
                            "range": { "start": { "line": l, "character": c }, "end": { "line": l, "character": c + 5 } }
                        }));
                    }
                    if script.reject
                        && let Some((l, c)) = at("self.tags.ok_or(")
                    {
                        items.push(serde_json::json!({
                            "severity": 1, "code": "E0308",
                            "message": "mismatched types",
                            "range": { "start": { "line": l, "character": c }, "end": { "line": l, "character": c + 4 } }
                        }));
                    }
                }
                serde_json::json!({ "kind": "full", "items": items })
            }
            _ => Value::Null,
        }
    })
    .await;
    Scripted { gateway, methods }
}

/// Every file of the checkout but `.git`, to show a preview wrote nothing.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).expect("read dir").flatten() {
            let path = entry.path();
            if path.file_name().is_some_and(|n| n == ".git") {
                continue;
            }
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.insert(path.clone(), std::fs::read(&path).expect("read"));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, &mut out);
    out
}

fn request<'a>(symbol: &'a str, verify: bool) -> BuilderRequest<'a> {
    BuilderRequest {
        symbol,
        verify,
        ..Default::default()
    }
}

/// Compiles `main.rs` with the other files next to it, denying every warning but dead code, and
/// returns what the program prints.
fn compile_and_run(files: &[(&str, &str)]) -> String {
    let dir = tempfile::Builder::new()
        .prefix("builder-run-")
        .tempdir()
        .expect("scratch dir");
    for (rel, text) in files {
        std::fs::write(dir.path().join(rel), text).expect("write");
    }
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let bin = dir.path().join("program");
    let built = Command::new(&rustc)
        .args(["--edition", "2021", "-D", "warnings", "-A", "dead_code"])
        .arg(dir.path().join("main.rs"))
        .arg("-o")
        .arg(&bin)
        .output()
        .unwrap_or_else(|e| panic!("cannot start {rustc:?}: {e}"));
    assert!(
        built.status.success(),
        "rustc rejects the generated builder:\n{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let ran = Command::new(&bin).output().expect("the program runs");
    assert!(
        ran.status.success(),
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    String::from_utf8(ran.stdout).expect("utf-8")
}

#[tokio::test]
async fn a_verified_builder_compiles_and_builds_in_any_setter_order() {
    let (ws, file) = workspace();
    let root = ws.root();
    let before = snapshot(&root);
    let scripted = gateway(&file, Script::default()).await;

    let preview = builder::preview(scripted.gateway.addr(), &root, &request("Config", true))
        .await
        .expect("a builder for Config");
    eprintln!("{}", preview.render());
    assert_eq!(preview.verification, Verification::Clean);
    assert!(preview.verified());
    assert!(preview.diagnostics().is_empty());
    assert_eq!(preview.file, "src/settings.rs");
    let plan = &preview.plan;
    assert_eq!(
        plan.fields
            .iter()
            .map(|f| f.name.as_str())
            .collect::<Vec<_>>(),
        CONFIG_FIELDS
    );
    let types: Vec<&str> = plan.fields.iter().map(|f| f.ty.as_str()).collect();
    assert_eq!(
        types,
        [
            "String",
            "u8",
            "Option<u32>",
            "HashMap<String, Vec<(u8, std::option::Option<Box<[u16; 4]>>)>>",
            "BTreeMap< u16, network::Endpoint, >",
            "fn(&str) -> std::result::Result<u8, String>",
            "Result",
            "[&'static str; 2]",
        ]
    );
    assert!(
        plan.code
            .contains("    pub fn r#match(mut self, value: Option<u32>) -> Self {")
    );
    assert!(
        plan.code
            .contains("    r#match: ::core::option::Option<Option<u32>>,")
    );
    assert!(
        plan.code
            .contains("pub fn build(self) -> ::core::result::Result<Config, ConfigBuilderError>")
    );
    assert!(plan.file_text.contains(&plan.code));
    let (end, _) = find(SETTINGS, "\n}\n\npub struct Pair");
    assert_eq!(plan.insert_after_line, end + 1);
    // Both analyzer checks ran: names at the insertion point, then diagnostics.
    let methods = scripted.methods.lock().unwrap().clone();
    assert!(methods.iter().any(|m| m == "textDocument/definition"));
    assert!(methods.iter().any(|m| m == "textDocument/diagnostic"));
    assert_eq!(snapshot(&root), before, "a preview writes nothing");

    let main = "mod settings;\nmod usage;\n\nfn main() {\n    usage::run();\n}\n";
    let printed = compile_and_run(&[
        ("main.rs", main),
        ("settings.rs", &plan.file_text),
        ("usage.rs", USAGE),
    ]);
    assert_eq!(printed, PRINTED);
}

#[tokio::test]
async fn without_verify_the_builder_is_unverified_and_the_analyzer_is_not_asked() {
    let (ws, file) = workspace();
    let root = ws.root();
    let before = snapshot(&root);
    let scripted = gateway(&file, Script::default()).await;
    let preview = builder::preview(scripted.gateway.addr(), &root, &request("Config", false))
        .await
        .expect("a builder for Config");
    assert!(!preview.verified());
    assert!(matches!(
        &preview.verification,
        Verification::Unverified { reason } if reason.contains("not requested")
    ));
    assert!(preview.render().contains("not verified"));
    let methods = scripted.methods.lock().unwrap().clone();
    assert!(
        !methods.iter().any(|m| m == "textDocument/diagnostic"),
        "{methods:?}"
    );
    assert!(
        !methods.iter().any(|m| m == "textDocument/definition"),
        "{methods:?}"
    );
    assert_eq!(snapshot(&root), before);
}

#[tokio::test]
async fn an_analyzer_that_cannot_see_the_scope_is_not_taken_for_a_clean_one() {
    let (ws, file) = workspace();
    let root = ws.root();
    let before = snapshot(&root);
    for (script, expected) in [
        (
            Script {
                silent: true,
                ..Default::default()
            },
            "did not report a deliberate error",
        ),
        (
            Script {
                blind: true,
                ..Default::default()
            },
            "did not resolve `Config`",
        ),
    ] {
        let scripted = gateway(&file, script).await;
        let preview = builder::preview(scripted.gateway.addr(), &root, &request("Config", true))
            .await
            .expect("the builder is still generated");
        match &preview.verification {
            Verification::Unverified { reason } => {
                assert!(reason.contains(expected), "{reason}")
            }
            other => panic!("expected unverified, got {other:?}"),
        }
        assert!(!preview.verified());
    }
    assert_eq!(snapshot(&root), before);
}

#[tokio::test]
async fn errors_in_the_builder_reject_it_with_their_lines() {
    let (ws, file) = workspace();
    let root = ws.root();
    let scripted = gateway(
        &file,
        Script {
            reject: true,
            ..Default::default()
        },
    )
    .await;
    let preview = builder::preview(scripted.gateway.addr(), &root, &request("Config", true))
        .await
        .expect("a rejected builder is still a preview");
    let (line, _) = find(&preview.plan.file_text, "self.tags.ok_or(");
    assert_eq!(
        preview.diagnostics(),
        [format!(
            "mismatched types [E0308] (src/settings.rs:{line}:{})",
            find(&preview.plan.file_text, "self.tags.ok_or(").1
        )]
    );
    assert!(!preview.verified());
    assert!(preview.render().contains("rejected:"));
}

#[tokio::test]
async fn collisions_and_unsupported_shapes_are_refused_and_nothing_is_written() {
    let (ws, file) = workspace();
    let root = ws.root();
    let before = snapshot(&root);
    let cases: Vec<(Script, BuilderRequest<'_>, &str)> = vec![
        (
            Script::default(),
            request("Pair", true),
            "generic parameters",
        ),
        (Script::default(), request("Point", true), "tuple struct"),
        (
            Script {
                indexed: vec!["ConfigBuilderError"],
                ..Default::default()
            },
            request("Config", false),
            "`ConfigBuilderError` is already declared in this workspace",
        ),
        (
            Script {
                in_scope: vec!["Extend"],
                ..Default::default()
            },
            BuilderRequest {
                builder_name: Some("Extend"),
                ..request("Config", true)
            },
            "`Extend` already names",
        ),
        (
            Script::default(),
            BuilderRequest {
                builder_name: Some("Endpoint"),
                ..request("Config", true)
            },
            "`Endpoint` already appears in this file",
        ),
        (
            Script {
                ghost_field: true,
                ..Default::default()
            },
            request("Config", true),
            "lists the fields [name, type, match, limits, endpoints, callback, score, tags, ghost]",
        ),
    ];
    for (script, request, expected) in cases {
        let scripted = gateway(&file, script).await;
        let err = builder::preview(scripted.gateway.addr(), &root, &request)
            .await
            .expect_err(expected);
        let err = format!("{err:#}");
        assert!(err.contains(expected), "{expected}\n=> {err}");
    }
    assert_eq!(snapshot(&root), before, "a refusal writes nothing");
}

/// An LSP range over the 0-based lines `from..=to`.
fn span(from: u64, to: u64) -> Value {
    serde_json::json!({
        "start": { "line": from, "character": 0 },
        "end": { "line": to, "character": 1 }
    })
}

/// The 0-based lines `Config` spans in `SETTINGS`, its doc comment included.
fn config_span() -> (u64, u64) {
    let (from, _) = find(SETTINGS, "/// Everything at once");
    let (to, _) = find(SETTINGS, "\n}\n\npub struct Pair");
    (u64::from(from) - 1, u64::from(to))
}

/// The 0-based line of `Config`'s field `field`.
fn field_line(field: &str) -> u64 {
    let needle = format!("pub {field}:").replace("pub limits", "pub(crate) limits");
    u64::from(find(SETTINGS, &needle).0) - 1
}

/// An outline holding only `Config`, spanning `range`, with `children` when given.
fn config_outline(range: Value, children: Option<Vec<Value>>) -> Value {
    let mut config = serde_json::json!({ "name": "Config", "kind": 23, "range": range });
    if let Some(children) = children {
        config["children"] = Value::Array(children);
    }
    serde_json::json!([config])
}

/// A definition answer the probe cannot read is not an answer that the name is free: a malformed
/// location for the builder's name, next to a valid one for the struct and a deliberate error the
/// analyzer reports, is unverified evidence. A null or empty answer still means "nothing there".
#[tokio::test]
async fn a_malformed_definition_answer_is_never_taken_for_a_free_name() {
    let (ws, file) = workspace();
    let root = ws.root();
    let before = snapshot(&root);
    let core = "file:///toolchain/lib/rustlib/src/core/src/lib.rs";
    let past_u32 = 1u64 << 32;
    let cases = [
        (
            "a location without a range",
            serde_json::json!([{ "uri": core }]),
        ),
        (
            "a location whose URI is not one",
            serde_json::json!([{ "uri": "not a uri", "range": span(8, 8) }]),
        ),
        (
            "a link to a URI that is not a file",
            serde_json::json!([{
                "targetUri": "untitled:Untitled-1",
                "targetRange": span(8, 8),
                "targetSelectionRange": span(8, 8)
            }]),
        ),
        (
            "a negative line",
            serde_json::json!([{ "uri": core, "range": {
                "start": { "line": -1, "character": 0 },
                "end": { "line": 0, "character": 1 }
            } }]),
        ),
        (
            "a line past u32",
            serde_json::json!([{ "uri": core, "range": span(past_u32 + 8, past_u32 + 8) }]),
        ),
        (
            "a range that ends before it starts",
            serde_json::json!([{ "uri": core, "range": span(9, 8) }]),
        ),
        ("a string", serde_json::json!("ConfigBuilder")),
        ("a null entry", serde_json::json!([null])),
    ];
    let mut wrong = Vec::new();
    for (case, answer) in cases {
        let scripted = gateway(
            &file,
            Script {
                builder_answer: Some(answer),
                ..Default::default()
            },
        )
        .await;
        match builder::preview(scripted.gateway.addr(), &root, &request("Config", true)).await {
            Ok(preview)
                if matches!(
                    &preview.verification,
                    Verification::Unverified { reason } if reason.contains("malformed definition answer")
                ) && !preview.verified() => {}
            Ok(preview) => wrong.push(format!("{case}: {:?}", preview.verification)),
            Err(err) => wrong.push(format!("{case}: refused: {err:#}")),
        }
    }
    assert!(
        wrong.is_empty(),
        "malformed evidence taken for an answer:\n{}",
        wrong.join("\n")
    );
    // No location at all is an answer: the name is free there.
    for answer in [Value::Null, serde_json::json!([])] {
        let scripted = gateway(
            &file,
            Script {
                builder_answer: Some(answer.clone()),
                ..Default::default()
            },
        )
        .await;
        let preview = builder::preview(scripted.gateway.addr(), &root, &request("Config", true))
            .await
            .expect("a builder for Config");
        assert_eq!(preview.verification, Verification::Clean, "{answer}");
    }
    assert_eq!(snapshot(&root), before, "a preview writes nothing");
}

/// A definition line at `u32::MAX` has no 1-based line; reading one overflowed.
#[tokio::test]
async fn a_definition_line_at_u32_max_is_unverified_not_a_panic() {
    let (ws, file) = workspace();
    let root = ws.root();
    let before = snapshot(&root);
    let max = u64::from(u32::MAX);
    let uri = url::Url::from_file_path(&file).unwrap().to_string();
    let scripted = gateway(
        &file,
        Script {
            target_answer: Some(serde_json::json!([{ "uri": uri, "range": span(max, max) }])),
            ..Default::default()
        },
    )
    .await;
    let preview = builder::preview(scripted.gateway.addr(), &root, &request("Config", true))
        .await
        .expect("the builder is still generated");
    match &preview.verification {
        Verification::Unverified { reason } => {
            assert!(reason.contains("malformed definition answer"), "{reason}")
        }
        other => panic!("expected unverified, got {other:?}"),
    }
    assert!(!preview.verified());
    assert_eq!(snapshot(&root), before, "a preview writes nothing");
}

/// An outline the analyzer garbled is not evidence of where the struct is or which fields it has:
/// coordinates a cast would truncate back into the file, fields without names, members listed in
/// a shape that is not a list. Each is refused, and nothing is written.
#[tokio::test]
async fn a_malformed_outline_is_refused() {
    let (ws, file) = workspace();
    let root = ws.root();
    let before = snapshot(&root);
    let (from, to) = config_span();
    let past_u32 = 1u64 << 32;
    let uri = url::Url::from_file_path(&file).unwrap().to_string();
    let flat_field = |f: &str, line: u64| {
        serde_json::json!({
            "name": f, "kind": 8, "containerName": "Config",
            "location": { "uri": uri, "range": span(line, line) }
        })
    };
    let mut flat_past_u32 = vec![serde_json::json!({
        "name": "Config", "kind": 23, "location": { "uri": uri, "range": span(from, to) }
    })];
    flat_past_u32.extend(
        CONFIG_FIELDS
            .iter()
            .map(|f| flat_field(f, past_u32 + field_line(f))),
    );
    let cases = [
        (
            "struct lines past u32 that truncate into the file",
            config_outline(span(past_u32 + from, past_u32 + to), None),
        ),
        (
            "fields without names",
            config_outline(
                span(from, to),
                Some(vec![serde_json::json!({ "kind": 8 }); CONFIG_FIELDS.len()]),
            ),
        ),
        (
            "a member without a kind",
            config_outline(
                span(from, to),
                Some(vec![serde_json::json!({ "name": "name" })]),
            ),
        ),
        (
            "children that are not a list",
            serde_json::json!([{
                "name": "Config", "kind": 23, "range": span(from, to),
                "children": { "name": "name", "kind": 8 }
            }]),
        ),
        ("flat fields on lines past u32", Value::Array(flat_past_u32)),
        (
            "an outline that is not a list",
            serde_json::json!({ "name": "Config", "kind": 23, "range": span(from, to) }),
        ),
    ];
    let mut wrong = Vec::new();
    for (case, outline) in cases {
        let scripted = gateway(
            &file,
            Script {
                outline: Some(outline),
                ..Default::default()
            },
        )
        .await;
        match builder::preview(scripted.gateway.addr(), &root, &request("Config", true)).await {
            Err(err) if format!("{err:#}").contains("outline of src/settings.rs is malformed") => {}
            Err(err) => wrong.push(format!("{case}: refused for another reason: {err:#}")),
            Ok(preview) => wrong.push(format!("{case}: accepted: {:?}", preview.verification)),
        }
    }
    assert!(
        wrong.is_empty(),
        "a malformed outline taken for evidence:\n{}",
        wrong.join("\n")
    );
    // The same outline, well formed, is still read: flat fields as the Rust engine lists them.
    let mut flat = vec![serde_json::json!({
        "name": "Config", "kind": 23, "location": { "uri": uri, "range": span(from, to) }
    })];
    flat.extend(CONFIG_FIELDS.iter().map(|f| flat_field(f, field_line(f))));
    let scripted = gateway(
        &file,
        Script {
            outline: Some(Value::Array(flat)),
            ..Default::default()
        },
    )
    .await;
    let preview = builder::preview(scripted.gateway.addr(), &root, &request("Config", true))
        .await
        .expect("a builder for Config");
    assert_eq!(preview.verification, Verification::Clean);
    assert!(preview.plan.notes.is_empty(), "{:?}", preview.plan.notes);
    assert_eq!(snapshot(&root), before, "a refusal writes nothing");
}

/// An outline line at `u32::MAX` has no 1-based line; reading one overflowed.
#[tokio::test]
async fn an_outline_line_at_u32_max_is_refused_not_a_panic() {
    let (ws, file) = workspace();
    let root = ws.root();
    let before = snapshot(&root);
    let (from, _) = config_span();
    let max = u64::from(u32::MAX);
    let scripted = gateway(
        &file,
        Script {
            outline: Some(config_outline(span(from, max), None)),
            ..Default::default()
        },
    )
    .await;
    let err = builder::preview(scripted.gateway.addr(), &root, &request("Config", true))
        .await
        .expect_err("a malformed outline is refused");
    let err = format!("{err:#}");
    assert!(
        err.contains("outline of src/settings.rs is malformed"),
        "{err}"
    );
    assert_eq!(snapshot(&root), before, "a refusal writes nothing");
}

/// Forty-eight fields of nested types, set in the reverse of declaration order, then one left
/// out: the builder is complete however many fields there are.
#[test]
fn a_wide_struct_builds_with_every_setter_and_names_the_missing_one() {
    let mut decl = String::from("#[derive(Debug)]\npub struct Wide {\n");
    let mut calls = String::new();
    for i in 0..48 {
        let (ty, value) = if i % 2 == 0 {
            ("u64".to_string(), format!("{i}"))
        } else {
            (
                "Vec<std::option::Option<(u8, String)>>".to_string(),
                format!("vec![Some(({i}, String::new())), None]"),
            )
        };
        decl.push_str(&format!("    pub f{i}: {ty},\n"));
        calls.insert_str(0, &format!("        .f{i}({value})\n"));
    }
    decl.push_str("}\n");
    let plan = builder::plan(&decl, "Wide", (1, 51), None).expect("a plan");
    assert_eq!(plan.fields.len(), 48);
    let main = format!(
        "mod wide;\nuse wide::WideBuilder;\n\nfn main() {{\n    let w = WideBuilder::new()\n{calls}        .build()\n        .unwrap();\n    println!(\"{{}} {{}}\", w.f46, w.f47.len());\n    let e = WideBuilder::new()\n{}        .build()\n        .unwrap_err();\n    println!(\"{{}}\", e.field());\n}}\n",
        calls.replace("        .f17(vec![Some((17, String::new())), None])\n", "")
    );
    let printed = compile_and_run(&[("main.rs", &main), ("wide.rs", &plan.file_text)]);
    assert_eq!(printed, "46 2\nf17\n");
}
