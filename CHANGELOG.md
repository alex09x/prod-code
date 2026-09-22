# Changelog

## Unreleased

### Fixed
- **A multi-file edit is written whole or not at all** (#70). Every write tool ends in
  `apply_workspace_edit`, which renamed and deleted during its first pass and wrote contents one
  file after another: the first failure returned a bare OS error and left whatever was already
  written in place. It now snapshots every path the edit touches before the first side effect,
  and on any failure puts each one back — the bytes it had, or no file where there was none —
  and says so: `the edit failed partway and was undone: N file(s) put back as they were`.
  Directories are never removed.
- **change-signature no longer loses a declaration that a call above it moved** (#58). The
  structural rewrite renders a call site it changes on one line; when that call sat above the
  declaration across several lines, everything below it moved up, and the declaration was looked
  for again at its old line and column and not found — reported as `the declaration moved while
  its call sites were rewritten`, naming nothing. The declaration is a declaration, not a call,
  and the rewrite never touches it, so it is now found by its own text. If that text is gone or
  appears twice, the refusal names the function and its signature. The command from the issue —
  reordering `execute_lsp_query`, 178 changed lines in 6 files — now succeeds.
- **Half a second off every CLI invocation** (#56). The gateway probed for installed language
  servers on every `StatusRequest`, `Gossip`, `ClusterRequest` and placement decision, and one
  of those probes is `npm root -g`, which spends 213 ms starting node. A CLI invocation asks
  two such questions before it can send its query — where the cluster is, and which node holds
  this workspace — so it paid the probe twice. The answer changes only when somebody installs a
  language server, so it is now taken once at startup, refreshed by the janitor's existing
  minute tick on a thread that may block, and read from memory on the request path. Measured on
  a Linux build node over loopback: `StatusRequest` 436.8 ms → 0.2 ms (median of 7), and
  `prod-code status` end to end 1315.5 ms → 5.0 ms (median of 5, development build).

### Added
- Change a declared type and see the whole job first (roadmap 7.1.4): `code_migrate_type` (MCP)
  and `prod-code migrate-type <file> --line N --character C --to <Type>` rewrite the declaration
  in memory — a struct field, a parameter, a return type or an annotated `let` — type-check the
  workspace in one overlay, and report every site the new type does not fit, grouped by file
  with the line of source at each. Where an error is exactly the old type meeting the new one,
  the report says what conversion would fix that site; it does not write it. Diagnostics that
  land on a `#[derive(…)]` line are counted separately, because the analyzer reports inside a
  derive it cannot expand and there is nothing at those positions to edit. `apply` writes the
  declaration alone and refuses while any site remains. This is the first half of a migration,
  not an automatic one, and says so.
- Promote an expression to a parameter (roadmap 7.1.2): `code_extract_parameter` (MCP) and
  `prod-code extract-parameter <file> <line> <col> --to <line>:<col> --name limit` take an
  expression out of a function body and make it a parameter, passing what the body used to say
  at every existing call site — so no current caller changes behaviour and the next one can
  choose. The parameter is added at the end of the list, keeping the list's shape; the type is
  the analyzer's where it gives one in a readable shape and the caller's otherwise;
  `replace_all` puts the parameter in every identical occurrence inside the body. A reference
  that is not a call with this arity is named rather than mangled, and an expression that names
  a local or anything private to the function is refused with that as the reason rather than
  with a raw diagnostic.
- Bundle parameters into a struct (roadmap 7.1.2): `code_introduce_parameter_object` (MCP) and
  `prod-code parameter-object <symbol> --param a --param b --name Opts` take several of a
  function's parameters and make them fields of a new `pub struct` written directly above it,
  in declaration order and with the types the declaration gave — a single lifetime is introduced
  when any of those types borrows. The declaration takes one parameter in place of them, every
  use of them in the body is rewritten to reach through it at the positions the analyzer
  reports, and every call site is rewritten in place: the bundled arguments become one struct
  literal where the first of them was, the others stay where they were, and an argument that is
  a closure, a method chain or a string containing a comma survives. A call site in another
  module of the same crate gets the import; one in a file that is not a module of the crate at
  all, such as a test, names the type in full instead. A use that is not a call with this
  arity is named rather than mangled. The whole change is type-checked in one overlay before
  anything is written.
- Move a declaration to another module (roadmap 7.1.1): `code_move` (MCP) and
  `prod-code move <symbol> --to <file>` take a function, struct, enum, trait or const out of one
  module and put it in another, with the imports that keep every user of it compiling. The item
  travels whole — signature, body, doc comment, attributes — and takes with it the `use`
  statements it actually spells, narrowed to the names it needs. Every file the analyzer lists
  as using it has its import rewritten (a grouped import keeps its other names) and any
  path-qualified reference requalified; a file that spelled the name bare gets the new import,
  one that only ever qualified it gets none. The positions the analyzer reported are adjusted
  for the hole the cut leaves in the file the item left, which is what makes the source file's
  own references find their new import. The whole change is type-checked in one overlay before
  anything is written, so a move that reaches for something private to the module it left is
  reported — with that explanation — rather than written. Nothing is written without `apply`.
  Rust only; the target module must already exist.
- `PROD_CODE_TIMING=1` now reports the work every invocation does *before* the query —
  `[timing] startup total=… discover_nodes=… workspace_identity=… engine_project=…
  pick_node=…`. The query timer started after all of it, which is why #56 could report a
  half second that no phase accounted for.

## v0.2.2 — 2026-09-22

Seven new tools since 0.2.1 — search by intent, slice a symbol's dependencies, try several
fixes at once, rewrite code structurally, build a fixture, change a signature with its call
sites, rename a schema field across languages — and the test suite that holds them: every file
in the workspace is at or above 80% of regions, up from 53%.

### Added
- Cross-language schema rename (roadmap 7.6): `code_schema_rename` (MCP) and
  `prod-code schema-rename <field> --to <new>` rename a schema field across every language
  that spells it — `order_id` in the `.proto` and in Rust, `OrderID` with a `json:"order_id"`
  tag in Go, `orderId` in TypeScript, the column in the SQL. All spellings (snake, camel,
  Pascal, Go's initialism form, SCREAMING, kebab) are found by a whole-word scan, which is
  discovery only; every identifier is then renamed by the analyzer of its own sub-project, so
  the change follows the symbol into files the scan never looked at, and only what no analyzer
  owns — schema files, and the name inside string literals — is edited textually at the
  positions that were found. Two renames that want the same characters are never merged: the
  second is skipped and reported. The result is type-checked per project before anything is
  written. A test bed is in `fixtures/polyglot-order`.
- Change signature (roadmap 7.1.1): `code_change_signature` (MCP) and
  `prod-code change-signature <fn> --param …` change what a function takes, with its call
  sites. `params` is the list the function should end up with — `name` keeps a parameter,
  `name: Type = expression` adds one and passes `expression` at every call site, anything not
  listed is removed. The arity and the types come from the declaration, so the structural rule
  that rewrites the call sites is built rather than guessed, and it is resolved in the
  declaring file's own scope, so calls match however they are spelled. What was rewritten is
  reconciled against the analyzer's reference list and anything it did not touch is named;
  dropping a parameter the body still uses is refused with the usages; and the declaration and
  every call site are type-checked together in an overlay before anything is written. Rust
  only.
- Fixture generation (roadmap 8.5): `code_generate_fixture` (MCP) and `prod-code fixture
  <Type>` build a compile-ready value for a type from the declaration the analyzer resolves the
  name to, filling every field by type, recursing into types declared in the workspace down to
  `depth` and falling back to `Default::default()` beyond it. The fixture is then type-checked
  in an in-memory overlay of the file that declares the type, so a missing field or a type
  without `Default` comes back as the analyzer's error rather than as a failed build. Nothing
  is written. Rust only.
- Structural codemod (roadmap 8.7): `code_codemod` (MCP) and `prod-code codemod
  "pattern ==>> replacement"` rewrite code on the syntax tree through rust-analyzer's own SSR
  engine, with `$name` placeholders bound by the match. A call split over three lines matches,
  a comment that looks like the pattern does not, and paths are resolved rather than compared
  as strings. The result is a unified diff of what would change; `apply: true` writes it.
  Rust only, and not interactive: the search resolves usages across the workspace, so a call
  takes tens of seconds on a warm engine and minutes on a cold one.
- Intent search (roadmap 8.4, first step): `code_search` (MCP) and `prod-code search "..."`
  find code by what it does when you do not know what it is called. The gateway indexes every
  declaration in the workspace copy together with the doc comment above it, its signature and
  its container, and ranks them with BM25 over those fields (name weighted highest) against the
  words of the question. Declarations belonging to tests are excluded unless the question is
  about tests, because a test's name repeats every word of the thing it tests. Lexical, not
  embeddings: the dense half of 8.4 is still open. The index is built on the first query and
  then kept current by the sync layer telling it which files it wrote, so a query never walks
  the tree. Measured: on a 1000-declaration repository a question answers in 2-42 ms; on a
  26712-declaration, 1051-file Go repository the first query costs 568 ms (the index build) and
  every later one 33 ms.
- Program slicing (roadmap 7.3): `code_slice` (MCP) and `prod-code slice` return only the code
  a symbol depends on. From the seed declaration the analyzer's own edges are followed, the
  functions it calls and the types, constants and traits its body mentions, each returned as a
  whole declaration with its file and line range; `depth` bounds the walk and `max_bytes` the
  result. Names resolving outside the workspace are listed, not expanded. Measured on this
  repository: a 31-line function's slice is 1.8 kB against 21 kB of source (92% smaller, 1.2 s);
  the gateway's shadow-run entry point is 11.5 kB against 284 kB across two files (96% smaller,
  1.3 s). No new wire message: it is built from documentSymbol and definition queries.
- Shadow runs (roadmap 7.4, second step): `code_shadow_run` / `prod-code shadow-run` run a
  command once per named hypothesis (complete proposed file contents) in a private shadow of
  the server workspace. On Linux a shadow is an overlay mount at the workspace's own path
  inside a user namespace, so warm build caches stay valid and hypotheses run in parallel;
  without user namespaces they run one at a time in place with the files restored. Every
  hypothesis reports exit code, parsed test counts and output tail; the outcomes are ranked
  (passed, fewest failures, most passed, smallest diff) and the winner comes back as a
  unified diff (`apply: true` writes it). Gateway `--shadow-dir` places the upper
  directories; leftovers are swept at start.

## v0.2.1 — 2026-09-20

### Added
- `code_validate_edits {edits: [{path, text}], also_check}`: several proposed files are
  validated together in one in-memory overlay, plus any extra files to check. When an edited
  file drops or renames a symbol, errors in other files that mention it carry a note naming the
  removed or renamed symbol and the file it vanished from; rust-analyzer stays silent on a
  qualified call to a function that no longer exists, so a `prod-code::stale-reference`
  warning is synthesised on that line.
- MCP hot reload: `prod-code mcp` polls its own binary every 3 s. When the installed file
  changes it finishes the in-flight request, sends `notifications/tools/list_changed`,
  re-executes itself with the same arguments and environment (`initialize` declares
  `tools.listChanged`), and the resumed process sends the notification again, so a running
  agent session gets the new tools and schemas without a restart.
- Gateway `--engines rust,go,cpp` allowlist: a node advertises and serves only the listed
  engines and refuses handshakes for the others, so a macOS node can be Swift-only and
  placement never sends Rust work to a workstation.
- `PROD_CODE_TIMING=1` prints the client's per-phase timing (connect, sync, handshake, query)
  to stderr; `divergent-bench --persistent` reports the same phases.
- Symbol-addressed queries: every position tool (`code_definition`, `code_references`,
  `code_hover`, `code_callers`, `code_callees`, `code_implementations`, `code_rename`,
  `code_safe_delete`, `code_assists`, `code_assist`, `code_type_at`) accepts `symbol`
  (`Metrics::record`, `pkg.Func`, `Class.method`) instead of `path`/`line`/`character`; the
  name is resolved through the analyzer's workspace symbol index (`workspace/symbol`, served
  in-process for Rust, forwarded for the LSP engines). Ambiguous names list the candidates.
- `code_symbols {query}`: workspace symbol search by name with file:line:col and container.
- `code_test` / `code_check` / `code_lint` with `path` narrow to the Cargo crate
  (`-p <name>`), Go package tree (`./dir/...`) or pytest path containing it.

### Fixed
- The engine of a checkout whose manifest sits one directory below the root is detected from
  that child (`project/go.mod`, `server/Cargo.toml`), as long as every child with a manifest
  agrees; a polyglot monorepo still resolves to "any engine".
- A workspace whose engine is unknown is no longer placed on a node that advertises a single
  engine. A Swift-only macOS node used to qualify for it, and the work failed there.
- Position tools' MCP schemas declare the `symbol` parameter (the tools accepted it, agents
  could not see it). `code_outline` hides local variables unless `include_locals` is set and
  `max_depth` limits nesting.
- A worktree is placed on the node that holds its origin repository (placement is keyed by the
  origin checkout), so the gateway can seed the worktree copy from the origin's files.
- Diagnostics reports (`code_diagnostics`, `code_validate_edit(s)`, `prod-code diagnostics`)
  drop rust-analyzer's `inactive-code` hints: code behind an inactive `cfg` is not an error.
- Worktree copies keep their own `target/` directory: no shared cargo state and no shared
  build lock between worktrees. The first load of a new worktree runs its build scripts once.
- Go engine is advertised only when both `gopls` and `go` are on the gateway's PATH (gopls
  without the go tool answers "no views"); the third Linux node got a Go toolchain.
- `GoEngine::document_symbols` surfaces gopls errors instead of returning an empty list.
- One node ran Ubuntu clangd 18, whose `workspace/symbol` reports header symbols under the wrong
  file; all Linux nodes now run clangd 22.1.6 from `~/.local/clangd`.

### Changed
- `TCP_NODELAY` on every gateway connection (client connect, gateway accept, gossip). Nagle
  plus delayed ACK stalled half of the didOpen→hover rounds by 32–43 ms; the server round trip
  is now p50 ~1 ms.
- Development process: every change is an issue and a pull request with the commands that
  reproduce and verify it (`CONTRIBUTING.md`); checks run on the build nodes, there is no
  hosted CI.
- Gateway channels moved to [`rapidfire`](https://github.com/alex09x/rapidfire) (zero-dependency
  MPSC): the per-session outgoing queue is drained in batches of 64 with one socket flush per
  batch, exec stdout/stderr chunks fan in through a bounded rapidfire channel, and metrics
  events are appended to disk by a background writer (`recv_many` batches of 256) instead of on
  the response path. Broadcast channels (engine notifications) stay on tokio.

- Usage metrics: every query, exec and sync round on a gateway is one event (agent —
  claude-code / codex / cli — client host and address, workspace, engine, method, file,
  position, duration, ok, item count), appended to `<storage>/../metrics/events-YYYY-MM-DD.jsonl`
  and summarised by `prod-code metrics [--since SECS] [--json]` across the cluster (per agent,
  host, workspace and method with p50/p95, exec runs with failures, sync volume).

- The MCP server keeps one gateway session per checkout for the life of the process: a tool
  call is one request instead of connect + sync + handshake + initialize (20 hovers: 2.8 s →
  0.6 s, ~10 ms each after the first). Local edits are pushed over the same connection before
  each call and open documents are updated; a dead connection is replaced transparently.
- The MCP server sends agent instructions at `initialize` (navigate semantically, validate
  before writing, build and test on the gateway, impact and diagnose).
- `prod-code status` probes the named node.

## v0.2.0 — 2026-09-20

Second release: every language, a real cluster, and the agent tools that make prod-code more
than a fast LSP. Since v0.1.0:

- Cluster (Phase 5 complete): gateways gossip every 5 s (`--peers`, `--advertise`) and every
  node knows the whole cluster; one seed address in `PROD_CODE_REMOTE` is enough, the client
  discovers the rest and caches it. Placement is decided by the cluster: the node that holds
  the workspace, else the quietest live node with the right engine; idle workspaces move off
  overloaded nodes. `prod-code cluster` shows the gossip view. The Rust engine now runs build
  scripts and expands proc macros (rust-analyzer's proc-macro server), so derives resolve.

- Failure dossier (Phase 8.2): `prod-code diagnose [FILTER]` and MCP `code_diagnose_failure`
  run the tests and explain each failure with the code at every mentioned location, the
  enclosing function and its callers, and what changed in the working tree.

- In-memory diagnostics and edit validation (Phase 7.7): `prod-code diagnostics <file>` and
  `prod-code validate <file>` (MCP `code_diagnostics`, `code_validate_edit`) report what the
  analyzer thinks of a file, or of a proposed new content, without a build and without
  writing: rust-analyzer diagnostics from the in-memory database, pull or published
  diagnostics from the managed servers. Type errors, unresolved names and hallucinated APIs
  are caught in well under a second on every language.

- Dead-code scan (Phase 8.6): `prod-code dead-code` and MCP `code_dead_code` list unreferenced
  functions, methods and types found through the analyzer, skipping tests and entry points and
  bucketing exported symbols and trait/interface methods separately. Batch features (impact,
  dead-code) run on one persistent gateway session instead of a connection per query.
- Rust document symbols carry their enclosing items (`containerName`: `tests`, `impl Shape for
  Circle`).

- The Rust engine analyses `cfg(test)` and `debug_assertions` code like rust-analyzer's IDE
  defaults, so `#[test]` functions exist in the call graph; callers are flagged as tests by
  the analyzer. Document symbols point at the item's name and carry its full extent.
- A synced project manifest (tsconfig, package.json, pyproject, CMakeLists, Package.swift,
  go.mod, Cargo.toml, prod-code.toml ...) restarts the workspace's engines on the next session.

- Impact analysis (Phase 8.1): `prod-code impact` (and MCP `code_impact`) lists the functions
  the working-tree diff touches, the callers that reach them through the call hierarchy and
  the affected tests, and emits (or with `--run` executes) the command that runs only those
  tests. Rust document symbols now carry their full extent.

- Definitions outside the checkout are readable (Phase 8.3): `prod-code def` shows the lines
  around a definition in the standard library, a dependency cache or a system header, and
  `prod-code source <path>` prints any such file from the gateway host; MCP `code_definition`
  embeds the snippet and `code_source` reads the file. Only toolchain, dependency and SDK
  roots are served.

- Code actions for every language (Phase 7.2): `prod-code assists | assist` and MCP
  `code_assists` / `code_assist` on Go, C/C++, TypeScript, Python and Swift through LSP code
  actions, including quick fixes driven by the server's diagnostics and command-backed
  refactorings (clangd extract-to-variable).
- Monorepos (Phase 3.1): a nested project of another language gets its own engine, its own
  placement (Swift package in a Rust repo lands on a macOS node) and its own `check` / `test`
  / `exec` working directory; `prod-code exec` runs where it was typed.

- Project tooling is detected per checkout for `check | lint | test`: TypeScript uses the
  package manager of the lock file (bun, pnpm, yarn, npm) and the configured test runner
  (vitest, jest, bun test, mocha, or the `test` script) with parsed results; Python runs
  through `uv run`, the checkout's `.venv`, or the system interpreter, with pytest or unittest
  and basedpyright pointed at the venv; C/C++ builds with CMake, Meson or Make and tests with
  ctest (after a build) or meson test. Rename now reaches every referencing file on pyright
  (files are opened for the duration of the rename), clangd (CMake is configured with
  compile_commands.json before clangd starts) and sourcekit-lsp. `exec` never pulls back
  virtual environments, node_modules or build directories.
- All six languages verified end to end on fixtures: hover / definition / references /
  symbols / callers / callees / implementations / rename / check / lint / test; Go on gopls
  (cross-file rename included), Rust in-memory.

- Per-repository Rust analysis options in `prod-code.toml` (`[rust] features = "all" | [..]`,
  `no_default_features`, `all_targets`, `sysroot`), with rust-analyzer-like defaults: all
  targets analysed and the standard library loaded from `rust-src`. Repositories that compile
  one module tree into several crates behind feature flags (BTCR's `src/strategy2`) need
  `features = "all"`, otherwise those modules resolve to nothing.
- Sync ships `rustc-wrapper` scripts and `*.sh`, and the gateway keeps the executable bit, so
  `cargo metadata` works on a workspace whose `.cargo/config.toml` sets `build.rustc-wrapper`.
  Verified on BTCR: 109 implementations of `StrategyInterface`, 235 references, callers with
  call sites, where before only syntax-level queries answered.

- Call hierarchy and implementations (Phase 7.5): `prod-code callers | callees | impls` and MCP
  `code_callers` / `code_callees` / `code_implementations` for every engine (rust-analyzer
  in-memory, gopls, clangd, native TypeScript, basedpyright, sourcekit-lsp), with call sites.
- Rust document symbols report their real kinds and lines (all were `Variable (line 1)`).
- Second macOS node: a MacBook Pro (Xcode 15.4 with iOS simulators, live GUI
  session) runs a gateway for Swift and Xcode UI tests.

- Sync watermarks are kept per gateway node: a checkout placed on a second node (or moved by
  failover) is uploaded to it in full instead of receiving an empty delta computed against the
  first node. An empty delta is still sent, so a node whose workspace copy was pruned answers
  "fresh" and the client resyncs before the query or `exec` runs. Files rewritten by the client
  for rename / assists / safe-delete are no longer recorded as synced (the gateway only computed
  those edits); the next sync uploads them, so hover after rename sees the new code.
- A third Linux node (Ryzen 9 7950X) joined the cluster with all Linux
  engines; a Mac Studio is the macOS node for Swift.

- Language engines (Phase 3.4–3.6): C/C++ (`clangd`), TypeScript (native TypeScript 7
  `tsc --lsp`, fallback `typescript-language-server`) and Python (`basedpyright`) workspaces
  get hover, definition, references and document symbols through the gateway; `prod-code
  check | lint | test` run `cmake --build` (configuring the build dir first), `tsc --noEmit` /
  `eslint` / `npm test`, `basedpyright` / `ruff` / `pytest`, with parsed diagnostics. Build and
  tool manifests (`CMakeLists.txt`, `compile_commands.json`, `.clangd`, `requirements*.txt`,
  `pytest.ini`, `tox.ini`, `Pipfile`, Bazel `BUILD`, `project.pbxproj`, ...) are now synced.
  Swift (Phase 3.7) runs on a macOS gateway node (`sourcekit-lsp` from Xcode): hover,
  definition, references, symbols, `swift build` diagnostics and `swift test` results (XCTest
  and swift-testing parsed). `cpp test` parses ctest output. Diagnostic paths are relative to
  the checkout instead of the server copy.
- Engine-aware placement (Phase 5.1): gateway status lists the engines whose language
  server is actually installed on the host; the client places a checkout only on a node that
  serves its engine, re-places a remembered node that no longer fits, and `prod-code cluster`
  shows each node's engines.
- MCP tools open files with the languageId of their extension (was always `rust`); LSP symbol
  kinds are named correctly in outlines.

- Session churn stress (Phase 5.5): `divergent-bench --persistent --churn N` kills N% of
  sessions mid-run without a goodbye and verifies the gateway retires them all.
- `prod-code check | lint | test --json` print the full structured report.

- Safe delete (Phase 7.1.1): `prod-code safe-delete <file> <line> <col>` and MCP
  `code_safe_delete` remove an item only when rust-analyzer finds no references to it in the
  workspace; otherwise the usages that block the deletion are listed.

- Load-aware placement (Phase 5.3 first step): gateways report host load and CPU count in
  their status; the first placement of a checkout picks the quietest alive node.

- Multi-gateway placement (Phase 5.1, client side): `--remote` / `PROD_CODE_REMOTE` take a
  comma-separated node list; a checkout is placed by rendezvous hashing, remembered locally,
  and fails over to the next alive node. `prod-code cluster` shows node status and placement.

- Code actions (Phase 7.1): `prod-code assists <file> <line> <col> [--to LINE:COL]` lists the
  rust-analyzer assists at a position or selection, `prod-code assist … <id> [--subtype N]`
  applies one; MCP tools `code_assists` / `code_assist`. Inline, extract function/variable/
  constant, promote to const, add explicit type, generate and rewrite assists and quick fixes
  all go through the same WorkspaceEdit path as rename.

- Typed remote verification (Phase 6.4): `prod-code check`, `prod-code lint`,
  `prod-code test [FILTER]` and MCP tools `code_check`, `code_lint`, `code_test`. The command
  runs on the gateway and the client parses the output into structured diagnostics
  (`error: [E0308] ... (src/lib.rs:12:5)`), pass/fail counts and per-failure output for Rust
  (cargo JSON, rustc text, libtest) and Go (`go build`/`go vet` lines, `go test -json`).

- Semantic rename (Phase 7.1.1): `prod-code rename <file> <line> <col> <new_name>`, MCP tool
  `code_rename`, and LSP `textDocument/rename` on the gateway. rust-analyzer computes the
  workspace-wide edit (including module file renames); the client applies it to the checkout
  and records the rewritten files in the sync watermark. Refused renames (no symbol, conflicts)
  are reported as errors instead of empty results.

## v0.1.0 — 2026-09-19

First release of prod-code, the Remote Code Intelligence gateway: one warm, in-memory
analysis server on the LAN that a fleet of AI coding agents and thin clients query instead of
each running its own language server and build on a laptop.

### Gateway and engines
- `prod-code-server`: TCP gateway with a JSON wire protocol, multi-tenant workspace manager
  with leader/follower loading, per-session views and path translation.
- In-memory Rust engine on `ra_ap_ide` (rust-analyzer as a library): the Cargo workspace is
  loaded once into a Salsa database; hover, definition, references and document symbols run
  from RAM in 1–4 ms server-side. Live buffers are applied straight into the database; a
  re-open with identical text is a no-op and keeps the caches warm.
- Per-session buffer overlays: concurrent sessions on one workspace each see their own
  unsaved edits; queries run under the engine lock together with view activation, so a
  concurrent edit can no longer cancel an in-flight query into a null result.
- Managed Go engine (gopls) and a generic LSP engine for other languages; engine kind is
  detected from the workspace manifest and reloaded if the detected kind changes.
- Janitor: engines idle for 30 minutes are unloaded (`--idle-evict-secs`), worktree workspace
  copies unused for 7 days are pruned (`--prune-worktree-days`); child language servers are
  killed with their engine; `~/.cargo/bin` is put first on PATH.

### Isolated workspace per git worktree
- Every git worktree identifies itself as `<origin>--wt-<hash>` and gets its own server
  workspace and analysis database; the main checkout keeps its own. Diverged worktrees can no
  longer see each other's edits (the shared mode remains available in the benchmark as a
  diagnostic).
- First contact sends a manifest (path, size, FNV-1a hash) instead of the tree: the gateway
  seeds a new worktree from the origin repository's copy, deletes what the client does not
  have and asks only for missing files. A fresh BTCR worktree: 0.4 s instead of ~7 s.

### Sync
- Watermark-based incremental sync per worktree: commits since the last sync
  (`git diff <base>`), dirty and untracked files, reverts of previously dirty files, and lock
  files; `git diff` is skipped when HEAD has not moved. Persistent state lives under
  `~/.local/share/prod_code/sync/`, versioned with the relevance filter.
- Pre-flight sync runs before the handshake so a new workspace directory is populated before
  engine detection; a client whose watermark disagrees with the gateway (reset server
  directory) self-heals with a full resync.
- File content travels as base64: a 10.8 MB workspace syncs in 0.76 s over 10G instead of 8 s.
- The MCP server watches the workspace tree and runs the pre-flight sync only after a change.

### Remote build and test execution (Phase 6 foundation)
- `prod-code exec -- <argv>` and the MCP tool `code_exec` run a command on the gateway inside
  the checkout's server copy, stream stdout/stderr back and return the remote exit code.
  Build artifacts stay on the server per workspace, so every worktree keeps a warm cache.
- Files the command creates, changes or deletes (formatters, generators, lockfiles) are
  written back into the checkout and recorded in the watermark. Commands run in their own
  process group and are killed on timeout or client disconnect.
- Measured on the prod-code repository itself: workspace clippy plus all crate tests in
  5.8 s on the 32-core gateway host, nothing compiled on the developer's machine.

### Clients
- `prod-code` CLI: `status`, `sync`, `hover`, `def`, `refs`, `symbols` (1-based positions),
  `lsp` (stdio bridge), `mcp`, `exec`, `bench`, `divergent-bench`; `PROD_CODE_TIMING=1` prints
  per-phase timings of a query.
- Native MCP server with `code_definition`, `code_references`, `code_outline`, `code_hover`,
  `code_status`, `code_sync`, `code_exec` for Claude, Codex and other agent frameworks.

### Benchmarks
- `bench`: persistent pipelined sessions; 40k hover/s at p50 1.4 ms, p99 3.5 ms against a
  warm Rust workspace.
- `divergent-bench`: forks four worktrees of a real repository (signature change, manifest
  change, untracked file with a new symbol), runs 10+ concurrent workers and asserts zero
  cross-worktree bleed; `--persistent` reuses one session per worker like an agent process.
  Isolated mode passes on BTCR (Rust) and CodeHaus (Go) with zero errors.

### Known limitations
- Shared (coalesced) workspaces are diagnostic only; production isolates worktrees.
- A one-shot CLI query costs ~80 ms, of which ~70 ms are two `git` subprocesses on the
  client; long-lived agents avoid this through the MCP server's change watcher.
- Cluster features (Phase 5), C/C++, TypeScript, Python and Swift engines (Phase 3.4–3.7) are
  not implemented yet; see ROADMAP.md.
