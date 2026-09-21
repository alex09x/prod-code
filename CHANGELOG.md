# Changelog

## Unreleased

### Added
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
