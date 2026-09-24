# prod-code: Engineering Roadmap

This document outlines the architectural milestones and engineering phases for building **prod-code** as a distributed, polyglot remote code-intelligence engine optimized for AI agent fleets and 10 GbE local network execution.

**Where it stands** (v0.3.0, 2026-09-24): 55 MCP tools, a cluster of three Linux nodes and a
macOS node for Swift, 707 tests, and every file a change touches held at or above 80% of regions.
The refactoring catalog (7.1) is complete for Rust. Across the other languages it works through
the language servers' own code actions, and `extract_parameter` and `introduce_parameter_object`
are still Rust-only. Partial: caches shared across worktrees beyond the C/C++ compiler cache — the
clangd index, `node_modules`, Python stubs and Swift's `ModuleCache` (3.4–3.7, 6.2, 6.3).

---

## Phase 1: Foundation & High-Speed Wire Transport

**Objective**: Establish the core client-server wire protocol over 10G TCP/QUIC with transparent path translation and zero-overhead client bridging.

- [x] **1.1. Protocol Specification (`crates/prod-code-protocol`)**
  - Binary framing layer with length-prefixed messages and NUL completion markers.
  - Session handshake with protocol version negotiation, client capabilities, and authentication tokens.
  - Streaming transport support: 10 GbE TCP stream with TCP_NODELAY and socket buffer tuning.
  - Fallback local transport: Unix domain socket / Windows named pipe for local execution.
- [x] **1.2. Bi-directional Path Translation**
  - Canonical URI/path rewriting between client workspace roots (`file:///Users/me/...`) and remote server paths (`file:///srv/prod-code/workspaces/...`).
  - Support for Git worktree patterns (shared common Git dir, isolated working trees).
- [x] **1.3. Ultra-Thin Client CLI (`crates/prod-code-client`)**
  - Drop-in executable replacing language servers in IDEs (`prod-code lsp`).
  - Stdio-to-TCP bidirectional streaming with zero allocations on hot paths.
  - Non-blocking watchdog and auto-reconnect logic on transient network disconnects.
  - Strict exit codes and stderr reporting (fail loudly, never exit 0 on unhandled daemon death).
- [x] **1.4. Server Gateway Skeleton (`crates/prod-code-gateway`)**
  - Multi-threaded TCP listener accepting concurrent agent and editor connections.
  - Session registry tracking active client IDs, workspace paths, and leased resources.
  - Non-blocking status reporting endpoint (`prod-code status`) returning instant JSON health snapshots.


---

## Phase 2: In-Memory Rust Engine Core

**Objective**: Deliver a purpose-built, multi-tenant Rust analysis engine directly utilizing `ra_ap_*` public APIs (`AnalysisHost`), featuring direct-edits and shared dependency caching in server RAM.

- [x] **2.1. Clean `ra_ap_*` Integration (`crates/prod-code-engine-rust`)**
  - Direct dependency on upstream `ra_ap_ide::AnalysisHost`, `ra_ap_project_model`, and `ra_ap_vfs`.
  - Zero build-time AST patching hacks: clean usage of public APIs and structured input mutation.
  - Persistent base database: Cargo metadata and crate graphs loaded and cached in server RAM once per workspace.
- [x] **2.2. Single-Owner Direct-Edit Fast Path**
  - Detect dedicated worktree sessions (single session per worktree path).
  - Apply unsaved document edits (`didChange`) directly into base Salsa file inputs.
  - Bypass overlay crate cones and global database invalidation locks for unshared workspaces.
  - Target: Maintain sub-15s p95 query latency under 15 concurrent agent worktrees.
- [x] **2.3. Safe FileId & Edition Handling**
  - Comply with `EditionedFileId` 24-bit mask (`0x007F_FFFF`) to prevent Rust Edition bit corruption.
  - Path-normalized FileId deduplication and reuse across concurrent sessions.
- [x] **2.4. Daemon Tracing & Health Observability**
  - Multi-core query offload via `RustEngineSnapshot` and Tokio blocking thread pool.
  - Complete request execution timing and in-flight tracking with `[LSP START]`, `[LSP DONE]`, and `[LSP SLOW >200ms]` warnings.
  - Native `tracing-subscriber` integration with structured log filtering (`RUST_LOG=info,prod_code=debug`).
  - Memory watchdog: monitor server RSS, alert at memory thresholds, dynamic metrics in `StatusResponse`.

---

## Phase 3: Polyglot Hub & First-Class Multi-Language Engines

**Objective**: Expand the daemon into a unified multi-language hub by adding supervised language analysis engines for Go, C/C++, TypeScript/JavaScript, Python, and Swift, sharing server-side warm AST caches across agent worktrees.

- [x] **3.1. Automatic Workspace Detection**
  - Inspect project roots for language manifests:
    - `Cargo.toml` -> Rust Engine (`ra_ap_*`)
    - `go.mod` / `go.work` -> Go Engine (`gopls`)
    - `compile_commands.json` / `CMakeLists.txt` / `Makefile` -> C/C++ Engine (`clangd`)
    - `package.json` / `tsconfig.json` -> TypeScript / JavaScript Engine (`vtsls`)
    - `pyproject.toml` / `requirements.txt` / `setup.py` / `Pipfile` -> Python Engine (`basedpyright`)
    - `Package.swift` / `*.xcodeproj` / `project.yml` -> Swift Engine (`sourcekit-lsp`)
  - Typed `EngineKind` enum, preference resolution, and monorepo detection.
  - Monorepo (2026-09-20): a nested project of another language (a SwiftPM package or Xcode project inside a Rust or Go repository) gets its own engine rooted at that directory; the client names it in the handshake (`engine_subpath`), placement follows that project's engine (Swift → macOS node), and `exec` / `check` / `test` run in the nested directory with its tooling. Nested crates of one Cargo workspace stay with the workspace. Verified on tako (Rust root, `swift/` package).
- [x] **3.2. Managed Go Engine (`crates/prod-code-engine-go`)**
  - Supervised `gopls` worker pool running in daemon mode.
  - Shared `GOCACHE` and `GOPATH/pkg/mod` volume on fast NVMe for instant warm symbol resolution across all worktrees.
  - Path mapping translation for Go workspace URIs and build tags.
- [x] **3.3. Generic LSP Engine (`crates/prod-code-engine-generic`)**
  - Pluggable adapter for external language servers (e.g. Pyright, Ruff, vtsls).
  - Lifecycle management: automatic process spawning, health pings, graceful shutdown on idle timeout.
- [~] **3.4. C / C++ Engine (`crates/prod-code-engine-cpp` / `clangd`)** — shipped 2026-09-19 through the generic engine: `clangd --background-index --compile-commands-dir=build`, `CMakeLists.txt` / `compile_commands.json` / `.clangd` synced, hover / definition / references / symbols verified on two Linux nodes; `prod-code check` configures the CMake build dir (with `compile_commands.json`) and parses gcc/clang diagnostics. A compiler cache shared across worktrees followed on 2026-09-24 (#243): ccache with `CCACHE_BASEDIR` set to each workspace; a second worktree of the fmt library built in 1.0 s against 24.0 s. Shared PCH / clangd index across worktrees still open.
  - Supervised `clangd` daemon with background indexing over `compile_commands.json`.
  - Shared precompiled header (PCH) and symbol index cache on server NVMe/RAM-disk across multiple worktrees.
  - Offloads multi-gigabyte AST indexing for massive C++ codebases (e.g. Chromium, ClickHouse, trading engines) from local laptops to 32–128 core servers.
- [~] **3.5. TypeScript & JavaScript Engine (`crates/prod-code-engine-ts` / `vtsls`)** — shipped 2026-09-19: the native TypeScript 7 language server (`tsc --lsp --stdio` from the global `typescript` install, no Node in the query path) with fallback to `typescript-language-server`; hover / definition / references / symbols verified on both nodes, `prod-code check` = `tsc --noEmit`. Shared `node_modules` volume still open.
  - Supervised `vtsls` worker pool running on server Bun/Node runtime.
  - Shared global `@types/*` and `node_modules` cache volume to eliminate duplicate multi-gigabyte `node_modules` across concurrent agent worktrees.
  - Instant type inference and signature resolution for React, Vue, Svelte, Next.js, and large monorepos (5–15 ms latency).
- [~] **3.6. Python Semantic Engine (`crates/prod-code-engine-python` / `basedpyright`)** — shipped 2026-09-19: `basedpyright-langserver` (fallback pyright / ruff / pylsp), hover / symbols verified on both nodes, `prod-code check` = `basedpyright --outputjson`, `prod-code test` = pytest with parsed failures. Shared venv stub cache still open.
  - Managed `basedpyright` / `pyright` daemon with shared virtual environment stub cache.
  - Accurate cross-file semantic reference discovery (`code_references`) eliminating the false-positive noise and token waste of text-based grep.
  - Deep type inference for Pydantic, FastAPI, PyTorch, and typing annotations.
- [~] **3.7. Swift Engine (`crates/prod-code-engine-swift` / `sourcekit-lsp`)** — shipped 2026-09-19 on a macOS node: a Mac Studio (launchd unit `com.prod-code.gateway`) runs Xcode's `sourcekit-lsp`; hover / definition / references / symbols verified on a SwiftPM fixture after `prod-code check` (`swift build`, diagnostics parsed), `prod-code test` parses XCTest and swift-testing output. The Linux gateways do not list `swift`, so the client places Swift checkouts on the Mac node only. Apple-framework code (AppKit/UIKit/SwiftUI, `.xcodeproj`) can only be served there; pure SwiftPM packages could also run on Linux with the swift.org toolchain (not installed). Shared `ModuleCache` still open.
  - Supervised `sourcekit-lsp` daemon with shared `ModuleCache` and SPM package resolution.
  - Native support for Swift 6 concurrency, cross-file symbol indexing, and iOS/macOS frameworks without workstation build lag.


---

## Phase 4: Dual Surface — Native MCP Server for AI Agents

**Objective**: Provide first-class support for autonomous coding agents via the Model Context Protocol (MCP), removing the overhead of JSON-RPC LSP parsing for LLMs.

- [x] **4.1. Native MCP Server (`crates/prod-code-mcp`)**
  - Expose high-level, typed semantic tools directly consumable by Claude, Codex, Agy, and other agent frameworks:
    - `code_definition(path, line, character)`
    - `code_references(path, line, character, include_declarations)`
    - `code_outline(path, max_depth)`
    - `code_hover(path, line, character)` / `code_type_at`
    - `code_status()`
    - `code_sync(path)`
  - Compact declaration output by default (optimized for LLM context window efficiency).
  - Stdio JSON-RPC 2.0 protocol implementation with tool discovery and execution.
- [x] **4.2. Worktree Ingestion & Fast Sync**
  - Command: `prod-code sync` — push delta / worktree state to the remote server over 10G in < 200 ms.
  - In-memory temporary overlays and Salsa direct file mutation for live buffer edits.
  - [x] Isolated server workspace and analysis database per git worktree (`<repo>--wt-<hash>`); first contact sends a size/hash manifest probe, the gateway seeds the copy from the origin repository and asks only for missing files (2026-09-19).
  - [x] Binary-safe file transfer: `FileDelta.content` is base64 on the wire (`base64_bytes` in the protocol crate), not a JSON byte array, which cost a four-fold inflation and dominated sync time. Shipped in `c2b36f3`; the round trip and the wire form are covered by tests in `prod-code-protocol`.
  - [x] Persistent MCP session: one long-lived session per checkout for the life of the process (`session::pooled_query`), which pushes local changes only when the file watcher saw any and re-opens the session when the gateway restarts. Shipped in `c2b36f3`. What remains is the CLI, where every invocation is a new process and pays connect + handshake once (~0.75 s measured over the LAN); the MCP server, which is how agents call it, pays it once per process.

---

## Phase 5: Multi-Server Clustering, Smart Discovery & Fleet Scale

**Objective**: Scale `prod-code` across multiple physical servers on the 10G LAN to support massive agent fleets (50+ concurrent workers) with dynamic load balancing, repository affinity, and zero-configuration service discovery.

- [x] **5.1. Cluster Gateway & L4/L7 Dispatcher** — client-side placement shipped 2026-09-19 (`--remote a:9400,b:9400`, rendezvous hashing, remembered placement, failover, engine-aware placement). Server-side dispatch 2026-09-20: the client asks any node `PlaceRequest {workspace, engine}` and the node answers from its gossip view — the node that already holds the workspace, otherwise the quietest live node that serves the engine; the client goes there directly (no mid-handshake redirect needed, so sync never happens on the wrong node). Five nodes: three Linux, two macOS.
  - Distributed router dispatching incoming agent connections to the least-loaded server node.
  - Consistent hashing based on repository identity (`sha256(repo_common_dir)`) so sessions for the same codebase share warm Salsa, gopls, and clangd in-memory caches.
  - Transparent TCP redirection: if a client connects to Node A but the workspace is warm on Node B, Node A issues a `WireMessage::Redirect { target_addr }` allowing sub-millisecond client hop without repeating initialization.
- [x] **5.2. Smart DNS & Service Discovery (`*.code.internal`)** — done 2026-09-20 without DNS: one seed address is enough (`PROD_CODE_REMOTE=192.0.2.10:9400`); the client asks it for the gossip view (`ClusterRequest`), adds every live member and caches the list in `~/.local/share/prod_code/cluster.json` for when the seed is down. Gateways learn peers transitively from gossip, so a node needs only one live `--peers` entry. mDNS/SRV publication judged unnecessary on a static LAN.
  - Embedded lightweight DNS / mDNS resolver mapping projects to designated server nodes (e.g. `btcr.code.internal` -> `192.0.2.10:9400`, `codehaus.code.internal` -> `192.0.2.11:9400`).
  - Allows zero-config CLI and MCP usage (`prod-code -r auto ...` or `PROD_CODE_CLUSTER=10G`), eliminating hardcoded IP addresses.
  - Dynamic SRV record publication for active daemon instances across the LAN.
- [x] **5.3. Cluster Capacity Gossip & Dynamic Workload Rebalancing** — 2026-09-20: every gateway heartbeats its peers every 5 s (`Gossip`: load average, CPU count, RSS, engines, loaded workspaces with session counts, known peers); a peer silent for 30 s counts as stale. Placement answers move an idle workspace (0 sessions) off a node above 1.0 load/CPU to a node below half that; active sessions are never moved. `prod-code cluster` prints the gossip view of the whole cluster from one node.
  - Background gossip heartbeat between daemon nodes reporting CPU load, available RAM, active engine count, and in-flight builds.
  - Automatic load shedding: when a node approaches memory limits (e.g. > 85% RSS) or runs heavy test suites, new projects are assigned to quieter nodes (e.g. 128-core `rama` with 250 GB RAM).
  - [x] Idle LRU eviction: workspaces untouched for > 30 minutes are unloaded (`--idle-evict-secs`) and stale `<repo>--wt-*` copies pruned after 7 days (`--prune-worktree-days`); done 2026-09-19.
- [x] **5.4. Isolated Proc-Macro Worker Farm** — 2026-09-20: the Rust engine runs build scripts on load (`cargo check` in the warm per-workspace target dir) and expands proc macros through rust-analyzer's out-of-process proc-macro server (`ProcMacroServerChoice::Sysroot`, up to 8 processes per workspace), so derives and attribute macros resolve (serde files: 0 false errors; prod-code loads in ~7 s with build scripts). `prod-code.toml [rust] build_scripts = false` switches it off for repositories whose build scripts cannot run on the gateway.
  - Offload compilation and execution of heavy Rust procedural macros into a sandboxed worker pool.
- [x] **5.5. Agent Fleet Stress Verification** — `divergent-bench --persistent --churn N` drops N% of sessions without a goodbye (simulated agent SIGKILL), reconnects and verifies the gateway retires every session: 24 workers, 960 queries, 193 dropped sessions, 0 errors, gateway healthy with 0 sessions afterwards (2026-09-19). `bench/masstest.py` and 50+ worker runs still open.
  - End-to-end load tests using `bench/masstest.py`:
    - 20+ concurrent workers across 500+ file codebases.
    - Continuous semantic queries mixed with uncommitted `didChange` edits.
    - Simulated worker SIGKILL churn waves to verify clean session retirement and zero daemon hangs.

---

## Phase 6: Polyglot Remote Build & Test Execution (RTE / RBE)

**Objective**: Offload heavy compilation, test execution, linters, and benchmarks across all supported languages (Rust, Go, C++, TypeScript, Python, Swift) from local workstations to high-performance remote server nodes (32–128 cores) over the 10 GbE LAN, eliminating local CPU lockups, thermal throttling, and battery drain.

### The Problem: Local Workstation Bottlenecks Across Stacks
- Compiling and running test suites across modern tech stacks is notoriously punishing on laptops:
  - **Rust**: `cargo check/test/clippy` monomorphization, macro expansion, and linking spike CPU to 100% and drain battery.
  - **C / C++**: `cmake/ninja` compilation of template-heavy headers consumes dozens of gigabytes and freezes developer UI.
  - **TypeScript**: `tsc --noEmit` and `vitest/jest` run single-threaded Node.js processes that choke on large mono-repos and duplicate multi-gigabyte `node_modules`.
  - **Go**: `go test -race ./...` compiles and links multiple package test binaries concurrently.
  - **Python**: `pytest` across large test suites stalls on single-machine CPU cores.
- For autonomous AI coding agents (Claude, Codex, Agy), 95% of compilation and test invocations are executed solely for **verification feedback** (checking if edits compile cleanly, if test assertions pass, and if linters are satisfied). The binary itself is rarely needed locally on macOS.
- Running multi-agent fleets with concurrent local builds quickly freezes workstation UI, locks file caches, and throttles agent iteration speed.

### Engineering Milestones

- [x] **6.1. Polyglot Remote Execution Wire Protocol (`crates/prod-code-protocol`)** — basic `ExecRequest` / streamed `ExecChunk` / `ExecExit` shipped 2026-09-19 (argv, env, timeout). `ExecExit.usage` added 2026-09-23 (#180): CPU user/sys time and peak RSS of the command and its children, read with `wait4`. Typed runs done 2026-09-23 (#214): `check`/`lint`/`test`/`benchmarks` take `--env` (MCP `env`); `--events` streams each diagnostic (cargo JSON) and test result (`cargo test`, `go test -json`) as a JSON line as it arrives, then the report; the report carries `usage`. vitest and pytest results come in the final report, not as events.
  - Define `RemoteExecRequest`:
    - `language`: `rust`, `go`, `cpp`, `typescript`, `python`, `swift`.
    - `command`: `check`, `test`, `lint`, `bench`, or custom runner command.
    - `args`: Command arguments and test filters (e.g. `["--lib", "test_order_manager"]` or `["-k", "test_auth"]`).
    - `env`: Explicit environment variables (e.g. `RUST_BACKTRACE=1`, `NODE_ENV=test`).
    - `format`: `raw` streaming or structured `json` (parsing Cargo `--message-format=json`, `go test -json`, `vitest --reporter=json`, `pytest --json-report`).
  - Define `RemoteExecStream` and `RemoteExecResult`:
    - Real-time streaming of stdout/stderr chunks over 10G TCP with sub-millisecond latency.
    - Structured compiler and test diagnostic events (spans, error codes, failed assertion diffs, stack traces) streamed directly to client/agent.
    - Final execution summary: exit code, wall-clock duration, server CPU user/sys time, peak memory RSS.

- [~] **6.2. Server-Side Execution Engine & Warm Polyglot Caches (`crates/prod-code-gateway`)** — commands run inside the synced workspace copy; `target/` and `node_modules/` persist per workspace between runs (2026-09-19).
  - Dispatch execution to dedicated high-performance Linux worker nodes (a 32-core x86 node, or a 128-core Ampere node / 250 GB RAM).
  - Persistent server-side build caches on fast NVMe / RAM-disk (`/dev/shm`):
    - Rust: shared `~/.cargo/registry`, `target/` on NVMe.
    - Go: warm shared `GOCACHE` and `GOPATH/pkg/mod`.
    - C++: shared `ccache` / `sccache` and precompiled headers.
    - TypeScript: shared global `pnpm` store and pre-resolved `@types/*`.
    - Python: pre-warmed `.venv` wheels and pycache.
  - Because code deltas are synced incrementally in < 2 ms, only modified files trigger re-compilation; dependencies stay permanently warm in server RAM.

- [~] **6.3. Concurrent Multi-Worktree Build Isolation** — every worktree owns `<repo>--wt-<hash>` with its own build cache (2026-09-19). Process-group supervision was checked on 2026-09-23. Each exec runs in its own process group (`process_group(0)`), and the whole tree is killed on a timeout or a client disconnect. `bash -c '(sleep 283 &); sleep 282'` left no process on the node after a 3 s timeout, and none when the client was killed.
  - Isolated build artifacts per worktree session to eliminate build cache lock contention across concurrent agents.
  - Shared read-only dependency artifact cache across worktrees.
  - Process group supervision: automatic SIGKILL tree cleanup on client disconnect or timeout.

- [x] **6.4. Client CLI & Native Agent MCP Integration** — `prod-code exec -- <cmd>` and MCP tool `code_exec`; typed `prod-code check | lint | test [FILTER]` and MCP `code_check`, `code_lint`, `code_test` with structured diagnostics (cargo JSON, rustc text, libtest failures, go build/vet, go test -json) (2026-09-19). `--json` prints the full report (2026-09-19). Benchmarks followed on 2026-09-23 (#178). `prod-code benchmarks [FILTER]` and MCP `code_benchmarks` run `cargo bench --workspace` or `go test -run '^$' -bench`. The results are parsed from criterion (estimate and interval, with a long name read from the line before), libtest (`ns/iter (+/- N)`) and Go (`ns/op`). `prod-code bench` stays the gateway's own load benchmark. CPU and peak RSS in the exec summary followed on 2026-09-23 (#180).
  - **Client CLI Commands**:
    - `prod-code check`: remote compilation check across any language with instant terminal diagnostics.
    - `prod-code test [FILTER]`: remote test runner with real-time test output streaming.
    - `prod-code lint`: remote linter (`clippy`, `golangci-lint`, `eslint`, `ruff`).
    - `prod-code bench [FILTER]`: remote benchmark runner on quiet, dedicated server cores without workstation thermal noise.
  - **Agent MCP Tools (`crates/prod-code-mcp`)**:
    - `code_check(path)`: returns structured compiler errors and warnings directly into agent context.
    - `code_test(path, filter)`: runs targeted tests and returns failures with panic backtraces and assertion diffs.
    - Instant verification feedback in 1–3 seconds per edit without touching local machine CPU or battery.

---

## Phase 7: Distributed Semantic Refactoring & Agent Intelligence Engine

**Objective**: Elevate `prod-code` from a code-reading intelligence layer into an active, AST-driven semantic transformation and refactoring engine, combining local file ownership with remote 128-core AST graph reasoning.

### The Problem: AI Code Editing Hallucinations & Inefficiencies
- Current AI coding agents rely on brittle string replacement (`regex`, `replace_file_content`) or rewriting whole files from memory. This routinely introduces syntax errors, renames unintended identifiers in comments/strings, breaks trait contracts, misses cross-file references in large mono-repos, and wastes hundreds of thousands of LLM tokens on trial-and-error edits.
- Standard RAG approaches feed entire multi-thousand-line files into model context windows, drowning the LLM in boilerplate and causing "lost in the middle" attention degradation.

### Engineering Milestones

- [~] **7.1. Full-Spectrum AST Refactoring Engine (IDE-Grade Parity Catalog)** — status 2026-09-24: the catalog is complete for Rust (every item below is `[x]` or *not applicable to Rust*, except the two marked Rust-only). The other engines have rename (`code_rename`) and their own servers' code actions (`code_assists` / `code_assist`), but not the tools built here (change_signature, move, extract_parameter, introduce_parameter_object, encapsulate_field, …), which read Rust. That multi-language half is what stays open (#13).
  - Implement a compiler-grade distributed refactoring engine providing full behavioral parity with modern IDE refactoring suites. Remote language servers (`rust-analyzer`, `clangd`, `gopls`, `vtsls`, `basedpyright`) compute mathematically sound, AST-level code transformations, resolving all symbol references across the monorepo and returning structured, atomic `WorkspaceEdit` payloads:
  - **Reconciled against the analyzer 2026-09-22.** This catalog was written from IntelliJ's
    refactoring menu and never checked against what rust-analyzer already offers, which made it
    read as far more outstanding work than it is. Every `[~] via code_assists` below was verified
    by asking the running analyzer for its actions at that kind of position; the assist id named
    is what it answered. What is left to build is four items, listed under **Still to build**,
    plus the ones marked *not applicable to Rust*.
  - **Still to build**: nothing of the original list. The conversion half of `type_migration`
    shipped 2026-09-23 (#119). `introduce_parameter_object`, `extract_parameter`, `extract_field`,
    the reporting half of `type_migration` and the workspace half of `encapsulate_field`
    shipped 2026-09-22. Nothing in rust-analyzer offers
    these, so each is a tool of its own, the same shape as `change_signature` and `move`: read
    the declaration, plan the edit, rewrite the use sites, type-check the whole thing in one
    overlay before writing.
  - **Probed again on 2026-09-23** with an example of each shape in a scratch crate
    (`prod-code assists <file> <line> <col>` at each position):
    `invert_if_to_guard` is offered as `convert_to_guarded_return` at an `if let` / `if` that
    ends the function; `wrap_return_value` is half offered, as `wrap_return_type_in_option` and
    `wrap_return_type_in_result`, which rewrite the signature and the returned values but none of
    the callers; `invert_boolean`, `make_static` and `generify` are not offered at all
    (`convert_bool_to_enum` and `unwrap_type_to_generic_arg` are different refactorings). The
    caller half of `wrap_return_value` shipped the same day (`code_wrap_return`), and so did `make_static` (`code_make_static`) and `invert_boolean` (`code_invert_boolean`); `generify` (`code_generify`) followed, so all five are built.
  
  - **7.1.1. The Core Five (Everyday Essential Refactorings)**:
    - [x] `refactor.rename(path, line, col, new_name)` — shipped 2026-09-19 for Rust: `prod-code rename`, MCP `code_rename`, LSP `textDocument/rename`; whole-workspace rewrite incl. module file moves, 1.8 ms server-side on the fixture, edits applied to the checkout and recorded in the sync watermark. Paired accessors followed on 2026-09-23 (#146): with `accessors`, a field's rename also renames the methods of its struct's `impl` blocks named after it (`f`, `get_f`, `set_f`, `f_mut`) with every call; each rename is the analyzer's, and they are merged into one change per file token by token against the checkout, refused where two would change the same identifier, and type-checked before writing. Comments and test names followed on 2026-09-23 (#174). With `comments`, the old name is replaced where it stands as a whole word in comments. Its snake_case form is replaced in the names of test functions (`order_total_rounds_up` -> `trade_total_rounds_up`). This covers every file the rename touches, in the same type-checked change. It is not combined with a rename that moves files. Prose around the name ("an `Order`") is not adjusted.
      - Semantic symbol renaming (types, traits, fields, parameters, local variables, modules/packages, filenames).
      - Cross-reference propagation: automatically updates all references, doc comments, test names, and optionally paired getters/setters/accessors in < 20 ms with zero regex hallucinations.
    - [x] `refactor.change_signature(path, symbol, new_params, new_return_type)` — shipped 2026-09-21 for Rust (parameters only): `code_change_signature` / `prod-code change-signature` take the parameter list the function should end up with, build the SSR rule for the call sites from the declaration's own arity and resolve it in the declaring file's scope, reconcile what was rewritten against the analyzer's references and name what was not, refuse to drop a parameter the body still uses, and type-check the declaration and every call site together before writing. The return type and the visibility followed on 2026-09-23 (#144): `returns` (`()` removes it) and `visibility` (`pub`, `pub(crate)`, …, `private`) are written into the declaration in the same edit, and every file that calls the function is type-checked against it, so a body that no longer returns the new type and a caller that no longer fits are reported before anything is written. Making a function `async`, with `.await` at every call, followed on 2026-09-23 (#176; see below). `async` followed on 2026-09-23 (#176): `async: true|false` (`--async`) adds or removes `async` in the declaration (before `unsafe`) and `.await` after every call the analyzer lists; a call that would await from a function that is not `async` is named and blocks the write unless `force`, as does the analyzer's E0728, and it is refused together with a change of the parameter order.
      - Add, remove, reorder, and rename parameters; modify return type, visibility, and async/throws modifiers.
      - Automatically generates type-safe default arguments or expressions across all existing call sites in the monorepo.
    - [x] `refactor.safe_delete(path, symbol)` — shipped 2026-09-19 for Rust: `prod-code safe-delete <file> <line> <col>`, MCP `code_safe_delete`, gateway `prodCode/safeDelete`; whole-workspace usage check via rust-analyzer, refusal returns the usage dossier, deletion uses the item's structure range. Cascading parameter removal shipped 2026-09-23 (#138): at a parameter, `code_safe_delete` / `prod-code safe-delete` remove it from the declaration and its argument from every call through `change_signature`, refused while the body uses it and type-checked before writing; a position that names no item is refused instead of deleting the smallest enclosing item's range. A trait method's parameter followed on 2026-09-23 (#194): given in the trait or in an implementation, it is removed by position from the trait's declaration, every implementation (whatever each calls it) and every call (a path call passes the receiver first); a body that uses it, an argument that does something (a call, a macro, `?`, `.await`) and a use of the method as a value block the write until `force`, and the whole change is type-checked first.
      - Whole-repository usage graph verification before deleting classes, structs, functions, fields, or parameters.
      - If active usages exist, returns a structured conflict dossier; supports safe cascading parameter removal across callers and override hierarchies.
    - [x] `refactor.move(path, symbol, target_destination)` — shipped 2026-09-22 for Rust, items between modules: `code_move` / `prod-code move <symbol> --to <file>` cut the declaration whole (doc comment and attributes with it), carry the `use` statements the item spells narrowed to the names it needs, rewrite or add the import in every file the analyzer lists as using it, requalify the references that name a path, and type-check the result in one overlay before writing. A target module that does not exist yet is created and declared in its parent (`pub mod x;` for a `pub` item) in the same change (#148, 2026-09-23). A whole module moves since 2026-09-23 (#188): `code_move_module` / `prod-code move-module <its file> --to <new file>` move the file and the directory of its submodules, take the `mod` declaration (attributes, doc comment, visibility) from the old parent to the new one, spell every path the analyzer lists anew (qualified paths get the new parent; a bare use in the old parent and a grouped import get an import of their own; an import in the new parent that would clash is dropped), turn `super::` in the moved file into the old parent's absolute path, and type-check the whole change before writing it. `code_move` on a `mod x;` line refuses and points there. A method moves to the type of one of its parameters since 2026-09-23 (#196): `code_move_method` / `prod-code move-method <file> <line> <col> --to-param <name>` make the parameter the receiver (borrowed as it was) and the old receiver a parameter in its place, swap `self` and the parameter in the body and spell out `Self`, put the method into the new type's inherent `impl` (made after the type when there is none), and swap receiver and argument at every call (`o.m(t, 1)` → `t.m(&o, 1)`, `A::m(o, t, 1)` → `B::m(t, o, 1)`); a call whose receiver or argument does something, a recursive method, a method used as a value, trait implementations and generic `impl` blocks are refused. `code_move` on a method refuses and points there. A static member (an associated function) moves too since 2026-09-23 (#198): `to_type` / `--to-type <Type>` puts it into that type's inherent `impl`, spells `Self` out, points every path to it (called or used as a value) at the new type, and drops an `impl` it leaves empty.
      - Moves structs, functions, classes, or files to new modules, packages, or namespaces.
      - Moves static members to another type; moves instance methods to a target parameter type (e.g. `fn foo(bar: &Bar)` -> `Bar::foo()`).
      - Automatically rewrites and cleans all `use` / `import` statements and qualified path references throughout the workspace.
    - [x] `refactor.inline(path, symbol)` — via `code_assists`: `inline_local_variable` and `promote_local_to_const` at a local, `inline_into_callers` and `inline_call` at a function. Inline Parameter shipped 2026-09-23 (#142) as `code_inline_parameter` / `prod-code inline-parameter <file> <line> <col>`: when every call passes the same literal, constant or path for a parameter, the value is bound at the top of the body (`let max: u32 = LIMIT;`) and the parameter leaves the declaration and every call; calls that disagree, a lowercase value that may be the caller's local, the function used as a value and a call inside the function itself are refused. Type-checked before writing.
      - Inline Function/Method: substitutes call sites with the function body, rebinding parameters and handling early returns.
      - Inline Variable/Constant: inlines computed expressions into usage sites and eliminates redundant bindings.
      - Inline Parameter: eliminates parameter by inlining constant values across all callers.

  - [x] Code actions: `prod-code assists <file> <line> <col> [--to LINE:COL]` lists what rust-analyzer offers at a position or selection and `prod-code assist … <id>` applies it (MCP `code_assists` / `code_assist`). This covers `inline_local_variable`, `inline_call`, `extract_function`, `extract_variable`, `extract_constant`, `extract_static`, `promote_local_to_const`, `add_explicit_type`, generate/rewrite assists and quick fixes in one mechanism (2026-09-19).
  - **7.1.2. The Extract & Introduce Family**:
    - [x] `refactor.extract_function(path, range, fn_name)` — via `code_assists` over a selection: `extract_function`. Named extraction with duplicates shipped 2026-09-23 (#186) as `code_extract_function` / `prod-code extract-function <file> <line> <col> --to LINE:COL --name …`. rust-analyzer extracts the selection. Every other place in the same file with the same text (whitespace aside) gets the same call, where the result type-checks with it; each duplicate left is reported with the reason. A result with a replaced duplicate is compiled (`cargo check` in a shadow) before it is written, because the analyzer does not check borrows. Near-duplicates and other files followed on 2026-09-23 (#212): copies are matched token for token; with `parameterize` a copy may differ in literals of the same kind, and each literal that differs becomes a parameter of the new function, typed by the analyzer's hover on the selection's own literal, with every call passing its place's literal (the parameters are kept only if such a copy is); with `other_files` the crate's other files are searched too, a copy there calls the function through its module path, and the function becomes `pub(crate)`; a method is not offered across files.
      - Remote compiler analyzes variable captures, borrow checker constraints, and lifetimes, returning the extracted signature and replacement call site.
      - **Automated Duplicate Code Detection**: automatically scans the entire file and workspace for duplicate or structurally identical AST patterns, offering to parameterize and replace all instances in one operation.
    - [x] `refactor.introduce_variable(path, range, var_name, replace_all)` — one occurrence via `code_assists` over a selection (`extract_variable`); every occurrence shipped 2026-09-23 (#150) as `code_introduce_variable` / `prod-code introduce-variable <file> <line> <col> --to LINE:COL --name …`: the binding goes above the statement that holds the first occurrence, in the innermost block that holds them all, and every whole-token occurrence in the function reads it. An expression that calls, expands a macro, uses `?` or awaits is refused. So is one whose names change between the binding and the last occurrence, or in a loop that runs a later occurrence again. Type-checked before writing.
      - Replaces selected expression with a local binding, with toggle to replace the single occurrence or all identical expressions.
    - [x] `refactor.extract_constant(path, range, const_name)` — via `code_assists` over a selection: `extract_constant` and `extract_static`.
      - Promotes magic numbers, string literals, or complex expressions to module-level or struct-level typed `const`/`static`.
    - [x] `refactor.extract_field(path, range, field_name)` — shipped 2026-09-22 for Rust as `code_extract_field` / `prod-code extract-field <file> <line> <col> --to LINE:COL --name … --type …`: the method reads `self.<field>`, the struct declares it, and every `Type { … }` and `Self { … }` initialises it (with the expression, or `init`); a pattern that lists every field is reported and blocks the write.
      - Promotes local variables or initialization logic to a struct/class field, adjusting constructor/initialization blocks.
    - [~] `refactor.extract_parameter(path, range, param_name)` — shipped 2026-09-22 for Rust: `code_extract_parameter` / `prod-code extract-parameter <file> <line> <col> --to <line>:<col> --name …` add the parameter at the end of the list keeping its shape, replace the selection (or every identical occurrence with `replace_all`) in the body, and pass the original expression at every call site so no existing caller changes behaviour. An expression that names a local is refused with that as the reason.
      - Promotes an internal expression to a function parameter, automatically passing the original expression at all existing call sites.
    - [~] `refactor.introduce_parameter_object(path, symbol, param_indices, struct_name)` — shipped 2026-09-22 for Rust: `code_introduce_parameter_object` / `prod-code parameter-object <symbol> --param … --name Opts` generate the struct above the declaration with the declared types (one lifetime when any of them borrows), rewrite the declaration and the body uses at the analyzer's positions, and rewrite every call site in place — the bundled arguments become one literal where the first of them was, and a caller in another module gets the import it needs. A use that is not a call with this arity is reported, not mangled.
      - Solves parameter bloat (> 3-4 arguments) by bundling related parameters into a typed DTO/struct/record, rewriting definition and all call sites.
    - [x] `refactor.extract_trait / extract_interface(path, symbol, method_names, trait_name)` — the whole impl via `code_assists` (`generate_trait_from_impl`); a chosen subset shipped 2026-09-23 (#153) as `code_extract_trait` / `prod-code extract-trait <file> <line> <col> --methods a,b --name …`. Only the named methods move into `trait Name` and `impl Name for Type`, and the rest stay inherent. The trait is as visible as the widest moved method. Every other file that references a moved method imports it. The change is type-checked before writing. Generic `impl` blocks are refused.
      - Extracts selected public method contracts into a new trait/interface, marks the original struct as implementing it, and updates caller type annotations to use the trait where applicable.
    - [x] `refactor.extract_delegate(path, symbol, delegate_methods, delegate_name)` — one field at a time via `code_assists` (`generate_delegate_trait`, `generate_delegate_methods`). A group of fields with its methods shipped 2026-09-23 (#172) as `code_extract_delegate` / `prod-code extract-delegate <file> <line> <col> --fields … --methods … --name … --field …`:
      - the fields move into a helper type the struct holds;
      - the methods that use only them move to it, and the struct keeps a forwarding method with the same signature;
      - every other access, found through the analyzer's references, goes through the new field;
      - every literal of the struct builds the helper.

      The result is type-checked before anything is written.
      - Extracts selected responsibilities into a separate helper class/struct, replacing direct implementations with an encapsulated delegate field.

  - **7.1.3. Hierarchy, Trait & Compositional Transformations**:
    - *Not applicable to Rust.* `refactor.pull_up / push_down(path, member_symbols, target_level)`:
      - Moves methods, fields, and constants up to superclasses/traits or down to specific subclasses/implementations.
    - [x] `refactor.encapsulate_field(path, struct_name, field_name)` — shipped 2026-09-22 for Rust as `code_encapsulate_field` / `prod-code encapsulate-field`: the field becomes private, a getter (by value for primitive `Copy` types, by reference otherwise) and, when anything writes it, a setter are added to the struct's `impl`, and every access outside the declaring file is rewritten — reads to `x.f()`, plain writes to `x.set_f(v)`. Uses that cannot be a method call (a struct literal or pattern outside the file, `+=`, `&mut x.f`) are reported and block the write; the change is type-checked in one overlay, with `verify: "compile"` for the borrows the analyzer does not check. The single-field assists (`generate_getter`, `generate_setter`, `change_visibility`) remain available through `code_assists`.
      - Converts public fields to private, generates idiomatic getters/setters/accessors, and rewrites all direct field accesses across the repository.
    - *Not applicable to Rust* — there is no inheritance to replace. `refactor.replace_inheritance_with_delegation(path, sub_type, base_type)`:
      - Enforces "Composition over Inheritance": wraps the base class in a private field and forwards inherited method calls.
    - *Not applicable as written* — Rust has no constructors; the useful half is generating a builder, which belongs with the generate assists. `refactor.replace_constructor_with_factory / builder(path, type_name)`:
      - Replaces raw struct instantiations with named static factory methods or a fluent builder pattern.
    - [x] `refactor.make_static / convert_to_method(path, function_name)` — `make_static` shipped 2026-09-23 for Rust as `code_make_static` / `prod-code make-static <file> --line N --character C` (rust-analyzer offers nothing here): a method whose body never mentions `self` loses its receiver, `value.method(args)` becomes `Type::method(args)` and `Type::method(value, args)` loses its first argument; a receiver that does something when evaluated (`load()?.method()`) is reported and blocks the write. The other direction shipped the same day (#120) as `code_convert_to_method` / `prod-code convert-to-method <file> --line N --character C` (rust-analyzer offers only `destructure_struct_binding` at the parameter): an associated function whose first parameter is the `impl`'s own type (`T`, `&T`, `&mut T`, `Self`) gets that parameter as its receiver, the parameter's uses in the body — as the analyzer resolves them — become `self`, and `Type::f(&mut x, a)` becomes `x.f(a)`. The function used as a value and a call inside the function itself stay as they are, valid through the path; a trait impl's function and a free function are refused.
      - Converts receiver-independent methods to static functions (or vice-versa), adjusting all call sites (`x.foo()` <-> `Type::foo(x)`).

  - **7.1.4. Data Flow & Advanced Type-System Refactorings**:
    - [x] `refactor.type_migration(path, symbol, target_type)` — shipped for Rust: `code_migrate_type` / `prod-code migrate-type` rewrite the declared type (field, parameter, return type, annotated `let`) in memory, type-check the workspace in one overlay, and report every site that no longer fits with its source line, suggesting the conversion where the error names both types (2026-09-22). With `convert` (2026-09-23, #119) `.into()` is written at every site where the old and new types meet and the overlay is checked again: a conversion stays only where the analyzer accepts it, a rejected one is taken back and its site reported as tried, and a set that causes an error anywhere else is dropped whole. `u32` → `u64` converts the widening sites and leaves the narrowing ones; `String` → `Box<str>` converts every site. Deliberately not done: a whole-program constraint graph that migrates the variables and signatures a value flows through. A site that should be migrated too is reported, and migrating it is its own run, with its own report.
      - Whole-program type migration: changes a symbol's type (e.g. `u32` -> `u64`, `String` -> `Uuid`, `T` -> `Option<T>` or `Result<T, E>`).
      - Solves whole-program data-flow constraint graph: computes transitively affected variables, return signatures, function parameters, and call sites.
      - Automatically injects necessary type conversions (`.into()`, `Some(...)`, `?`) or returns a guided conflict dossier for ambiguous coercions.
    - [x] `refactor.invert_boolean(path, symbol)` — shipped 2026-09-23 for Rust as `code_invert_boolean` / `prod-code invert-boolean <file> --line N --character C --to NEW` (rust-analyzer offers only `convert_bool_to_enum`, a different refactoring): a function returning `bool` gets the new name, its body returns the negation (every `return` of the function, not of a closure or nested `fn`), every call gains a `!` or loses the one it had, a call followed by `.`, `?` or an index is parenthesized, a reference that is not a call is named, and a recursive predicate is refused. A `bool` field or `let` binding followed the same day (#132): every read gains a `!` or loses the one it had, every write (assignment, initialiser, struct literal field, shorthand) stores the negation, and a borrow, a compound assignment, a binding pattern, a format-string use, a derived `Default` or a serde derive is reported and blocks the write; a local's type comes from the analyzer when it is not annotated, because `!` also compiles on an integer.
      - Inverts boolean variable, field, or function predicate (e.g. `is_valid` -> `is_invalid`, `has_access` -> `access_revoked`).
      - Flips internal return expressions and inverts every single caller/usage with `!` negation across the entire monorepo.
    - [x] `refactor.generify(path, symbol)` — shipped 2026-09-23 for Rust as `code_generify` / `prod-code generify <function> --param NAME --bound TRAIT [--as T]` (#117; rust-analyzer offers only `unwrap_type_to_generic_arg`, a different refactoring): the parameter's type becomes a type parameter with the given bound, a reference in front of it is kept (`&Vec<u32>` → `&T`), a function that already has generics gets one more, and a type parameter name already in use is refused. Callers are not rewritten — the type argument is inferred — but every file that calls the function is type-checked in the same overlay, which catches both a body that needs more than the bound and a caller that stops compiling unedited (a `"x".into()` that took its target from the old type). The bound is the user's choice; nothing is inferred from the body.
      - Introduces generic type parameters `<T>` where concrete or dynamic types were used, updating callers with explicit or inferred type arguments.
    - [x] `refactor.wrap_return_value(path, symbol, wrapper_type)` — shipped 2026-09-23 for Rust as `code_wrap_return` / `prod-code wrap-return <file> --line N --character C --wrapper option|result [--error TYPE]`: rust-analyzer's `wrap_return_type_in_option` / `_in_result` rewrite the signature and the returned values (the `_` error type filled in with `error`), every caller that already returns the same wrapper gets `?`, and every other caller is reported with its line and blocks the write; a recursive call is left for a person. Type-checked in one overlay, `verify: "compile"` available.
      - Wraps function return types into `Result<T, Error>`, `Option<T>`, or custom envelopes, updating all return statements and wrapping call sites with `?` or `match`.

  - **7.1.5. Modernization & Control Flow Transformations**:
    - [x] `refactor.invert_if_to_guard(path, range)` — via `code_assists` at an `if let` or `if` that ends a function or a loop body: `convert_to_guarded_return` (probe of 2026-09-23).
      - Flips conditional branches to early returns (`guard clauses`), reducing nested block indentation depth from 5+ levels to 1.
    - *Not applicable to Rust* — a `match` on an enum is the idiom, not a smell to remove. `refactor.replace_conditional_with_polymorphism(path, range)`:
      - Replaces large `match`/`switch`/`if-else` cascades on enum/type tags with polymorphic trait/interface method dispatch.
    - [x] `refactor.loop_to_iterator(path, range)` — the analyzer offers `convert_for_loop_with_for_each` and `convert_for_loop_to_while_let` at a loop. Both keep the mutable accumulator. Accumulator loops shipped 2026-09-23 (#164) as `code_loop_to_iterator` / `prod-code loop-to-iterator <file> <line> <col>`, in three shapes, each optionally under one `if`:
      - a sum from zero, into `.map(..).sum()`;
      - a count into a `usize`, into `.filter(..).count()`;
      - a `Vec` built with `push`, into `.collect()`.

      A name that holds a reference is iterated with `.iter()`, as the analyzer's hover shows. `mut` stays only when the analyzer asks for it. `break`, `continue`, `return`, `?`, `.await`, a second use of the accumulator and a non-identity start are refused. The result is type-checked before writing.
      - Converts imperative `for`/`while` loops with mutable accumulators into idiomatic functional iterator chains (`.map().filter().fold()`).
    - [x] `refactor.structural_replace(path_pattern, search_template, replace_template)` — shipped 2026-09-21 as `code_codemod` / `prod-code codemod` (roadmap 8.7), on rust-analyzer's SSR. Listed here as well because the catalog was written before it existed.
      - Structural Search and Replace (SSR) engine: AST pattern templates with typed meta-variables (e.g. `$expr$.then($cb$)` -> `await $expr$`), transforming code across thousands of files irrespective of whitespace or variable naming.

  - [x] **7.1.6. Proactive Conflict Resolution & Transactional Applicator** — the first and third
    halves are how every write tool already works: each computes the whole multi-file edit,
    type-checks it in one overlay and refuses with the reason (a dropped parameter the body uses,
    a private sibling left behind, a local an extracted expression names) before writing, and each
    is one call. The overlay check did not see an unresolved type or a module path (#63); it does since
    #181 (2026-09-23), which reads rust-analyzer's unresolved-reference highlighting. Since
    #63 every write tool also takes `verify: "compile"`, which runs `cargo check` on the proposed files
    in a shadow of the workspace and writes only what the compiler accepts too. The applicator is
    transactional since #70: every path an edit touches is snapshotted before the first write and
    put back if any write fails, so a multi-file refactor lands whole or not at all.
    - **Conflict Detection & Pre-Validation**: detects shadowed identifiers, unresolvable ambiguities, visibility violations, and trait constraint breaches *before* applying any changes, emitting a structured conflict preview.
    - **Client-Side Atomic Transactional Applicator**: applies `TextEdit` batches directly to local files with microsecond latency, featuring automatic snapshot & instant rollback if any disk write fails.
    - **Zero-Prompt Agent Automation**: AI coding agents can execute complex multi-file architectural refactors with single RPC calls without hallucinating intermediate edits.

- [x] **7.2. Automated Compiler "Fix-It" & CodeAction Engine (Zero-Prompt Repair)** — code actions on every engine 2026-09-20: `prod-code assists | assist` and MCP `code_assists` / `code_assist` map to LSP `textDocument/codeAction` for gopls, clangd, the native TypeScript server, basedpyright and sourcekit-lsp (rust-analyzer stays in-process). Quick fixes get the server's diagnostics as context (push model cached, pull model queried), lazy edits are resolved, and command-only actions run through `workspace/executeCommand` with the `workspace/applyEdit` captured. Verified: TypeScript add-import, pyright ignore-comment, clangd extract-variable, gopls source actions, sourcekit convert-to-async. Auto-applying fix-its from a failing check shipped 2026-09-23 for Rust (#158). `fix: true` on `code_check` / `code_lint` (`prod-code check --fix` / `lint --fix`) applies every suggestion rustc and clippy mark `MachineApplicable`, all parts together, in one transactional edit. A suggestion reported by several targets is applied once. A fix outside the workspace, overlapping another, or on a line that changed is skipped with the reason. Then the check runs again. The other languages followed on 2026-09-23 (#205): `lint --fix` on Python, TypeScript and C++ runs the linter's own fix mode on the node (`ruff check --fix`, `eslint --fix` / `biome lint --write`, `clang-tidy -fix`), brings the files it rewrote back into the checkout, names each and lints again; `lint` on a C++ project runs clang-tidy over its sources with the build's compilation database (CMake or Meson; clang-tidy is installed in user space on the Linux build nodes). Go's `go vet` has no fixes, and the report says so.
  - Compilers and linters (`rustc`, `clippy`, `clang-tidy`, `gopls`, `ruff`) natively produce machine-applicable `CodeAction` / `Fix-It` recommendations.
  - Wire protocol endpoint: `code_quickfix(file, diagnostic_id)` returning pre-computed compiler diffs.
  - AI agents can inspect and apply exact compiler-suggested fixes in one step (e.g. missing trait imports, mutable borrow corrections, lifetime annotations) with zero LLM token consumption or hallucination loops.

- [x] **7.3. Program Slicing & Context Tree-Shaking (10x Token Reduction)** — shipped 2026-09-21: `code_slice` / `prod-code slice` walk the analyzer's definition and call edges from a seed symbol and return the declarations it depends on, grouped by file, with a depth and a byte budget; measured 92-96% smaller than the files an agent would otherwise read, in about a second. Data-flow slicing inside a body is not attempted: the unit is a declaration.
  - Program Dependency Graph (PDG) and data-flow analysis on remote server:
    - Given a target function or bug location, slice away all unreferenced structs, unrelated methods, and irrelevant imports.
    - `code_slice(path, symbol)`: extracts a minimal, self-contained semantic slice (e.g. 60 lines instead of 4,000 lines) representing 100% of data and control flow.
    - Reduces LLM context consumption by 85–95%, drastically lowering inference costs and model reasoning errors.

- [x] **7.4. Speculative In-Memory Shadow Workspaces (Parallel Multi-Hypothesis Execution)** — second step shipped 2026-09-21: `code_shadow_run` / `prod-code shadow-run` run a command per named hypothesis in overlay shadows of the workspace copy (user namespace + overlayfs mounted at the workspace path, warm caches valid, hypotheses in parallel; in-place sequential fallback), rank the outcomes and return only the winner's diff. First step 2026-09-20: `code_validate_edits` places several proposed file contents in one private analyzer overlay and reports diagnostics per file (plus `also_check` for unchanged callers), so a multi-file refactor is judged before anything is written. Remaining: named shadow branches, remote test runs per hypothesis, winning diff.
  - When an AI agent explores multiple competing architectural solutions or bug-fix hypotheses:
    - Server creates lightweight in-memory VFS overlays (`shadow-branch-1`, `shadow-branch-2`, `shadow-branch-3`) in RAM (`/dev/shm`).
    - Remote execution engine (Phase 6) runs full test suites against all hypotheses simultaneously across 128 server cores.
    - The server returns only the winning hypothesis's unified diff back to the client.
    - Local Mac disk and Git history remain clean of failed experimental churn.

- [x] **7.5. Call Graph & Type Hierarchy Navigation** — shipped 2026-09-19: `prod-code callers | callees | impls <file> <line> <col>` and MCP `code_callers`, `code_callees`, `code_implementations`; the Rust engine answers through rust-analyzer's call hierarchy and goto-implementation in-memory, the managed engines (gopls, clangd, native tsc, basedpyright, sourcekit-lsp) through the standard LSP call-hierarchy requests; verified on Rust, C++, TypeScript, Python and Swift fixtures. `code_dead_code` shipped with 8.6. Callers and callees to a depth shipped 2026-09-24 (#222): `depth` / `--depth N` walks the hierarchy transitively and prints a tree, a function already shown is marked instead of expanded again, and the tree stops at 300 functions; `callers --symbol handle_callers --depth 3` on this repository gives 121 functions in 5.7 s. Supertypes shipped 2026-09-24 (#224): `code_supertypes` / `prod-code supertypes` name the traits a Rust type implements (derives read from its attributes, written impls from its implementations; inherent impls are not supertypes) and a trait's supertraits; other languages ask their server's type hierarchy (`typeHierarchy/supertypes`) and say so when it has none.
  - Graph-level codebase exploration endpoints:
    - `code_callers(path, line, col)`: incoming call hierarchy across the entire workspace/monorepo in < 5 ms.
    - `code_callees(path, line, col)`: outgoing call graph tree.
    - `code_implementations(path, line, col)`: all structs/classes implementing a trait, interface, or abstract class.
    - `code_dead_code()`: whole-program graph reachability analysis identifying unused functions and types post-refactoring.

- [x] **7.6. Cross-Language Full-Stack Schema Refactoring** — shipped 2026-09-21 as `code_schema_rename` / `prod-code schema-rename`: one field, renamed across the languages that spell it differently (snake, camel, Pascal, Go's initialism form, SCREAMING, kebab). Identifiers are renamed by the analyzer of their own sub-project — which is what carries the change into files the scan never looked at — and only schema files and string literals (a `json:` tag, an SQL query) are edited as text, at the positions found. Colliding renames are skipped and reported rather than merged, identifiers in comments are reported rather than rewritten, and the result is type-checked per project before it is written. Verified on `fixtures/polyglot-order` (proto + SQL + Go + TypeScript + Rust). Both open items done 2026-09-23 (#216): OpenAPI documents (YAML or JSON with an `openapi`/`swagger` key) and GraphQL schemas are read for their structure — the field is rewritten where it is a key or a whole value (OpenAPI) or a name outside comments and description strings (GraphQL), and every mention in prose is listed; `--repo PATH` (MCP `repos`) plans several repositories with their own analyzers and writes all of them or none, putting back the ones already written when one cannot be written.
  - Unified multi-language schema evolution across polyglot repositories:
    - Changing a backend schema (Protobuf, OpenAPI, SQL, or Rust/Go data models) automatically coordinates with frontend TypeScript interfaces, API clients, and UI components.
    - Emits an atomic multi-repository `WorkspaceEdit` synchronizing backend and frontend simultaneously.

- [x] **7.7. Pre-Validation On-the-Fly (Instant Hallucination Interception)** — shipped 2026-09-20: `prod-code diagnostics <file>` and `prod-code validate <file> [--from NEW | stdin]` (MCP `code_diagnostics`, `code_validate_edit {path, new_text}`) return the analyzer's diagnostics for a file or for a proposed replacement text, in memory and without writing anything: rust-analyzer's full diagnostics from the Salsa database (type errors, unresolved names, unused items — no cargo check), pull diagnostics from the native TypeScript server and gopls, published diagnostics from basedpyright, clangd and sourcekit-lsp. Measured 0.1–0.6 s per validation on the fixtures. Multi-file proposals followed (`code_validate_edits`, `validate --with`), and every MCP write tool type-checks its own edit before writing it. A unified diff or a WorkspaceEdit is checked too since 2026-09-23 (#200): `prod-code validate --diff PATCH|-` and `code_validate_edits` with `diff` or `workspace_edit` apply the change in memory (each hunk where it says, or where its old lines moved to; one that fits nowhere is refused by number; a new file is created in the overlay, a deleted one named) and check every file it touches together.
  - Streamed syntax and type check verification during agent code generation.
  - Intercepts invalid method invocations, incorrect argument types, or borrow-checker errors before the agent even finishes generating its turn, providing immediate feedback and eliminating multi-turn debugging cycles.

---

## Phase 8: Autonomous Agent Fleet Superpowers & Workflow Acceleration

**Objective**: Equip autonomous AI coding agents (Claude, Codex, Agy) with specialized semantic tools that eliminate trial-and-error reasoning loops, slash LLM token waste, and accelerate development velocity across multi-thousand file repositories.

### The Problem: Agent Productivity Bottlenecks in Production
- AI agents spend up to 70% of their execution time and context budget on repetitive diagnostic overhead: reading dozens of files trying to locate where a test panicked, running entire test suites after a 3-line edit, guessing function names with regex grep, writing hundreds of lines of boilerplate test mocks, and leaving zombie code behind after large refactors.

### Engineering Milestones

- [x] **8.1. Selective Test Execution & Blast Radius (`code_impact_analysis`)** — shipped 2026-09-20 as `prod-code impact [--base REF] [--depth N] [--run] [--json]` and MCP `code_impact`: the diff's line ranges are mapped onto document symbols to find the changed functions, the call hierarchy is walked upwards (default 4 levels) and callers that follow the language's test conventions become the affected tests, with the command that runs only them (`cargo test -- names`, `go test -run '^(A|B)$'`, `pytest -k`, `vitest|jest -t`, `swift test --filter`). Changes outside functions (module-level code, manifests) are reported as needing the full suite. Verified on Go and Rust fixtures. Beyond naming conventions since 2026-09-23 (#201): a caller is a test by its attribute (`#[test]`, `#[tokio::test]`, `#[rstest]`, `@Test`), by the registration it sits in (gtest `TEST`/`TEST_F`/`TEST_P`, Catch2 `TEST_CASE`, selected by `ctest -R` under its registered name), or as a `test*` method of a `unittest.TestCase` (its file is passed to pytest, which would not collect it by name). `prod-code impact --ci` runs the selection, or the whole suite when the selection cannot be trusted (lines changed outside functions, no index), says which and why, writes a Markdown summary to `$GITHUB_STEP_SUMMARY`, and exits with the tests' status.
  - Compare working tree uncommitted edits against base commit via call graph and AST dependency trees.
  - Calculate exact "blast radius": modified functions, impacted downstream callers, and test suites directly covering the modified paths.
  - Selectively run only the affected tests (e.g. runs 3 relevant tests in 200 ms instead of 800 tests in 5 minutes).
  - Proactively warn agents if an updated signature left unadjusted call sites in sibling files before full compilation is attempted.

- [x] **8.2. Automated Root-Cause Failure Dossier (`code_diagnose_failure`)** — shipped 2026-09-20: `prod-code diagnose [FILTER]` and MCP `code_diagnose_failure` run the tests and, per failure, return the failure output, the source around every location it mentions (Rust panics/`-->` notes, Go `file:line`, Python tracebacks, JS/TS stacks, Swift/C), the enclosing function with its callers, the working-tree diff of that file and the list of changed files; bare Go file names are resolved through the failing test's package. Suspect ranking shipped 2026-09-23 (#168). For each failure the dossier lists the changed functions whose callers graph reaches the failing test, nearest first, with the number of calls, plus the diff of their file when no failure site shows it. `impact` walks the graph from each changed function on its own, over a cache so no function is asked twice, and records which test each walk reached and at what depth. Suggested fixes followed on 2026-09-23 (#206): when the tests do not build (Rust), the dossier lists the compiler's machine-applicable fixes for the errors and names `prod-code check --fix`, which applies them. A failing assertion has no fix a tool can know, and the dossier stops at its evidence there.
  - When test suites fail (assertions, panics, unhandled exceptions), the server parses stack traces and maps frame pointers back to AST source spans.
  - Extracts runtime values and correlates failure expressions with recent diff lines into a structured JSON dossier:
    `{ failing_test, panic_line, expression, runtime_values, suspect_recent_changes }`.
  - Enables agents to diagnose and fix regressions in a single turn without reading extraneous files or burning reasoning tokens.

- [x] **8.3. External Dependency & Vendor Source Navigation (`code_definition_external`)** — shipped 2026-09-20: a definition outside the checkout (Rust std via rust-src, cargo registry/git caches, GOROOT and the Go module cache, `/usr/include`, Homebrew, Xcode SDKs, npm/bun installs, uv pythons) is read from the gateway host through `ReadFileRequest`; `prod-code def` prints the lines around it, `prod-code source <path> [--line N --context K]` shows any such file, MCP `code_definition` embeds the snippet and `code_source` reads the file. The gateway serves only those roots plus its workspace copies. Verified: clangd → `/usr/include/time.h`, rust-analyzer → `core/src/iter/traits/iterator.rs`, gopls → `fmt/print.go`. Hover on external symbols already worked. Nothing is open.
  - Transparent jump-to-definition into third-party libraries (`~/.cargo/registry`, `node_modules`, `GOPATH/pkg/mod`, Python virtualenv wheels, system C++ headers).
  - Returns exact type signatures, trait definitions, and docstrings directly from the server's pre-warmed dependency cache into agent context.
  - Prevents agents from hallucinating method names or argument orders of external crates and packages.

- [x] **8.4. Natural-Language Semantic Code Search (`code_search_semantic`)** — lexical half shipped 2026-09-21: `code_search` / `prod-code search` rank every declaration and the doc comment above it against the words of a question (BM25 over name, container, signature and doc; tests excluded unless the question is about tests), 33 ms over a 26712-declaration index (568 ms for the first query, which builds it). Dense half shipped 2026-09-24 (#218): the gateway embeds every declaration (kind, name as words, container, signature, doc) with BGE-small (int8 ONNX, 34 MB, run in process by ONNX Runtime) in a background pass after the index is built, re-embeds a file that changes, and fuses the dense ranking with BM25 by reciprocal rank; without a model (`models/bge-small-en-v1.5` beside the workspaces directory, or `PROD_CODE_EMBED_MODEL`) the search is lexical only and says so. Measured on this repository with `eval_ranking_on_this_repository` (13 questions phrased in other words than the code, top-3): lexical 5, dense 6, fused 7; 2,400 declarations embedded in 11.4 s (211/s), 1.7 ms per query vector. bge-base-en-v1.5 reached 8 at 2.5x the cost and jina-embeddings-v2-base-code 7, so BGE-small stays. The ranking is still weak on questions whose one lexical match is a strong wrong one.
  - Hybrid neural-lexical code search: dense embeddings (BGE) fused with typed AST symbol graphs on the server.
  - Allows agents to locate code by intent and behavior (e.g. *"where do we handle websocket reconnection on drop"*) rather than guessing exact identifier names via brittle grep.
  - Returns exact symbols, file locations, line numbers, and doc comments in < 10 ms.

- [x] **8.5. Instant Test Fixture & Mock Generator (`code_generate_fixture`)** — shipped 2026-09-21: `code_generate_fixture` / `prod-code fixture` build the value from the declaration the analyzer points at (not from hover, which elides fields past the tenth), verify it in an in-memory overlay before returning it, and name every type that fell back to `Default::default()`. Rust only.
  - Compiler-backed generation of test mocks, builders, and dummy fixtures for complex data structures with dozens of fields.
  - Generates valid, type-safe, compile-ready code populated with default or randomized values in 1 step.
  - Eliminates hundreds of lines of manual boilerplate authoring and associated compiler type-mismatch errors.

- [x] **8.6. Dead Code & Orphan Pruning (`code_prune_orphans`)** — scan shipped 2026-09-20: `prod-code dead-code [--include-exported] [--max-files N] [--json]` and MCP `code_dead_code` check every function, method and type of the checkout for references through the analyzer over one persistent session (522 symbols of prod-code in 1.5 s). Tests (test modules via symbol containers, test files by convention) and entry points are skipped; exported/public symbols are counted separately; trait-impl methods (Rust) and methods in interface languages go to a "may be reached through a trait / interface" bucket. Known false positives: items referenced only from attributes (`#[serde(with = ...)]`). Automatic pruning shipped 2026-09-23 (#162) as `code_prune_orphans` / `prod-code prune [--apply]`. Every item on the scan's `dead` list is removed with the analyzer's safe delete. Exported symbols and trait-reachable methods stay. Each answer is reduced to the lines it changes, and the answers are merged into one edit. A deletion that overlaps another waits for the next run. The whole result is type-checked in one overlay before anything is written. What the removals orphan is found by the next run: the scratch crate needed two runs to remove `leftover`, `Unused` and then `helper`, and a third found nothing.
  - Whole-program graph reachability analysis traversing entry points (`main`, `lib`, public APIs, route handlers).
  - Detects unreachable functions, dead types, and orphaned imports left behind by large refactors.
  - Emits an atomic single-commit cleanup patch.


- [x] **8.7. Structural AST Codemod Engine (`code_codemod`)** — shipped 2026-09-21: `code_codemod` / `prod-code codemod` run rust-analyzer's structural search and replace over the workspace (`pattern ==>> replacement`, `$name` placeholders), return a unified diff and apply it on request; `path` restricts where edits land, not how long the search takes. Rust only, because the engine is the analyzer's.
  - Pattern-based structural code transformations (AST pattern matching).
  - Matches syntax trees regardless of whitespace, formatting, or variable names.
  - Executes large-scale library migrations and API upgrades across hundreds of files in sub-second time.
