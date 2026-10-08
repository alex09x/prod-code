/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::cert;
use crate::package;
use clap::Subcommand;
use prod_code_client::divergent_bench::WorkspaceMode;
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Run as an editor's language server (stdio LSP): the language's own server (rust-analyzer,
    /// gopls, clangd, basedpyright, the TypeScript server, sourcekit-lsp) runs on the node
    Lsp {
        /// The language the server is for (rust, go, c, cpp, python, typescript, javascript,
        /// swift); by default the checkout root's
        #[arg(long)]
        language: Option<String>,
        /// Attempt in-process auto-reconnect and LSP state replay on transient network disconnects
        #[arg(long, env = "PROD_CODE_RECONNECT", default_value_t = false)]
        reconnect: bool,
        /// Watchdog idle ping interval in seconds (0 disables periodic watchdog pings)
        #[arg(long, env = "PROD_CODE_WATCHDOG_SECS", default_value_t = 30)]
        watchdog_secs: u64,
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
        /// Force rebalancing of the active workspace to the quietest roomiest node across the cluster.
        #[arg(long, default_value_t = false)]
        rebalance: bool,
    },
    /// Inspect DNS and SRV service discovery resolution for *.code.internal or cluster nodes.
    Resolve {
        /// The domain or service query to resolve (e.g. `shop.code.internal`, `cluster.code.internal`, `_prod-code._tcp.code.internal`).
        domain: String,
        /// One JSON object with resolution details, endpoints, and dynamic SRV records.
        #[arg(long, default_value_t = false)]
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
        /// Explicitly build the package index (e.g. for SwiftPM) if needed to find cross-file references.
        #[arg(long, default_value_t = false)]
        build_index: bool,
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
        /// Explicitly build the package index (e.g. for SwiftPM) if needed to find cross-file callers.
        #[arg(long, default_value_t = false)]
        build_index: bool,
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
        /// Levels to walk: 1 is the direct ones; more gives a tree (at most 6).
        #[arg(long, default_value_t = 1)]
        depth: usize,
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
        /// Incremental stream validation: validate code as streamed line-by-line or chunk-by-chunk from stdin,
        /// intercepting hallucinated methods and type errors on the fly before turn completion (Roadmap 7.7).
        #[arg(long)]
        stream: bool,
        /// Single chunk to feed to an incremental stream validation session (Roadmap 7.7).
        #[arg(long, conflicts_with = "from")]
        chunk: Option<String>,
        /// Session ID for stateful incremental stream validation (Roadmap 7.7).
        #[arg(long)]
        session: Option<String>,
        /// Mark the stream session as closed/final on this chunk (Roadmap 7.7).
        #[arg(long)]
        close: bool,
        /// Reset the stream session state before feeding this chunk (Roadmap 7.7).
        #[arg(long)]
        reset: bool,
        /// Enforce compiler and borrow-checker verification proof in a shadow copy on the node (Roadmap 7.7).
        #[arg(long)]
        borrow_check: bool,
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
    /// Remove every orphan the dead-code scan finds, in one type-checked edit: prod-code prune [--apply] [--patch] [--commit]
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
        /// Output a formatted Git commit patch (compatible with git apply / git am)
        #[arg(long)]
        patch: bool,
        /// Create a Git commit after applying the pruned changes
        #[arg(long)]
        commit: bool,
        #[arg(long)]
        json: bool,
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
        /// Emit standard LSP WorkspaceEdit (documentChanges) JSON payload.
        #[arg(long = "workspace-edit", default_value_t = false)]
        workspace_edit: bool,
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
        #[arg(long)]
        line: u32,
        /// 1-based column of the switch/match/if statement
        #[arg(long)]
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
        #[arg(long)]
        line: Option<u32>,
        /// 1-based column of the type declaration
        #[arg(long)]
        character: Option<u32>,
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
        /// Run hypotheses in a lightweight RAM-backed (/dev/shm) in-memory overlay shadow root (Roadmap 7.4).
        #[arg(long, alias = "in-memory", default_value_t = false)]
        ram: bool,
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
        /// Type-3 gapped/reordered statement clone matching
        #[arg(long, default_value_t = false)]
        type3: bool,
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
    /// Manage cluster TLS certificates, Root CA, node certificates, pinning, and PKI trust roots
    Cert {
        #[command(subcommand)]
        cmd: cert::CertCommands,
    },
}
