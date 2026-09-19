# Changelog

## Unreleased

- Sync watermarks are kept per gateway node: a checkout placed on a second node (or moved by
  failover) is uploaded to it in full instead of receiving an empty delta computed against the
  first node. An empty delta is still sent, so a node whose workspace copy was pruned answers
  "fresh" and the client resyncs before the query or `exec` runs. Files rewritten by the client
  for rename / assists / safe-delete are no longer recorded as synced (the gateway only computed
  those edits); the next sync uploads them, so hover after rename sees the new code.
- Third Linux node ram9 (192.168.2.143, Ryzen 9 7950X) joined the cluster with all Linux
  engines; the Mac Studio (192.168.2.242) is the macOS node for Swift.

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
