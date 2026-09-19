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

## Phase 3: Polyglot Hub & Managed Go Engine

**Objective**: Expand the daemon into a unified multi-language hub by adding supervised Go language analysis and pluggable LSP adapters.

- [ ] **3.1. Automatic Workspace Detection**
  - Inspect project roots for language manifests:
    - `Cargo.toml` -> Rust Engine
    - `go.mod` -> Go Engine
    - `pyproject.toml` / `requirements.txt` -> Python Engine
    - `package.json` -> TypeScript / JavaScript Engine
- [ ] **3.2. Managed Go Engine (`crates/prod-code-engine-go`)**
  - Supervised `gopls` worker pool running in daemon mode.
  - Shared `GOCACHE` and `GOPATH/pkg/mod` volume on fast NVMe for instant warm symbol resolution across all worktrees.
  - Path mapping translation for Go workspace URIs and build tags.
- [ ] **3.3. Generic LSP Engine (`crates/prod-code-engine-generic`)**
  - Pluggable adapter for external language servers (e.g. Pyright, Ruff, vtsls).
  - Lifecycle management: automatic process spawning, health pings, graceful shutdown on idle timeout.

---

## Phase 4: Dual Surface — Native MCP Server for AI Agents

**Objective**: Provide first-class support for autonomous coding agents via the Model Context Protocol (MCP), removing the overhead of JSON-RPC LSP parsing for LLMs.

- [ ] **4.1. Native MCP Server (`crates/prod-code-mcp`)**
  - Expose high-level, typed semantic tools directly consumable by Claude, Codex, Agy, and other agent frameworks:
    - `code_definition(path, line, character)`
    - `code_references(path, line, character, include_declarations)`
    - `code_outline(path, max_depth)`
    - `code_diagnostics(path)`
    - `code_type_at(path, line, character)`
    - `code_callers(path, line, character)`
  - Compact declaration output by default (optimized for LLM context window efficiency).
- [ ] **4.2. Worktree Ingestion & Fast Sync**
  - Command: `prod-code sync` — push delta / worktree state to the remote server over 10G in < 200 ms.
  - Support for in-memory temporary overlays so agent scratch edits don't require filesystem writes.

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
