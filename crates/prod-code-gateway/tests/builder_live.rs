//! Typed builders (#459) against the real gateway and its rust-analyzer: the declaration is found
//! through the analyzer's own symbols, the names the builder introduces are looked up at the
//! insertion point, the builder is checked in the scope it would be inserted into, and the
//! result is then compiled and run. Nothing here is scripted, and nothing is skipped: without
//! the server binary, `rustc` or a verdict from the analyzer the test fails.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use prod_code_mcp::fixture::builder::{self, BuilderPreview, BuilderRequest, Verification};

/// `prod-code-server` on a port of its own choosing, with empty storage.
struct Gateway {
    child: Child,
    addr: SocketAddr,
    _storage: tempfile::TempDir,
}

impl Gateway {
    fn start() -> Self {
        let storage = tempfile::tempdir().expect("storage dir");
        let mut child = Command::new(env!("CARGO_BIN_EXE_prod-code-server"))
            .env("PROD_CODE_STORAGE", storage.path())
            .env("PROD_CODE_PEERS", "")
            .env("PROD_CODE_BIND", "127.0.0.1:0")
            .env("RUST_LOG", "info")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("the server binary starts");
        let stdout = child.stdout.take().expect("stdout is piped");
        let (tx, rx) = std::sync::mpsc::channel();
        // The daemon logs the address it bound; the pipe is drained after that so it never
        // fills and stops the child.
        std::thread::spawn(move || {
            use std::io::BufRead;
            let mut reader = std::io::BufReader::new(stdout);
            let mut line = String::new();
            let mut found = false;
            while matches!(reader.read_line(&mut line), Ok(n) if n > 0) {
                if !found
                    && let Some(rest) = line.split("listening on ").nth(1)
                    && let Ok(addr) = rest.trim().parse::<SocketAddr>()
                {
                    found = true;
                    let _ = tx.send(addr);
                }
                line.clear();
            }
        });
        let addr = rx
            .recv_timeout(Duration::from_secs(60))
            .expect("the gateway says where it is listening within a minute");
        let deadline = Instant::now() + Duration::from_secs(30);
        while std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_err() {
            assert!(Instant::now() < deadline, "the gateway never listened");
            std::thread::sleep(Duration::from_millis(100));
        }
        Self {
            child,
            addr,
            _storage: storage,
        }
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        unsafe {
            libc::kill(self.child.id() as i32, libc::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

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

pub struct Packet<'a, T: Clone, const N: usize>
where
    T: PartialEq,
{
    pub label: &'a str,
    pub payload: T,
    pub bytes: [u8; N],
    pub nested: std::option::Option<Vec<T>>,
}

pub struct Defaults<T: Clone = String, const N: usize = 4>
where
    T: PartialEq,
{
    pub value: T,
    pub bytes: [u8; N],
}

pub struct SelfBound<T>
where
    Self: Sized,
{
    pub value: T,
}

pub struct Point(pub i32, pub i32);

pub struct Taken {
    pub a: u8,
}
"#;

const OTHER: &str = "pub struct TakenBuilder;\n";

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

const PACKET_USAGE: &str = r#"use crate::settings::{Packet, PacketBuilder, PacketBuilderError};

pub fn run() {
    let packet: Packet<'_, String, 3> = PacketBuilder::new()
        .nested(Some(vec!["nested".to_string()]))
        .bytes([1, 2, 3])
        .payload("payload".to_string())
        .label("label")
        .build()
        .expect("every field is set");
    println!("{} {} {:?} {:?}", packet.label, packet.payload, packet.bytes, packet.nested);

    let missing: Result<Packet<'_, String, 3>, PacketBuilderError> = PacketBuilder::new()
        .nested(None)
        .payload("payload".to_string())
        .label("label")
        .build();
    let error = match missing {
        Err(error) => error,
        Ok(_) => panic!("missing bytes unexpectedly built"),
    };
    println!("{}", error);
}
"#;

const DEFAULT_USAGE: &str = r#"use crate::settings::{Defaults, DefaultsBuilder};

pub fn run() {
    let value: Defaults = DefaultsBuilder::new()
        .value("default".to_string())
        .bytes([4, 3, 2, 1])
        .build()
        .expect("defaults remain legal");
    println!("{} {:?}", value.value, value.bytes);
}
"#;

/// A committed checkout, not in a dot-directory (some tools pass over hidden ones).
fn checkout() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::Builder::new()
        .prefix("builder-live-")
        .tempdir()
        .expect("checkout dir");
    let root = std::fs::canonicalize(dir.path()).expect("canonical root");
    for (rel, text) in [
        (
            "Cargo.toml",
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n",
        ),
        ("src/main.rs", "mod other;\nmod settings;\n\nfn main() {}\n"),
        ("src/settings.rs", SETTINGS),
        ("src/other.rs", OTHER),
    ] {
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
    assert!(git(&["add", "-A"]).success());
    assert!(
        git(&[
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "user.name=test",
            "commit",
            "-qm",
            "demo",
        ])
        .success()
    );
    (dir, root)
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).expect("read dir").flatten() {
            let path = entry.path();
            if path
                .file_name()
                .is_some_and(|n| n == ".git" || n == "target")
            {
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

fn request(symbol: &str) -> BuilderRequest<'_> {
    BuilderRequest {
        symbol,
        verify: true,
        ..Default::default()
    }
}

/// The analyzer loads the crate before it can answer; a preview writes nothing, so it is asked
/// again until it gives a verdict. An analyzer that never does fails the test.
async fn verified(addr: SocketAddr, root: &Path, request: &BuilderRequest<'_>) -> BuilderPreview {
    let mut last = String::from("never asked");
    for _ in 0..120 {
        match builder::preview(addr, root, request).await {
            Ok(preview) if preview.verified() => return preview,
            Ok(preview) => {
                if let Verification::Rejected { .. } = preview.verification {
                    panic!("the analyzer rejects the builder:\n{}", preview.render());
                }
                last = preview.render();
            }
            Err(err) => last = format!("{err:#}"),
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    panic!(
        "no verdict from the analyzer for `{}`:\n{last}",
        request.symbol
    );
}

async fn rejected(addr: SocketAddr, root: &Path, request: &BuilderRequest<'_>) -> BuilderPreview {
    let mut last = String::from("never asked");
    for _ in 0..120 {
        match builder::preview(addr, root, request).await {
            Ok(preview) if matches!(preview.verification, Verification::Rejected { .. }) => {
                return preview;
            }
            Ok(preview) => last = preview.render(),
            Err(err) => last = format!("{err:#}"),
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    panic!(
        "no rejected verdict from the analyzer for `{}`:\n{last}",
        request.symbol
    );
}

fn compile_and_run(files: &[(&str, &str)]) -> String {
    let dir = tempfile::Builder::new()
        .prefix("builder-run-")
        .tempdir()
        .expect("scratch dir");
    for (rel, text) in files {
        std::fs::write(dir.path().join(rel), text).expect("write");
    }
    let bin = dir.path().join("program");
    let built = Command::new("rustc")
        .args(["--edition", "2021", "-D", "warnings", "-A", "dead_code"])
        .arg(dir.path().join("main.rs"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("rustc runs");
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rust_analyzer_verifies_a_generated_builder_that_compiles_and_runs() {
    let gateway = Gateway::start();
    let (_dir, root) = checkout();
    let before = snapshot(&root);

    let preview = verified(gateway.addr, &root, &request("Config")).await;
    eprintln!("{}", preview.render());
    // The fields were checked against the analyzer's outline, not only read from the source.
    assert!(preview.plan.notes.is_empty(), "{:?}", preview.plan.notes);
    assert_eq!(preview.file, "src/settings.rs");
    let plan = &preview.plan;
    assert_eq!(
        plan.fields
            .iter()
            .map(|f| (f.name.as_str(), f.ty.as_str()))
            .collect::<Vec<_>>(),
        [
            ("name", "String"),
            ("r#type", "u8"),
            ("r#match", "Option<u32>"),
            (
                "limits",
                "HashMap<String, Vec<(u8, std::option::Option<Box<[u16; 4]>>)>>"
            ),
            ("endpoints", "BTreeMap< u16, network::Endpoint, >"),
            ("callback", "fn(&str) -> std::result::Result<u8, String>"),
            ("score", "Result"),
            ("tags", "[&'static str; 2]"),
        ]
    );
    assert_eq!(snapshot(&root), before, "a preview writes nothing");
    let main = "mod settings;\nmod usage;\n\nfn main() {\n    usage::run();\n}\n";
    let printed = compile_and_run(&[
        ("main.rs", main),
        ("settings.rs", &plan.file_text),
        ("usage.rs", USAGE),
    ]);
    assert_eq!(printed, PRINTED);

    let packet = verified(gateway.addr, &root, &request("Packet")).await;
    assert_eq!(
        packet
            .plan
            .fields
            .iter()
            .map(|field| (field.name.as_str(), field.ty.as_str()))
            .collect::<Vec<_>>(),
        [
            ("label", "&'a str"),
            ("payload", "T"),
            ("bytes", "[u8; N]"),
            ("nested", "std::option::Option<Vec<T>>"),
        ]
    );
    assert!(
        packet.plan.code.contains(
            "pub struct PacketBuilder<'a, T: Clone, const N: usize> where T: PartialEq, {"
        )
    );
    assert!(packet.plan.code.contains(
        "impl<'a, T: Clone, const N: usize> PacketBuilder<'a, T, N> where T: PartialEq, {"
    ));
    assert!(
        packet
            .plan
            .code
            .contains("Result<Packet<'a, T, N>, PacketBuilderError>")
    );
    let printed = compile_and_run(&[
        ("main.rs", main),
        ("settings.rs", &packet.plan.file_text),
        ("usage.rs", PACKET_USAGE),
    ]);
    assert_eq!(
        printed,
        "label payload [1, 2, 3] Some([\"nested\"])\n`Packet` field `bytes` was never set\n"
    );

    let defaults = verified(gateway.addr, &root, &request("Defaults")).await;
    assert!(defaults.plan.code.contains(
        "pub struct DefaultsBuilder<T: Clone = String, const N: usize = 4> where T: PartialEq, {"
    ));
    assert!(
        defaults
            .plan
            .code
            .contains("impl<T: Clone, const N: usize> DefaultsBuilder<T, N> where T: PartialEq, {")
    );
    assert!(!defaults.plan.code.contains("impl<T: Clone ="));
    assert!(
        !defaults
            .plan
            .code
            .contains("impl<T: Clone, const N: usize =")
    );
    let printed = compile_and_run(&[
        ("main.rs", main),
        ("settings.rs", &defaults.plan.file_text),
        ("usage.rs", DEFAULT_USAGE),
    ]);
    assert_eq!(printed, "default [4, 3, 2, 1]\n");

    // A struct in an inline module: the builder goes into that module, indented like it, and
    // the analyzer checks it there.
    let nested = verified(gateway.addr, &root, &request("Endpoint")).await;
    assert!(
        nested.plan.code.contains(
            "\n    pub struct EndpointBuilder {\n        host: ::core::option::Option<String>,"
        ),
        "{}",
        nested.plan.code
    );
    assert!(
        nested
            .plan
            .file_text
            .contains("    }\n\n    /// Builds a `Endpoint`")
    );

    // Refusals, each with its reason, none of them writing anything.
    for (request, expected) in [
        (request("Point"), "tuple struct"),
        (
            request("SelfBound"),
            "where clause of `SelfBound` spells `Self`",
        ),
        (
            request("Taken"),
            "`TakenBuilder` is already declared in this workspace",
        ),
        (
            BuilderRequest {
                builder_name: Some("Endpoint"),
                ..request("Config")
            },
            "`Endpoint` already appears in this file",
        ),
        // The index lists the standard library's names as well.
        (
            BuilderRequest {
                builder_name: Some("Extend"),
                ..request("Config")
            },
            "`Extend` is already declared",
        ),
        // And the crates of the extern prelude. (What the index misses and only resolving the
        // name at the insertion point finds is covered by the scripted test in the MCP crate; a
        // clean verdict here already needed that resolution to find `Config` in place.)
        (
            BuilderRequest {
                builder_name: Some("core"),
                ..request("Config")
            },
            "`core` is already declared",
        ),
    ] {
        let err = builder::preview(gateway.addr, &root, &request)
            .await
            .expect_err(expected);
        let err = format!("{err:#}");
        eprintln!("{expected} => {err}");
        assert!(err.contains(expected), "{expected}\n=> {err}");
    }
    assert_eq!(snapshot(&root), before, "a refusal writes nothing");

    let (_bad_dir, bad_root) = checkout();
    let bad_path = bad_root.join("src/settings.rs");
    let bad_source =
        format!("{SETTINGS}\n\npub struct BadField {{\n    pub value: MissingType,\n}}\n");
    std::fs::write(&bad_path, bad_source).expect("write the bad-field fixture");
    let bad_before = snapshot(&bad_root);
    let bad = rejected(gateway.addr, &bad_root, &request("BadField")).await;
    assert!(
        bad.diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.contains("MissingType")),
        "{}",
        bad.render()
    );
    assert_eq!(
        snapshot(&bad_root),
        bad_before,
        "a rejected preview writes nothing"
    );
}
