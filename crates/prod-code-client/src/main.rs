//! prod-code client: ultra-thin CLI bridge for editors and AI coding agents over 10G LAN.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use futures_util::{SinkExt, StreamExt};
use prod_code_client::divergent_bench::{self, DivergentBenchConfig, WorkspaceMode};
use prod_code_mcp::report::ReportRequest;
use prod_code_mcp::verify::VerifyKind;
use prod_code_protocol::{
    HandshakeRequest, PROTOCOL_VERSION, ProdCodeCodec, WireMessage, supported_protocol_versions,
    validate_selected_protocol_version,
};
use std::env;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tokio::io::BufReader;
use tokio_util::codec::Framed;
use url::Url;

mod update;
mod package;

#[derive(Parser, Debug)]
#[command(
    name = "prod-code",
    author = "Alex <alex@prod.codes>",
    version,
    about = "Remote Code Intelligence Client"
)]
struct Cli {
    /// Remote gateway address(es): `host:port[,host:port...]`. With several nodes the
    /// workspace is placed on one of them (rendezvous hashing, remembered locally, failover
    /// to the next alive node). Defaults to PROD_CODE_REMOTE or 127.0.0.1:9400.
    #[arg(
        short,
        long,
        global = true,
        env = "PROD_CODE_REMOTE",
        default_value = "127.0.0.1:9400"
    )]
    remote: String,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run as an editor's language server (stdio LSP): the language's own server (rust-analyzer,
    /// gopls, clangd, basedpyright, the TypeScript server, sourcekit-lsp) runs on the node
    Lsp {
        /// The language the server is for (rust, go, c, cpp, python, typescript, javascript,
        /// swift); by default the checkout root's
        #[arg(long)]
        language: Option<String>,
    },
    /// Run as Model Context Protocol (MCP) server for AI coding agents.
    Mcp,
    /// Probe the status and latency of the gateway this checkout is placed on.
    Status {
        /// One JSON object: the gateway's status as sent, the address, the round trip and
        /// whether the node is healthy
        #[arg(long)]
        json: bool,
    },
    /// Show every configured gateway node, its status, and where this checkout is placed.
    Cluster {
        /// One JSON object: the gossip view, each node's status or error, and the placement
        #[arg(long)]
        json: bool,
    },
    /// Push current worktree delta to remote storage over 10G LAN (or pull files with --pull).
    Sync {
        /// Optional subpath or file to sync.
        path: Option<PathBuf>,
        /// Pull files from the remote gateway workspace into the local checkout instead of pushing.
        #[arg(long, default_value_t = false)]
        pull: bool,
    },
    /// Pull files from the remote gateway workspace into the local checkout: prod-code pull <file...>
    Pull {
        /// Files or relative paths to pull from the remote gateway workspace.
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Jump to symbol definition: prod-code def <file> <line> <col>, or --symbol NAME
    Def {
        #[arg(required_unless_present = "symbol")]
        file: Option<PathBuf>,
        #[arg(required_unless_present = "symbol")]
        line: Option<u32>,
        #[arg(required_unless_present = "symbol")]
        col: Option<u32>,
        /// The symbol by name (`Type::method`, `module::function`) instead of a position; with
        /// FILE, the one declared or used there (#330).
        #[arg(long, conflicts_with_all = ["line", "col"])]
        symbol: Option<String>,
        /// Also print the definition's code, numbered, with its doc comments (#306).
        #[arg(long, default_value_t = false)]
        body: bool,
    },
    /// Inspect symbol type & docs: prod-code hover <file> <line> <col>, or --symbol NAME
    Hover {
        #[arg(required_unless_present = "symbol")]
        file: Option<PathBuf>,
        #[arg(required_unless_present = "symbol")]
        line: Option<u32>,
        #[arg(required_unless_present = "symbol")]
        col: Option<u32>,
        /// The symbol by name (`Type::method`, `module::function`) instead of a position; with
        /// FILE, the one declared or used there (#330).
        #[arg(long, conflicts_with_all = ["line", "col"])]
        symbol: Option<String>,
    },
    /// Find all references to symbol: prod-code refs <file> <line> <col>, or --symbol NAME
    Refs {
        #[arg(required_unless_present = "symbol")]
        file: Option<PathBuf>,
        #[arg(required_unless_present = "symbol")]
        line: Option<u32>,
        #[arg(required_unless_present = "symbol")]
        col: Option<u32>,
        /// The symbol by name (`Type::method`, `module::function`) instead of a position; with
        /// FILE, the one declared or used there (#330).
        #[arg(long, conflicts_with_all = ["line", "col"])]
        symbol: Option<String>,
        /// Another checkout to search as well, the name resolved there too (repeatable; with
        /// --symbol) (#375).
        #[arg(long = "in", value_name = "DIR")]
        also_in: Vec<PathBuf>,
    },
    /// Who calls the function at a position: prod-code callers <file> <line> <col>, or --symbol NAME
    Callers {
        #[arg(required_unless_present = "symbol")]
        file: Option<PathBuf>,
        #[arg(required_unless_present = "symbol")]
        line: Option<u32>,
        #[arg(required_unless_present = "symbol")]
        col: Option<u32>,
        /// The symbol by name (`Type::method`, `module::function`) instead of a position; with
        /// FILE, the one declared or used there (#330).
        #[arg(long, conflicts_with_all = ["line", "col"])]
        symbol: Option<String>,
        /// Levels to walk: 1 is the direct ones; more gives a tree (at most 6).
        #[arg(long, default_value_t = 1)]
        depth: usize,
    },
    /// What the function at a position calls: prod-code callees <file> <line> <col>, or --symbol NAME
    Callees {
        #[arg(required_unless_present = "symbol")]
        file: Option<PathBuf>,
        #[arg(required_unless_present = "symbol")]
        line: Option<u32>,
        #[arg(required_unless_present = "symbol")]
        col: Option<u32>,
        /// The symbol by name (`Type::method`, `module::function`) instead of a position; with
        /// FILE, the one declared or used there (#330).
        #[arg(long, conflicts_with_all = ["line", "col"])]
        symbol: Option<String>,
        /// Levels to walk: 1 is the direct ones; more gives a tree (at most 6).
        #[arg(long, default_value_t = 1)]
        depth: usize,
    },
    /// Implementations of the trait / interface at a position: prod-code impls <file> <line> <col>, or --symbol NAME
    Impls {
        #[arg(required_unless_present = "symbol")]
        file: Option<PathBuf>,
        #[arg(required_unless_present = "symbol")]
        line: Option<u32>,
        #[arg(required_unless_present = "symbol")]
        col: Option<u32>,
        /// The symbol by name (`Type::method`, `module::function`) instead of a position; with
        /// FILE, the one declared or used there (#330).
        #[arg(long, conflicts_with_all = ["line", "col"])]
        symbol: Option<String>,
    },
    /// What the type at a position implements, or what the trait requires: prod-code supertypes
    /// <file> <line> <col>, or --symbol NAME
    Supertypes {
        #[arg(required_unless_present = "symbol")]
        file: Option<PathBuf>,
        #[arg(required_unless_present = "symbol")]
        line: Option<u32>,
        #[arg(required_unless_present = "symbol")]
        col: Option<u32>,
        /// The symbol by name (`Type`, `module::Trait`) instead of a position; with FILE, the one
        /// declared or used there (#330).
        #[arg(long, conflicts_with_all = ["line", "col"])]
        symbol: Option<String>,
    },
    /// Declarations named like a query across the workspace: prod-code symbols <name>. Given an
    /// existing file instead, its outline (the same as `prod-code outline <file>`).
    Symbols { target: String },
    /// The declarations of a file or directory, nested: prod-code outline <path>
    Outline {
        file: PathBuf,
        /// Also list the local variables inside functions and methods.
        #[arg(long, default_value_t = false)]
        locals: bool,
        /// Only these kinds, comma-separated: function, method, struct, class, field, ...
        #[arg(long, value_delimiter = ',')]
        kinds: Vec<String>,
        /// Only what the language exports (Go capitals, Rust `pub`, Swift `public`, TS `export`).
        #[arg(long, default_value_t = false)]
        exported: bool,
        /// Most bytes listed (a directory's default is 40000; 0 = no budget).
        #[arg(long)]
        max_bytes: Option<usize>,
        /// Most symbols listed.
        #[arg(long)]
        max_items: Option<usize>,
    },
    /// Blast radius of the uncommitted changes: changed functions, their callers and the affected tests
    Impact {
        /// Git ref to diff against (default: working tree vs HEAD)
        #[arg(long)]
        base: Option<String>,
        /// Caller levels to follow
        #[arg(long, default_value_t = 4)]
        depth: usize,
        /// Run the affected tests afterwards
        #[arg(long)]
        run: bool,
        /// For CI: run the affected tests, or the whole suite when the selection cannot be
        /// trusted; write a Markdown summary to `$GITHUB_STEP_SUMMARY` when it is set; exit
        /// with the tests' status
        #[arg(long, conflicts_with = "run")]
        ci: bool,
        #[arg(long)]
        json: bool,
    },
    /// Report a bug in prod-code itself as a GitHub issue (private details are removed first;
    /// similar issues, open or closed, are listed and nothing is filed unless --force)
    ReportIssue {
        /// A searchable title: what went wrong, in which tool or command
        #[arg(long)]
        title: String,
        /// What was run, what came back, what was expected
        #[arg(long, conflicts_with = "body_file")]
        body: Option<String>,
        /// Read the body from this file (`-` for stdin)
        #[arg(long)]
        body_file: Option<PathBuf>,
        /// Id of a private record of the details the issue cannot carry; the issue names it
        #[arg(long)]
        private_ref: Option<String>,
        /// File it even when similar issues exist
        #[arg(long)]
        force: bool,
        /// Show the scrubbed issue without filing it
        #[arg(long)]
        dry_run: bool,
        /// A label, repeated for each: one type (bug, enhancement, documentation, perf; bug when
        /// none is given) and the areas it is about (gateway, client, mcp, cluster, worktree,
        /// infra, test)
        #[arg(long = "label")]
        labels: Vec<String>,
    },
    /// Usage metrics of every node: who queried what, how often, how fast; exec runs; syncs
    Metrics {
        /// Window in seconds (default 24h; 0 = everything the nodes hold in memory)
        #[arg(long, default_value_t = 86_400)]
        since: u64,
        #[arg(long)]
        json: bool,
    },
    /// Run the tests and explain every failure: site, code, callers, what changed
    ///
    /// Includes printed Rust assert_eq/assert_ne operands and supported Node assert values.
    /// Rust retains left/right order; no expected/actual role is inferred. Truncated or
    /// unsupported output remains raw text, and no expression is evaluated.
    Diagnose {
        /// Test filter (as for `prod-code test`)
        filter: Option<String>,
        #[arg(long, default_value_t = 0)]
        timeout_secs: u64,
        #[arg(long)]
        json: bool,
    },
    /// Analyzer diagnostics for a file, in memory (no build): prod-code diagnostics <file>
    Diagnostics {
        file: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Check a proposed replacement for a file without writing it: prod-code validate <file> --from NEW (or stdin).
    /// Supports source files (via remote LSP), JSON manifests, Markdown documentation, and SVG/XML graphics (via syntax parsers).
    Validate {
        /// The file the proposed content is for (not needed with `--diff`)
        file: Option<PathBuf>,
        /// Path of the proposed content; stdin when omitted
        #[arg(long)]
        from: Option<PathBuf>,
        /// Check a proposed unified diff against CURRENT on-disk files: a path, or `-` for
        /// stdin. Submit it before applying the edits; Git HEAD is not the patch base. Every
        /// touched file is checked together. For already-written edits, use FILE --from FILE
        /// and --with OTHER=OTHER instead.
        #[arg(long, conflicts_with_all = ["from", "with"])]
        diff: Option<PathBuf>,
        /// Another proposed file, checked together with the first in one overlay: FILE=NEW.
        /// Repeat it for each file of a multi-file change.
        #[arg(long = "with", value_name = "FILE=NEW")]
        with: Vec<String>,
        /// Also run the project's check command (`cargo check`, `go build`, `tsc`) on the
        /// proposed text in a shadow copy on the node: errors the analyzer does not report
        /// (borrow checker, private items of another crate) (#376).
        #[arg(long)]
        compile: bool,
        #[arg(long)]
        json: bool,
    },
    /// Unreferenced functions, methods and types across the checkout
    DeadCode {
        /// Also list exported / public symbols nothing in the checkout uses
        #[arg(long)]
        include_exported: bool,
        /// Whole-program graph reachability analysis from entry points (main, public APIs, tests, route handlers)
        #[arg(long)]
        reachability: bool,
        /// Stop after this many source files
        #[arg(long, default_value_t = 400)]
        max_files: usize,
        #[arg(long)]
        json: bool,
    },
    /// Remove every orphan the dead-code scan finds, in one type-checked edit: prod-code prune [--apply]
    Prune {
        /// Stop after this many source files
        #[arg(long, default_value_t = 400)]
        max_files: usize,
        /// Whole-program graph reachability analysis to detect and prune circular unreachable dead code
        #[arg(long)]
        reachability: bool,
        /// Write the change instead of only reporting.
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Write even when the result does not compile.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Show a source file that lives on the gateway (std, registry, SDK): prod-code source <path> [--line N] [--context K]
    Source {
        path: String,
        #[arg(long)]
        line: Option<u32>,
        #[arg(long, default_value_t = 20)]
        context: u32,
    },
    /// List code actions (inline, extract, generate, rewrite, quick fixes) at a 1-based
    /// position or selection: prod-code assists <file> <line> <col> [--to LINE:COL]
    Assists {
        file: PathBuf,
        line: u32,
        col: u32,
        /// End of a selection as LINE:COL (1-based).
        #[arg(long)]
        to: Option<String>,
    },
    /// Apply one code action by id at a position or selection and write the edits locally:
    /// prod-code assist <file> <line> <col> <id> [--to LINE:COL] [--subtype N]
    Assist {
        file: PathBuf,
        line: u32,
        col: u32,
        id: String,
        #[arg(long)]
        to: Option<String>,
        #[arg(long)]
        subtype: Option<u64>,
    },
    /// Delete an unreferenced item at its 1-based declaration-name position. Go support is
    /// limited to ordinary unexported, non-generic functions or named value/pointer receiver
    /// methods with bodies. Interface obligations and embedding refuse; writes require remote
    /// compilation under the active Go build flags (not all platforms):
    /// prod-code safe-delete <file> <line> <col>
    SafeDelete { file: PathBuf, line: u32, col: u32 },
    /// Rename the symbol at 1-based <line> <col> across the workspace and apply the edits
    /// locally: prod-code rename <file> <line> <col> <new_name>
    Rename {
        file: PathBuf,
        line: u32,
        col: u32,
        new_name: String,
        /// At a field: rename its accessors too (`f()`, `get_f()`, `set_f()`, `f_mut()`).
        #[arg(long, default_value_t = false)]
        accessors: bool,
        /// Also rename the old name in comments and in the names of tests that exercise it.
        #[arg(long, default_value_t = false)]
        comments: bool,
        /// Write the rename even when the result does not compile.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Compile-check the workspace remotely (cargo check / go build) with structured diagnostics.
    Check {
        /// The crate, package or directory to run in (a nested project, or one member of a
        /// workspace), as `path` for the MCP tools; the current directory by default (#323).
        #[arg(short = 'p', long)]
        path: Option<PathBuf>,
        #[arg(long, default_value_t = 0)]
        timeout_secs: u64,
        /// Print the full report as JSON instead of text.
        #[arg(long, default_value_t = false)]
        json: bool,
        /// Apply the compiler's machine-applicable fixes, then check again (Rust).
        #[arg(long, default_value_t = false)]
        fix: bool,
        /// An environment variable for the command (repeatable): `--env RUST_BACKTRACE=1`.
        #[arg(long = "env", value_name = "KEY=VALUE", conflicts_with = "fix")]
        env: Vec<String>,
        /// Print each diagnostic and test result as a JSON line as it arrives, and the report as
        /// the last line.
        #[arg(long, default_value_t = false)]
        events: bool,
    },
    /// Lint the workspace remotely (cargo clippy -D warnings, golangci-lint or go vet, ruff,
    /// eslint / biome, clang-tidy) with structured findings.
    Lint {
        /// The crate, package or directory to run in (a nested project, or one member of a
        /// workspace), as `path` for the MCP tools; the current directory by default (#323).
        #[arg(short = 'p', long)]
        path: Option<PathBuf>,
        #[arg(long, default_value_t = 0)]
        timeout_secs: u64,
        #[arg(long, default_value_t = false)]
        json: bool,
        /// Apply the fixes, then lint again: clippy's machine-applicable ones for Rust, the
        /// linter's own fix mode for Python, TypeScript, C++ and Go with a golangci config.
        #[arg(long, default_value_t = false)]
        fix: bool,
        /// An environment variable for the command (repeatable): `--env RUST_BACKTRACE=1`.
        #[arg(long = "env", value_name = "KEY=VALUE", conflicts_with = "fix")]
        env: Vec<String>,
        /// Print each diagnostic and test result as a JSON line as it arrives, and the report as
        /// the last line.
        #[arg(long, default_value_t = false)]
        events: bool,
    },
    /// Run the project's benchmarks remotely (cargo bench / go test -bench) with parsed results.
    Benchmarks {
        /// Benchmark name filter.
        filter: Option<String>,
        /// The crate, package or directory to run in (a nested project, or one member of a
        /// workspace), as `path` for the MCP tools; the current directory by default (#323).
        #[arg(short = 'p', long)]
        path: Option<PathBuf>,
        #[arg(long, default_value_t = 0)]
        timeout_secs: u64,
        #[arg(long, default_value_t = false)]
        json: bool,
        /// An environment variable for the command (repeatable): `--env RUST_BACKTRACE=1`.
        #[arg(long = "env", value_name = "KEY=VALUE")]
        env: Vec<String>,
        /// Print each diagnostic and test result as a JSON line as it arrives, and the report as
        /// the last line.
        #[arg(long, default_value_t = false)]
        events: bool,
    },
    /// Run tests remotely (cargo test / go test -json), optionally filtered by name.
    Test {
        /// Test name filter (cargo test TESTNAME / go test -run).
        filter: Option<String>,
        /// The crate, package or directory to run in (a nested project, or one member of a
        /// workspace), as `path` for the MCP tools; the current directory by default (#323).
        #[arg(short = 'p', long)]
        path: Option<PathBuf>,
        #[arg(long, default_value_t = 0)]
        timeout_secs: u64,
        #[arg(long, default_value_t = false)]
        json: bool,
        /// An environment variable for the command (repeatable): `--env RUST_BACKTRACE=1`.
        #[arg(long = "env", value_name = "KEY=VALUE")]
        env: Vec<String>,
        /// Print each diagnostic and test result as a JSON line as it arrives, and the report as
        /// the last line.
        #[arg(long, default_value_t = false)]
        events: bool,
    },
    /// Run a build/test/lint command on the remote gateway inside this checkout's server copy:
    /// prod-code exec -- cargo test -p my-crate
    Exec {
        /// Kill the command after this many seconds (0 = server default, 1 hour).
        #[arg(long, default_value_t = 0)]
        timeout_secs: u64,
        /// Do not copy back files the command changed on the server (formatters, generators,
        /// lockfiles are pulled back by default).
        #[arg(long, default_value_t = false)]
        no_pull: bool,
        /// An environment variable for the command (repeatable): `--env RUST_BACKTRACE=1`.
        #[arg(long = "env", value_name = "KEY=VALUE")]
        env: Vec<String>,
        /// Command and arguments (put `--` before them).
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Generate a Rust value or typed builder, checked by the analyzer before it is printed.
    /// Builders support named fields with ordinary lifetime/type/const parameters and bounds.
    /// Every field is required; Self-dependent bounds, macros and complex const expressions are refused.
    /// Preview only: no files are written. `--no-verify` prints an explicitly unverified draft.
    Fixture {
        /// The type to build.
        symbol: String,
        /// How deep to build nested workspace values (value mode only).
        #[arg(long, default_value_t = 2, conflicts_with = "builder")]
        depth: u32,
        /// Generate one typed setter per field and a build method instead of a value.
        #[arg(long)]
        builder: bool,
        /// Override the generated builder name (default: TypeBuilder).
        #[arg(long, requires = "builder")]
        builder_name: Option<String>,
        /// Skip the type check.
        #[arg(long, default_value_t = false)]
        no_verify: bool,
        /// The file that declares it, when the name is ambiguous.
        #[arg(long)]
        path: Option<String>,
        /// Generate realistic non-zero dummy test data instead of default empty/zero values.
        #[arg(long, default_value_t = false)]
        randomized: bool,
        /// Generate a test mock implementation with call tracking for an interface, trait, protocol or struct.
        #[arg(long, default_value_t = false, conflicts_with = "builder")]
        mock: bool,
        /// Explicit target language: rust, go, typescript, python, cpp, swift.
        #[arg(long = "lang")]
        language: Option<String>,
    },
    /// Rename a schema field across every language that spells it.
    SchemaRename {
        /// The field as the schema spells it (`order_id`).
        field: String,
        /// What it becomes (`trade_id`); spelled per language automatically.
        #[arg(long)]
        to: String,
        /// Only look under this directory.
        #[arg(long)]
        path: Option<String>,
        /// Another repository to rename in as part of the same change (repeatable): each is
        /// checked by its own analyzers, and `--apply` writes all of them or none.
        #[arg(long = "repo", value_name = "PATH")]
        repos: Vec<String>,
        /// `compile`: also run `cargo check` on the result in a shadow of the workspace, and write
        /// only if the compiler accepts it too. Seconds rather than milliseconds.
        #[arg(long)]
        verify: Option<String>,
        /// Write the rename instead of only reporting it.
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Allow a short name, many occurrences, and a result that does not compile.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Change a declared type and report every site that no longer fits.
    MigrateType {
        /// The declaration by name, or a file with `--line`. A struct field is rarely in the
        /// workspace symbol index, so give it a position.
        symbol: String,
        /// The type it should become.
        #[arg(long = "to")]
        to: String,
        /// 1-based line, when the first argument is a file path.
        #[arg(long)]
        line: Option<u32>,
        /// 1-based column of the declared name, with `--line`.
        #[arg(long, default_value_t = 1)]
        character: u32,
        /// The file that declares it, when the name is ambiguous.
        #[arg(long)]
        path: Option<String>,
        /// Write language-idiomatic conversions where the old and new types meet.
        #[arg(long, default_value_t = false)]
        convert: bool,
        /// Transitively propagate type changes to downstream variables, parameters, and returns.
        #[arg(long, default_value_t = false)]
        transitive: bool,
        /// Write the declaration instead of only reporting.
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Write the declaration while sites still do not fit.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Make a parameter generic: its concrete type becomes a bounded type parameter.
    Generify {
        /// The function by name, or a file with `--line`.
        symbol: String,
        /// The parameter to make generic.
        #[arg(long)]
        param: String,
        /// The trait bound, such as `AsRef<[u32]>`.
        #[arg(long, default_value = "")]
        bound: String,
        /// The new type parameter's name.
        #[arg(long = "as", default_value = "T")]
        type_param: String,
        /// 1-based line, when the first argument is a file path.
        #[arg(long)]
        line: Option<u32>,
        /// 1-based column of the function's name, with `--line`.
        #[arg(long, default_value_t = 1)]
        character: u32,
        /// Optional explicit file path.
        #[arg(long)]
        path: Option<String>,
        /// Optional explicit function name.
        #[arg(long)]
        function: Option<String>,
        /// Write the change instead of only reporting.
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Write even when the result does not compile.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Invert a predicate, a bool field or a bool variable: a new name, the opposite meaning, and
    /// every use unchanged in effect.
    InvertBoolean {
        /// The function by name, or a file with `--line` (needed for a field or a variable).
        symbol: String,
        /// The new name.
        #[arg(long = "to")]
        new_name: String,
        /// 1-based line, when the first argument is a file path.
        #[arg(long)]
        line: Option<u32>,
        /// 1-based column of the function's name, with `--line`.
        #[arg(long, default_value_t = 1)]
        character: u32,
        /// Optional path to the file when symbol is just a function name.
        #[arg(long)]
        path: Option<String>,
        /// Optional function name.
        #[arg(long)]
        function: Option<String>,
        /// `compile`: also run `cargo check` on the result in a shadow of the workspace.
        #[arg(long)]
        verify: Option<String>,
        /// Write the change instead of only reporting.
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Write even when the result does not compile.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Turn an associated function into a method, with every call site.
    ConvertToMethod {
        /// The function by name (`Type::function`), or a file with `--line`.
        symbol: String,
        /// 1-based line, when the first argument is a file path.
        #[arg(long)]
        line: Option<u32>,
        /// 1-based column of the function's name, with `--line`.
        #[arg(long, default_value_t = 1)]
        character: u32,
        /// The file that declares it, when the name is ambiguous.
        #[arg(long)]
        path: Option<String>,
        /// Name of the function to convert to method.
        #[arg(long)]
        method: Option<String>,
        /// Optional class or struct name declaring the function.
        #[arg(long)]
        class: Option<String>,
        /// `compile`: also run compiler check on the result in a shadow of the workspace.
        #[arg(long)]
        verify: Option<String>,
        /// Write the change instead of only reporting.
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Write even when the result does not compile.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Inline a parameter every caller passes the same constant for: the value moves into the body.
    InlineParameter {
        /// The file that declares the function.
        file: PathBuf,
        /// 1-based line of the parameter's name.
        #[arg(default_value = None)]
        line: Option<u32>,
        /// 1-based column of the parameter's name.
        #[arg(default_value = None)]
        col: Option<u32>,
        /// Function name.
        #[arg(long)]
        function: Option<String>,
        /// Parameter name to inline.
        #[arg(long)]
        param: Option<String>,
        /// `compile`: also run compiler check on the result in a shadow of the workspace.
        #[arg(long)]
        verify: Option<String>,
        /// Write the change instead of only reporting.
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Write even when a reference is not a call or the result does not compile.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Move some fields of a struct or class, with the methods that use only them, into a helper type it holds.
    ExtractDelegate {
        /// The file that declares the struct or class.
        file: PathBuf,
        /// 1-based line of the `struct`/`class` keyword.
        #[arg(default_value = None)]
        line: Option<u32>,
        /// 1-based column on that line.
        #[arg(default_value = None)]
        col: Option<u32>,
        /// Name of the struct or class (alternative to line/col).
        #[arg(long, aliases = ["class", "type"])]
        symbol: Option<String>,
        /// The fields that move, comma-separated.
        #[arg(long, value_delimiter = ',')]
        fields: Vec<String>,
        /// The methods that move with them, comma-separated.
        #[arg(long, value_delimiter = ',', default_value = "")]
        methods: Vec<String>,
        /// Name of the helper type.
        #[arg(long)]
        name: String,
        /// Name of the field that holds it.
        #[arg(long)]
        field: String,
        /// `compile`: also run compiler check on the result in a shadow of the workspace.
        #[arg(long)]
        verify: Option<String>,
        /// Write the change instead of only reporting.
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Write even when the result does not compile.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Extract a Rust trait from named inherent methods, imported where they are called.
    ///
    /// Preserves ordinary lifetime/type/const parameters, bounds and method generics.
    /// Attributed impls, conditional methods, Self-dependent impl bounds, opaque impl Trait
    /// returns, macros, specialization and existing trait impls are refused even with force.
    /// Caller type annotations are not migrated.
    ExtractTrait {
        /// The file that holds the `impl` block.
        file: PathBuf,
        /// 1-based line of the `impl` header (or any line inside the block).
        line: u32,
        /// 1-based column on that line.
        col: u32,
        /// The methods that move into the trait, comma-separated.
        #[arg(long, value_delimiter = ',')]
        methods: Vec<String>,
        /// Name of the new trait.
        #[arg(long)]
        name: String,
        /// Do not migrate caller type annotations from concrete type to extracted trait.
        #[arg(long = "no-migrate-callers", default_value_t = false)]
        no_migrate_callers: bool,
        /// Write the change instead of only reporting.
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Write even when the result does not compile.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Turn a loop that only builds up an accumulator into an iterator chain or functional expression across polyglot languages.
    LoopToIterator {
        /// The file that holds the loop.
        file: PathBuf,
        /// 1-based line of the `for`.
        line: Option<u32>,
        /// 1-based column on that line.
        col: Option<u32>,
        /// Symbol or function/accumulator name identifying the loop.
        #[arg(long)]
        symbol: Option<String>,
        /// Write the change instead of only reporting.
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Write even when the result does not compile.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Extract the selection into a new function and replace duplicates across TS, Python, Go, C++, Swift, and Rust.
    ExtractFunction {
        /// The file that holds the selection.
        file: PathBuf,
        /// 1-based line where the selection starts.
        line: u32,
        /// 1-based column where the selection starts.
        col: u32,
        /// End of the selection as LINE:COL (1-based, just past it).
        #[arg(long)]
        to: String,
        /// Name of the new function.
        #[arg(long)]
        name: String,
        /// Extract the selection alone; leave the same code elsewhere as it is.
        #[arg(long, default_value_t = false)]
        no_duplicates: bool,
        /// Also take copies that differ only in literals; each differing literal becomes a
        /// parameter of the new function.
        #[arg(long, default_value_t = false)]
        parameterize: bool,
        /// Also look for copies across the crate or workspace's other files.
        #[arg(long, default_value_t = false)]
        other_files: bool,
        /// `compile`: also run `cargo check` on the result in a shadow of the workspace.
        #[arg(long)]
        verify: Option<String>,
        /// Write the change instead of only reporting.
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Write even when the result does not compile.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Introduce a variable for an expression and replace every occurrence in the function.
    IntroduceVariable {
        /// The file that holds the expression.
        file: PathBuf,
        /// 1-based line where the selection starts.
        line: u32,
        /// 1-based column where the selection starts.
        col: u32,
        /// End of the selection as LINE:COL (1-based, just past the expression).
        #[arg(long)]
        to: String,
        /// Name of the new variable.
        #[arg(long)]
        name: String,
        /// Write the change instead of only reporting.
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Write even when the result does not compile.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Turn a method that never uses `self` into an associated function, with every call site.
    MakeStatic {
        /// The method by name (`Type::method`), or a file with `--line`.
        symbol: String,
        /// 1-based line, when the first argument is a file path.
        #[arg(long)]
        line: Option<u32>,
        /// 1-based column of the method's name, with `--line`.
        #[arg(long, default_value_t = 1)]
        character: u32,
        /// The file that declares it, when the name is ambiguous.
        #[arg(long)]
        path: Option<String>,
        /// Name of the method to make static.
        #[arg(long)]
        method: Option<String>,
        /// Optional class or struct name declaring the method.
        #[arg(long)]
        class: Option<String>,
        /// `compile`: also run compiler check on the result in a shadow of the workspace.
        #[arg(long)]
        verify: Option<String>,
        /// Write the change instead of only reporting.
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Write even when a receiver with effects would be dropped or it does not compile.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Wrap what a function returns in Option, Result, Promise, or Pointer across languages; callers that can propagate get `?` or `await`.
    WrapReturn {
        /// The function by name, or a file with `--line`.
        symbol: String,
        /// `option`, `result`, `promise`, `pointer`, or a custom envelope type name.
        #[arg(long)]
        wrapper: String,
        /// Optional constructor or factory expression for wrapping returned values.
        #[arg(long)]
        constructor: Option<String>,
        /// For `result`: the error type, such as `anyhow::Error` or `Error`.
        #[arg(long)]
        error: Option<String>,
        /// The file that declares it.
        #[arg(long)]
        path: Option<String>,
        /// Optional function name (when symbol argument is a file).
        #[arg(long)]
        function: Option<String>,
        /// 1-based line, when the first argument is a file path.
        #[arg(long)]
        line: Option<u32>,
        /// 1-based column of the function's name, with `--line`.
        #[arg(long, default_value_t = 1)]
        character: u32,
        /// `compile`: also run compiler check on the result in a shadow of the workspace.
        #[arg(long)]
        verify: Option<String>,
        /// Write the change instead of only reporting.
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Write even when a caller cannot propagate or the result does not compile.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Make a public field private and turn every access to it outside its file into a getter or
    /// setter call.
    EncapsulateField {
        /// The field by name (`Type::field`), or a file with `--line`.
        symbol: String,
        /// 1-based line, when the first argument is a file path.
        #[arg(long)]
        line: Option<u32>,
        /// 1-based column of the field's name, with `--line`.
        #[arg(long, default_value_t = 1)]
        character: u32,
        /// The file that declares it, when the name is ambiguous.
        #[arg(long)]
        path: Option<String>,
        /// Name of the field to encapsulate, when symbol is a class or file.
        #[arg(long)]
        field: Option<String>,
        /// Optional class or struct name declaring the field.
        #[arg(long)]
        class: Option<String>,
        /// Return the field by value (`true`, it must be `Copy`) or by reference (`false`).
        #[arg(long)]
        by_value: Option<bool>,
        /// `compile`: also run `cargo check` on the result in a shadow of the workspace, and write
        /// only if the compiler accepts it too. Seconds rather than milliseconds.
        #[arg(long)]
        verify: Option<String>,
        /// Write the change instead of only reporting.
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Write even when a use cannot be rewritten or the result does not compile.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Replace constructor and raw struct instantiations with a named static factory method.
    ReplaceConstructorWithFactory {
        /// File that declares the struct or class
        file: PathBuf,
        /// Name of the struct or class
        #[arg(long)]
        type_name: String,
        /// Name of the factory method (default: language convention, e.g. "new", "create")
        #[arg(long)]
        name: Option<String>,
        /// Verify with compiler check
        #[arg(long)]
        verify: Option<String>,
        /// Apply the changes to disk
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Force apply even if analyzer warnings/errors occur
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Replace constructor and raw struct instantiations with a fluent builder pattern.
    ReplaceConstructorWithBuilder {
        /// File that declares the struct or class
        file: PathBuf,
        /// Name of the struct or class
        #[arg(long)]
        type_name: String,
        /// Name of the builder type (default: "<Type>Builder")
        #[arg(long)]
        name: Option<String>,
        /// Verify with compiler check
        #[arg(long)]
        verify: Option<String>,
        /// Apply the changes to disk
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Force apply even if analyzer warnings/errors occur
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Pull up members (methods, fields, constants) from a subclass or sub-trait into its superclass or super-trait.
    PullUp {
        /// File that declares the subclass or sub-trait
        file: PathBuf,
        /// Name of the subclass, derived class, or sub-trait
        #[arg(long)]
        class: String,
        /// Names of members to pull up (comma-separated)
        #[arg(long, value_delimiter = ',')]
        members: Vec<String>,
        /// Optional name of the superclass (auto-detected from inheritance if omitted)
        #[arg(long = "target-class")]
        target_class: Option<String>,
        /// Do not clean up duplicate members in sibling subclasses
        #[arg(long, default_value_t = false)]
        no_clean_siblings: bool,
        /// Verify with compiler check
        #[arg(long)]
        verify: Option<String>,
        /// Apply the changes to disk
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Force apply even if warnings or non-fatal diagnostics occur
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Push down members (methods, fields, constants) from a superclass or super-trait into its subclasses or sub-traits.
    PushDown {
        /// File that declares the superclass or super-trait
        file: PathBuf,
        /// Name of the superclass, base class, or super-trait
        #[arg(long)]
        class: String,
        /// Names of members to push down (comma-separated)
        #[arg(long, value_delimiter = ',')]
        members: Vec<String>,
        /// Optional list of specific subclass names to push down to (all discovered subclasses if omitted)
        #[arg(long = "target-classes", value_delimiter = ',')]
        target_classes: Vec<String>,
        /// Verify with compiler check
        #[arg(long)]
        verify: Option<String>,
        /// Apply the changes to disk
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Force apply even if warnings or non-fatal diagnostics occur
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Replace inheritance with composition and delegation.
    #[command(alias = "replace-inheritance")]
    ReplaceInheritanceWithDelegation {
        /// File that declares the subclass
        file: PathBuf,
        /// Name of the subclass to refactor
        #[arg(long = "sub-type", visible_alias = "class")]
        sub_type: String,
        /// Optional name of the base class to decouple from (auto-detected if omitted)
        #[arg(long = "base-type")]
        base_type: Option<String>,
        /// Optional name for the delegate field (defaults to base class name)
        #[arg(long = "field-name")]
        field_name: Option<String>,
        /// Optional explicit list of method names to forward (auto-discovered if omitted)
        #[arg(long, value_delimiter = ',')]
        methods: Vec<String>,
        /// Verify with compiler check
        #[arg(long)]
        verify: Option<String>,
        /// Apply the changes to disk
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Force apply even if warnings or non-fatal diagnostics occur
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Replace conditional logic (switch/match/if-else) with polymorphism.
    #[command(alias = "replace-conditional")]
    ReplaceConditionalWithPolymorphism {
        /// File that holds the conditional statement
        file: PathBuf,
        /// 1-based line of the switch/match/if statement
        #[arg(long, default_value_t = 0)]
        line: u32,
        /// 1-based column of the switch/match/if statement
        #[arg(long, default_value_t = 0)]
        character: u32,
        /// Name of the base class, interface, protocol, or trait
        #[arg(long = "base-name", visible_alias = "base")]
        base_name: String,
        /// Name of the polymorphic method to generate
        #[arg(long = "method-name", visible_alias = "method")]
        method_name: String,
        /// Optional method parameters (comma-separated, e.g. "amount: number")
        #[arg(long, value_delimiter = ',')]
        params: Vec<String>,
        /// Optional return type of the method
        #[arg(long = "return-type")]
        return_type: Option<String>,
        /// Optional target variable to invoke method on
        #[arg(long = "target-var")]
        target_var: Option<String>,
        /// Verify with compiler check
        #[arg(long)]
        verify: Option<String>,
        /// Apply the changes to disk
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Force apply even if warnings or non-fatal diagnostics occur
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Extract an interface, protocol, or abstract class from a class or struct.
    ExtractInterface {
        /// File that declares the class or struct
        file: PathBuf,
        /// Name of the class or struct to extract an interface from
        #[arg(long = "symbol", visible_alias = "type")]
        symbol: String,
        /// Name of the new interface, protocol, or abstract class
        #[arg(long = "name", visible_alias = "interface")]
        name: String,
        /// Optional subset of method names to include (comma-separated, defaults to all public methods)
        #[arg(long, value_delimiter = ',')]
        methods: Vec<String>,
        /// 1-based line of the type declaration
        #[arg(long, default_value_t = 0)]
        line: u32,
        /// 1-based column of the type declaration
        #[arg(long, default_value_t = 0)]
        character: u32,
        /// Do not migrate caller type annotations across the workspace to the extracted interface
        #[arg(long = "no-migrate-callers", default_value_t = false)]
        no_migrate_callers: bool,
        /// Verify with compiler check
        #[arg(long)]
        verify: Option<String>,
        /// Apply the changes to disk
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Force apply even if warnings or non-fatal diagnostics occur
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Promote an expression in a method into a field of its type, initialised wherever the
    /// type is built.
    ExtractField {
        /// The file the selection is in.
        file: PathBuf,
        /// 1-based line where the expression starts.
        line: Option<u32>,
        /// 1-based column where it starts.
        character: Option<u32>,
        /// Where it ends, as `LINE:COL` (the column is exclusive).
        #[arg(long = "to")]
        to: Option<String>,
        /// Selection range as `START_LINE:START_COL-END_LINE:END_COL`.
        #[arg(long)]
        range: Option<String>,
        /// Expression text to extract if line/col not given.
        #[arg(long)]
        expression: Option<String>,
        /// What the new field is called.
        #[arg(long)]
        name: String,
        /// The field's type.
        #[arg(long = "type")]
        ty: Option<String>,
        /// What every construction site initialises it with (default: the expression).
        #[arg(long)]
        init: Option<String>,
        /// Read the field at every identical occurrence in the method, not only the selection.
        #[arg(long, default_value_t = false)]
        replace_all: bool,
        /// `compile`: also run `cargo check` on the result in a shadow of the workspace, and write
        /// only if the compiler accepts it too. Seconds rather than milliseconds.
        #[arg(long)]
        verify: Option<String>,
        /// Write the change instead of only reporting.
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Write even when a pattern would break or the result does not compile.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Promote an expression in a function body into a parameter, passed at every call site
    /// (Rust, TypeScript, JavaScript, Python, Go, C, C++, Swift).
    ExtractParameter {
        /// The file the selection is in.
        file: PathBuf,
        /// 1-based line where the expression starts.
        line: u32,
        /// 1-based column where it starts.
        character: u32,
        /// Where it ends, as `LINE:COL` (the column is exclusive).
        #[arg(long = "to")]
        to: String,
        /// What the new parameter is called.
        #[arg(long)]
        name: String,
        /// The parameter's type, when the analyzer gives none.
        #[arg(long = "type")]
        ty: Option<String>,
        /// Replace every identical occurrence in the body, not only the selection.
        #[arg(long, default_value_t = false)]
        replace_all: bool,
        /// `compile` (Rust files only): also run `cargo check` on the result in a shadow of the
        /// workspace, and write only if the compiler accepts it too. Seconds rather than
        /// milliseconds.
        #[arg(long)]
        verify: Option<String>,
        /// Write the change instead of only reporting it.
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Write even when the result does not compile.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Bundle parameters into one object, updating the body and call sites
    /// (Rust, TypeScript, JavaScript, Python, Go, C, C++, Swift).
    /// JavaScript uses a plain object and syntax diagnostics; unsafe call shapes are refused.
    ParameterObject {
        /// The function, by name (`move_item`, `Session::open_text`, `Canvas.draw`).
        symbol: String,
        /// A parameter to bundle, by the name the declaration gives it. Repeat the flag.
        #[arg(long = "param", required = true)]
        params: Vec<String>,
        /// The new type's name, UpperCamelCase; JavaScript uses it only to derive the binding.
        #[arg(long)]
        name: String,
        /// What the new parameter is called in the body (default: the name in snake_case, or in
        /// lowerCamelCase in TypeScript, JavaScript, Go and Swift).
        #[arg(long)]
        binding: Option<String>,
        /// The file that declares it, when the name is ambiguous.
        #[arg(long)]
        path: Option<String>,
        /// `compile`: also run `cargo check` on the result in a shadow of the workspace, and write
        /// only if the compiler accepts it too. Rust only; seconds rather than milliseconds.
        #[arg(long)]
        verify: Option<String>,
        /// Write the change instead of only reporting it.
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Write even when the result does not compile.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Move a declaration into another module across polyglot languages, with the imports that keep it compiling.
    Move {
        /// The item, by name (`snake_case`, `Session::open_text`, `calculate`).
        symbol: String,
        /// The target module's file, e.g. `crates/x/src/fixture.rs` or `src/helpers.ts`. A file that does not exist
        /// yet is created and initialized.
        #[arg(long = "to")]
        to: String,
        /// The file that declares it, when the name is ambiguous.
        #[arg(long)]
        path: Option<String>,
        /// `compile`: also run compiler check on the result in a shadow of the workspace, and write
        /// only if the compiler accepts it too (Rust only; seconds rather than milliseconds).
        #[arg(long)]
        verify: Option<String>,
        /// Write the move instead of only reporting it.
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Write even when the result does not compile.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Move a method to the type of one of its parameters, swapping receiver and argument.
    MoveMethod {
        /// The file that declares the method.
        file: PathBuf,
        /// 1-based line of the method's name.
        line: u32,
        /// 1-based column of the method's name.
        col: u32,
        /// The parameter whose type the method moves to.
        #[arg(long = "to-param")]
        to_param: Option<String>,
        /// For an associated function (no `self`): the type it moves to.
        #[arg(long = "to-type")]
        to_type: Option<String>,
        /// Write the change instead of only reporting it.
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Write even when something blocks it or it does not compile.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Move a whole module to another parent (`a::b` becomes `c::b`), with its file, its
    /// submodules and every path to it.
    MoveModule {
        /// The module's file, e.g. `src/a/b.rs` or `src/a/b/mod.rs`.
        file: PathBuf,
        /// Where the module's file goes, e.g. `src/c/b.rs`.
        #[arg(long = "to")]
        to: PathBuf,
        /// `compile`: also run `cargo check` on the result in a shadow of the workspace.
        #[arg(long)]
        verify: Option<String>,
        /// Write the move instead of only reporting it.
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Write even when the result does not compile.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Change what a function takes, with its call sites: Rust reorders, adds and removes
    /// parameters; Go reorders, removes provably unused ones, adds typed literal parameters, or
    /// replaces one primitive result of an ordinary free function or named value/pointer receiver
    /// method.
    ///
    /// Rust: refuses changes to argument evaluation or destruction order (including possible
    /// Deref coercions).
    ///
    /// Go (a `.go` file, through gopls v0.23.0): a permutation of the named parameters of a
    /// declared function or method, optionally omitting unused parameters. Grouped parameters
    /// (`a, b int`) move one by one; receivers stay as declared. `--returns` replaces only a
    /// single unnamed, unshadowed primitive result of an ordinary non-generic, non-variadic free
    /// function or named value/pointer receiver method, and requires the named parameter list to
    /// be exactly unchanged. A retained variadic parameter stays last; removing it removes its
    /// entire argument tail.
    /// Both body inspection and gopls references must prove every removed parameter unused.
    /// Dropped arguments must be literals or simple variables; calls, selectors, indexing,
    /// receives, conversions and operators are refused because their evaluation can matter.
    /// Every call and the declaration must match exactly, and the result is type-checked.
    /// Go additions retain every old parameter in order and use explicit primitive types with
    /// numeric/string/rune literals. Ordinary non-generic functions and named value/pointer
    /// receiver methods are supported. Method values/expressions, interface obligations,
    /// generic receivers, receiver-name capture, variadics, grouped-parameter interior insertion
    /// and combined changes refuse.
    /// Packages and test callers must compile on the node before preview or apply. This preserves
    /// the node's build flags and does not require Go on the client.
    /// Refused for Go without writing, whatever `--force` says: arbitrary added values,
    /// `--visibility`, `--async`, `--verify`, named/multiple/void/composite results, generic or
    /// variadic result changes, receiver methods with interface obligations or non-call references,
    /// shadowed primitive names, parameter changes combined with `--returns`, unnamed or `_`
    /// parameters, any generic removal,
    /// a generic function that has calls, a function used as a value, an unreconciled call,
    /// and uncertain argument reordering (`true`, `false` and `nil` count as variables).
    ChangeSignature {
        /// The function, by name (`validate_texts`, `Session::open_text`, `Price`, `Cart.Add`).
        symbol: String,
        /// One entry of the new parameter list, in order: `name` keeps it, `name: Type = expr`
        /// adds it (Go: primitive type and numeric/string/rune literal only); an omitted parameter
        /// is removed. Go requires proof
        /// that a removed parameter is unused and dropping its arguments is safe. Repeat the flag.
        #[arg(
            long = "param",
            required_unless_present = "remove_all",
            conflicts_with = "remove_all"
        )]
        params: Vec<String>,
        /// Explicitly request an empty parameter list. The same removal safety checks apply.
        #[arg(long, conflicts_with = "params")]
        remove_all: bool,
        /// Rust: the return type it should have; `()` removes it. Go: replaces one existing,
        /// unnamed, unshadowed primitive result of an ordinary non-generic, non-variadic free
        /// function or named value/pointer receiver method;
        /// parameters must be listed exactly as declared and Go compiler verification always runs
        /// remotely.
        #[arg(long)]
        returns: Option<String>,
        /// Rust only: its visibility, `pub`, `pub(crate)`, `pub(super)`, or `private`.
        #[arg(long)]
        visibility: Option<String>,
        /// Rust only: `true` makes it `async` and awaits every call; `false` takes both away.
        #[arg(long = "async")]
        asyncness: Option<bool>,
        /// The file that declares it, when the name is ambiguous.
        #[arg(long)]
        path: Option<String>,
        /// Rust only: `compile` also runs `cargo check` on the result in a shadow of the
        /// workspace, and writes only if the compiler accepts it too. Seconds rather than
        /// milliseconds. Refused for Go before anything is written.
        #[arg(long)]
        verify: Option<String>,
        /// Write the change instead of only reporting it.
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Rust: override body-use/compiler errors; never incomplete references or effect-order
        /// checks. Overrides no Go refusal.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Structural search and replace across the workspace: `pattern ==>> replacement`.
    Codemod {
        /// The rule, for example `$a.unwrap() ==>> $a.expect("invariant")`.
        rule: String,
        /// A file used to resolve the paths the pattern mentions.
        #[arg(long)]
        path: Option<String>,
        /// Write the edits into the checkout instead of only reporting them.
        #[arg(long, default_value_t = false)]
        apply: bool,
    },
    /// Find code by what it does: ranked declarations with the doc comment that matched.
    Search {
        /// What the code does, in words.
        query: String,
        /// Hits to return.
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// Restrict to declarations under this directory.
        #[arg(long)]
        path: Option<String>,
    },
    /// Print only the code a symbol depends on: its declaration plus the items it uses.
    Slice {
        /// Symbol name (`Metrics::record`, `pkg.Func`), or a file with `--line`.
        target: String,
        /// 1-based line, when `target` is a file path.
        #[arg(long)]
        line: Option<u32>,
        /// 1-based column, when `target` is a file path.
        #[arg(long, default_value_t = 1)]
        character: u32,
        /// How many edges to follow from the seed.
        #[arg(long, default_value_t = 2)]
        depth: u32,
        /// Stop once the slice reaches this many bytes.
        #[arg(long, default_value_t = 24576)]
        max_bytes: usize,
        /// Perform intra-function backward data-flow and control-dependency slicing inside function body.
        #[arg(long, default_value_t = false)]
        dataflow: bool,
        /// Target line for intra-function data-flow slicing criterion (1-based line).
        #[arg(long)]
        target_line: Option<u32>,
        /// Target variable name for intra-function data-flow slicing criterion.
        #[arg(long)]
        target_var: Option<String>,
    },
    /// Try several hypotheses (sets of proposed file contents) against a command, each in a
    /// private shadow of the server workspace; print every outcome and the winner's diff.
    ShadowRun {
        /// JSON spec: {"hypotheses":[{"name":"h1","edits":[{"path":"src/x.rs","file":"h1-x.rs"}],"delete":["old.rs"]}]}
        /// (`file` is read relative to the spec file; `new_text` inlines the content).
        spec: PathBuf,
        /// Kill a hypothesis after this many seconds (0 = server default, 1 hour).
        #[arg(long, default_value_t = 0)]
        timeout_secs: u64,
        /// Hypotheses to run at once (0 = server default: cores / 8).
        #[arg(long, default_value_t = 0)]
        parallel: usize,
        /// Write the winning hypothesis into the checkout.
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Command and arguments (put `--` before them).
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Benchmark throughput and concurrency across workspaces and worktrees.
    Bench {
        /// Target workspace directories (or worktrees). If omitted, uses current working directory.
        #[arg(short, long, num_args = 1..)]
        workspaces: Vec<PathBuf>,
        /// Number of concurrent client worker connections.
        #[arg(short, long, default_value_t = 16)]
        concurrency: usize,
        /// Pipeline depth per connection (in-flight queries sent without waiting).
        #[arg(short, long, default_value_t = 8)]
        depth: usize,
        /// Benchmark duration in seconds.
        #[arg(long, default_value_t = 5)]
        duration_secs: u64,
    },
    /// Multi-worktree divergence and correctness benchmark: forks isolated git worktrees from a
    /// base repo (a real Rust or Go repository), applies controlled mutations (signature change,
    /// dependency manifest change, untracked file), then hammers them with concurrent LSP queries
    /// from a simulated agent fleet to assert zero cross-worktree bleed.
    DivergentBench {
        /// Base git repository to fork worktrees from. If omitted, a disposable scratch
        /// repository with a synthetic fixture crate is created instead.
        #[arg(long)]
        base_repo: Option<PathBuf>,
        /// Scratch directory to materialize the origin clone and worktrees in. Defaults to a
        /// temp directory that is cleaned up afterwards.
        #[arg(long)]
        workdir: Option<PathBuf>,
        /// Number of concurrent simulated workers (minimum 10).
        #[arg(long, default_value_t = 12)]
        workers: usize,
        /// Number of queries each worker issues against its assigned worktree.
        #[arg(long, default_value_t = 5)]
        queries_per_worker: usize,
        /// Keep the generated scratch worktrees on disk after the run for inspection.
        #[arg(long, default_value_t = false)]
        keep_workdir: bool,
        /// Server workspace mapping: `shared` coalesces all worktrees onto one server
        /// workspace (production behaviour), `isolated` gives each worktree its own.
        #[arg(long, value_enum, default_value_t = WorkspaceMode::Isolated)]
        mode: WorkspaceMode,
        /// One gateway session per worker (sync/handshake once, then only queries), like a
        /// long-lived MCP agent. Default: fresh connection with pre-flight sync per query.
        #[arg(long, default_value_t = false)]
        persistent: bool,
        /// Persistent mode: drop the connection without a goodbye after this percentage of
        /// queries (simulated agent SIGKILL) and verify the gateway retires the sessions.
        #[arg(long, default_value_t = 0)]
        churn: u8,
        /// How many worktrees to fork, a multiple of 4: every mutation kind (untouched,
        /// signature change, manifest change, untracked file) that many times over four.
        #[arg(long, default_value_t = 4)]
        worktrees: usize,
    },
    /// Analyze architectural dependencies, calculate coupling metrics, and detect cycles
    Dependencies {
        /// Scope: 'crates' (default) or 'modules'
        #[arg(long, default_value = "crates")]
        scope: String,
        /// Path to narrow scope
        #[arg(long)]
        path: Option<PathBuf>,
        /// Output JSON report
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Scan workspace for code duplications and Type-1/Type-2 code clones
    Duplicates {
        /// Minimum consecutive duplicated lines
        #[arg(long, default_value_t = 6)]
        min_lines: usize,
        /// Parameterized clone matching
        #[arg(long, default_value_t = true)]
        parameterized: bool,
        /// Maximum number of clone groups to display
        #[arg(long, default_value_t = 20)]
        max_groups: usize,
        /// Path to narrow scan
        #[arg(long)]
        path: Option<PathBuf>,
        /// Output JSON report
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Polyglot Structural AST Pattern Search across languages using metavariables ($name)
    #[command(alias = "struct-search")]
    StructuralSearch {
        /// AST Pattern, e.g. '$a.unwrap()' or 'errors.Wrap($err, $msg)'
        pattern: String,
        /// Path to narrow search
        #[arg(long)]
        path: Option<PathBuf>,
        /// Output JSON report
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Synthesize valid in-scope expressions evaluating to a requested target type
    #[command(alias = "propose-expr")]
    ProposeExpression {
        /// Source file path
        file: PathBuf,
        /// 1-based line number where the expression is needed
        line: u32,
        /// The expected target type, e.g. 'String', '&str', 'u64', 'Option<T>'
        target_type: String,
        /// Output JSON report
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Check for and install updates to prod-code from GitHub releases
    #[command(alias = "self-update")]
    Update {
        /// Only check for updates without installing
        #[arg(long)]
        check: bool,
        /// Force re-installation even if the latest version is already installed
        #[arg(long)]
        force: bool,
        /// Install a specific version / tag (e.g. v0.3.19)
        #[arg(long)]
        tag: Option<String>,
    },
    /// Manage native packages, verify cryptographic integrity, and monitor cluster fleet versions
    Package {
        #[command(subcommand)]
        subcommand: package::PackageSubcommands,
    },
}

fn command_path_tokens(command: Option<&Commands>) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let Some(cmd) = command else {
        return paths;
    };
    match cmd {
        Commands::Def { file: Some(f), .. }
        | Commands::Hover { file: Some(f), .. }
        | Commands::Refs { file: Some(f), .. }
        | Commands::Callers { file: Some(f), .. }
        | Commands::Callees { file: Some(f), .. }
        | Commands::Impls { file: Some(f), .. }
        | Commands::Supertypes { file: Some(f), .. }
        | Commands::SafeDelete { file: f, .. }
        | Commands::Rename { file: f, .. }
        | Commands::Assists { file: f, .. }
        | Commands::Assist { file: f, .. }
        | Commands::Diagnostics { file: f, .. }
        | Commands::Outline { file: f, .. }
        | Commands::InlineParameter { file: f, .. }
        | Commands::ExtractDelegate { file: f, .. }
        | Commands::ExtractTrait { file: f, .. }
        | Commands::LoopToIterator { file: f, .. }
        | Commands::ExtractFunction { file: f, .. }
        | Commands::IntroduceVariable { file: f, .. }
        | Commands::ReplaceConstructorWithFactory { file: f, .. }
        | Commands::ReplaceConstructorWithBuilder { file: f, .. }
        | Commands::PullUp { file: f, .. }
        | Commands::PushDown { file: f, .. }
        | Commands::ReplaceInheritanceWithDelegation { file: f, .. }
        | Commands::ReplaceConditionalWithPolymorphism { file: f, .. }
        | Commands::ExtractInterface { file: f, .. }
        | Commands::ExtractParameter { file: f, .. }
        | Commands::MoveMethod { file: f, .. }
        | Commands::ProposeExpression { file: f, .. } => {
            paths.push(f.clone());
        }
        Commands::MoveModule { file, to, .. } => {
            paths.push(file.clone());
            paths.push(to.clone());
        }
        Commands::Move { to, path, .. } => {
            paths.push(PathBuf::from(to));
            if let Some(p) = path {
                paths.push(PathBuf::from(p));
            }
        }
        Commands::MakeStatic { path, symbol, .. }
        | Commands::WrapReturn { path, symbol, .. }
        | Commands::ParameterObject { path, symbol, .. }
        | Commands::MigrateType { path, symbol, .. }
        | Commands::Generify { path, symbol, .. }
        | Commands::InvertBoolean { path, symbol, .. }
        | Commands::ConvertToMethod { path, symbol, .. }
        | Commands::EncapsulateField { path, symbol, .. } => {
            if let Some(p) = path {
                paths.push(PathBuf::from(p));
            }
            paths.push(PathBuf::from(symbol));
        }
        Commands::SchemaRename { path, .. }
        | Commands::Codemod { path, .. }
        | Commands::Search { path, .. }
        | Commands::ChangeSignature { path, .. }
        | Commands::Fixture { path, .. } => {
            if let Some(p) = path {
                paths.push(PathBuf::from(p));
            }
        }
        Commands::Symbols { target } | Commands::Slice { target, .. } => {
            paths.push(PathBuf::from(target));
        }
        Commands::Source { path, .. } => {
            paths.push(PathBuf::from(path));
        }
        Commands::Sync { path: Some(p), .. }
        | Commands::Check { path: Some(p), .. }
        | Commands::Lint { path: Some(p), .. }
        | Commands::Test { path: Some(p), .. }
        | Commands::Benchmarks { path: Some(p), .. }
        | Commands::Dependencies { path: Some(p), .. }
        | Commands::Duplicates { path: Some(p), .. }
        | Commands::StructuralSearch { path: Some(p), .. } => {
            paths.push(p.clone());
        }
        Commands::Pull { files } => {
            paths.extend(files.clone());
        }
        Commands::Validate {
            file,
            from,
            diff,
            with,
            ..
        } => {
            if let Some(f) = file {
                paths.push(f.clone());
            }
            if let Some(fr) = from {
                paths.push(fr.clone());
            }
            if let Some(d) = diff {
                paths.push(d.clone());
            }
            for w in with {
                if let Some((target, replacement)) = w.split_once('=') {
                    paths.push(PathBuf::from(target));
                    paths.push(PathBuf::from(replacement));
                } else {
                    paths.push(PathBuf::from(w));
                }
            }
        }
        Commands::Exec { command, .. } => {
            for arg in command {
                if arg.contains('=') {
                    continue;
                }
                let pieces: Vec<String> = arg
                    .split(|c: char| c.is_whitespace() || c == '\'' || c == '"' || c == ';')
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect();
                paths.push(PathBuf::from(arg));
                for piece in pieces {
                    paths.push(PathBuf::from(piece));
                }
            }
        }
        _ => {}
    }
    paths
}

#[tokio::main]
async fn main() -> Result<()> {
    // Everything down to the dispatch below runs on every invocation, whatever the
    // subcommand, and until this timer existed none of it was measured: the query timer
    // starts after it. See the report on stderr under `PROD_CODE_TIMING=1`.
    let mut startup = QueryTiming::labelled("startup");
    let cli = Cli::parse();
    startup.mark("parse_args");

    let seeds = prod_code_mcp::cluster::parse_remotes(&cli.remote)?;
    // Nodes named with `--remote` on the command line are the ones to use, as given (#125);
    // otherwise one seed is enough and the rest of the cluster comes from its gossip view.
    let pinned = env::args().any(|a| a == "-r" || a == "--remote" || a.starts_with("--remote="));
    let remotes = if pinned {
        seeds.clone()
    } else {
        prod_code_mcp::cluster::discover_nodes(&seeds).await
    };
    startup.mark("discover_nodes");

    let cwd_root = env::current_dir()
        .ok()
        .map(|d| find_workspace_root(&d).unwrap_or(d));
    startup.mark("workspace_root");
    // Placement follows the origin repository: every worktree lands on the node that holds
    // the origin's copy, so seeding from that copy and the shared cargo target directory
    // work. The workspace name itself stays per worktree (`<repo>--wt-<hash>`).
    let cwd_workspace = cwd_root
        .as_deref()
        .map(|root| {
            let identity = prod_code_mcp::sync::workspace_identity(root);
            identity.base.unwrap_or(identity.name)
        })
        .unwrap_or_default();
    startup.mark("workspace_identity");
    // The engine a query needs is that of the nearest project of the file it names (or of
    // the current directory): a SwiftPM package inside a Rust repository must land on a
    // macOS node even though the repository root is Rust.
    let (cwd_subproject, cwd_engine) = cwd_root
        .as_deref()
        .map(|root| {
            let tokens = command_path_tokens(cli.command.as_ref());
            let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
            let file_hint = tokens.iter().find_map(|p| {
                let candidate = if p.is_absolute() {
                    p.clone()
                } else {
                    root.join(p)
                };
                if candidate.is_file() {
                    let canonical = std::fs::canonicalize(&candidate).ok()?;
                    if canonical.starts_with(&canonical_root) {
                        return Some(canonical);
                    }
                }
                None
            });
            let hint = file_hint
                .or_else(|| {
                    tokens.iter().find_map(|p| {
                        let candidate = if p.is_absolute() {
                            p.clone()
                        } else {
                            root.join(p)
                        };
                        if candidate.is_dir() && candidate != *root {
                            let canonical = std::fs::canonicalize(&candidate).ok()?;
                            if canonical.starts_with(&canonical_root) && canonical != canonical_root {
                                return Some(canonical);
                            }
                        }
                        None
                    })
                })
                .or_else(|| {
                    env::current_dir()
                        .ok()
                        .and_then(|cwd| std::fs::canonicalize(&cwd).ok())
                        .filter(|cwd| cwd.starts_with(&canonical_root))
                })
                .unwrap_or_else(|| canonical_root.clone());
            prod_code_mcp::sync::engine_project(root, &hint)
        })
        .unwrap_or((None, None));
    // A nested project of another language is placed under its own key, so its node does not
    // displace the checkout's own placement and back again on the next query (#125).
    let placement_key = match (&cwd_subproject, cwd_engine) {
        (Some(_), Some(engine)) => format!("{cwd_workspace}#{engine}"),
        _ => cwd_workspace.clone(),
    };
    // An editor's server names its language, which may not be the root's: a Swift package's
    // server in a Rust checkout goes to a macOS node, under a key of its own (#332).
    let lsp_engine = match &cli.command {
        Some(Commands::Lsp {
            language: Some(language),
        }) => Some(
            prod_code_client::editor_files::engine_for_language(language).with_context(|| {
                format!(
                    "prod-code lsp has no server for `{language}`: use rust, go, c, cpp, \
                     python, typescript, javascript or swift"
                )
            })?,
        ),
        _ => None,
    };
    let placement_key = match lsp_engine {
        Some(engine) if Some(engine) != cwd_engine => format!("{cwd_workspace}#{engine}"),
        _ => placement_key,
    };
    let cwd_engine = lsp_engine.or(cwd_engine);
    startup.mark("engine_project");

    if let Some(Commands::Cluster { json }) = cli.command {
        startup.report();
        return run_cluster(&remotes, &placement_key, cwd_engine, json).await;
    }

    // A Go module whose cgo includes macOS headers builds only on macOS; a Linux node would
    // report the headers as missing on every check (#248).
    let macos_cgo = match (cwd_root.as_deref(), cwd_engine) {
        (Some(root), Some("go")) => prod_code_mcp::sync::macos_only_cgo(
            &cwd_subproject
                .as_deref()
                .map_or_else(|| root.to_path_buf(), |sub| root.join(sub)),
        ),
        _ => None,
    };
    startup.mark("macos_only_cgo");
    let picked = prod_code_mcp::cluster::pick_node(
        &remotes,
        &placement_key,
        cwd_engine,
        macos_cgo.as_ref().map(|_| "macos"),
    )
    .await
    .map_err(|err| match &macos_cgo {
        Some((file, named)) => err.context(format!(
            "this Go module uses macOS-only cgo ({file}: {named})"
        )),
        None => err,
    });
    // A bug report needs no workspace: the node only fills in one line of it, and a checkout
    // the cluster cannot place is exactly what may need reporting (#307).
    if let Some(Commands::ReportIssue {
        title,
        body,
        body_file,
        private_ref,
        force,
        dry_run,
        labels,
    }) = cli.command
    {
        startup.report();
        let unplaced = picked.as_ref().err().map(|err| format!("{err:#}"));
        return run_report_issue(
            picked.ok(),
            unplaced,
            ReportArgs {
                title,
                body,
                body_file,
                private_ref,
                force,
                dry_run,
                labels,
            },
        )
        .await;
    }
    // Update needs no workspace or cluster connection: it interacts with GitHub releases.
    if let Some(Commands::Update { check, force, tag }) = cli.command {
        startup.report();
        return update::run_update(check, force, tag).await;
    }
    // Package management needs no specific project workspace.
    if let Some(Commands::Package { subcommand }) = cli.command {
        startup.report();
        match subcommand {
            package::PackageSubcommands::Status { json } => return package::run_package_status(json).await,
            package::PackageSubcommands::Verify => return package::run_package_verify().await,
            package::PackageSubcommands::Install { force, tag, system } => {
                return package::run_package_install(force, tag, system).await;
            }
            package::PackageSubcommands::Sync { node } => {
                let r = node.or_else(|| picked.ok());
                return package::run_package_sync(r).await;
            }
        }
    }
    if let Some(Commands::Status { json }) = cli.command {
        startup.report();
        let target = if pinned {
            seeds[0]
        } else {
            picked.unwrap_or(seeds[0])
        };
        let note = if !pinned && target.ip().is_loopback() && target.port() != 9400 {
            Some("loopback forward".to_string())
        } else {
            None
        };
        return run_status_probe(target, json, note).await;
    }
    // An editor learns why its server could not start from the answer to its `initialize`,
    // not from a process that is gone before it asks (#338).
    if let (Some(Commands::Lsp { .. }), Err(err)) = (&cli.command, &picked) {
        return refuse_lsp(err).await;
    }
    let remote = picked?;
    prod_code_mcp::cluster::set_routing(remotes.clone(), cwd_workspace.clone());
    startup.mark("pick_node");
    startup.report();

    match cli.command.unwrap_or(Commands::Lsp { language: None }) {
        Commands::Lsp { .. } => run_lsp_bridge(remote, lsp_engine).await,
        Commands::Status { .. } => unreachable!("handled before placement"),
        Commands::Cluster { json } => run_cluster(&remotes, &placement_key, cwd_engine, json).await,
        Commands::Metrics { since, json } => run_metrics(&remotes, since, json).await,
        Commands::ReportIssue { .. } => unreachable!("handled before placement"),
        Commands::Mcp => run_mcp_server(remote).await,
        Commands::Sync { path, pull } => {
            if pull {
                let files = match path {
                    Some(p) => vec![p],
                    None => anyhow::bail!(
                        "--pull requires at least one file or path to pull (e.g. `prod-code sync --pull path/to/file.rs` or `prod-code pull <files...>`); to push current changes omit --pull"
                    ),
                };
                run_pull(remote, files).await
            } else {
                run_sync(remote, path).await
            }
        }
        Commands::Pull { files } => run_pull(remote, files).await,
        Commands::Def {
            file,
            line,
            col,
            symbol,
            body,
        } => match symbol {
            Some(symbol) => {
                let mut args = symbol_args(&symbol, file);
                args["body"] = serde_json::json!(body);
                run_tool(remote, "code_definition", args).await
            }
            None if body => {
                let (file, line, col) = position(file, line, col)?;
                let file = std::fs::canonicalize(&file).unwrap_or(file);
                run_tool(
                    remote,
                    "code_definition",
                    serde_json::json!({
                        "path": file.to_string_lossy(),
                        "line": line,
                        "character": col,
                        "body": true,
                    }),
                )
                .await
            }
            None => {
                let (file, line, col) = position(file, line, col)?;
                run_definition(remote, &file, line, col).await
            }
        },
        Commands::Hover {
            file,
            line,
            col,
            symbol,
        } => match symbol {
            Some(symbol) => run_by_symbol(remote, "code_hover", &symbol, file).await,
            None => {
                let (file, line, col) = position(file, line, col)?;
                run_hover(remote, &file, line, col).await
            }
        },
        Commands::Refs {
            file,
            line,
            col,
            symbol,
            also_in,
        } => {
            // Answered by the MCP tool in every form: it refuses a position on no name, asks a
            // dependency's item from its uses (#373) and searches other checkouts (#375).
            let mut args = match symbol {
                Some(symbol) => symbol_args(&symbol, file),
                None => {
                    let (file, line, col) = position(file, line, col)?;
                    let file = std::fs::canonicalize(&file).unwrap_or(file);
                    serde_json::json!({
                        "path": file.to_string_lossy(),
                        "line": line,
                        "character": col,
                    })
                }
            };
            if !also_in.is_empty() {
                args["also_in"] = also_in
                    .into_iter()
                    .map(|dir| {
                        let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
                        serde_json::json!(dir.to_string_lossy())
                    })
                    .collect();
            }
            run_refs(remote, args).await
        }
        Commands::Callers {
            file,
            line,
            col,
            symbol,
            depth,
        } => run_call_tree(remote, "code_callers", file, line, col, symbol, depth).await,
        Commands::Callees {
            file,
            line,
            col,
            symbol,
            depth,
        } => run_call_tree(remote, "code_callees", file, line, col, symbol, depth).await,
        Commands::Impls {
            file,
            line,
            col,
            symbol,
        } => match symbol {
            Some(symbol) => run_by_symbol(remote, "code_implementations", &symbol, file).await,
            None => {
                let (file, line, col) = position(file, line, col)?;
                run_implementations(remote, &file, line, col).await
            }
        },
        Commands::Supertypes {
            file,
            line,
            col,
            symbol,
        } => match symbol {
            Some(symbol) => run_by_symbol(remote, "code_supertypes", &symbol, file).await,
            None => {
                let (file, line, col) = position(file, line, col)?;
                let file = std::fs::canonicalize(&file).unwrap_or(file);
                run_tool(
                    remote,
                    "code_supertypes",
                    serde_json::json!({ "path": file.to_string_lossy(), "line": line, "character": col }),
                )
                .await
            }
        },
        Commands::Symbols { target } => {
            if Path::new(&target).is_file() {
                run_symbols(
                    remote,
                    Path::new(&target),
                    &prod_code_mcp::tools::OutlineOptions::all(usize::MAX, false, "pass --locals"),
                )
                .await
            } else {
                run_tool(
                    remote,
                    "code_symbols",
                    serde_json::json!({ "query": target }),
                )
                .await
            }
        }
        Commands::Outline {
            file,
            locals,
            kinds,
            exported,
            max_bytes,
            max_items,
        } => {
            let is_dir = file.is_dir();
            let options = prod_code_mcp::tools::OutlineOptions {
                max_depth: usize::MAX,
                include_locals: locals,
                hint: "pass --locals".to_string(),
                kinds: (!kinds.is_empty()).then_some(kinds),
                exported_only: exported,
                max_bytes: match max_bytes {
                    Some(0) => None,
                    Some(bytes) => Some(bytes),
                    None => is_dir.then_some(prod_code_mcp::tools::DIRECTORY_OUTLINE_BYTES),
                },
                max_items: max_items.filter(|n| *n > 0),
            };
            run_symbols(remote, &file, &options).await
        }
        Commands::Source {
            path,
            line,
            context,
        } => run_source(remote, cwd_root.as_deref(), &path, line, context).await,
        Commands::Impact {
            base,
            depth,
            run,
            ci,
            json,
        } => run_impact(remote, base.as_deref(), depth, run, ci, json).await,
        Commands::DeadCode {
            include_exported,
            reachability,
            max_files,
            json,
        } => run_dead_code(remote, include_exported, reachability, max_files, json).await,
        Commands::Prune {
            max_files,
            apply,
            force,
            reachability,
        } => {
            run_tool(
                remote,
                "code_prune_orphans",
                serde_json::json!({
                    "max_files": max_files,
                    "apply": apply,
                    "force": force,
                    "reachability": reachability,
                }),
            )
            .await
        }
        Commands::Diagnostics { file, json } => run_diagnostics(remote, &file, None, json).await,
        Commands::Diagnose {
            filter,
            timeout_secs,
            json,
        } => run_diagnose(remote, filter.as_deref(), timeout_secs, json).await,
        Commands::Validate {
            file,
            from,
            diff,
            with,
            compile,
            json,
        } => {
            if compile && json {
                anyhow::bail!("--compile reports as text; leave out --json");
            }
            if let Some(diff) = diff {
                let patch = if diff.as_os_str() == "-" {
                    let mut buf = String::new();
                    std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
                    buf
                } else {
                    std::fs::read_to_string(&diff)
                        .with_context(|| format!("failed to read {}", diff.display()))?
                };
                return run_tool(
                    remote,
                    "code_validate_edits",
                    serde_json::json!({ "diff": patch, "compile": compile }),
                )
                .await;
            }
            let file = file.context("give the file to validate, or `--diff PATCH`")?;
            let text = match from {
                Some(path) => std::fs::read_to_string(&path)
                    .with_context(|| format!("failed to read {}", path.display()))?,
                None => {
                    let mut buf = String::new();
                    std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
                    buf
                }
            };
            if compile {
                run_validate_compiled(remote, &file, text, &with).await
            } else if with.is_empty() {
                run_diagnostics(remote, &file, Some(text), json).await
            } else {
                run_validate_together(remote, &file, text, &with, json).await
            }
        }
        Commands::Rename {
            file,
            line,
            col,
            new_name,
            accessors,
            comments,
            force,
        } => {
            run_rename(
                remote, &file, line, col, &new_name, accessors, comments, force,
            )
            .await
        }
        Commands::SafeDelete { file, line, col } => run_safe_delete(remote, &file, line, col).await,
        Commands::Assists {
            file,
            line,
            col,
            to,
        } => run_assist(remote, &file, line, col, to.as_deref(), None, None).await,
        Commands::Assist {
            file,
            line,
            col,
            id,
            to,
            subtype,
        } => run_assist(remote, &file, line, col, to.as_deref(), Some(&id), subtype).await,
        Commands::Check {
            timeout_secs,
            json,
            fix: true,
            path,
            ..
        } => run_fix(remote, VerifyKind::Check, timeout_secs, json, path).await,
        Commands::Check {
            timeout_secs,
            json,
            env,
            events,
            path,
            ..
        } => {
            run_verify(
                remote,
                VerifyKind::Check,
                VerifyArgs {
                    filter: None,
                    timeout_secs,
                    json,
                    env,
                    events,
                    path,
                },
            )
            .await
        }
        Commands::Lint {
            timeout_secs,
            json,
            fix: true,
            path,
            ..
        } => run_fix(remote, VerifyKind::Lint, timeout_secs, json, path).await,
        Commands::Lint {
            timeout_secs,
            json,
            env,
            events,
            path,
            ..
        } => {
            run_verify(
                remote,
                VerifyKind::Lint,
                VerifyArgs {
                    filter: None,
                    timeout_secs,
                    json,
                    env,
                    events,
                    path,
                },
            )
            .await
        }
        Commands::Test {
            filter,
            timeout_secs,
            json,
            env,
            events,
            path,
        } => {
            run_verify(
                remote,
                VerifyKind::Test,
                VerifyArgs {
                    filter,
                    timeout_secs,
                    json,
                    env,
                    events,
                    path,
                },
            )
            .await
        }
        Commands::Benchmarks {
            filter,
            timeout_secs,
            json,
            env,
            events,
            path,
        } => {
            run_verify(
                remote,
                VerifyKind::Bench,
                VerifyArgs {
                    filter,
                    timeout_secs,
                    json,
                    env,
                    events,
                    path,
                },
            )
            .await
        }
        Commands::Exec {
            timeout_secs,
            no_pull,
            env,
            command,
        } => run_exec(remote, command, env, timeout_secs, !no_pull).await,
        Commands::Fixture {
            symbol,
            depth,
            no_verify,
            path,
            builder,
            builder_name,
            randomized,
            mock,
            language,
        } => {
            run_fixture_cli(
                remote,
                symbol,
                depth,
                !no_verify,
                path,
                builder,
                builder_name,
                randomized,
                mock,
                language,
            )
            .await
        }
        Commands::SchemaRename {
            field,
            to,
            path,
            repos,
            verify,
            apply,
            force,
        } => run_schema_rename_cli(remote, field, to, path, repos, verify, apply, force).await,
        Commands::MigrateType {
            symbol,
            to,
            line,
            character,
            path,
            convert,
            transitive,
            apply,
            force,
        } => {
            run_migrate_type_cli(
                remote, symbol, to, line, character, path, convert, transitive, apply, force,
            )
            .await
        }
        Commands::Generify {
            symbol,
            param,
            bound,
            type_param,
            line,
            character,
            path,
            function,
            apply,
            force,
        } => {
            let mut args = serde_json::json!({
                "param": param,
                "bound": bound,
                "type_param": type_param,
                "apply": apply,
                "force": force,
            });
            if let Some(p) = path {
                args["path"] = serde_json::Value::String(p);
                args["symbol"] = serde_json::Value::String(function.unwrap_or(symbol));
            } else if let Some(line) = line {
                args["path"] = serde_json::Value::String(symbol);
                args["line"] = serde_json::Value::from(line);
                args["character"] = serde_json::Value::from(character);
                if let Some(f) = function {
                    args["symbol"] = serde_json::Value::String(f);
                }
            } else {
                let p = Path::new(&symbol);
                if p.extension().is_some() {
                    args["path"] = serde_json::Value::String(symbol);
                    if let Some(f) = function {
                        args["symbol"] = serde_json::Value::String(f);
                    }
                } else {
                    args["symbol"] = serde_json::Value::String(symbol);
                }
            }
            run_tool(remote, "code_generify", args).await
        }
        Commands::InvertBoolean {
            symbol,
            new_name,
            line,
            character,
            path,
            function,
            verify,
            apply,
            force,
        } => {
            let mut args =
                serde_json::json!({ "new_name": new_name, "apply": apply, "force": force });
            let is_file_path = std::path::Path::new(&symbol).extension().is_some() || std::path::Path::new(&symbol).exists();
            if let Some(line) = line {
                args["path"] = serde_json::Value::String(symbol);
                args["line"] = serde_json::Value::from(line);
                args["character"] = serde_json::Value::from(character);
            } else if is_file_path {
                args["path"] = serde_json::Value::String(symbol);
            } else {
                args["symbol"] = serde_json::Value::String(symbol);
            }
            if let Some(path) = path {
                args["path"] = serde_json::Value::String(path);
            }
            if let Some(function) = function {
                args["function"] = serde_json::Value::String(function);
            }
            if let Some(verify) = verify {
                args["verify"] = serde_json::Value::String(verify);
            }
            run_tool(remote, "code_invert_boolean", args).await
        }
        Commands::ConvertToMethod {
            symbol,
            line,
            character,
            path,
            method,
            class,
            verify,
            apply,
            force,
        } => {
            let mut args = serde_json::json!({ "apply": apply, "force": force });
            let is_file_path = std::path::Path::new(&symbol).extension().is_some() || std::path::Path::new(&symbol).exists();
            if let Some(line) = line {
                args["path"] = serde_json::Value::String(symbol);
                args["line"] = serde_json::Value::from(line);
                args["character"] = serde_json::Value::from(character);
            } else if is_file_path {
                args["path"] = serde_json::Value::String(symbol);
            } else if method.is_some() {
                if path.is_some() {
                    args["class_name"] = serde_json::Value::String(symbol);
                } else {
                    args["path"] = serde_json::Value::String(symbol);
                }
            } else {
                args["symbol"] = serde_json::Value::String(symbol);
            }
            if let Some(path) = path {
                args["path"] = serde_json::Value::String(path);
            }
            if let Some(method) = method {
                args["method"] = serde_json::Value::String(method);
            }
            if let Some(class) = class {
                args["class_name"] = serde_json::Value::String(class);
            }
            if let Some(verify) = verify {
                args["verify"] = serde_json::Value::String(verify);
            }
            run_tool(remote, "code_convert_to_method", args).await
        }
        Commands::InlineParameter {
            file,
            line,
            col,
            function,
            param,
            verify,
            apply,
            force,
        } => {
            let abs = std::fs::canonicalize(&file).unwrap_or(file);
            let mut args = serde_json::json!({
                "path": abs.to_string_lossy(),
                "apply": apply,
                "force": force,
            });
            if let Some(l) = line {
                args["line"] = serde_json::json!(l);
            }
            if let Some(c) = col {
                args["character"] = serde_json::json!(c);
            }
            if let Some(f) = function {
                args["function"] = serde_json::Value::String(f);
            }
            if let Some(p) = param {
                args["parameter"] = serde_json::Value::String(p);
            }
            if let Some(v) = verify {
                args["verify"] = serde_json::Value::String(v);
            }
            run_tool(remote, "code_inline_parameter", args).await
        }
        Commands::ExtractDelegate {
            file,
            line,
            col,
            symbol,
            fields,
            methods,
            name,
            field,
            verify,
            apply,
            force,
        } => {
            let abs = std::fs::canonicalize(&file).unwrap_or(file);
            let mut args = serde_json::json!({
                "path": abs.to_string_lossy(),
                "fields": fields,
                "methods": methods,
                "name": name,
                "field": field,
                "apply": apply,
                "force": force,
            });
            if let Some(l) = line {
                args["line"] = serde_json::Value::Number(l.into());
            }
            if let Some(c) = col {
                args["character"] = serde_json::Value::Number(c.into());
            }
            if let Some(s) = symbol {
                args["symbol"] = serde_json::Value::String(s);
            }
            if let Some(v) = verify {
                args["verify"] = serde_json::Value::String(v);
            }
            run_tool(remote, "code_extract_delegate", args).await
        }
        Commands::ExtractTrait {
            file,
            line,
            col,
            methods,
            name,
            no_migrate_callers,
            apply,
            force,
        } => {
            let abs = std::fs::canonicalize(&file).unwrap_or(file);
            run_tool(
                remote,
                "code_extract_trait",
                serde_json::json!({
                    "path": abs.to_string_lossy(),
                    "line": line,
                    "character": col,
                    "methods": methods,
                    "name": name,
                    "migrate_callers": !no_migrate_callers,
                    "apply": apply,
                    "force": force,
                }),
            )
            .await
        }
        Commands::LoopToIterator {
            file,
            line,
            col,
            symbol,
            apply,
            force,
        } => {
            let abs = std::fs::canonicalize(&file).unwrap_or(file);
            let mut payload = serde_json::json!({
                "path": abs.to_string_lossy(),
                "apply": apply,
                "force": force,
            });
            if let Some(l) = line {
                payload["line"] = serde_json::json!(l);
            }
            if let Some(c) = col {
                payload["character"] = serde_json::json!(c);
            }
            if let Some(s) = symbol {
                payload["symbol"] = serde_json::json!(s);
            }
            run_tool(remote, "code_loop_to_iterator", payload).await
        }
        Commands::ExtractFunction {
            file,
            line,
            col,
            to,
            name,
            no_duplicates,
            parameterize,
            other_files,
            verify,
            apply,
            force,
        } => {
            let (end_line, end_col) = to
                .split_once(':')
                .and_then(|(l, c)| Some((l.parse::<u32>().ok()?, c.parse::<u32>().ok()?)))
                .context("--to takes LINE:COL")?;
            let abs = std::fs::canonicalize(&file).unwrap_or(file);
            let mut args = serde_json::json!({
                "path": abs.to_string_lossy(),
                "line": line,
                "character": col,
                "end_line": end_line,
                "end_character": end_col,
                "name": name,
                "duplicates": !no_duplicates,
                "parameterize": parameterize,
                "other_files": other_files,
                "apply": apply,
                "force": force,
            });
            if let Some(verify) = verify {
                args["verify"] = serde_json::Value::String(verify);
            }
            run_tool(remote, "code_extract_function", args).await
        }
        Commands::IntroduceVariable {
            file,
            line,
            col,
            to,
            name,
            apply,
            force,
        } => {
            let (end_line, end_col) = to
                .split_once(':')
                .and_then(|(l, c)| Some((l.parse::<u32>().ok()?, c.parse::<u32>().ok()?)))
                .context("--to takes LINE:COL")?;
            let abs = std::fs::canonicalize(&file).unwrap_or(file);
            run_tool(
                remote,
                "code_introduce_variable",
                serde_json::json!({
                    "path": abs.to_string_lossy(),
                    "line": line,
                    "character": col,
                    "end_line": end_line,
                    "end_character": end_col,
                    "name": name,
                    "apply": apply,
                    "force": force,
                }),
            )
            .await
        }
        Commands::MakeStatic {
            symbol,
            line,
            character,
            path,
            method,
            class,
            verify,
            apply,
            force,
        } => {
            let mut args = serde_json::json!({ "apply": apply, "force": force });
            let is_file_path = std::path::Path::new(&symbol).extension().is_some() || std::path::Path::new(&symbol).exists();
            if let Some(line) = line {
                args["path"] = serde_json::Value::String(symbol);
                args["line"] = serde_json::Value::from(line);
                args["character"] = serde_json::Value::from(character);
            } else if is_file_path {
                args["path"] = serde_json::Value::String(symbol);
            } else if method.is_some() {
                if path.is_some() {
                    args["class_name"] = serde_json::Value::String(symbol);
                } else {
                    args["path"] = serde_json::Value::String(symbol);
                }
            } else {
                args["symbol"] = serde_json::Value::String(symbol);
            }
            if let Some(path) = path {
                args["path"] = serde_json::Value::String(path);
            }
            if let Some(method) = method {
                args["method"] = serde_json::Value::String(method);
            }
            if let Some(class) = class {
                args["class_name"] = serde_json::Value::String(class);
            }
            if let Some(verify) = verify {
                args["verify"] = serde_json::Value::String(verify);
            }
            run_tool(remote, "code_make_static", args).await
        }
        Commands::WrapReturn {
            symbol,
            wrapper,
            constructor,
            error,
            path,
            function,
            line,
            character,
            verify,
            apply,
            force,
        } => {
            let mut args =
                serde_json::json!({ "wrapper": wrapper, "apply": apply, "force": force });
            if let Some(c) = constructor {
                args["constructor"] = serde_json::Value::String(c);
            }
            if let Some(p) = path {
                args["path"] = serde_json::Value::String(p);
                args["symbol"] = serde_json::Value::String(function.unwrap_or(symbol));
            } else if let Some(line) = line {
                args["path"] = serde_json::Value::String(symbol);
                args["line"] = serde_json::Value::from(line);
                args["character"] = serde_json::Value::from(character);
                if let Some(f) = function {
                    args["symbol"] = serde_json::Value::String(f);
                }
            } else {
                let p = Path::new(&symbol);
                if p.extension().is_some() {
                    args["path"] = serde_json::Value::String(symbol);
                    if let Some(f) = function {
                        args["symbol"] = serde_json::Value::String(f);
                    }
                } else {
                    args["symbol"] = serde_json::Value::String(symbol);
                }
            }
            if let Some(error) = error {
                args["error"] = serde_json::Value::String(error);
            }
            if let Some(verify) = verify {
                args["verify"] = serde_json::Value::String(verify);
            }
            run_tool(remote, "code_wrap_return", args).await
        }
        Commands::EncapsulateField {
            symbol,
            line,
            character,
            path,
            field,
            class,
            by_value,
            verify,
            apply,
            force,
        } => {
            run_encapsulate_field_cli(
                remote, symbol, line, character, path, field, class, by_value, verify, apply, force,
            )
            .await
        }
        Commands::ReplaceConstructorWithFactory {
            file,
            type_name,
            name,
            verify,
            apply,
            force,
        } => {
            let cwd = env::current_dir().context("Failed to get current working directory")?;
            let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
            let mut args = serde_json::json!({
                "path": file.to_string_lossy(),
                "type_name": type_name,
                "apply": apply,
                "force": force,
            });
            if let Some(n) = name {
                args["factory_name"] = serde_json::Value::String(n);
            }
            if let Some(v) = verify {
                args["verify"] = serde_json::Value::String(v);
            }
            let result =
                prod_code_mcp::tools::execute_tool(remote, &root, "code_replace_constructor_with_factory", args)
                    .await?;
            for content in &result.content {
                let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
                println!("{text}");
            }
            if result.is_error {
                std::process::exit(1);
            }
            Ok(())
        }
        Commands::ReplaceConstructorWithBuilder {
            file,
            type_name,
            name,
            verify,
            apply,
            force,
        } => {
            let cwd = env::current_dir().context("Failed to get current working directory")?;
            let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
            let mut args = serde_json::json!({
                "path": file.to_string_lossy(),
                "type_name": type_name,
                "apply": apply,
                "force": force,
            });
            if let Some(n) = name {
                args["builder_name"] = serde_json::Value::String(n);
            }
            if let Some(v) = verify {
                args["verify"] = serde_json::Value::String(v);
            }
            let result =
                prod_code_mcp::tools::execute_tool(remote, &root, "code_replace_constructor_with_builder", args)
                    .await?;
            for content in &result.content {
                let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
                println!("{text}");
            }
            if result.is_error {
                std::process::exit(1);
            }
            Ok(())
        }
        Commands::PullUp {
            file,
            class,
            members,
            target_class,
            no_clean_siblings,
            verify,
            apply,
            force,
        } => {
            let cwd = env::current_dir().context("Failed to get current working directory")?;
            let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
            let mut args = serde_json::json!({
                "path": file.to_string_lossy(),
                "class_name": class,
                "members": members,
                "clean_siblings": !no_clean_siblings,
                "apply": apply,
                "force": force,
            });
            if let Some(t) = target_class {
                args["target_class"] = serde_json::Value::String(t);
            }
            if let Some(v) = verify {
                args["verify"] = serde_json::Value::String(v);
            }
            let result =
                prod_code_mcp::tools::execute_tool(remote, &root, "code_pull_up", args)
                    .await?;
            for content in &result.content {
                let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
                println!("{text}");
            }
            if result.is_error {
                std::process::exit(1);
            }
            Ok(())
        }
        Commands::PushDown {
            file,
            class,
            members,
            target_classes,
            verify,
            apply,
            force,
        } => {
            let cwd = env::current_dir().context("Failed to get current working directory")?;
            let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
            let mut args = serde_json::json!({
                "path": file.to_string_lossy(),
                "class_name": class,
                "members": members,
                "apply": apply,
                "force": force,
            });
            if !target_classes.is_empty() {
                args["target_classes"] = serde_json::Value::Array(
                    target_classes.into_iter().map(serde_json::Value::String).collect(),
                );
            }
            if let Some(v) = verify {
                args["verify"] = serde_json::Value::String(v);
            }
            let result =
                prod_code_mcp::tools::execute_tool(remote, &root, "code_push_down", args)
                    .await?;
            for content in &result.content {
                let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
                println!("{text}");
            }
            if result.is_error {
                std::process::exit(1);
            }
            Ok(())
        }
        Commands::ReplaceInheritanceWithDelegation {
            file,
            sub_type,
            base_type,
            field_name,
            methods,
            verify,
            apply,
            force,
        } => {
            let cwd = env::current_dir().context("Failed to get current working directory")?;
            let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
            let mut args = serde_json::json!({
                "path": file.to_string_lossy(),
                "sub_type": sub_type,
                "apply": apply,
                "force": force,
            });
            if let Some(b) = base_type {
                args["base_type"] = serde_json::Value::String(b);
            }
            if let Some(f) = field_name {
                args["field_name"] = serde_json::Value::String(f);
            }
            if !methods.is_empty() {
                args["methods"] = serde_json::Value::Array(
                    methods.into_iter().map(serde_json::Value::String).collect(),
                );
            }
            if let Some(v) = verify {
                args["verify"] = serde_json::Value::String(v);
            }
            let result =
                prod_code_mcp::tools::execute_tool(remote, &root, "code_replace_inheritance_with_delegation", args)
                    .await?;
            for content in &result.content {
                let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
                println!("{text}");
            }
            if result.is_error {
                std::process::exit(1);
            }
            Ok(())
        }
        Commands::ReplaceConditionalWithPolymorphism {
            file,
            line,
            character,
            base_name,
            method_name,
            params,
            return_type,
            target_var,
            verify,
            apply,
            force,
        } => {
            let cwd = env::current_dir().context("Failed to get current working directory")?;
            let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
            let mut args = serde_json::json!({
                "path": file.to_string_lossy(),
                "base_name": base_name,
                "method_name": method_name,
                "line": line,
                "character": character,
                "apply": apply,
                "force": force,
            });
            if !params.is_empty() {
                args["params"] = serde_json::Value::Array(
                    params.into_iter().map(serde_json::Value::String).collect(),
                );
            }
            if let Some(r) = return_type {
                args["return_type"] = serde_json::Value::String(r);
            }
            if let Some(t) = target_var {
                args["target_var"] = serde_json::Value::String(t);
            }
            if let Some(v) = verify {
                args["verify"] = serde_json::Value::String(v);
            }
            let result =
                prod_code_mcp::tools::execute_tool(remote, &root, "code_replace_conditional_with_polymorphism", args)
                    .await?;
            for content in &result.content {
                let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
                println!("{text}");
            }
            if result.is_error {
                std::process::exit(1);
            }
            Ok(())
        }
        Commands::ExtractInterface {
            file,
            symbol,
            name,
            methods,
            line,
            character,
            no_migrate_callers,
            verify,
            apply,
            force,
        } => {
            let cwd = env::current_dir().context("Failed to get current working directory")?;
            let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
            let mut args = serde_json::json!({
                "path": file.to_string_lossy(),
                "symbol": symbol,
                "interface_name": name,
                "line": line,
                "character": character,
                "migrate_callers": !no_migrate_callers,
                "apply": apply,
                "force": force,
            });
            if !methods.is_empty() {
                args["methods"] = serde_json::Value::Array(
                    methods.into_iter().map(serde_json::Value::String).collect(),
                );
            }
            if let Some(v) = verify {
                args["verify"] = serde_json::Value::String(v);
            }
            let result =
                prod_code_mcp::tools::execute_tool(remote, &root, "code_extract_interface", args)
                    .await?;
            for content in &result.content {
                let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
                println!("{text}");
            }
            if result.is_error {
                std::process::exit(1);
            }
            Ok(())
        }
        Commands::ExtractField {
            file,
            line,
            character,
            to,
            range,
            expression,
            name,
            ty,
            init,
            replace_all,
            verify,
            apply,
            force,
        } => {
            let cwd = env::current_dir().context("Failed to get current working directory")?;
            let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
            let mut args = serde_json::json!({
                "path": file.to_string_lossy(),
                "name": name,
                "replace_all": replace_all,
                "apply": apply,
                "force": force,
            });
            if let Some(l) = line {
                args["line"] = serde_json::Value::Number(l.into());
            }
            if let Some(c) = character {
                args["character"] = serde_json::Value::Number(c.into());
            }
            if let Some(to_pos) = to {
                let (end_line, end_character): (u32, u32) = to_pos
                    .split_once(':')
                    .and_then(|(l, c)| Some((l.trim().parse().ok()?, c.trim().parse().ok()?)))
                    .context("--to takes LINE:COL, for example --to 42:31")?;
                args["end_line"] = serde_json::Value::Number(end_line.into());
                args["end_character"] = serde_json::Value::Number(end_character.into());
            }
            if let Some(r) = range {
                args["range"] = serde_json::Value::String(r);
            }
            if let Some(expr) = expression {
                args["expression"] = serde_json::Value::String(expr);
            }
            if let Some(t) = ty {
                args["type"] = serde_json::Value::String(t);
            }
            if let Some(init) = init {
                args["init"] = serde_json::Value::String(init);
            }
            if let Some(verify) = verify {
                args["verify"] = serde_json::Value::String(verify);
            }
            let result =
                prod_code_mcp::tools::execute_tool(remote, &root, "code_extract_field", args)
                    .await?;
            for content in &result.content {
                let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
                println!("{text}");
            }
            if result.is_error {
                std::process::exit(1);
            }
            Ok(())
        }
        Commands::ExtractParameter {
            file,
            line,
            character,
            to,
            name,
            ty,
            replace_all,
            verify,
            apply,
            force,
        } => {
            run_extract_parameter_cli(
                remote,
                &file,
                line,
                character,
                &to,
                name,
                ty,
                replace_all,
                verify,
                apply,
                force,
            )
            .await
        }
        Commands::ParameterObject {
            symbol,
            params,
            name,
            binding,
            path,
            verify,
            apply,
            force,
        } => {
            run_parameter_object_cli(
                remote, symbol, params, name, binding, path, verify, apply, force,
            )
            .await
        }
        Commands::Move {
            symbol,
            to,
            path,
            verify,
            apply,
            force,
        } => run_move_cli(remote, symbol, to, path, verify, apply, force).await,
        Commands::MoveMethod {
            file,
            line,
            col,
            to_param,
            to_type,
            apply,
            force,
        } => {
            let abs = std::fs::canonicalize(&file).unwrap_or(file);
            let mut args = serde_json::json!({
                "path": abs.to_string_lossy(),
                "line": line,
                "character": col,
                "apply": apply,
                "force": force,
            });
            if let Some(to_param) = to_param {
                args["to_param"] = serde_json::Value::String(to_param);
            }
            if let Some(to_type) = to_type {
                args["to_type"] = serde_json::Value::String(to_type);
            }
            run_tool(remote, "code_move_method", args).await
        }
        Commands::MoveModule {
            file,
            to,
            verify,
            apply,
            force,
        } => {
            let abs = std::fs::canonicalize(&file).unwrap_or(file);
            let to = if to.is_absolute() {
                to
            } else {
                std::env::current_dir()?.join(to)
            };
            let mut args = serde_json::json!({
                "path": abs.to_string_lossy(),
                "to": to.to_string_lossy(),
                "apply": apply,
                "force": force,
            });
            if let Some(verify) = verify {
                args["verify"] = serde_json::Value::String(verify);
            }
            run_tool(remote, "code_move_module", args).await
        }
        Commands::ChangeSignature {
            symbol,
            params,
            remove_all: _,
            returns,
            visibility,
            asyncness,
            path,
            verify,
            apply,
            force,
        } => {
            run_change_signature_cli(
                remote, symbol, params, returns, visibility, asyncness, path, verify, apply, force,
            )
            .await
        }
        Commands::Codemod { rule, path, apply } => run_codemod_cli(remote, rule, path, apply).await,
        Commands::Search { query, limit, path } => run_search_cli(remote, query, limit, path).await,
        Commands::Slice {
            target,
            line,
            character,
            depth,
            max_bytes,
            dataflow,
            target_line,
            target_var,
        } => {
            let options = prod_code_mcp::slice::SliceOptions {
                depth,
                max_bytes,
                dataflow,
                target_line,
                target_var,
            };
            run_slice(remote, target, line, character, options).await
        }
        Commands::ShadowRun {
            spec,
            timeout_secs,
            parallel,
            apply,
            command,
        } => run_shadow_cli(remote, spec, timeout_secs, parallel, apply, command).await,
        Commands::Bench {
            workspaces,
            concurrency,
            depth,
            duration_secs,
        } => run_benchmark(remote, workspaces, concurrency, depth, duration_secs).await,
        Commands::DivergentBench {
            base_repo,
            workdir,
            workers,
            queries_per_worker,
            keep_workdir,
            mode,
            persistent,
            churn,
            worktrees,
        } => {
            run_divergent_bench(DivergentBenchConfig {
                remote,
                base_repo,
                workdir,
                workers,
                queries_per_worker,
                keep_workdir,
                mode,
                persistent,
                churn_percent: churn,
                worktrees,
            })
            .await
        }
        Commands::Dependencies { scope, path, json } => {
            let cwd = env::current_dir().context("Failed to get current working directory")?;
            let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
            let mut args = serde_json::json!({
                "scope": scope,
            });
            if let Some(p) = path.as_ref() {
                args["path"] = serde_json::Value::String(p.to_string_lossy().into_owned());
            }
            if json {
                let dep_scope = match scope.as_str() {
                    "modules" => prod_code_mcp::dependencies::DependencyScope::Modules,
                    _ => prod_code_mcp::dependencies::DependencyScope::Crates,
                };
                let report = prod_code_mcp::dependencies::analyze_dependencies(&root, dep_scope, path.as_deref())?;
                println!("{}", serde_json::to_string_pretty(&report)?);
                Ok(())
            } else {
                let result = prod_code_mcp::tools::execute_tool(remote, &root, "code_dependencies", args).await?;
                for content in &result.content {
                    let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
                    println!("{text}");
                }
                if result.is_error {
                    std::process::exit(1);
                }
                Ok(())
            }
        }
        Commands::Duplicates {
            min_lines,
            parameterized,
            max_groups,
            path,
            json,
        } => {
            let cwd = env::current_dir().context("Failed to get current working directory")?;
            let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
            let mut args = serde_json::json!({
                "min_lines": min_lines,
                "parameterized": parameterized,
                "max_groups": max_groups,
            });
            if let Some(p) = path.as_ref() {
                args["path"] = serde_json::Value::String(p.to_string_lossy().into_owned());
            }
            if json {
                let options = prod_code_mcp::duplicates::DuplicateOptions {
                    min_lines,
                    parameterized,
                    max_groups,
                };
                let report = prod_code_mcp::duplicates::find_duplicates(&root, path.as_deref(), options)?;
                println!("{}", serde_json::to_string_pretty(&report)?);
                Ok(())
            } else {
                let result = prod_code_mcp::tools::execute_tool(remote, &root, "code_find_duplicates", args).await?;
                for content in &result.content {
                    let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
                    println!("{text}");
                }
                if result.is_error {
                    std::process::exit(1);
                }
                Ok(())
            }
        }
        Commands::StructuralSearch {
            pattern,
            path,
            json,
        } => {
            let cwd = env::current_dir().context("Failed to get current working directory")?;
            let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
            let mut args = serde_json::json!({
                "pattern": pattern,
            });
            if let Some(p) = path.as_ref() {
                args["path"] = serde_json::Value::String(p.to_string_lossy().into_owned());
            }
            if json {
                let report = prod_code_mcp::codemod::run_structural_search(&root, &pattern, path.as_deref())?;
                println!("{}", serde_json::to_string_pretty(&report)?);
                Ok(())
            } else {
                let result = prod_code_mcp::tools::execute_tool(remote, &root, "code_structural_search", args).await?;
                for content in &result.content {
                    let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
                    println!("{text}");
                }
                if result.is_error {
                    std::process::exit(1);
                }
                Ok(())
            }
        }
        Commands::ProposeExpression {
            file,
            line,
            target_type,
            json,
        } => {
            let cwd = env::current_dir().context("Failed to get current working directory")?;
            let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
            let abs = if file.is_absolute() {
                file
            } else {
                cwd.join(file)
            };
            let args = serde_json::json!({
                "path": abs.to_string_lossy(),
                "line": line,
                "target_type": target_type,
            });
            if json {
                let report = prod_code_mcp::expression_synthesis::propose_expressions_in_scope(
                    &root,
                    &abs.to_string_lossy(),
                    line,
                    &target_type,
                )?;
                println!("{}", serde_json::to_string_pretty(&report)?);
                Ok(())
            } else {
                let result = prod_code_mcp::tools::execute_tool(remote, &root, "code_propose_expression", args).await?;
                for content in &result.content {
                    let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
                    println!("{text}");
                }
                if result.is_error {
                    std::process::exit(1);
                }
                Ok(())
            }
        }
        Commands::Update { check, force, tag } => update::run_update(check, force, tag).await,
        Commands::Package { .. } => unreachable!(),
    }
}

/// Dynamically detect the base repository name if current directory is a git worktree or repository.
pub fn detect_workspace_name(dir: &Path) -> Option<String> {
    // Every git worktree gets its own isolated server workspace; see
    // `prod_code_mcp::sync::workspace_identity`.
    Some(prod_code_mcp::sync::workspace_identity(dir).name)
}

/// Find the root of the workspace or git worktree containing the specified file.
pub fn find_workspace_root(file_path: &Path) -> Option<PathBuf> {
    let abs_path = if file_path.is_absolute() {
        std::fs::canonicalize(file_path).unwrap_or_else(|_| file_path.to_path_buf())
    } else if let Ok(cwd) = env::current_dir() {
        let joined = cwd.join(file_path);
        std::fs::canonicalize(&joined).unwrap_or(joined)
    } else {
        file_path.to_path_buf()
    };

    let mut current = if abs_path.is_file() {
        abs_path.parent()?
    } else {
        abs_path.as_path()
    };

    let mut candidate_manifest = None;

    loop {
        if current.join(".git").exists() {
            return Some(current.to_path_buf());
        }
        if candidate_manifest.is_none()
            && (current.join("Cargo.toml").exists()
                || current.join("go.mod").exists()
                || current.join("package.json").exists()
                || current.join("pyproject.toml").exists()
                || current.join("Package.swift").exists())
        {
            candidate_manifest = Some(current.to_path_buf());
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => break,
        }
    }

    candidate_manifest
}

/// Helper to connect, initialize, and execute a targeted LSP request against the remote gateway.
/// Phase timings for one part of an invocation, printed to stderr when `PROD_CODE_TIMING=1`.
/// `label` says which part, so the work before a query is reported separately from the query.
struct QueryTiming {
    enabled: bool,
    label: &'static str,
    start: std::time::Instant,
    last: std::time::Instant,
    phases: Vec<(&'static str, f64)>,
}

impl QueryTiming {
    fn new() -> Self {
        Self::labelled("query")
    }

    fn labelled(label: &'static str) -> Self {
        let now = std::time::Instant::now();
        Self {
            enabled: env::var_os("PROD_CODE_TIMING").is_some(),
            label,
            start: now,
            last: now,
            phases: Vec::new(),
        }
    }

    fn mark(&mut self, phase: &'static str) {
        if !self.enabled {
            return;
        }
        let now = std::time::Instant::now();
        self.phases
            .push((phase, (now - self.last).as_secs_f64() * 1000.0));
        self.last = now;
    }

    fn report(&self) {
        if !self.enabled {
            return;
        }
        let total = self.start.elapsed().as_secs_f64() * 1000.0;
        let parts: Vec<String> = self
            .phases
            .iter()
            .map(|(name, ms)| format!("{name}={ms:.1}ms"))
            .collect();
        let label = self.label;
        eprintln!("[timing] {label} total={total:.1}ms {}", parts.join(" "));
    }
}

async fn execute_lsp_query(
    mut remote: SocketAddr,
    file_path: &Path,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    let cwd = env::current_dir().context("Failed to determine current working directory")?;

    let abs_path = if file_path.is_absolute() {
        std::fs::canonicalize(file_path).unwrap_or_else(|_| file_path.to_path_buf())
    } else {
        let joined = cwd.join(file_path);
        std::fs::canonicalize(&joined).unwrap_or(joined)
    };

    let ws_root = find_workspace_root(&abs_path).unwrap_or_else(|| cwd.clone());
    let ws_root_str = ws_root.to_string_lossy().to_string();
    let base_ws_name = detect_workspace_name(&ws_root);

    let file_uri = Url::from_file_path(&abs_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", abs_path))?
        .to_string();

    let file_content = tokio::fs::read_to_string(&abs_path)
        .await
        .with_context(|| format!("Failed to read file {:?}", abs_path))?;

    let mut timing = QueryTiming::new();
    let mut framed = {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let stream = prod_code_protocol::transport::connect(remote)
                .await
                .with_context(|| format!("Failed to connect to remote gateway at {remote}"))?;
            let mut framed = Framed::new(stream, ProdCodeCodec::new());
            timing.mark("connect");

            // 1. Handshake
            // 1a. Transparent pre-flight sync before the handshake: manifest probe on first
            // contact (seeded from the origin repository's copy), watermark delta afterwards.
            let identity = prod_code_mcp::sync::workspace_identity(&ws_root);
            if let Err(e) =
                prod_code_mcp::sync::push_workspace_sync(&mut framed, &ws_root, &identity, None)
                    .await
            {
                tracing::warn!(error = %e, "pre-flight workspace sync failed");
            }
            timing.mark("preflight_sync");

            let (engine_subpath, expected_engine) =
                prod_code_mcp::sync::engine_project(&ws_root, &abs_path);
            let supported_versions = supported_protocol_versions();
            framed
                .send(WireMessage::HandshakeRequest(HandshakeRequest {
                    protocol_version: PROTOCOL_VERSION,
                    supported_versions: Some(supported_versions.clone()),
                    capabilities: None,
                    client_name: "prod-code-cli".to_string(),
                    client_pid: std::process::id(),
                    auth_token: None,
                    client_workspace_root: ws_root_str.clone(),
                    preferred_engine: engine_subpath
                        .as_ref()
                        .and(expected_engine)
                        .map(str::to_string),
                    base_workspace_name: base_ws_name.clone(),
                    engine_subpath,
                    client_agent: Some(prod_code_protocol::detect_client_agent()),
                    client_host: Some(prod_code_protocol::client_host()),
                    purpose: None,
                    redirect_count: (attempt - 1) as u32,
                }))
                .await?;

            let handshake = match framed.next().await {
                Some(Ok(WireMessage::HandshakeResponse(resp))) => resp,
                Some(Ok(WireMessage::Redirect { target_addr, reason })) => {
                    if attempt > 3 {
                        anyhow::bail!("too many gateway redirects: {reason:?}");
                    }
                    tracing::info!(%target_addr, ?reason, "received transparent redirect from gateway");
                    if let Ok(addr) = target_addr.parse::<SocketAddr>() {
                        remote = addr;
                        continue;
                    } else {
                        anyhow::bail!("invalid redirect target address: {target_addr}");
                    }
                }
                other => anyhow::bail!("Unexpected handshake response: {:?}", other),
            };
            validate_selected_protocol_version(handshake.protocol_version, &supported_versions)
                .context("gateway returned an incompatible handshake response")?;

            // Self-heal: the gateway keyed this workspace on an empty or reset directory (its
            // detected engine does not match our manifest) while our watermark still claims
            // everything was sent. Forget the watermark, push the full tree and reconnect;
            // the gateway reloads a workspace whose engine kind changed.
            if attempt == 1
                && let Some(expected) = expected_engine
                && handshake.detected_engine != expected
            {
                prod_code_mcp::sync::clear_sync_cache(&ws_root);
                let _ = framed
                    .send(WireMessage::Disconnect {
                        reason: "engine mismatch; resyncing workspace".to_string(),
                    })
                    .await;
                continue;
            }
            // Files the gateway lost from its copy (#262) that the pre-flight sync did not
            // carry: the next sync sends them again.
            prod_code_mcp::sync::resend_lost_files(
                &ws_root,
                &prod_code_mcp::sync::gateway_node(&framed),
                &handshake.stale_paths,
            );
            timing.mark("handshake");
            break framed;
        }
    };

    // 2. LSP Initialize
    let folder_name = ws_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("workspace");
    let init_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "processId": null,
            "rootUri": format!("file://{}", ws_root.to_string_lossy()),
            "workspaceFolders": [
                {
                    "name": folder_name,
                    "uri": format!("file://{}", ws_root.to_string_lossy())
                }
            ],
            "capabilities": {
                "workspace": {
                    "workspaceFolders": true,
                    "configuration": true
                },
                "textDocument": {
                    "hover": {
                        "contentFormat": ["markdown", "plaintext"]
                    },
                    "definition": {
                        "linkSupport": true
                    },
                    "documentSymbol": {
                        "hierarchicalDocumentSymbolSupport": true
                    },
                    "references": {}
                }
            }
        }
    });
    framed
        .send(WireMessage::LspPayload(init_req.to_string()))
        .await?;

    // Await init response (matching id: 1)
    let init_deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(10);
    while tokio::time::Instant::now() < init_deadline {
        let remaining = init_deadline - tokio::time::Instant::now();
        match tokio::time::timeout(remaining, framed.next()).await {
            Ok(Some(Ok(WireMessage::LspPayload(resp_json)))) => {
                // An answer has no method: a server's own request may carry id 1 too (#391).
                if serde_json::from_str::<serde_json::Value>(&resp_json)
                    .ok()
                    .filter(|val| val.get("method").is_none())
                    .and_then(|val| val.get("id").and_then(|id| id.as_i64()))
                    == Some(1)
                {
                    break;
                }
            }
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(e))) => anyhow::bail!("Frame decode error during initialize: {}", e),
            Ok(None) => anyhow::bail!("Server closed connection during initialize"),
            Err(_) => anyhow::bail!("Timeout waiting for initialize response"),
        }
    }

    // 3. LSP Initialized notification
    timing.mark("initialize");
    let initialized = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "initialized",
        "params": {}
    });
    framed
        .send(WireMessage::LspPayload(initialized.to_string()))
        .await?;

    // 4. LSP didOpen notification
    let language_id = prod_code_mcp::lang::language_id_for_path(&abs_path);
    let did_open = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": file_uri,
                "languageId": language_id,
                "version": 1,
                "text": file_content
            }
        }
    });
    framed
        .send(WireMessage::LspPayload(did_open.to_string()))
        .await?;
    timing.mark("did_open_sent");

    // 5. Send targeted query with id = 2
    let query_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": method,
        "params": params
    });
    framed
        .send(WireMessage::LspPayload(query_req.to_string()))
        .await?;

    // 6. Read response matching id = 2
    let query_deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(60);
    while tokio::time::Instant::now() < query_deadline {
        let remaining = query_deadline - tokio::time::Instant::now();
        match tokio::time::timeout(remaining, framed.next()).await {
            Ok(Some(Ok(WireMessage::LspPayload(resp_json)))) => {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&resp_json)
                    .map_err(|_| ())
                    .and_then(|v| {
                        if v.get("method").is_none()
                            && v.get("id").and_then(|id| id.as_i64()) == Some(2)
                        {
                            Ok(v)
                        } else {
                            Err(())
                        }
                    })
                {
                    timing.mark("query_response");
                    timing.report();
                    let did_close = serde_json::json!({
                        "jsonrpc": "2.0",
                        "method": "textDocument/didClose",
                        "params": {
                            "textDocument": {
                                "uri": file_uri
                            }
                        }
                    });
                    let _ = framed
                        .send(WireMessage::LspPayload(did_close.to_string()))
                        .await;
                    let _ = framed
                        .send(WireMessage::Disconnect {
                            reason: "query finished".to_string(),
                        })
                        .await;
                    if let Some(err) = val.get("error") {
                        let message = err
                            .get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("unknown error");
                        anyhow::bail!("{method} failed: {message}");
                    }
                    return Ok(val
                        .get("result")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null));
                }
            }
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(e))) => anyhow::bail!("Frame decode error: {}", e),
            Ok(None) => anyhow::bail!("Remote closed connection prematurely"),
            Err(_) => anyhow::bail!("Timeout waiting for LSP response to {}", method),
        }
    }

    anyhow::bail!("No response received for query {}", method)
}

async fn run_hover(remote: SocketAddr, file: &Path, line: u32, col: u32) -> Result<()> {
    let abs_path = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let file_uri = Url::from_file_path(&abs_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path"))?
        .to_string();

    let lsp_line = line.saturating_sub(1);
    let lsp_col = col.saturating_sub(1);

    let params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "position": { "line": lsp_line, "character": lsp_col }
    });

    let result = execute_lsp_query(remote, file, "textDocument/hover", params).await?;

    if let Some(contents) = result.get("contents") {
        if let Some(value) = contents.get("value").and_then(|v| v.as_str()) {
            println!("{value}");
            return Ok(());
        } else if let Some(arr) = contents.as_array() {
            for item in arr {
                if let Some(v) = item.get("value").and_then(|v| v.as_str()) {
                    println!("{v}");
                }
            }
            return Ok(());
        }
    }

    println!("{:#}", result);
    Ok(())
}

async fn run_definition(remote: SocketAddr, file: &Path, line: u32, col: u32) -> Result<()> {
    let abs_path = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let file_uri = Url::from_file_path(&abs_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path"))?
        .to_string();

    let lsp_line = line.saturating_sub(1);
    let lsp_col = col.saturating_sub(1);

    let params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "position": { "line": lsp_line, "character": lsp_col }
    });

    let result = execute_lsp_query(remote, file, "textDocument/definition", params).await?;
    let ws_root = find_workspace_root(&abs_path).unwrap_or_else(|| abs_path.clone());
    let mut shown = 0;

    if let Some(arr) = result.as_array() {
        if arr.is_empty() {
            println!("No definition found.");
        } else {
            for loc in arr {
                let uri = loc
                    .get("uri")
                    .or_else(|| loc.get("targetUri"))
                    .and_then(|u| u.as_str())
                    .unwrap_or("");
                let range = loc.get("range").or_else(|| loc.get("targetSelectionRange"));
                let start_line = range
                    .and_then(|r| r.get("start"))
                    .and_then(|s| s.get("line"))
                    .and_then(|l| l.as_u64())
                    .unwrap_or(0)
                    + 1;
                let start_col = range
                    .and_then(|r| r.get("start"))
                    .and_then(|s| s.get("character"))
                    .and_then(|c| c.as_u64())
                    .unwrap_or(0)
                    + 1;
                println!("📍 Definition: {uri}:{start_line}:{start_col}");
                // A definition outside the checkout lives only on the gateway: show it.
                let path = prod_code_mcp::remote_fs::uri_to_path(uri);
                if prod_code_mcp::remote_fs::is_external(&ws_root, &path) && shown < 3 {
                    shown += 1;
                    match prod_code_mcp::remote_fs::read_remote_file(remote, &path, 0).await {
                        Ok((bytes, _)) => {
                            let text = String::from_utf8_lossy(&bytes);
                            print!(
                                "{}",
                                prod_code_mcp::remote_fs::snippet(&text, start_line as u32, 8)
                            );
                        }
                        Err(e) => println!("   (external source not readable: {e})"),
                    }
                }
            }
        }
    } else if let Some(obj) = result.as_object() {
        let uri = obj.get("uri").and_then(|u| u.as_str()).unwrap_or("");
        let start_line = obj
            .get("range")
            .and_then(|r| r.get("start"))
            .and_then(|s| s.get("line"))
            .and_then(|l| l.as_u64())
            .unwrap_or(0)
            + 1;
        let start_col = obj
            .get("range")
            .and_then(|r| r.get("start"))
            .and_then(|s| s.get("character"))
            .and_then(|c| c.as_u64())
            .unwrap_or(0)
            + 1;
        println!("📍 Definition: {uri}:{start_line}:{start_col}");
    } else {
        println!("{:#}", result);
    }

    Ok(())
}

/// Impact analysis of the working tree (or of the commits since `base`), optionally running
/// the affected tests on the gateway.
async fn run_impact(
    remote: SocketAddr,
    base: Option<&str>,
    depth: usize,
    run: bool,
    ci: bool,
    json: bool,
) -> Result<()> {
    use std::io::Write;
    let cwd = env::current_dir()?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let started = std::time::Instant::now();
    let report = prod_code_mcp::impact::analyze(remote, &root, base, depth).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.render());
        eprintln!(
            "[prod-code impact] analysed in {:.2}s",
            started.elapsed().as_secs_f64()
        );
    }
    // CI runs the selection only when it can be trusted, the whole suite otherwise (#201, #434).
    let (command, why) = if ci {
        let decision = report.ci_decision();
        let command = match decision.run {
            // Running nothing in place of a whole suite would pass a change nobody tested.
            prod_code_mcp::impact::CiRun::WholeSuite => Some(
                prod_code_mcp::verify::plan_command_with(
                    &prod_code_mcp::verify::detect_tools(&root),
                    &report.language,
                    prod_code_mcp::verify::VerifyKind::Test,
                    None,
                )
                .map_err(|e| {
                    anyhow::anyhow!(
                        "impact --ci has to run {}, but there is no test command for {}: {e:#}",
                        decision.why,
                        report.language
                    )
                })?,
            ),
            prod_code_mcp::impact::CiRun::Selected(selected) => Some(selected),
            prod_code_mcp::impact::CiRun::Nothing => None,
        };
        (command, decision.why)
    } else {
        (report.test_command.clone(), String::new())
    };
    if ci {
        println!("[prod-code impact --ci] {why}");
        if let Ok(path) = env::var("GITHUB_STEP_SUMMARY") {
            use std::io::Write as _;
            let summary = report.ci_summary(command.as_deref(), &why);
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                let _ = f.write_all(summary.as_bytes());
            }
        }
        if command.is_none() {
            return Ok(());
        }
    }
    if run || ci {
        let Some(command) = command else {
            println!("nothing to run");
            return Ok(());
        };
        println!("$ {}", command.join(" "));
        let outcome = prod_code_mcp::exec::run_remote(
            remote,
            &root,
            None,
            command,
            vec![("CARGO_TERM_COLOR".to_string(), "never".to_string())],
            0,
            false,
            |is_stderr, data| {
                if is_stderr {
                    let _ = std::io::stderr().write_all(data);
                } else {
                    let _ = std::io::stdout().write_all(data);
                }
            },
        )
        .await?;
        std::process::exit(outcome.exit.exit_code.unwrap_or(1));
    }
    Ok(())
}

/// Usage metrics merged across every node of the cluster.
async fn run_metrics(nodes: &[SocketAddr], since: u64, json: bool) -> Result<()> {
    let mut all = Vec::new();
    for node in nodes {
        match prod_code_mcp::cluster::node_metrics(*node, since).await {
            Ok(m) => all.push(m),
            Err(e) => eprintln!("{node}: {e}"),
        }
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&all)?);
        return Ok(());
    }
    let window = if since == 0 {
        "all in memory".to_string()
    } else {
        format!("last {}h{:02}m", since / 3600, (since % 3600) / 60)
    };
    println!("⚡ prod-code usage ({window}, {} node(s))", all.len());
    println!("────────────────────────────────────────────────────────────────────────");
    // Queries: by agent → workspace → method, summed over nodes.
    type Key = (String, String, String, String);
    type Agg = (u64, u64, u64, u64);
    let mut by_key: std::collections::BTreeMap<Key, Agg> = std::collections::BTreeMap::new();
    for m in &all {
        for q in &m.queries {
            let e = by_key
                .entry((
                    q.agent.clone(),
                    q.host.clone(),
                    q.workspace.clone(),
                    q.method.clone(),
                ))
                .or_default();
            e.0 += q.count;
            e.1 += q.errors;
            e.2 = e.2.max(q.p50_ms);
            e.3 = e.3.max(q.p95_ms);
        }
    }
    if by_key.is_empty() {
        println!("no queries in the window");
    } else {
        println!(
            "{:<12} {:<18} {:<28} {:<34} {:>7} {:>5} {:>7} {:>7}",
            "agent", "host", "workspace", "method", "count", "err", "p50ms", "p95ms"
        );
        for ((agent, host, ws, method), (count, errors, p50, p95)) in &by_key {
            println!(
                "{:<12} {:<18} {:<28} {:<34} {:>7} {:>5} {:>7} {:>7}",
                truncate(agent, 12),
                truncate(host, 18),
                truncate(ws, 28),
                truncate(method.trim_start_matches("textDocument/"), 34),
                count,
                errors,
                p50,
                p95
            );
        }
    }
    let mut execs: Vec<_> = all.iter().flat_map(|m| m.execs.iter().cloned()).collect();
    if !execs.is_empty() {
        execs.sort_by_key(|e| std::cmp::Reverse(e.total_ms));
        println!("────────────────────────────────────────────────────────────────────────");
        println!(
            "{:<12} {:<18} {:<28} {:<40} {:>5} {:>4} {:>8}",
            "agent", "host", "workspace", "command", "runs", "fail", "total s"
        );
        for e in execs.iter().take(30) {
            println!(
                "{:<12} {:<18} {:<28} {:<40} {:>5} {:>4} {:>8.1}",
                truncate(&e.agent, 12),
                truncate(&e.host, 18),
                truncate(&e.workspace, 28),
                truncate(&e.command, 40),
                e.count,
                e.failures,
                e.total_ms as f64 / 1000.0
            );
        }
    }
    println!("────────────────────────────────────────────────────────────────────────");
    for m in &all {
        println!(
            "{:<22} syncs {:>5}  files {:>7}  {:>8.1} MB  events in memory {}",
            m.node,
            m.sync_rounds,
            m.sync_files,
            m.sync_bytes as f64 / 1_048_576.0,
            m.events_in_memory
        );
    }
    Ok(())
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let cut: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

/// Runs the tests and prints a dossier for every failure.
async fn run_diagnose(
    remote: SocketAddr,
    filter: Option<&str>,
    timeout_secs: u64,
    json: bool,
) -> Result<()> {
    let cwd = env::current_dir()?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let hint = if cwd != root { Some(cwd.as_path()) } else { None };
    let report = prod_code_mcp::dossier::diagnose(remote, &root, hint, filter, timeout_secs).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.render());
    }
    if report.tests_failed > 0 || !report.build_errors.is_empty() {
        std::process::exit(1);
    }
    Ok(())
}

/// Diagnostics of a file as it is, or as it would be with `proposed` content (nothing is
/// written). Exits 1 when there are errors.
/// The position a command was given, when it was not given `--symbol`.
fn position(
    file: Option<PathBuf>,
    line: Option<u32>,
    col: Option<u32>,
) -> Result<(PathBuf, u32, u32)> {
    match (file, line, col) {
        (Some(file), Some(line), Some(col)) => Ok((file, line, col)),
        _ => anyhow::bail!("give <file> <line> <col>, or --symbol NAME"),
    }
}

/// Runs one MCP tool from the current checkout and prints what it says; exit 1 on an error.
async fn run_tool(remote: SocketAddr, tool: &str, args: serde_json::Value) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let mut remote = remote;
    let mut result = prod_code_mcp::tools::execute_tool(remote, &root, tool, args.clone()).await;
    if let Err(ref e) = result
        && prod_code_mcp::is_retryable_connection_error(tool, e)
    {
        if let Some(new_addr) = prod_code_mcp::rediscover_node(remote, &root).await {
            remote = new_addr;
            let identity = prod_code_mcp::sync::workspace_identity(&root);
            let name = identity.base.unwrap_or(identity.name);
            prod_code_mcp::cluster::remember_placement(&name, new_addr);
            result = prod_code_mcp::tools::execute_tool(remote, &root, tool, args).await;
        }
    }
    let result = result?;
    for content in &result.content {
        let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
        println!("{text}");
    }
    if result.is_error {
        std::process::exit(1);
    }
    Ok(())
}

/// Runs `code_references` tool from the current checkout and prints references; exit 1 on error or no references.
async fn run_refs(remote: SocketAddr, args: serde_json::Value) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let mut remote = remote;
    let mut result = prod_code_mcp::tools::execute_tool(remote, &root, "code_references", args.clone()).await;
    if let Err(ref e) = result
        && prod_code_mcp::is_retryable_connection_error("code_references", e)
    {
        if let Some(new_addr) = prod_code_mcp::rediscover_node(remote, &root).await {
            remote = new_addr;
            let identity = prod_code_mcp::sync::workspace_identity(&root);
            let name = identity.base.unwrap_or(identity.name);
            prod_code_mcp::cluster::remember_placement(&name, new_addr);
            result = prod_code_mcp::tools::execute_tool(remote, &root, "code_references", args).await;
        }
    }
    let result = result?;
    let mut has_refs = false;
    for content in &result.content {
        let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
        if text.contains("Found ") && text.contains(" reference(s)") {
            has_refs = true;
        }
        println!("{text}");
    }
    if result.is_error || !has_refs {
        std::process::exit(1);
    }
    Ok(())
}

/// `callers` / `callees`: the call hierarchy to `depth` levels, at a position or of `--symbol`,
/// answered by the same code as the MCP tools.
async fn run_call_tree(
    remote: SocketAddr,
    tool: &str,
    file: Option<PathBuf>,
    line: Option<u32>,
    col: Option<u32>,
    symbol: Option<String>,
    depth: usize,
) -> Result<()> {
    let args = match symbol {
        Some(symbol) => {
            let mut args = symbol_args(&symbol, file);
            args["depth"] = serde_json::json!(depth);
            args
        }
        None => {
            let (file, line, col) = position(file, line, col)?;
            let file = std::fs::canonicalize(&file).unwrap_or(file);
            serde_json::json!({
                "path": file.to_string_lossy(),
                "line": line,
                "character": col,
                "depth": depth,
            })
        }
    };
    run_tool(remote, tool, args).await
}

/// A position command given `--symbol`: the MCP tool resolves the name, exactly as for an agent,
/// among the candidates in `file` when one is given.
async fn run_by_symbol(
    remote: SocketAddr,
    tool: &str,
    symbol: &str,
    file: Option<PathBuf>,
) -> Result<()> {
    run_tool(remote, tool, symbol_args(symbol, file)).await
}

/// The MCP arguments of `--symbol NAME [FILE]`.
fn symbol_args(symbol: &str, file: Option<PathBuf>) -> serde_json::Value {
    let mut args = serde_json::json!({ "symbol": symbol });
    if let Some(file) = file {
        let file = std::fs::canonicalize(&file).unwrap_or(file);
        args["path"] = serde_json::json!(file.to_string_lossy());
    }
    args
}

/// `validate --compile`: the proposed files through `code_validate_edits` with `compile: true`,
/// so the project's check command runs on them in a shadow copy on the node (#376).
async fn run_validate_compiled(
    remote: SocketAddr,
    file: &Path,
    text: String,
    with: &[String],
) -> Result<()> {
    let abs = |p: &Path| {
        std::fs::canonicalize(p).unwrap_or_else(|_| {
            env::current_dir()
                .map(|cwd| cwd.join(p))
                .unwrap_or_else(|_| p.to_path_buf())
        })
    };
    let mut edits = vec![serde_json::json!({
        "path": abs(file).to_string_lossy(),
        "new_text": text,
    })];
    for pair in with {
        let (target, from) = pair
            .split_once('=')
            .with_context(|| format!("--with takes FILE=NEW, got `{pair}`"))?;
        let new_text =
            std::fs::read_to_string(from).with_context(|| format!("failed to read {from}"))?;
        edits.push(serde_json::json!({
            "path": abs(Path::new(target)).to_string_lossy(),
            "new_text": new_text,
        }));
    }
    eprintln!("Running the remote compiler check for the proposed changes...");
    run_tool(
        remote,
        "code_validate_edits",
        serde_json::json!({ "edits": edits, "compile": true }),
    )
    .await
}

/// Several proposed files checked together in one overlay, so a change to one is judged against
/// the proposed state of the others (a constant added in one file and imported in another).
async fn run_validate_together(
    remote: SocketAddr,
    file: &Path,
    text: String,
    with: &[String],
    json: bool,
) -> Result<()> {
    let abs = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let first = abs(file);
    let cwd = env::current_dir()?;
    let root = find_workspace_root(&first).unwrap_or(cwd);
    let mut edits = vec![(first, text)];
    for pair in with {
        let (target, from) = pair
            .split_once('=')
            .with_context(|| format!("--with takes FILE=NEW, got `{pair}`"))?;
        let new_text =
            std::fs::read_to_string(from).with_context(|| format!("failed to read {from}"))?;
        edits.push((abs(Path::new(target)), new_text));
    }
    let started = std::time::Instant::now();
    let reports = prod_code_mcp::diagnostics::validate_texts(remote, &root, &edits, &[]).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&reports)?);
    } else {
        for report in &reports {
            print!("{}", report.render());
        }
        eprintln!(
            "[prod-code] {} file(s) analysed together in {:.2}s",
            edits.len(),
            started.elapsed().as_secs_f64()
        );
    }
    if reports.iter().any(|r| !r.ok()) {
        std::process::exit(1);
    }
    Ok(())
}

async fn run_diagnostics(
    remote: SocketAddr,
    file: &Path,
    proposed: Option<String>,
    json: bool,
) -> Result<()> {
    let abs_path = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let cwd = env::current_dir()?;
    let root = find_workspace_root(&abs_path).unwrap_or(cwd);
    let started = std::time::Instant::now();
    let report = match proposed {
        Some(text) => {
            prod_code_mcp::diagnostics::validate_text(remote, &root, &abs_path, &text).await?
        }
        None => prod_code_mcp::diagnostics::diagnostics(remote, &root, &abs_path).await?,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.render());
        eprintln!(
            "[prod-code] analysed in {:.2}s",
            started.elapsed().as_secs_f64()
        );
    }
    if !report.ok() {
        std::process::exit(1);
    }
    Ok(())
}

/// Scans the checkout for unreferenced symbols.
async fn run_dead_code(
    remote: SocketAddr,
    include_exported: bool,
    reachability: bool,
    max_files: usize,
    json: bool,
) -> Result<()> {
    let cwd = env::current_dir()?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let started = std::time::Instant::now();
    let report = prod_code_mcp::dead_code::find_dead_code_opts(
        remote,
        &root,
        prod_code_mcp::dead_code::DeadCodeOptions {
            include_exported,
            max_files,
            reachability,
        },
    )
    .await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.render());
        eprintln!(
            "[prod-code dead-code] scanned in {:.2}s",
            started.elapsed().as_secs_f64()
        );
    }
    Ok(())
}

/// Prints a source file from the workspace or gateway host, optionally a window around one line.
async fn run_source(
    remote: SocketAddr,
    root: Option<&Path>,
    path: &str,
    line: Option<u32>,
    context: u32,
) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = root
        .map(Path::to_path_buf)
        .unwrap_or_else(|| find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone()));
    let (bytes, truncated) = prod_code_mcp::remote_fs::read_source(remote, &root, path).await?;
    let text = String::from_utf8_lossy(&bytes);
    match line {
        Some(line) => print!(
            "{}",
            prod_code_mcp::remote_fs::snippet(&text, line, context)
        ),
        None => print!("{text}"),
    }
    if truncated {
        eprintln!("[prod-code] {path}: output truncated at 2 MiB");
    }
    Ok(())
}

/// Prints `uri:line:col` for every LSP `Location` in `arr`.
fn print_locations(arr: &[serde_json::Value]) {
    for loc in arr {
        let uri = loc.get("uri").and_then(|u| u.as_str()).unwrap_or("");
        let start = loc.get("range").and_then(|r| r.get("start"));
        let line = start
            .and_then(|s| s.get("line"))
            .and_then(|l| l.as_u64())
            .unwrap_or(0)
            + 1;
        let col = start
            .and_then(|s| s.get("character"))
            .and_then(|c| c.as_u64())
            .unwrap_or(0)
            + 1;
        println!("  • {uri}:{line}:{col}");
    }
}

async fn run_implementations(remote: SocketAddr, file: &Path, line: u32, col: u32) -> Result<()> {
    let abs_path = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let file_uri = Url::from_file_path(&abs_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path"))?
        .to_string();
    let params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "position": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) },
    });
    let result = execute_lsp_query(remote, file, "textDocument/implementation", params).await?;
    let arr = match &result {
        serde_json::Value::Array(a) => a.clone(),
        serde_json::Value::Object(_) => vec![result.clone()],
        _ => Vec::new(),
    };
    if arr.is_empty() {
        println!("No implementations found.");
    } else {
        println!("Found {} implementation(s):", arr.len());
        print_locations(&arr);
    }
    Ok(())
}

async fn run_symbols(
    remote: SocketAddr,
    file: &Path,
    options: &prod_code_mcp::tools::OutlineOptions,
) -> Result<()> {
    let abs_path = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    if abs_path.is_dir() {
        let cwd = env::current_dir().context("Failed to determine current working directory")?;
        let ws_root = find_workspace_root(&abs_path).unwrap_or_else(|| cwd.clone());
        let text =
            prod_code_mcp::tools::outline_directory(remote, &ws_root, &abs_path, file, options)
                .await?;
        let has_symbols = text.lines().any(|l| l.trim_start().starts_with('['));
        if !has_symbols {
            eprintln!("no outline symbols found for {}", file.display());
            std::process::exit(1);
        }
        println!("{text}");
        return Ok(());
    }

    // The MCP tool's outline: Markdown headings, an error for a language no server serves,
    // never a bare `null` (#362).
    let cwd = env::current_dir().context("Failed to determine current working directory")?;
    let ws_root = find_workspace_root(&abs_path).unwrap_or(cwd);
    let text = prod_code_mcp::tools::outline_file(
        remote,
        &ws_root,
        &abs_path,
        &file.display().to_string(),
        options,
    )
    .await?;
    let has_symbols = text.lines().any(|l| l.trim_start().starts_with('['));
    if !has_symbols {
        eprintln!("no outline symbols found for {}", file.display());
        std::process::exit(1);
    }
    println!("{text}");
    Ok(())
}

/// The arguments of `report-issue`, as given.
struct ReportArgs {
    title: String,
    body: Option<String>,
    body_file: Option<PathBuf>,
    private_ref: Option<String>,
    force: bool,
    dry_run: bool,
    labels: Vec<String>,
}

/// Files (or drafts) a prod-code bug report. `remote` is where the checkout is placed, when it
/// could be; `unplaced` is why it could not, which then goes into the report (scrubbed, like
/// the rest of it), since a checkout the cluster refuses is itself worth reporting (#307).
async fn run_report_issue(
    remote: Option<std::net::SocketAddr>,
    unplaced: Option<String>,
    args: ReportArgs,
) -> anyhow::Result<()> {
    let mut body = match (args.body, args.body_file) {
        (Some(body), _) => body,
        (None, Some(path)) if path.as_os_str() == "-" => {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)?;
            text
        }
        (None, Some(path)) => std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?,
        (None, None) => anyhow::bail!("give the report a --body or a --body-file"),
    };
    if let Some(reason) = unplaced {
        body.push_str(&format!(
            "\n\nThis checkout could not be placed on a node: {}",
            reason.replace('\n', " ")
        ));
    }
    let outcome = prod_code_mcp::report::report(
        remote,
        ReportRequest {
            title: &args.title,
            body: &body,
            force: args.force,
            dry_run: args.dry_run,
            private_ref: args.private_ref.as_deref(),
            labels: &args.labels,
        },
        &prod_code_mcp::report::gh_program(),
    )
    .await?;
    println!("{}", outcome.render());
    Ok(())
}

/// A gateway's status as one JSON object for scripts (#398): the fields as the gateway sent
/// them, plus the address asked, the round trip, and whether the node is healthy and if not why.
fn status_snapshot(
    remote: SocketAddr,
    rtt: std::time::Duration,
    status: &prod_code_protocol::StatusResponse,
) -> serde_json::Value {
    let mut snapshot = serde_json::to_value(status).unwrap_or_default();
    if let Some(object) = snapshot.as_object_mut() {
        let pressure = status.host.pressure();
        object.insert("remote".into(), serde_json::json!(remote.to_string()));
        object.insert(
            "rtt_ms".into(),
            serde_json::json!(rtt.as_secs_f64() * 1000.0),
        );
        object.insert("healthy".into(), serde_json::json!(pressure.is_none()));
        object.insert("pressure".into(), serde_json::json!(pressure));
    }
    snapshot
}

/// Query remote gateway for health and status snapshot.
async fn run_status_probe(
    remote: SocketAddr,
    json: bool,
    fallback_note: Option<String>,
) -> Result<()> {
    let start = std::time::Instant::now();
    let stream = prod_code_protocol::transport::connect(remote)
        .await
        .with_context(|| format!("Failed to connect to prod-code gateway at {remote}"))?;
    let rtt = start.elapsed();

    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    framed.send(WireMessage::StatusRequest).await?;

    if let Some(msg) = framed.next().await {
        match msg? {
            WireMessage::StatusResponse(resp) if json => {
                let mut snapshot = status_snapshot(remote, rtt, &resp);
                if let Some(note) = &fallback_note {
                    if let Some(obj) = snapshot.as_object_mut() {
                        obj.insert("fallback_transport".to_string(), serde_json::json!(note));
                    }
                }
                println!("{}", serde_json::to_string_pretty(&snapshot)?);
            }
            WireMessage::StatusResponse(resp) => {
                let hours = resp.uptime_seconds / 3600;
                let minutes = (resp.uptime_seconds % 3600) / 60;
                let seconds = resp.uptime_seconds % 60;

                println!("⚡ prod-code Remote Code Intelligence Gateway");
                println!("────────────────────────────────────────────────────");
                let note_suffix = fallback_note
                    .as_deref()
                    .map(|n| format!(" [{n}]"))
                    .unwrap_or_default();
                println!("Remote Address:    {remote} ({:.2?} RTT){note_suffix}", rtt);
                println!("Server PID:        {}", resp.server_pid);
                println!("Uptime:            {}h {}m {}s", hours, minutes, seconds);
                if let Some(mb) = resp.memory_rss_mb() {
                    println!("Memory RSS:        {:.2} MB", mb);
                }
                println!("Active Sessions:   {}", resp.active_sessions);
                println!("Running Commands:  {}", resp.running_commands.len());
                for line in resp.running_lines() {
                    println!("  • {line}");
                }
                println!("Loaded Workspaces: {}", resp.loaded_workspaces);
                println!(
                    "Queries Handled:   {} (in-flight: {})",
                    resp.total_queries, resp.active_queries
                );
                println!("Engines Available: {}", resp.detected_engines.join(", "));
                let host = resp.host.describe();
                if !host.is_empty() {
                    println!("Host Resources:    {host}");
                }
                match resp.host.pressure() {
                    Some(why) => println!(
                        "Status:            SHORT ({why}): new workspaces go to other nodes"
                    ),
                    None => println!("Status:            HEALTHY"),
                }
            }
            // A gateway that requires a token this client did not send says so (#402).
            WireMessage::Disconnect { reason } => {
                anyhow::bail!("the gateway at {remote} closed the connection: {reason}")
            }
            other => anyhow::bail!("Unexpected response from gateway: {:?}", other),
        }
    } else {
        anyhow::bail!("Gateway closed connection without responding");
    }

    Ok(())
}

/// How often the editor bridge looks for local changes to push to the node (#316).
const BRIDGE_SYNC_POLL: std::time::Duration = std::time::Duration::from_millis(500);

/// How long the editor bridge waits after a failed push before it tries again.
const BRIDGE_SYNC_RETRY: std::time::Duration = std::time::Duration::from_secs(5);

/// Pushes the checkout's changes since the last sync on a connection of its own.
async fn push_checkout(remote: SocketAddr, root: &Path) -> Result<()> {
    let stream = prod_code_protocol::transport::connect(remote)
        .await
        .with_context(|| format!("Failed to connect to remote gateway at {remote}"))?;
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    let identity = prod_code_mcp::sync::workspace_identity(root);
    prod_code_mcp::sync::push_workspace_sync(&mut framed, root, &identity, None).await?;
    let _ = framed
        .send(WireMessage::Disconnect {
            reason: "sync finished".to_string(),
        })
        .await;
    Ok(())
}

/// Keeps the node's copy of `root` current while an editor runs `prod-code lsp`: whenever the
/// file watcher saw a change (a save, a checkout, a generated file), the delta is pushed on a
/// connection of its own, so the language server session never waits for it (#316). `pushing`
/// is held for each push, which a save also takes.
async fn keep_checkout_synced(
    remote: SocketAddr,
    root: PathBuf,
    pushing: std::sync::Arc<tokio::sync::Mutex<()>>,
) {
    loop {
        tokio::time::sleep(BRIDGE_SYNC_POLL).await;
        let generation = prod_code_mcp::watch::current_generation(&root);
        if !prod_code_mcp::watch::sync_due(&root, generation) {
            continue;
        }
        let pushed = {
            let _one_at_a_time = pushing.lock().await;
            push_checkout(remote, &root).await
        };
        match pushed {
            Ok(()) => prod_code_mcp::watch::mark_synced(&root, generation),
            Err(err) => {
                tracing::debug!(error = %format!("{err:#}"), "background sync failed");
                tokio::time::sleep(BRIDGE_SYNC_RETRY).await;
            }
        }
    }
}

/// The file `PROD_CODE_LSP_TRACE` names, where the bridge logs every message it carries: the
/// time, the direction, the method or id, and the size.
type LspTrace = Option<std::sync::Arc<std::sync::Mutex<std::fs::File>>>;

fn lsp_trace() -> LspTrace {
    let path = env::var_os("PROD_CODE_LSP_TRACE")?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()?;
    Some(std::sync::Arc::new(std::sync::Mutex::new(file)))
}

fn trace_message(trace: &LspTrace, direction: &str, raw: &str) {
    let Some(file) = trace else {
        return;
    };
    let id = serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|v| v.get("id").cloned())
        .map(|id| id.to_string())
        .unwrap_or_default();
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let method = prod_code_client::editor_files::method_of(raw).unwrap_or_default();
    if let Ok(mut file) = file.lock() {
        let line = format!("{millis} {direction} {method} id={id} {}B\n", raw.len());
        let _ = std::io::Write::write_all(&mut *file, line.as_bytes());
    }
}

/// Tells the editor why its language server could not start: the answer to its `initialize` is
/// the error (with, on macOS, where to allow local network access), and so is the answer to
/// every request after it, until the editor lets go (#338).
async fn refuse_lsp(err: &anyhow::Error) -> Result<()> {
    let message = prod_code_client::editor_files::startup_error_message(err);
    eprintln!("{message}");
    let mut stdin = BufReader::new(tokio::io::stdin());
    let mut stdout = tokio::io::stdout();
    prod_code_client::editor_files::refuse_session(&mut stdin, &mut stdout, &message).await?;
    Ok(())
}

/// Run full-duplex stdio LSP bridge connecting local editor to remote daemon over TCP.
///
/// The checkout is pushed before the handshake, under the name the sync used, and kept
/// current while the editor runs: a node that never saw the project would otherwise detect
/// no language in an empty copy and answer every request with nothing (#316). The session is
/// an editor's, so the node runs the language's own server for it (#332); `engine` names the
/// language when it is not the checkout root's. Files the server points at that exist only on
/// the node are mirrored locally (#333).
async fn run_lsp_bridge(remote: SocketAddr, engine: Option<&'static str>) -> Result<()> {
    let cwd = env::current_dir().context("Failed to determine current working directory")?;
    let cwd = std::fs::canonicalize(&cwd).unwrap_or(cwd);
    let cwd_str = cwd.to_string_lossy().to_string();
    let identity = prod_code_mcp::sync::workspace_identity(&cwd);

    let (framed, handshake_resp) =
        match open_editor_session(remote, engine, &cwd, cwd_str, identity).await {
            Ok(session) => session,
            Err(err) => return refuse_lsp(&err).await,
        };

    tracing::debug!(
        session_id = handshake_resp.session_id,
        engine = handshake_resp.detected_engine,
        "Connected to remote gateway"
    );
    // Files the gateway lost from its copy (#262): the next sync from this checkout sends them
    // again.
    prod_code_mcp::sync::resend_lost_files(
        &cwd,
        &prod_code_mcp::sync::gateway_node(&framed),
        &handshake_resp.stale_paths,
    );

    let files = std::sync::Arc::new(prod_code_client::editor_files::RemoteFiles::new(
        remote,
        &cwd,
        Path::new(&handshake_resp.server_workspace_root),
        &prod_code_client::editor_files::default_cache(),
    ));
    let trace = lsp_trace();
    let pushing = std::sync::Arc::new(tokio::sync::Mutex::new(()));
    let (mut socket_tx, mut socket_rx) = framed.split();
    let keeper = tokio::spawn(keep_checkout_synced(
        remote,
        cwd.clone(),
        std::sync::Arc::clone(&pushing),
    ));

    // Spawn background task to read responses from server and write LSP to stdout
    let stdout_files = std::sync::Arc::clone(&files);
    let stdout_trace = trace.clone();
    // The server's messages and the bridge's own warnings share the editor's stdout.
    let editor_out = std::sync::Arc::new(tokio::sync::Mutex::new(tokio::io::stdout()));
    let stdout_out = std::sync::Arc::clone(&editor_out);
    // Why the gateway's side ended, when it did; dropped unsent when the editor's side did.
    let (closed_tx, mut closed_rx) = tokio::sync::oneshot::channel::<String>();
    let stdout_task = tokio::spawn(async move {
        let why = loop {
            match socket_rx.next().await {
                Some(Ok(WireMessage::LspPayload(json))) => {
                    let json = stdout_files.to_editor(json).await;
                    trace_message(&stdout_trace, "<-", &json);
                    let mut stdout = stdout_out.lock().await;
                    if prod_code_client::editor_files::write_frame(&mut *stdout, &json)
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                Some(Ok(WireMessage::Disconnect { reason })) => {
                    break format!("closed the session: {reason}");
                }
                Some(Err(err)) => break format!("broke the connection: {err}"),
                None => break "closed the connection".to_string(),
                Some(Ok(_)) => {}
            }
        };
        let _ = closed_tx.send(why);
    });

    // Main loop: read standard LSP from stdin and forward as WireMessage::LspPayload over TCP
    let mut stdin_reader = BufReader::new(tokio::io::stdin());
    loop {
        let frame = tokio::select! {
            frame = prod_code_client::editor_files::read_frame(&mut stdin_reader) => {
                frame.context("reading the editor's message")?
            }
            closed = &mut closed_rx => match closed {
                // The gateway went away while the editor still talks to it: say so and exit
                // with a failure, which an editor answers by starting the server again. Left
                // to the editor's next message, this hung and then exited 0 (#394).
                Ok(why) => {
                    keeper.abort();
                    eprintln!("prod-code lsp: the gateway at {remote} {why}");
                    std::process::exit(1);
                }
                // The editor stopped reading its answers.
                Err(_) => break,
            },
        };
        let Some(json_payload) = frame else {
            // Stdin EOF (editor exited)
            let _ = socket_tx
                .send(WireMessage::Disconnect {
                    reason: "stdin EOF".to_string(),
                })
                .await;
            break;
        };
        trace_message(&trace, "->", &json_payload);
        // What the editor saved, or saw change, reaches the node before the server hears of it:
        // rust-analyzer checks the crate on save, and must check what was saved (#332).
        if matches!(
            prod_code_client::editor_files::method_of(&json_payload).as_deref(),
            Some("textDocument/didSave" | "workspace/didChangeWatchedFiles")
        ) {
            let generation = prod_code_mcp::watch::current_generation(&cwd);
            let pushed = {
                let _one_at_a_time = pushing.lock().await;
                push_checkout(remote, &cwd).await
            };
            match pushed {
                Ok(()) => prod_code_mcp::watch::mark_synced(&cwd, generation),
                Err(err) => {
                    tracing::debug!(error = %format!("{err:#}"), "sync before a save failed");
                    // The server still hears of the save, which keeps it in step with the
                    // editor, but its check reads the node's previous copy: the editor is told
                    // (#350).
                    let warning = prod_code_client::editor_files::push_failed_warning(&err);
                    trace_message(&trace, "<-", &warning);
                    let mut stdout = editor_out.lock().await;
                    let _ =
                        prod_code_client::editor_files::write_frame(&mut *stdout, &warning).await;
                }
            }
        }
        if let Err(err) = socket_tx
            .send(WireMessage::LspPayload(files.to_node(&json_payload)))
            .await
        {
            keeper.abort();
            eprintln!("prod-code lsp: the gateway at {remote} broke the connection: {err}");
            std::process::exit(1);
        }
    }

    keeper.abort();
    let _ = stdout_task.await;
    Ok(())
}

/// Connects to the node, pushes the checkout and asks for an editor's session.
async fn open_editor_session(
    remote: SocketAddr,
    engine: Option<&str>,
    cwd: &Path,
    cwd_str: String,
    identity: prod_code_mcp::sync::WorkspaceIdentity,
) -> Result<(
    Framed<tokio::net::TcpStream, ProdCodeCodec>,
    prod_code_protocol::HandshakeResponse,
)> {
    let stream = prod_code_protocol::transport::connect(remote)
        .await
        .with_context(|| format!("Failed to connect to remote gateway at {remote}"))?;
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    let generation = prod_code_mcp::watch::current_generation(cwd);
    prod_code_mcp::sync::push_workspace_sync(&mut framed, cwd, &identity, None)
        .await
        .context("workspace sync before the language server session failed")?;
    prod_code_mcp::watch::mark_synced(cwd, generation);
    let supported_versions = supported_protocol_versions();
    framed
        .send(WireMessage::HandshakeRequest(HandshakeRequest {
            protocol_version: PROTOCOL_VERSION,
            supported_versions: Some(supported_versions.clone()),
            capabilities: None,
            client_name: "prod-code-client".to_string(),
            client_pid: std::process::id(),
            auth_token: None,
            client_workspace_root: cwd_str,
            preferred_engine: engine.map(str::to_string),
            base_workspace_name: Some(identity.name.clone()),
            engine_subpath: None,
            client_agent: Some(prod_code_protocol::detect_client_agent()),
            client_host: Some(prod_code_protocol::client_host()),
            purpose: Some(prod_code_protocol::PURPOSE_EDITOR.to_string()),
            redirect_count: 0,
        }))
        .await?;
    let handshake_resp = match framed.next().await {
        Some(Ok(WireMessage::HandshakeResponse(resp))) => resp,
        Some(Ok(WireMessage::Disconnect { reason })) => {
            anyhow::bail!("the gateway refused the session: {reason}")
        }
        Some(Ok(other)) => anyhow::bail!("Expected HandshakeResponse, got {:?}", other),
        Some(Err(err)) => return Err(err.into()),
        None => anyhow::bail!("Server closed connection during handshake"),
    };
    validate_selected_protocol_version(handshake_resp.protocol_version, &supported_versions)
        .context("gateway returned an incompatible editor handshake response")?;
    Ok((framed, handshake_resp))
}

async fn run_mcp_server(remote: SocketAddr) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    prod_code_mcp::run_stdio_mcp_server(remote, cwd).await
}

async fn run_sync(remote: SocketAddr, subpath: Option<PathBuf>) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let start = std::time::Instant::now();
    let identity = prod_code_mcp::sync::workspace_identity(&cwd);

    let stream = prod_code_protocol::transport::connect(remote)
        .await
        .with_context(|| format!("Failed to connect to remote gateway at {remote}"))?;
    let mut framed = Framed::new(stream, ProdCodeCodec::new());

    let outcome =
        prod_code_mcp::sync::push_workspace_sync(&mut framed, &cwd, &identity, subpath.as_deref())
            .await?;
    let _ = framed
        .send(WireMessage::Disconnect {
            reason: "sync finished".to_string(),
        })
        .await;

    let total_ms = start.elapsed().as_millis();
    let kb = (outcome.bytes_transferred as f64) / 1024.0;
    println!("⚡ prod-code Fast-Sync Completed in {total_ms}ms");
    println!("────────────────────────────────────────────────────");
    println!("Local Workspace:   {}", cwd.display());
    println!("Server Workspace:  {}", identity.name);
    if !outcome.server_workspace_root.is_empty() {
        println!("Remote Path:       {}", outcome.server_workspace_root);
    }
    println!("Files Planned:     {}", outcome.planned);
    if outcome.probed {
        println!(
            "Manifest Probe:    {} files already on server{}",
            outcome.planned.saturating_sub(outcome.files_updated),
            if outcome.seeded {
                " (seeded from origin copy)"
            } else {
                ""
            }
        );
    }
    println!("Files Updated:     {}", outcome.files_updated);
    println!("Files Deleted:     {}", outcome.files_deleted);
    println!("Data Transferred:  {kb:.1} KB");
    println!("Status:            SYNCHRONIZED");
    Ok(())
}

async fn run_pull(remote: SocketAddr, files: Vec<PathBuf>) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let touched = prod_code_mcp::sync::pull_remote_files(remote, &cwd, &files).await?;
    if !touched.is_empty() {
        println!(
            "📥 Successfully pulled {} file(s) from gateway:\n  {}",
            touched.len(),
            touched.join("\n  ")
        );
    } else {
        println!("No files were pulled.");
    }
    Ok(())
}


fn find_first_code_file(dir: &Path) -> Option<(PathBuf, u32, u32)> {
    let mut builder = ignore::WalkBuilder::new(dir);
    builder.hidden(true).git_ignore(true).max_depth(Some(4));

    for entry in builder.build().flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !matches!(ext, "rs" | "go" | "py" | "ts") || path.to_string_lossy().contains("/tests/") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };

        for (line_idx, line) in content.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("//")
                || trimmed.starts_with("/*")
                || trimmed.starts_with("#")
                || trimmed.is_empty()
            {
                continue;
            }
            if let Some(pos) = trimmed.find("pub const ") {
                return Some((path.to_path_buf(), line_idx as u32, (pos + 12) as u32));
            }
            if let Some(pos) = trimmed.find("pub struct ") {
                return Some((path.to_path_buf(), line_idx as u32, (pos + 13) as u32));
            }
            if let Some(pos) = trimmed.find("pub fn ") {
                return Some((path.to_path_buf(), line_idx as u32, (pos + 9) as u32));
            }
            if let Some(pos) = trimmed.find("func ") {
                return Some((path.to_path_buf(), line_idx as u32, (pos + 6) as u32));
            }
        }
    }
    None
}

/// Delete an unreferenced item through the remote analyzer and apply the edit locally.
async fn run_safe_delete(remote: SocketAddr, file: &Path, line: u32, col: u32) -> Result<()> {
    let abs_path = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    // The tool decides between deleting an item and removing a parameter with its arguments.
    run_tool(
        remote,
        "code_safe_delete",
        serde_json::json!({
            "path": abs_path.to_string_lossy(),
            "line": line,
            "character": col,
        }),
    )
    .await
}

fn parse_line_col(spec: &str) -> Result<(u32, u32)> {
    let (l, c) = spec
        .split_once(':')
        .ok_or_else(|| anyhow::anyhow!("expected LINE:COL, got {spec}"))?;
    Ok((l.trim().parse()?, c.trim().parse()?))
}

/// List code actions at a position (no `id`) or apply one (`id`) and write its edits locally.
async fn run_assist(
    remote: SocketAddr,
    file: &Path,
    line: u32,
    col: u32,
    to: Option<&str>,
    id: Option<&str>,
    subtype: Option<u64>,
) -> Result<()> {
    let abs_path = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let cwd = env::current_dir()?;
    let ws_root = find_workspace_root(&abs_path).unwrap_or(cwd);
    let file_uri = Url::from_file_path(&abs_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path"))?
        .to_string();
    let end = match to {
        Some(spec) => parse_line_col(spec)?,
        None => (line, col),
    };
    let params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "range": {
            "start": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) },
            "end": { "line": end.0.saturating_sub(1), "character": end.1.saturating_sub(1) }
        }
    });
    match id {
        None => {
            let list = execute_lsp_query(remote, file, "prodCode/assists", params).await?;
            let items = list.as_array().cloned().unwrap_or_default();
            if items.is_empty() {
                println!("no code actions at {}:{line}:{col}", file.display());
            }
            for item in items {
                let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                let kind = item.get("kind").and_then(|v| v.as_str()).unwrap_or("");
                let label = item.get("label").and_then(|v| v.as_str()).unwrap_or("");
                match item.get("subtype").and_then(|v| v.as_u64()) {
                    Some(st) => println!("{id} --subtype {st}  [{kind}]  {label}"),
                    None => println!("{id}  [{kind}]  {label}"),
                }
            }
            Ok(())
        }
        Some(id) => {
            // The same handler as the MCP tool, so the CLI gets the same clean-up (#97).
            let started = std::time::Instant::now();
            let mut args = serde_json::json!({
                "path": abs_path.to_string_lossy(),
                "line": line,
                "character": col,
                "end_line": end.0,
                "end_character": end.1,
                "id": id,
            });
            if let Some(st) = subtype {
                args["subtype"] = serde_json::json!(st);
            }
            let result =
                prod_code_mcp::tools::execute_tool(remote, &ws_root, "code_assist", args).await?;
            for content in &result.content {
                let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
                println!("{text}");
            }
            if result.is_error {
                std::process::exit(1);
            }
            println!("[{:.2}s]", started.elapsed().as_secs_f64());
            Ok(())
        }
    }
}

/// Rename a symbol through the remote analyzer and apply the resulting edits to the checkout.
#[allow(clippy::too_many_arguments)]
async fn run_rename(
    remote: SocketAddr,
    file: &Path,
    line: u32,
    col: u32,
    new_name: &str,
    accessors: bool,
    comments: bool,
    force: bool,
) -> Result<()> {
    // The same path as the MCP tool, so the CLI gets the same check before writing (#98).
    let abs_path = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let cwd = env::current_dir()?;
    let ws_root = find_workspace_root(&abs_path).unwrap_or(cwd);
    let started = std::time::Instant::now();
    let args = serde_json::json!({
        "path": abs_path.to_string_lossy(),
        "line": line,
        "character": col,
        "new_name": new_name,
        "accessors": accessors,
        "comments": comments,
        "force": force,
    });
    let result = prod_code_mcp::tools::execute_tool(remote, &ws_root, "code_rename", args).await?;
    for content in &result.content {
        let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
        println!("{text}");
    }
    if result.is_error {
        std::process::exit(1);
    }
    println!("[{:.2}s]", started.elapsed().as_secs_f64());
    Ok(())
}

/// The cluster as one JSON object for scripts (#398): the gossip view of the first node that
/// answers, each configured node's status snapshot or the error it gave, and where the checkout
/// is placed.
async fn cluster_snapshot(
    nodes: &[SocketAddr],
    workspace_name: &str,
    engine: Option<&str>,
) -> serde_json::Value {
    let mut gossip = serde_json::Value::Null;
    for node in nodes {
        if let Ok(view) = prod_code_mcp::cluster::cluster_view(*node).await {
            gossip = serde_json::to_value(view).unwrap_or_default();
            break;
        }
    }
    let mut listed = Vec::new();
    for node in nodes {
        let started = std::time::Instant::now();
        listed.push(match prod_code_mcp::cluster::node_status(*node).await {
            Ok(status) => {
                let mut snapshot = status_snapshot(*node, started.elapsed(), &status);
                snapshot["up"] = serde_json::json!(true);
                snapshot
            }
            Err(e) => serde_json::json!({
                "remote": node.to_string(),
                "up": false,
                "error": format!("{e:#}"),
            }),
        });
    }
    let home = prod_code_mcp::cluster::rendezvous_order(nodes, workspace_name)
        .first()
        .map(|n| n.to_string());
    let remembered = prod_code_mcp::cluster::remembered_node(workspace_name);
    let placed_on_is_cached = remembered.is_some_and(|r| !nodes.contains(&r));
    serde_json::json!({
        "gossip": gossip,
        "nodes": listed,
        "workspace": workspace_name,
        "engine": engine,
        "home": home,
        "placed_on": remembered.map(|n| n.to_string()),
        "placed_on_is_cached": placed_on_is_cached,
    })
}

/// Show every gateway node and the placement of the current checkout.
async fn run_cluster(
    nodes: &[SocketAddr],
    workspace_name: &str,
    engine: Option<&str>,
    json: bool,
) -> Result<()> {
    if json {
        let snapshot = cluster_snapshot(nodes, workspace_name, engine).await;
        println!("{}", serde_json::to_string_pretty(&snapshot)?);
        return Ok(());
    }
    println!("⚡ prod-code cluster ({} node(s))", nodes.len());
    println!("────────────────────────────────────────────────────");
    // The gossip view of the first node that answers: what every node holds.
    for node in nodes {
        if let Ok(view) = prod_code_mcp::cluster::cluster_view(*node).await {
            println!("gossip view from {}:", view.this_node);
            for peer in &view.nodes {
                let ws: Vec<String> = peer
                    .workspaces
                    .iter()
                    .map(|w| format!("{}[{}:{}]", w.name, w.engine, w.sessions))
                    .collect();
                println!(
                    "  {:<22} {:<5} load {:>5.2}/cpu  seen {:>3}s ago  {}",
                    peer.addr,
                    if peer.alive { "UP" } else { "STALE" },
                    peer.status.load_per_cpu().unwrap_or(0.0),
                    peer.last_seen_secs,
                    if ws.is_empty() {
                        "-".to_string()
                    } else {
                        ws.join(" ")
                    }
                );
                if let Some(why) = peer.status.host.pressure() {
                    println!("  {:<22} short: {why}", "");
                }
            }
            println!("────────────────────────────────────────────────────");
            break;
        }
    }
    let home = prod_code_mcp::cluster::rendezvous_order(nodes, workspace_name)
        .first()
        .copied();
    let remembered = prod_code_mcp::cluster::remembered_node(workspace_name);
    for node in nodes {
        let started = std::time::Instant::now();
        match prod_code_mcp::cluster::node_status(*node).await {
            Ok(status) => {
                println!(
                    "{node:<22} UP    {:>6.2} ms  load {:>5.2}/cpu ({} cpus)  uptime {}h{:02}m  workspaces {}  sessions {}  commands {}  rss {:.0} MB",
                    started.elapsed().as_secs_f64() * 1000.0,
                    status.load_per_cpu().unwrap_or(0.0),
                    status.cpu_count.unwrap_or(0),
                    status.uptime_seconds / 3600,
                    (status.uptime_seconds % 3600) / 60,
                    status.loaded_workspaces,
                    status.active_sessions,
                    status.running_commands.len(),
                    status.memory_rss_mb().unwrap_or(0.0)
                );
                let engines: Vec<&str> = status
                    .detected_engines
                    .iter()
                    .filter(|e| e.as_str() != "generic-lsp")
                    .map(|e| e.split(' ').next().unwrap_or(e))
                    .collect();
                println!("{:<22} engines: {}", "", engines.join(", "));
                let host = status.host.describe();
                if !host.is_empty() {
                    match status.host.pressure() {
                        Some(_) => {
                            println!("{:<22} host: {host} — short, takes no new workspaces", "")
                        }
                        None => println!("{:<22} host: {host}", ""),
                    }
                }
            }
            Err(e) => println!("{node:<22} DOWN  {e}"),
        }
    }
    println!("────────────────────────────────────────────────────");
    println!("Workspace:           {workspace_name}");
    println!("Engine needed:       {}", engine.unwrap_or("(any)"));
    if let Some(home) = home {
        println!("Home node (hash):    {home}");
    }
    match remembered {
        Some(node) if nodes.contains(&node) => println!("Placed on:           {node}"),
        Some(node) => println!("Placed on:           {node} (cached placement)"),
        None => println!("Placed on:           (not yet)"),
    }
    Ok(())
}

/// Where a check, lint, test or benchmark run is scoped: `--path`, relative to `cwd`, or `cwd`.
fn verify_scope(cwd: &Path, path: Option<&Path>) -> Result<PathBuf> {
    let Some(path) = path else {
        return Ok(cwd.to_path_buf());
    };
    let path = cwd.join(path);
    anyhow::ensure!(path.exists(), "--path {} does not exist", path.display());
    Ok(std::fs::canonicalize(&path).unwrap_or(path))
}

/// The arguments of `check`, `lint`, `test` and `benchmarks`, as given.
struct VerifyArgs {
    filter: Option<String>,
    timeout_secs: u64,
    json: bool,
    env: Vec<String>,
    events: bool,
    /// The project to run in; the current directory when absent.
    path: Option<PathBuf>,
}

/// Typed remote verification: check / lint / test with parsed diagnostics. `env` holds
/// `KEY=VALUE` pairs for the command; `events` prints each [`RunEvent`] as a JSON line as it
/// arrives and the report as the last one.
///
/// [`RunEvent`]: prod_code_mcp::verify::RunEvent
async fn run_verify(remote: SocketAddr, kind: VerifyKind, args: VerifyArgs) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let scope = verify_scope(&cwd, args.path.as_deref())?;
    let env = args
        .env
        .iter()
        .map(|pair| {
            pair.split_once('=')
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .with_context(|| format!("--env takes KEY=VALUE, got `{pair}`"))
        })
        .collect::<Result<Vec<_>>>()?;
    let report = prod_code_mcp::verify::run_verify_with(
        remote,
        &root,
        Some(&scope),
        kind,
        args.filter.as_deref(),
        args.timeout_secs,
        &env,
        |event| {
            if args.events
                && let Ok(line) = serde_json::to_string(&event)
            {
                println!("{line}");
            }
        },
    )
    .await?;
    if args.events {
        println!(
            "{}",
            serde_json::json!({ "event": "report", "report": report })
        );
    } else if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.render(200));
    }
    std::process::exit(if report.ok() { 0 } else { 1 });
}

/// `check --fix` / `lint --fix`: apply the compiler's machine-applicable fixes, then run again.
async fn run_fix(
    remote: SocketAddr,
    kind: VerifyKind,
    timeout_secs: u64,
    json: bool,
    path: Option<PathBuf>,
) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let scope = verify_scope(&cwd, path.as_deref())?;
    let fixed =
        prod_code_mcp::fixit::check_and_fix(remote, &root, Some(&scope), kind, timeout_secs)
            .await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&fixed)?);
    } else {
        print!("{}", fixed.render(200));
    }
    std::process::exit(if fixed.ok() { 0 } else { 1 });
}

/// Print a value fixture, mock, or builder preview, preserving an unsuccessful verification status.
#[allow(clippy::too_many_arguments)]
async fn run_fixture_cli(
    remote: SocketAddr,
    symbol: String,
    depth: u32,
    verify: bool,
    path: Option<String>,
    builder: bool,
    builder_name: Option<String>,
    randomized: bool,
    mock: bool,
    language: Option<String>,
) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let hint = path.map(|p| {
        let path = PathBuf::from(&p);
        if path.is_absolute() {
            path
        } else {
            root.join(path)
        }
    });
    if builder {
        let preview = prod_code_mcp::fixture::builder::preview(
            remote,
            &root,
            &prod_code_mcp::fixture::builder::BuilderRequest {
                symbol: &symbol,
                hint: hint.as_deref(),
                builder_name: builder_name.as_deref(),
                verify,
            },
        )
        .await?;
        println!("{}", preview.render());
        anyhow::ensure!(
            !verify || preview.verified(),
            "builder verification did not succeed; nothing was written"
        );
        return Ok(());
    }
    let parsed_lang = language.as_deref().and_then(|l| match l.to_ascii_lowercase().as_str() {
        "rust" | "rs" => Some(prod_code_mcp::parameter_object::Language::Rust),
        "go" | "golang" => Some(prod_code_mcp::parameter_object::Language::Go),
        "typescript" | "ts" => Some(prod_code_mcp::parameter_object::Language::TypeScript),
        "javascript" | "js" => Some(prod_code_mcp::parameter_object::Language::JavaScript),
        "python" | "py" => Some(prod_code_mcp::parameter_object::Language::Python),
        "c" => Some(prod_code_mcp::parameter_object::Language::C),
        "cpp" | "c++" => Some(prod_code_mcp::parameter_object::Language::Cpp),
        "swift" => Some(prod_code_mcp::parameter_object::Language::Swift),
        _ => None,
    });
    let fixture = prod_code_mcp::fixture::generate_with_options(
        remote,
        &root,
        &symbol,
        prod_code_mcp::fixture::FixtureOptions {
            depth,
            verify,
            hint,
            randomized,
            mock,
            language: parsed_lang,
        },
    )
    .await?;
    println!("{}", fixture.render());
    if !fixture.diagnostics.is_empty() {
        std::process::exit(1);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_schema_rename_cli(
    remote: SocketAddr,
    field: String,
    to: String,
    path: Option<String>,
    repos: Vec<String>,
    verify: Option<String>,
    apply: bool,
    force: bool,
) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let mut args = serde_json::json!({ "field": field, "to": to, "apply": apply, "force": force });
    if let Some(path) = path {
        args["path"] = serde_json::Value::String(path);
    }
    if !repos.is_empty() {
        // Relative to where the command was typed, as a shell user means it.
        let repos = repos
            .iter()
            .map(|r| cwd.join(r).to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        args["repos"] = serde_json::json!(repos);
    }
    if let Some(verify) = verify {
        args["verify"] = serde_json::Value::String(verify);
    }
    let result =
        prod_code_mcp::tools::execute_tool(remote, &root, "code_schema_rename", args).await?;
    for content in &result.content {
        let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
        println!("{text}");
    }
    if result.is_error {
        std::process::exit(1);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_migrate_type_cli(
    remote: SocketAddr,
    symbol: String,
    to: String,
    line: Option<u32>,
    character: u32,
    path: Option<String>,
    convert: bool,
    transitive: bool,
    apply: bool,
    force: bool,
) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let mut args =
        serde_json::json!({ "to": to, "convert": convert, "transitive": transitive, "apply": apply, "force": force });
    match line {
        // A position: the first argument is the file, not a name to resolve.
        Some(line) => {
            args["path"] = serde_json::Value::String(symbol);
            args["line"] = serde_json::Value::from(line);
            args["character"] = serde_json::Value::from(character);
        }
        None => args["symbol"] = serde_json::Value::String(symbol),
    }
    if let Some(path) = path {
        args["path"] = serde_json::Value::String(path);
    }
    let result =
        prod_code_mcp::tools::execute_tool(remote, &root, "code_migrate_type", args).await?;
    for content in &result.content {
        let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
        println!("{text}");
    }
    if result.is_error {
        std::process::exit(1);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_encapsulate_field_cli(
    remote: SocketAddr,
    symbol: String,
    line: Option<u32>,
    character: u32,
    path: Option<String>,
    field: Option<String>,
    class: Option<String>,
    by_value: Option<bool>,
    verify: Option<String>,
    apply: bool,
    force: bool,
) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let mut args = serde_json::json!({ "apply": apply, "force": force });
    let is_file_path = std::path::Path::new(&symbol).extension().is_some() || cwd.join(&symbol).exists();
    if let Some(line) = line {
        // A position: the first argument is the file, not a name to resolve.
        args["path"] = serde_json::Value::String(symbol);
        args["line"] = serde_json::Value::from(line);
        args["character"] = serde_json::Value::from(character);
    } else if is_file_path {
        args["path"] = serde_json::Value::String(symbol);
    } else if field.is_some() {
        if path.is_some() {
            args["class_name"] = serde_json::Value::String(symbol);
        } else {
            args["path"] = serde_json::Value::String(symbol);
        }
    } else {
        args["symbol"] = serde_json::Value::String(symbol);
    }
    if let Some(path) = path {
        args["path"] = serde_json::Value::String(path);
    }
    if let Some(field) = field {
        args["field"] = serde_json::Value::String(field);
    }
    if let Some(class) = class {
        args["class_name"] = serde_json::Value::String(class);
    }
    if let Some(by_value) = by_value {
        args["by_value"] = serde_json::Value::Bool(by_value);
    }
    if let Some(verify) = verify {
        args["verify"] = serde_json::Value::String(verify);
    }
    let result =
        prod_code_mcp::tools::execute_tool(remote, &root, "code_encapsulate_field", args).await?;
    for content in &result.content {
        let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
        println!("{text}");
    }
    if result.is_error {
        std::process::exit(1);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_extract_parameter_cli(
    remote: SocketAddr,
    file: &Path,
    line: u32,
    character: u32,
    to: &str,
    name: String,
    ty: Option<String>,
    replace_all: bool,
    verify: Option<String>,
    apply: bool,
    force: bool,
) -> Result<()> {
    let (end_line, end_character): (u32, u32) = to
        .split_once(':')
        .and_then(|(l, c)| {
            let line = l.trim().parse::<u32>().ok()?;
            let col = c.trim().parse::<u32>().ok()?;
            Some((line, col))
        })
        .context("--to takes LINE:COL, for example --to 42:31")?;
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let mut args = serde_json::json!({
        "path": file.to_string_lossy(),
        "line": line,
        "character": character,
        "end_line": end_line,
        "end_character": end_character,
        "name": name,
        "replace_all": replace_all,
        "apply": apply,
        "force": force,
    });
    if let Some(ty) = ty {
        args["type"] = serde_json::Value::String(ty);
    }
    if let Some(verify) = verify {
        args["verify"] = serde_json::Value::String(verify);
    }
    let result =
        prod_code_mcp::tools::execute_tool(remote, &root, "code_extract_parameter", args).await?;
    for content in &result.content {
        let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
        println!("{text}");
    }
    if result.is_error {
        std::process::exit(1);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_parameter_object_cli(
    remote: SocketAddr,
    symbol: String,
    params: Vec<String>,
    name: String,
    binding: Option<String>,
    path: Option<String>,
    verify: Option<String>,
    apply: bool,
    force: bool,
) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let mut args = serde_json::json!({
        "symbol": symbol, "params": params, "name": name, "apply": apply, "force": force
    });
    if let Some(binding) = binding {
        args["binding"] = serde_json::Value::String(binding);
    }
    if let Some(path) = path {
        args["path"] = serde_json::Value::String(path);
    }
    if let Some(verify) = verify {
        args["verify"] = serde_json::Value::String(verify);
    }
    let result =
        prod_code_mcp::tools::execute_tool(remote, &root, "code_introduce_parameter_object", args)
            .await?;
    for content in &result.content {
        let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
        println!("{text}");
    }
    if result.is_error {
        std::process::exit(1);
    }
    Ok(())
}

async fn run_move_cli(
    remote: SocketAddr,
    symbol: String,
    to: String,
    path: Option<String>,
    verify: Option<String>,
    apply: bool,
    force: bool,
) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let mut args =
        serde_json::json!({ "symbol": symbol, "to": to, "apply": apply, "force": force });
    if let Some(path) = path {
        args["path"] = serde_json::Value::String(path);
    }
    if let Some(verify) = verify {
        args["verify"] = serde_json::Value::String(verify);
    }
    let result = prod_code_mcp::tools::execute_tool(remote, &root, "code_move", args).await?;
    for content in &result.content {
        let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
        println!("{text}");
    }
    if result.is_error {
        std::process::exit(1);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_change_signature_cli(
    remote: SocketAddr,
    symbol: String,
    params: Vec<String>,
    returns: Option<String>,
    visibility: Option<String>,
    asyncness: Option<bool>,
    path: Option<String>,
    verify: Option<String>,
    apply: bool,
    force: bool,
) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let mut args =
        serde_json::json!({ "symbol": symbol, "params": params, "apply": apply, "force": force });
    if let Some(returns) = returns {
        args["returns"] = serde_json::Value::String(returns);
    }
    if let Some(asyncness) = asyncness {
        args["async"] = serde_json::Value::Bool(asyncness);
    }
    if let Some(visibility) = visibility {
        args["visibility"] = serde_json::Value::String(visibility);
    }
    if let Some(path) = path {
        args["path"] = serde_json::Value::String(path);
    }
    if let Some(verify) = verify {
        args["verify"] = serde_json::Value::String(verify);
    }
    let result =
        prod_code_mcp::tools::execute_tool(remote, &root, "code_change_signature", args).await?;
    for content in &result.content {
        let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
        println!("{text}");
    }
    if result.is_error {
        std::process::exit(1);
    }
    Ok(())
}

async fn run_codemod_cli(
    remote: SocketAddr,
    rule: String,
    path: Option<String>,
    apply: bool,
) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let mut args = serde_json::json!({ "rule": rule, "apply": apply });
    if let Some(path) = path {
        args["path"] = serde_json::Value::String(path);
    }
    let result = prod_code_mcp::tools::execute_tool(remote, &root, "code_codemod", args).await?;
    for content in &result.content {
        let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
        println!("{text}");
    }
    if result.is_error {
        std::process::exit(1);
    }
    Ok(())
}

async fn run_search_cli(
    remote: SocketAddr,
    query: String,
    limit: usize,
    path: Option<String>,
) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let subpath = path.or_else(|| prod_code_mcp::exec::subdir_of(&root, &cwd));
    let resp =
        prod_code_mcp::search::search(remote, &root, &query, limit, subpath.as_deref()).await?;
    println!("{}", prod_code_mcp::search::render(&resp, &query));
    Ok(())
}

async fn run_slice(
    remote: SocketAddr,
    target: String,
    line: Option<u32>,
    character: u32,
    options: prod_code_mcp::slice::SliceOptions,
) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let (file, line, col) = match line {
        Some(line) => {
            let path = PathBuf::from(&target);
            let abs = if path.is_absolute() {
                path
            } else {
                root.join(path)
            };
            (std::fs::canonicalize(&abs).unwrap_or(abs), line, character)
        }
        None => {
            let hit = prod_code_mcp::tools::resolve_symbol(remote, &root, &target, None).await?;
            println!(
                "{} {} at {}:{}:{}",
                hit.kind,
                hit.name,
                hit.path
                    .strip_prefix(&root)
                    .unwrap_or(&hit.path)
                    .to_string_lossy(),
                hit.line,
                hit.col
            );
            (hit.path, hit.line, hit.col)
        }
    };
    let report =
        prod_code_mcp::slice::slice_with_options(remote, &root, &file, line, col, options).await?;
    let rendered = report.render();
    if report.items.is_empty() {
        eprintln!("no slice items found for {target}");
        std::process::exit(1);
    }
    println!("{rendered}");
    Ok(())
}

async fn run_shadow_cli(
    remote: SocketAddr,
    spec: PathBuf,
    timeout_secs: u64,
    parallel: usize,
    apply: bool,
    command: Vec<String>,
) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let subdir = prod_code_mcp::exec::subdir_of(&root, &cwd);
    let text = std::fs::read_to_string(&spec)
        .with_context(|| format!("cannot read {}", spec.display()))?;
    let json: serde_json::Value =
        serde_json::from_str(&text).with_context(|| format!("{} is not JSON", spec.display()))?;
    let specs = prod_code_mcp::shadow::parse_specs(&root, &json, spec.parent())?;
    let outcome = prod_code_mcp::shadow::run_shadow(
        remote,
        &root,
        subdir.as_deref(),
        &specs,
        command.clone(),
        vec![("CARGO_TERM_COLOR".to_string(), "never".to_string())],
        timeout_secs,
        parallel,
        64 * 1024,
    )
    .await?;
    let applied = match (apply, outcome.winner) {
        (true, Some(i)) => Some(prod_code_mcp::shadow::apply_hypothesis(&root, &specs[i])?),
        _ => None,
    };
    println!(
        "{}",
        prod_code_mcp::shadow::render_report(&outcome, &command, applied.as_deref(), 4000)
    );
    if outcome.winner.is_none() {
        std::process::exit(1);
    }
    Ok(())
}

async fn run_exec(
    remote: SocketAddr,
    command: Vec<String>,
    env_args: Vec<String>,
    timeout_secs: u64,
    pull_changes: bool,
) -> Result<()> {
    use std::io::Write;
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    // Commands run where they were typed: a subdirectory of the checkout maps to the same
    // subdirectory of the server copy.
    let subdir = prod_code_mcp::exec::subdir_of(&root, &cwd);
    let mut env_pairs = Vec::new();
    if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        env_pairs.push(("CARGO_TERM_COLOR".to_string(), "always".to_string()));
    }
    for pair in &env_args {
        let (key, value) = pair
            .split_once('=')
            .with_context(|| format!("--env takes KEY=VALUE, got `{pair}`"))?;
        env_pairs.push((key.to_string(), value.to_string()));
    }
    let started = std::time::Instant::now();
    let outcome = prod_code_mcp::exec::run_remote(
        remote,
        &root,
        subdir.as_deref(),
        command.clone(),
        env_pairs,
        timeout_secs,
        pull_changes,
        |is_stderr, data| {
            if is_stderr {
                let mut e = std::io::stderr().lock();
                let _ = e.write_all(data);
                let _ = e.flush();
            } else {
                let mut o = std::io::stdout().lock();
                let _ = o.write_all(data);
                let _ = o.flush();
            }
        },
    )
    .await?;
    let changed_code = outcome.changed_code();
    let exit = outcome.exit;
    if let Some(err) = &exit.error {
        anyhow::bail!("remote exec failed: {err}");
    }
    if !outcome.pulled_files.is_empty() {
        eprintln!(
            "[prod-code exec] {} file(s) changed by the command written back: {}",
            outcome.pulled_files.len(),
            outcome.pulled_files.join(", ")
        );
    }
    if !outcome.kept_files.is_empty() {
        eprintln!(
            "[prod-code exec] {} file(s) changed here while the command ran were kept, and the node's version was not written: {}",
            outcome.kept_files.len(),
            outcome.kept_files.join(", ")
        );
    }
    if let Some(warning) =
        prod_code_mcp::exec::platform_warning(&root, exit.platform.as_deref(), &changed_code)
    {
        eprintln!("[prod-code exec] {warning}");
    }
    eprintln!(
        "[prod-code exec] {} in {:.1}s (server {:.1}s{}) on {}{}",
        match (exit.timed_out, exit.exit_code) {
            (true, _) => "timed out".to_string(),
            (false, Some(code)) => format!("exit {code}"),
            (false, None) => "killed".to_string(),
        },
        started.elapsed().as_secs_f64(),
        exit.duration_ms as f64 / 1000.0,
        exit.usage
            .map(|u| format!(", {}", u.render()))
            .unwrap_or_default(),
        exit.server_workspace_root,
        exit.platform
            .as_deref()
            .map(|p| format!(" ({p})"))
            .unwrap_or_default()
    );
    std::process::exit(exit.exit_code.unwrap_or(1));
}

/// Run concurrent pipelined benchmark against remote gateway across workspaces and worktrees.
async fn run_benchmark(
    remote: SocketAddr,
    workspaces: Vec<PathBuf>,
    concurrency: usize,
    depth: usize,
    duration_secs: u64,
) -> Result<()> {
    let target_workspaces: Vec<PathBuf> = if workspaces.is_empty() {
        vec![env::current_dir()?]
    } else {
        workspaces
    };

    println!("⚡ prod-code Multi-Tenant Benchmark (Pipelined Concurrent Load)");
    println!("────────────────────────────────────────────────────────────────");
    println!("Target Remote:       {remote}");
    println!("Concurrency:         {concurrency} worker connections");
    println!("Pipeline Depth:      {depth} in-flight queries per worker");
    println!("Duration:            {duration_secs}s");
    println!("Target Workspaces:   {} total", target_workspaces.len());
    for (i, ws) in target_workspaces.iter().enumerate() {
        let name = detect_workspace_name(ws).unwrap_or_else(|| "default".to_string());
        println!("  • [{}] {} (base: {})", i + 1, ws.display(), name);
    }
    println!("────────────────────────────────────────────────────────────────");
    println!("Connecting workers and starting load test...");

    let start_instant = std::time::Instant::now();
    let end_deadline = start_instant + std::time::Duration::from_secs(duration_secs);

    let mut handles = Vec::new();

    for worker_id in 0..concurrency {
        let ws_path = target_workspaces[worker_id % target_workspaces.len()].clone();
        let handle = tokio::spawn(async move {
            let mut latencies_us = Vec::new();
            let mut completed: u64 = 0;
            let mut errors: u64 = 0;

            let ws_str = ws_path.to_string_lossy().to_string();
            let base_name = detect_workspace_name(&ws_path);

            let stream = match prod_code_protocol::transport::connect(remote).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("Worker {worker_id} connection failed: {e}");
                    return (completed, errors + 1, latencies_us);
                }
            };
            let mut framed = Framed::new(stream, ProdCodeCodec::new());

            // 1. Handshake
            let supported_versions = supported_protocol_versions();
            let handshake = HandshakeRequest {
                protocol_version: PROTOCOL_VERSION,
                supported_versions: Some(supported_versions.clone()),
                capabilities: None,
                client_name: format!("bench-worker-{worker_id}"),
                client_pid: std::process::id(),
                auth_token: None,
                client_workspace_root: ws_str.clone(),
                preferred_engine: None,
                base_workspace_name: base_name,
                engine_subpath: None,
                client_agent: Some(prod_code_protocol::detect_client_agent()),
                client_host: Some(prod_code_protocol::client_host()),
                purpose: None,
                redirect_count: 0,
            };

            if framed
                .send(WireMessage::HandshakeRequest(handshake))
                .await
                .is_err()
            {
                return (completed, errors + 1, latencies_us);
            }

            match framed.next().await {
                Some(Ok(WireMessage::HandshakeResponse(response)))
                    if validate_selected_protocol_version(
                        response.protocol_version,
                        &supported_versions,
                    )
                    .is_ok() => {}
                _ => return (completed, errors + 1, latencies_us),
            }

            // 2. Initialize LSP
            let init_req = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "processId": null,
                    "rootUri": format!("file://{ws_str}"),
                    "capabilities": {}
                }
            });

            if framed
                .send(WireMessage::LspPayload(init_req.to_string()))
                .await
                .is_err()
            {
                return (completed, errors + 1, latencies_us);
            }

            let _ = framed.next().await;

            let initialized = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "initialized",
                "params": {}
            });
            let _ = framed
                .send(WireMessage::LspPayload(initialized.to_string()))
                .await;

            let (code_file, sym_line, sym_col) = find_first_code_file(&ws_path)
                .unwrap_or_else(|| (ws_path.join("src/main.rs"), 5, 5));
            let file_uri = format!("file://{}", code_file.to_string_lossy());

            let mut req_id: u64 = 2;
            let mut in_flight: std::collections::HashMap<u64, std::time::Instant> =
                std::collections::HashMap::new();

            while std::time::Instant::now() < end_deadline {
                // Keep the pipeline filled up to `depth`
                while in_flight.len() < depth && std::time::Instant::now() < end_deadline {
                    let id = req_id;
                    req_id += 1;
                    let hover_req = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "method": "textDocument/hover",
                        "params": {
                            "textDocument": { "uri": file_uri },
                            "position": { "line": sym_line, "character": sym_col }
                        }
                    });

                    let send_time = std::time::Instant::now();
                    if framed
                        .send(WireMessage::LspPayload(hover_req.to_string()))
                        .await
                        .is_ok()
                    {
                        in_flight.insert(id, send_time);
                    } else {
                        errors += 1;
                        break;
                    }
                }

                // Drain ready responses
                tokio::select! {
                    msg_opt = framed.next() => {
                        match msg_opt {
                            Some(Ok(WireMessage::LspPayload(payload))) => {
                                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&payload) {
                                    let maybe_id = val
                                        .get("id")
                                        .filter(|_| val.get("method").is_none())
                                        .and_then(|v| v.as_u64());
                                    if let Some(send_time) = maybe_id.and_then(|id| in_flight.remove(&id)) {
                                        let elapsed = send_time.elapsed().as_micros() as u64;
                                        latencies_us.push(elapsed);
                                        completed += 1;
                                    }
                                }
                            }
                            Some(Ok(_)) => {}
                            Some(Err(_)) | None => {
                                errors += 1;
                                break;
                            }
                        }
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
                }
            }

            let _ = framed
                .send(WireMessage::Disconnect {
                    reason: "bench finished".to_string(),
                })
                .await;
            (completed, errors, latencies_us)
        });
        handles.push(handle);
    }

    let mut total_completed = 0;
    let mut total_errors = 0;
    let mut all_latencies = Vec::new();

    for h in handles {
        if let Ok((comp, errs, mut lats)) = h.await {
            total_completed += comp;
            total_errors += errs;
            all_latencies.append(&mut lats);
        }
    }

    let elapsed = start_instant.elapsed().as_secs_f64();
    let qps = if elapsed > 0.0 {
        (total_completed as f64) / elapsed
    } else {
        0.0
    };

    all_latencies.sort_unstable();
    let p50 = if !all_latencies.is_empty() {
        all_latencies[all_latencies.len() * 50 / 100] as f64 / 1000.0
    } else {
        0.0
    };
    let p90 = if !all_latencies.is_empty() {
        all_latencies[all_latencies.len() * 90 / 100] as f64 / 1000.0
    } else {
        0.0
    };
    let p95 = if !all_latencies.is_empty() {
        all_latencies[all_latencies.len() * 95 / 100] as f64 / 1000.0
    } else {
        0.0
    };
    let p99 = if !all_latencies.is_empty() {
        all_latencies[all_latencies.len() * 99 / 100] as f64 / 1000.0
    } else {
        0.0
    };
    let min = if !all_latencies.is_empty() {
        all_latencies[0] as f64 / 1000.0
    } else {
        0.0
    };

    println!("\n📊 Benchmark Results:");
    println!("────────────────────────────────────────────────────────────────");
    println!("Elapsed Time:        {:.2}s", elapsed);
    println!("Completed Queries:   {}", total_completed);
    println!("Errors:              {}", total_errors);
    println!("Throughput:          \x1b[1;32m{:.1} QPS\x1b[0m", qps);
    println!("Latency (min):       {:.2} ms", min);
    println!("Latency (p50):       {:.2} ms", p50);
    println!("Latency (p90):       {:.2} ms", p90);
    println!("Latency (p95):       {:.2} ms", p95);
    println!("Latency (p99):       {:.2} ms", p99);
    println!("────────────────────────────────────────────────────────────────");

    Ok(())
}

/// Run the multi-worktree divergence and correctness benchmark against the remote gateway.
async fn run_divergent_bench(config: DivergentBenchConfig) -> Result<()> {
    println!("⚡ prod-code Divergent Worktree Benchmark (Multi-Agent Fleet Simulation)");
    println!("────────────────────────────────────────────────────────────────");
    println!("Target Remote:       {}", config.remote);
    println!(
        "Base Repo:           {}",
        config
            .base_repo
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<scratch fixture repo>".to_string())
    );
    println!("Workspace Mode:      {}", config.mode.label());
    println!(
        "Session Model:       {}",
        if config.persistent {
            "persistent"
        } else {
            "connect per query"
        }
    );
    println!("Concurrent Workers:  {}", config.workers);
    println!("Queries Per Worker:  {}", config.queries_per_worker);
    println!("────────────────────────────────────────────────────────────────");
    println!("Forking git worktrees, applying controlled mutations, syncing to gateway...");

    let report = divergent_bench::run(config).await?;
    report.print();

    if !report.all_passed {
        anyhow::bail!("divergent worktree benchmark FAILED correctness verification");
    }

    Ok(())
}
