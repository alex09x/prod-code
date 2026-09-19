# prod-code: Engineering Roadmap

This document outlines the architectural milestones and engineering phases for building **prod-code** as a distributed, polyglot remote code-intelligence engine optimized for AI agent fleets and 10 GbE local network execution.

---

## Phase 1: Foundation & High-Speed Wire Transport

**Objective**: Establish the core client-server wire protocol over 10G TCP/QUIC with transparent path translation and zero-overhead client bridging.

- [x] **1.1. Protocol Specification (`crates/prod-code-protocol`)**
  - Binary framing layer with length-prefixed messages and NUL completion markers.
  - Session handshake with protocol version negotiation, client capabilities, and authentication tokens.
  - Streaming transport support: 10 GbE TCP stream with TCP_NODELAY and socket buffer tuning.
  - Fallback local transport: Unix domain socket / Windows named pipe for local execution.
- [x] **1.2. Bi-directional Path Translation**
  - Canonical URI/path rewriting between client workspace roots (`file:///Users/alex09x/...`) and remote server paths (`file:///srv/prod-code/workspaces/...`).
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
- [x] **3.2. Managed Go Engine (`crates/prod-code-engine-go`)**
  - Supervised `gopls` worker pool running in daemon mode.
  - Shared `GOCACHE` and `GOPATH/pkg/mod` volume on fast NVMe for instant warm symbol resolution across all worktrees.
  - Path mapping translation for Go workspace URIs and build tags.
- [x] **3.3. Generic LSP Engine (`crates/prod-code-engine-generic`)**
  - Pluggable adapter for external language servers (e.g. Pyright, Ruff, vtsls).
  - Lifecycle management: automatic process spawning, health pings, graceful shutdown on idle timeout.
- [ ] **3.4. C / C++ Engine (`crates/prod-code-engine-cpp` / `clangd`)**
  - Supervised `clangd` daemon with background indexing over `compile_commands.json`.
  - Shared precompiled header (PCH) and symbol index cache on server NVMe/RAM-disk across multiple worktrees.
  - Offloads multi-gigabyte AST indexing for massive C++ codebases (e.g. Chromium, ClickHouse, trading engines) from local laptops to 32–128 core servers.
- [ ] **3.5. TypeScript & JavaScript Engine (`crates/prod-code-engine-ts` / `vtsls`)**
  - Supervised `vtsls` worker pool running on server Bun/Node runtime.
  - Shared global `@types/*` and `node_modules` cache volume to eliminate duplicate multi-gigabyte `node_modules` across concurrent agent worktrees.
  - Instant type inference and signature resolution for React, Vue, Svelte, Next.js, and large monorepos (5–15 ms latency).
- [ ] **3.6. Python Semantic Engine (`crates/prod-code-engine-python` / `basedpyright`)**
  - Managed `basedpyright` / `pyright` daemon with shared virtual environment stub cache.
  - Accurate cross-file semantic reference discovery (`code_references`) eliminating the false-positive noise and token waste of text-based grep.
  - Deep type inference for Pydantic, FastAPI, PyTorch, and typing annotations.
- [ ] **3.7. Swift Engine (`crates/prod-code-engine-swift` / `sourcekit-lsp`)**
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

---

## Phase 5: Multi-Server Clustering & Fleet Scale

**Objective**: Scale `prod-code` across multiple physical servers on the 10G LAN to support massive agent fleets (50+ concurrent workers).

- [ ] **5.1. Cluster Gateway & L4/L7 Dispatcher**
  - Distributed router dispatching incoming agent connections to the least-loaded server node.
  - Consistent hashing based on workspace repository identity so sessions for the same codebase share warm Salsa and Go caches.
- [ ] **5.2. Isolated Proc-Macro Worker Farm**
  - Offload compilation and execution of heavy Rust procedural macros into a sandboxed worker pool.
- [ ] **5.3. Agent Fleet Stress Verification**
  - End-to-end load tests using `bench/masstest.py`:
    - 20+ concurrent workers across 500+ file codebases.
    - Continuous semantic queries mixed with uncommitted `didChange` edits.
    - Simulated worker SIGKILL churn waves to verify clean session retirement and zero daemon hangs.

---

## Phase 6: Remote Build & Test Execution (RTE / RBE)

**Objective**: Offload heavy Rust (and polyglot) compilation, test execution, clippy verification, and benchmarks from local workstations to high-performance remote server nodes (32–128 cores) over the 10 GbE LAN, eliminating local CPU lockups, thermal throttling, and battery drain.

### The Problem: Local Rust Compilation Bottleneck
- Compiling and testing large Rust projects (`cargo check`, `cargo test`, `cargo clippy`, `cargo bench`) on developer workstations and laptops is notoriously slow: LLVM code generation, monomorphization, macro expansion, and final linking cause heavy CPU spikes, thermal throttling, noisy cooling fans, and severe battery drain.
- For autonomous AI coding agents (Claude, Codex, Agy), 95% of compilation and test invocations are executed solely for **verification feedback** (confirming whether changes compile without errors, verifying that unit/integration test assertions pass, and checking clippy lints). The binary itself is rarely needed locally on macOS.
- Running multi-agent fleets with concurrent local builds quickly freezes workstation UI, locks `target/` directories, and throttles agent iteration speed.

### Engineering Milestones

- [ ] **6.1. Remote Execution Wire Protocol (`crates/prod-code-protocol`)**
  - Define `RemoteExecRequest`:
    - `command`: `check`, `test`, `clippy`, `bench`, or arbitrary binary runner.
    - `args`: Command arguments and test filters (e.g. `["--lib", "test_order_manager"]`).
    - `env`: Explicit environment variables (e.g. `RUST_BACKTRACE=1`, feature flags).
    - `format`: `raw` streaming or `json` (parsing Cargo's `--message-format=json` and libtest JSON output into structured events).
  - Define `RemoteExecStream` and `RemoteExecResult`:
    - Real-time streaming of stdout/stderr chunks over 10G TCP with sub-millisecond latency.
    - Structured compiler diagnostic events (spans, error codes, suggestions) streamed directly to client/agent.
    - Final execution summary: exit code, wall-clock duration, server CPU user/sys time, peak memory RSS.

- [ ] **6.2. Server-Side Execution Engine & Warm Target Caches (`crates/prod-code-gateway`)**
  - Dispatch execution to dedicated high-performance Linux worker nodes (`booster` with 32 cores, or `rama` with 128 Ampere cores / 250 GB RAM).
  - Persistent server-side `target/` directories located on high-speed NVMe or RAM-disk (`/dev/shm`).
  - Pre-warmed shared Cargo cache (`~/.cargo/registry`, `~/.cargo/git`) and toolchains (stable, beta, nightly).
  - Because code deltas are synced incrementally via Phase 4.2 in < 2 ms, only modified crates recompile; dependencies stay permanently warm in server RAM.

- [ ] **6.3. Concurrent Multi-Worktree Build Isolation**
  - Isolated build artifacts per worktree session to eliminate `target/.cargo-lock` build contention across concurrent agents.
  - Shared read-only dependency artifact cache (`sccache` integration on server or hardlinked shared target directories).
  - Process group supervision: automatic SIGKILL tree cleanup on client disconnect or timeout.

- [ ] **6.4. Client CLI & Native Agent MCP Integration**
  - **Client CLI Commands**:
    - `prod-code check`: remote `cargo check` with instant terminal diagnostics.
    - `prod-code test [FILTER]`: remote `cargo test` with real-time test output streaming.
    - `prod-code clippy`: remote `cargo clippy -- -D warnings`.
    - `prod-code bench [BENCH_NAME]`: remote `cargo bench` running on quiet, isolated server cores without desktop thermal noise.
  - **Agent MCP Tools (`crates/prod-code-mcp`)**:
    - `code_check(path)`: returns structured compiler errors and warnings directly into agent context.
    - `code_test(path, filter)`: runs targeted tests and returns failures with panic backtraces and diffs.
    - Instant verification feedback in 1–3 seconds per edit without touching local machine CPU.

