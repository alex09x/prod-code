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

## Phase 5: Multi-Server Clustering, Smart Discovery & Fleet Scale

**Objective**: Scale `prod-code` across multiple physical servers on the 10G LAN to support massive agent fleets (50+ concurrent workers) with dynamic load balancing, repository affinity, and zero-configuration service discovery.

- [ ] **5.1. Cluster Gateway & L4/L7 Dispatcher**
  - Distributed router dispatching incoming agent connections to the least-loaded server node.
  - Consistent hashing based on repository identity (`sha256(repo_common_dir)`) so sessions for the same codebase share warm Salsa, gopls, and clangd in-memory caches.
  - Transparent TCP redirection: if a client connects to Node A but the workspace is warm on Node B, Node A issues a `WireMessage::Redirect { target_addr }` allowing sub-millisecond client hop without repeating initialization.
- [ ] **5.2. Smart DNS & Service Discovery (`*.code.internal`)**
  - Embedded lightweight DNS / mDNS resolver mapping projects to designated server nodes (e.g. `btcr.code.internal` -> `192.168.2.168:9400`, `codehaus.code.internal` -> `192.168.2.190:9400`).
  - Allows zero-config CLI and MCP usage (`prod-code -r auto ...` or `PROD_CODE_CLUSTER=10G`), eliminating hardcoded IP addresses.
  - Dynamic SRV record publication for active daemon instances across the LAN.
- [ ] **5.3. Cluster Capacity Gossip & Dynamic Workload Rebalancing**
  - Background gossip heartbeat between daemon nodes reporting CPU load, available RAM, active engine count, and in-flight builds.
  - Automatic load shedding: when a node approaches memory limits (e.g. > 85% RSS) or runs heavy test suites, new projects are assigned to quieter nodes (e.g. 128-core `rama` with 250 GB RAM).
  - Idle LRU eviction: workspaces untouched for > 30 minutes are gracefully serialized/quiesced to free RAM for active agent fleets.
- [ ] **5.4. Isolated Proc-Macro Worker Farm**
  - Offload compilation and execution of heavy Rust procedural macros into a sandboxed worker pool.
- [ ] **5.5. Agent Fleet Stress Verification**
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

- [ ] **6.1. Polyglot Remote Execution Wire Protocol (`crates/prod-code-protocol`)**
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

- [ ] **6.2. Server-Side Execution Engine & Warm Polyglot Caches (`crates/prod-code-gateway`)**
  - Dispatch execution to dedicated high-performance Linux worker nodes (`booster` with 32 cores, or `rama` with 128 Ampere cores / 250 GB RAM).
  - Persistent server-side build caches on fast NVMe / RAM-disk (`/dev/shm`):
    - Rust: shared `~/.cargo/registry`, `target/` on NVMe.
    - Go: warm shared `GOCACHE` and `GOPATH/pkg/mod`.
    - C++: shared `ccache` / `sccache` and precompiled headers.
    - TypeScript: shared global `pnpm` store and pre-resolved `@types/*`.
    - Python: pre-warmed `.venv` wheels and pycache.
  - Because code deltas are synced incrementally in < 2 ms, only modified files trigger re-compilation; dependencies stay permanently warm in server RAM.

- [ ] **6.3. Concurrent Multi-Worktree Build Isolation**
  - Isolated build artifacts per worktree session to eliminate build cache lock contention across concurrent agents.
  - Shared read-only dependency artifact cache across worktrees.
  - Process group supervision: automatic SIGKILL tree cleanup on client disconnect or timeout.

- [ ] **6.4. Client CLI & Native Agent MCP Integration**
  - **Client CLI Commands**:
    - `prod-code check`: remote compilation check across any language with instant terminal diagnostics.
    - `prod-code test [FILTER]`: remote test runner with real-time test output streaming.
    - `prod-code lint`: remote linter (`clippy`, `golangci-lint`, `eslint`, `ruff`).
    - `prod-code bench [FILTER]`: remote benchmark runner on quiet, dedicated server cores without workstation thermal noise.
  - **Agent MCP Tools (`crates/prod-code-mcp`)**:
    - `code_check(path)`: returns structured compiler errors and warnings directly into agent context.
    - `code_test(path, filter)`: runs targeted tests and returns failures with panic backtraces and assertion diffs.
    - Instant verification feedback in 1–3 seconds per edit without touching local machine CPU or battery.


