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
  - [x] Isolated server workspace and analysis database per git worktree (`<repo>--wt-<hash>`); first contact sends a size/hash manifest probe, the gateway seeds the copy from the origin repository and asks only for missing files (2026-09-19).
  - [ ] Binary-safe file transfer: `FileDelta.content` is a JSON byte array today (4x inflation, ~5-8 s for 10 MB); switch to base64 or a binary frame to meet the < 200 ms target.
  - [ ] Persistent MCP session: reuse one gateway session per agent process instead of connect + git status + handshake per query.

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
  - [x] Idle LRU eviction: workspaces untouched for > 30 minutes are unloaded (`--idle-evict-secs`) and stale `<repo>--wt-*` copies pruned after 7 days (`--prune-worktree-days`); done 2026-09-19.
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

---

## Phase 7: Distributed Semantic Refactoring & Agent Intelligence Engine

**Objective**: Elevate `prod-code` from a code-reading intelligence layer into an active, AST-driven semantic transformation and refactoring engine, combining local file ownership with remote 128-core AST graph reasoning.

### The Problem: AI Code Editing Hallucinations & Inefficiencies
- Current AI coding agents rely on brittle string replacement (`regex`, `replace_file_content`) or rewriting whole files from memory. This routinely introduces syntax errors, renames unintended identifiers in comments/strings, breaks trait contracts, misses cross-file references in large mono-repos, and wastes hundreds of thousands of LLM tokens on trial-and-error edits.
- Standard RAG approaches feed entire multi-thousand-line files into model context windows, drowning the LLM in boilerplate and causing "lost in the middle" attention degradation.

### Engineering Milestones

- [ ] **7.1. AST-Level Atomic Refactoring Protocol (`WorkspaceEdit`)**
  - Remote language servers (`rust-analyzer`, `clangd`, `gopls`, `vtsls`, `basedpyright`) compute mathematically sound, type-safe code transformations and return structured `WorkspaceEdit` payloads:
    - `refactor.rename(path, line, col, new_name)`: Exact semantic symbol renaming across hundreds of files in < 20 ms without regex hallucinations.
    - `refactor.extract_function(path, range, fn_name)`: Remote compiler analyzes variable captures, borrow checker constraints, and lifetimes, returning the extracted function signature and replacement call site.
    - `refactor.change_signature(path, symbol, new_params)`: Automatic type-safe propagation of parameter additions, reorderings, or default values across all call sites in the repository.
    - `refactor.organize_imports(path)`: Server-driven deterministic import sorting, alias resolution, and dead import removal.
  - **Client-Side Atomic Transactional Applicator**:
    - Applies `TextEdit` batches directly to local files with microsecond latency.
    - Automatic snapshot & atomic rollback if any disk write fails.

- [ ] **7.2. Automated Compiler "Fix-It" & CodeAction Engine (Zero-Prompt Repair)**
  - Compilers and linters (`rustc`, `clippy`, `clang-tidy`, `gopls`, `ruff`) natively produce machine-applicable `CodeAction` / `Fix-It` recommendations.
  - Wire protocol endpoint: `code_quickfix(file, diagnostic_id)` returning pre-computed compiler diffs.
  - AI agents can inspect and apply exact compiler-suggested fixes in one step (e.g. missing trait imports, mutable borrow corrections, lifetime annotations) with zero LLM token consumption or hallucination loops.

- [ ] **7.3. Program Slicing & Context Tree-Shaking (10x Token Reduction)**
  - Program Dependency Graph (PDG) and data-flow analysis on remote server:
    - Given a target function or bug location, slice away all unreferenced structs, unrelated methods, and irrelevant imports.
    - `code_slice(path, symbol)`: extracts a minimal, self-contained semantic slice (e.g. 60 lines instead of 4,000 lines) representing 100% of data and control flow.
    - Reduces LLM context consumption by 85–95%, drastically lowering inference costs and model reasoning errors.

- [ ] **7.4. Speculative In-Memory Shadow Workspaces (Parallel Multi-Hypothesis Execution)**
  - When an AI agent explores multiple competing architectural solutions or bug-fix hypotheses:
    - Server creates lightweight in-memory VFS overlays (`shadow-branch-1`, `shadow-branch-2`, `shadow-branch-3`) in RAM (`/dev/shm`).
    - Remote execution engine (Phase 6) runs full test suites against all hypotheses simultaneously across 128 server cores.
    - The server returns only the winning hypothesis's unified diff back to the client.
    - Local Mac disk and Git history remain clean of failed experimental churn.

- [ ] **7.5. Call Graph & Type Hierarchy Navigation**
  - Graph-level codebase exploration endpoints:
    - `code_callers(path, line, col)`: incoming call hierarchy across the entire workspace/monorepo in < 5 ms.
    - `code_callees(path, line, col)`: outgoing call graph tree.
    - `code_implementations(path, line, col)`: all structs/classes implementing a trait, interface, or abstract class.
    - `code_dead_code()`: whole-program graph reachability analysis identifying unused functions and types post-refactoring.

- [ ] **7.6. Cross-Language Full-Stack Schema Refactoring**
  - Unified multi-language schema evolution across polyglot repositories:
    - Changing a backend schema (Protobuf, OpenAPI, SQL, or Rust/Go data models) automatically coordinates with frontend TypeScript interfaces, API clients, and UI components.
    - Emits an atomic multi-repository `WorkspaceEdit` synchronizing backend and frontend simultaneously.

- [ ] **7.7. Pre-Validation On-the-Fly (Instant Hallucination Interception)**
  - Streamed syntax and type check verification during agent code generation.
  - Intercepts invalid method invocations, incorrect argument types, or borrow-checker errors before the agent even finishes generating its turn, providing immediate feedback and eliminating multi-turn debugging cycles.

---

## Phase 8: Autonomous Agent Fleet Superpowers & Workflow Acceleration

**Objective**: Equip autonomous AI coding agents (Claude, Codex, Agy) with specialized semantic tools that eliminate trial-and-error reasoning loops, slash LLM token waste, and accelerate development velocity across multi-thousand file repositories.

### The Problem: Agent Productivity Bottlenecks in Production
- AI agents spend up to 70% of their execution time and context budget on repetitive diagnostic overhead: reading dozens of files trying to locate where a test panicked, running entire test suites after a 3-line edit, guessing function names with regex grep, writing hundreds of lines of boilerplate test mocks, and leaving zombie code behind after large refactors.

### Engineering Milestones

- [ ] **8.1. Selective Test Execution & Blast Radius (`code_impact_analysis`)**
  - Compare working tree uncommitted edits against base commit via call graph and AST dependency trees.
  - Calculate exact "blast radius": modified functions, impacted downstream callers, and test suites directly covering the modified paths.
  - Selectively run only the affected tests (e.g. runs 3 relevant tests in 200 ms instead of 800 tests in 5 minutes).
  - Proactively warn agents if an updated signature left unadjusted call sites in sibling files before full compilation is attempted.

- [ ] **8.2. Automated Root-Cause Failure Dossier (`code_diagnose_failure`)**
  - When test suites fail (assertions, panics, unhandled exceptions), the server parses stack traces and maps frame pointers back to AST source spans.
  - Extracts runtime values and correlates failure expressions with recent diff lines into a structured JSON dossier:
    `{ failing_test, panic_line, expression, runtime_values, suspect_recent_changes }`.
  - Enables agents to diagnose and fix regressions in a single turn without reading extraneous files or burning reasoning tokens.

- [ ] **8.3. External Dependency & Vendor Source Navigation (`code_definition_external`)**
  - Transparent jump-to-definition into third-party libraries (`~/.cargo/registry`, `node_modules`, `GOPATH/pkg/mod`, Python virtualenv wheels, system C++ headers).
  - Returns exact type signatures, trait definitions, and docstrings directly from the server's pre-warmed dependency cache into agent context.
  - Prevents agents from hallucinating method names or argument orders of external crates and packages.

- [ ] **8.4. Natural-Language Semantic Code Search (`code_search_semantic`)**
  - Hybrid neural-lexical code search: dense embeddings (BGE) fused with typed AST symbol graphs on the server.
  - Allows agents to locate code by intent and behavior (e.g. *"where do we handle websocket reconnection on drop"*) rather than guessing exact identifier names via brittle grep.
  - Returns exact symbols, file locations, line numbers, and doc comments in < 10 ms.

- [ ] **8.5. Instant Test Fixture & Mock Generator (`code_generate_fixture`)**
  - Compiler-backed generation of test mocks, builders, and dummy fixtures for complex data structures with dozens of fields.
  - Generates valid, type-safe, compile-ready code populated with default or randomized values in 1 step.
  - Eliminates hundreds of lines of manual boilerplate authoring and associated compiler type-mismatch errors.

- [ ] **8.6. Dead Code & Orphan Pruning (`code_prune_orphans`)**
  - Whole-program graph reachability analysis traversing entry points (`main`, `lib`, public APIs, route handlers).
  - Detects unreachable functions, dead types, and orphaned imports left behind by large refactors.
  - Emits an atomic single-commit cleanup patch.


- [ ] **8.7. Structural AST Codemod Engine (`code_codemod`)**
  - Pattern-based structural code transformations (AST pattern matching).
  - Matches syntax trees regardless of whitespace, formatting, or variable names.
  - Executes large-scale library migrations and API upgrades across hundreds of files in sub-second time.
