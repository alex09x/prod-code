<div align="center">

# ⚡ `prod-code`
### Remote Code Intelligence (RCI) for AI Agent Fleets & Distributed Workspaces

[![GitHub release](https://img.shields.io/github/v/release/alex09x/prod-code?color=blue&style=flat-square)](https://github.com/alex09x/prod-code/releases)
[![License](https://img.shields.io/badge/license-MIT%20%2F%20Apache--2.0-blue?style=flat-square)](LICENSE-MIT)
[![Fleet Scale](https://img.shields.io/badge/fleet--scale-AI_Agent_Optimized-success?style=flat-square)]()
[![Rust](https://img.shields.io/badge/rust-2024_edition-dea584?style=flat-square&logo=rust)]()
[![Go](https://img.shields.io/badge/go-1.24+-00ADD8?style=flat-square&logo=go)]()
[![Protocol](https://img.shields.io/badge/protocol-LSP_%2B_Native_MCP-purple?style=flat-square)]()

<p align="center">
  <b>Decouple semantic code intelligence from local developer machines.</b><br>
  Purpose-built for autonomous AI coding agent fleets, multi-worktree parallelism, and high-throughput polyglot development with native Model Context Protocol (MCP) and drop-in LSP.
</p>

</div>

---


## For agents

Register the MCP server once and every project gets the tools:

```
claude mcp add --scope user prod-code -e PROD_CODE_REMOTE=192.0.2.10:9400 -- prod-code mcp
codex  mcp add prod-code --env PROD_CODE_REMOTE=192.0.2.10:9400 -- prod-code mcp
```

The server tells the agent how to work at `initialize` (the same text as
`prod-code-mcp`'s `AGENT_INSTRUCTIONS`): navigate with `code_definition` / `code_references` /
`code_callers` instead of grep, validate every proposed file with `code_validate_edit` before
writing it, build and test on the gateway with `code_check` / `code_test` / `code_exec`, narrow
test runs with `code_impact`, explain failures with `code_diagnose_failure`. Local edits are
synced automatically before each call; the agent never manages the gateway.

### Addressing symbols by name

Every position tool takes `symbol` instead of `path`/`line`/`character`:

```
code_callers   {"symbol": "Metrics::record"}
code_hover     {"symbol": "pkg.Func"}          code_references {"symbol": "Class.method", "path": "src/a.ts"}
code_symbols   {"query": "record"}             → [Method] Metrics::record — crates/gw/src/metrics.rs:102:12
```

The name goes through the analyzer's workspace symbol index; qualifiers are matched against the
enclosing item and `path` (file or directory) only disambiguates. A tie between different
locations comes back as an error listing the candidates.

## Shadow runs: several fixes against the tests at once

An agent with two candidate fixes no longer writes one, runs the tests, reverts and writes
the other. `code_shadow_run` (CLI: `prod-code shadow-run spec.json -- cargo test -p x`)
takes named hypotheses, each a complete set of proposed file contents, and the gateway runs
the command once per hypothesis in a private shadow of the workspace copy:

* on Linux a shadow is an overlay mount **at the workspace's own path** inside a user
  namespace: cargo, go and tsc see the same absolute paths, their fingerprints and the warm
  `target/` stay valid, every write goes to the hypothesis's upper directory and the
  workspace copy is never touched; hypotheses run in parallel (`parallel`, default cores / 8);
* without user namespaces (macOS nodes, kernels that restrict them) hypotheses run one after
  another in place and the touched files are restored afterwards.

The result lists every hypothesis (exit code, test counts parsed from the runner's output,
changed lines), ranks them (passed, fewest failures, most passed, smallest diff), prints the
winner's unified diff and the output tail of every failing one; `apply: true` writes the
winner into the checkout. One hypothesis is a dry run of a fix; a hypothesis without edits is
the baseline. Upper directories live under `--shadow-dir` (default `shadow` next to the
storage directory; a tmpfs path keeps hypothesis builds in RAM) and are removed after each
run. Ubuntu 24.04 needs `kernel.apparmor_restrict_unprivileged_userns=0` for the overlay
mode.

## Cluster

Any number of gateways form a cluster: start each with `--peers <one live peer>` and
`--advertise <its host:port>`; membership spreads by gossip (every 5 s: load, engines, loaded
workspaces). Clients need one seed:

```
export PROD_CODE_REMOTE=192.0.2.10:9400   # any node; the rest is discovered
prod-code cluster                            # the gossip view of every node
```

A checkout is placed by the cluster on the node that already holds it, otherwise on the
quietest node that serves its language (Swift lands on a macOS node); the placement is
remembered per checkout and an idle workspace drifts off an overloaded node.

### macOS nodes: signing

macOS treats every ad-hoc-signed build as a new program (the linker identifier carries a
hash), so a gateway that gossips to LAN peers triggers the Local Network / privacy prompt
after each redeploy. `scripts/deploy-mac-node.sh <host> <advertise> <peers> [release|dev]`
builds on the node (or installs `PROD_CODE_SERVER_BIN`, e.g. a release binary), signs it
with the identity in `PROD_CODE_SIGN_IDENTITY` and the stable identifier
`com.prod-code.gateway`, installs it atomically and restarts the launchd agent. The grant is
remembered per identifier + team, so the prompt appears once.

A macOS node exists for Swift, so the unit is written with `--engines swift`
(`PROD_CODE_ENGINES` to change it): the node advertises and serves only those engines and
refuses handshakes for the rest, and placement never sends Rust, Go, C++ or Python work to a
workstation that happens to have their toolchains installed.

## Finding code by what it does

`code_search` (CLI: `prod-code search "..."`) answers a question about the codebase with
declarations rather than file matches. The gateway keeps an index of every declaration in the
workspace copy with the doc comment above it, and ranks them against the question's words:

```
$ prod-code search "how do we decide which node runs a workspace"
10 hit(s) for `how do we decide which node runs a workspace` in 16 ms (1001 declarations, 40 files)

 1. [function] pick_node  crates/prod-code-mcp/src/cluster.rs:107
    pub async fn pick_node(
    Chooses the gateway for `workspace_name` among `nodes`: the remembered placement when it is
    still one of the nodes, alive and able to serve `engine`, otherwise the quietest alive node
```

It is lexical, so a question sharing no words with the code or its comments finds nothing, and
`code_symbols` remains the way to look up a name you already know. Declarations that belong to
tests are left out unless the question mentions tests. The index is built on the first query
and then kept current by the sync layer, which tells it which files it wrote: 568 ms for the
first query against a 26712-declaration repository, 33 ms for every one after it.

## Reading a symbol without reading its files

`code_slice` (CLI: `prod-code slice <symbol|file --line N>`) returns the code a symbol
depends on instead of the files it lives in. From the seed declaration it follows the
analyzer's own edges, the functions the body calls and the types, constants and traits it
mentions, and returns each as a whole declaration with its file and line range:

```
$ prod-code slice crates/prod-code-gateway/src/shadow.rs --line 469 --depth 1
slice of `run_shadow`: 10 item(s), 11474 bytes from 284105 bytes of source (96% smaller)
outside the workspace, not followed: Arc, Duration, Framed, HashSet, Instant, PathBuf, and 28 more

=== crates/prod-code-gateway/src/main.rs

[struct] ServerState  crates/prod-code-gateway/src/main.rs:87-106 (depth 1, used by run_shadow)
...
```

`depth` bounds how far the walk goes (default 2), `max_bytes` bounds the result, and names
that resolve outside the workspace are listed rather than expanded. The unit is a
declaration: there is no data-flow slicing inside a body.

## Usage metrics

Each gateway records every query, command and sync round as one JSON line under
`~/prod-code-storage/metrics/events-YYYY-MM-DD.jsonl` (importable into ClickHouse) and keeps
the recent ones in memory. `prod-code metrics --since 86400` merges every node: who (agent and
host) asked what (workspace, method) how often and how fast, which commands ran and failed,
and how much was synced. Clients identify themselves as `claude-code`, `codex`, `cli` or
whatever `PROD_CODE_AGENT` says.

## What each language gets

| | Rust | Go | C/C++ | TypeScript | Python | Swift |
|---|---|---|---|---|---|---|
| engine | rust-analyzer in-process | gopls | clangd | TypeScript 7 native LSP | basedpyright | sourcekit-lsp (macOS node) |
| hover / def / refs / symbols / callers / callees / impls | yes | yes | yes | yes | yes | yes |
| rename | yes (+ module files) | yes | yes | yes | yes | yes |
| assists / safe-delete | yes | - | - | - | - | - |
| check | cargo check | go build | cmake / meson / make | tsc (bunx / pnpm / yarn / npx) | basedpyright (uv / .venv aware) | swift build / xcodebuild |
| lint | clippy | go vet | - | eslint / biome | ruff | - |
| test | cargo test | go test | ctest / meson test | vitest / jest / bun test / mocha | pytest / unittest | swift test / xcodebuild test |

Tooling is detected from the checkout (lock files, package.json, pyproject, CMakeLists...).
Dependencies live on the node: run `prod-code exec -- bun install`, `-- uv sync`, `-- npm ci`
once per checkout and the language servers and test runners use them.

## Per-repository options (`prod-code.toml`)

Put a `prod-code.toml` at the checkout root to tune how the gateway analyses it. It is synced
like any manifest and read when the workspace is loaded (restart or idle-evict the gateway
after changing it):

```toml
[rust]
features = "all"            # or ["feat-a", "feat-b"]; default: the crate's default features
no_default_features = false
all_targets = true          # tests, benches and examples are analysed (default)
sysroot = true              # standard library from rust-src (default)
```

Use `features = "all"` when the same module tree is compiled into several crates behind
feature flags: rust-analyzer attaches each file to one crate, and a module behind a disabled
feature is dead there.

## 🎯 The Problem: Why Traditional Language Servers Fail at Agent Scale

Modern language servers (`rust-analyzer`, `gopls`, `pyright`, `tsserver`) were architected for a single human developer typing in an interactive desktop editor. 

When deploying **fleets of autonomous AI coding agents** (Claude, Codex, Agy, Cursor Agent) across dozens of parallel Git worktrees, the architecture collapses:

| Bottleneck | Traditional Language Servers (`rust-analyzer` / `gopls`) | `prod-code` Remote Code Intelligence |
|:---|:---|:---|
| **Memory Footprint** | 20–60+ GB RAM duplicated per session. Developer Mac freezes or OOMs. | **0 MB on client**. Entire Salsa DB / AST cache stays in server RAM (128+ GB). |
| **CPU / Battery** | 100% CPU lockups on AST re-indexing; fans spin at full speed on laptop. | **0% CPU on client**. Heavy analysis runs on dedicated 32–64 core server CPUs. |
| **Overlay Contention** | Global database write locks on every text edit. Concurrent sessions queue up (p95: **71s**). | **Single-Owner Direct-Edits**: edits write directly to in-memory inputs (p95: **12s**). |
| **Protocol Overhead** | Heavy bidirectional JSON-RPC state synchronization gymnastics. | **Dual Surface**: Standard LSP for IDEs + **Native MCP** tools for AI agents. |
| **Multi-Language Ops** | Fractured supervisor processes per language (`gopls`, `analyzed`, `pyright`). | **Unified Polyglot Gateway**: single 10G port routes Rust, Go, Python, and TS. |

---

## 🏗️ Architecture

```text
 ┌────────────────────────────────────────────────────────┐
 │   Developer Laptop / Agent Fleet Pod                   │
 │   • Thin Client (`prod-code`, < 15 MB binary)          │
 │   • Local IDEs (Cursor / VS Code / Neovim) via stdio   │
 │   • AI Agents (Claude / Codex / Agy) via MCP           │
 │   Local Resource Usage: ~0% CPU, < 10 MB RAM           │
 └───────────────────────────┬────────────────────────────┘
                             │
                             │ 10 GbE TCP / QUIC (~0.05–0.1 ms RTT, 1.2 GB/s)
                             ▼
 ┌────────────────────────────────────────────────────────┐
 │   Remote Compute Node (128+ GB RAM, 32–64 Cores, NVMe) │
 │                                                        │
 │   ┌────────────────────────────────────────────────┐   │
 │   │  Universal Gateway (:9400)                     │   │
 │   │  • Dual-Surface: LSP Multiplexer + MCP Server  │   │
 │   │  • Bi-directional Path Translation             │   │
 │   │  • Multi-Tenant Session Registry               │   │
 │   └───────────────────────┬────────────────────────┘   │
 │                           │                            │
 │         ┌─────────────────┼─────────────────┐          │
 │         ▼                 ▼                 ▼          │
 │   ┌───────────┐     ┌───────────┐     ┌───────────┐    │
 │   │Rust Engine│     │ Go Engine │     │ Python/TS │    │
 │   │• RA Salsa │     │• gopls    │     │• Managed  │    │
 │   │  in RAM   │     │  pool     │     │  workers  │    │
 │   │• Direct   │     │• Shared   │     │• Scaled   │    │
 │   │  Edits    │     │  GOCACHE  │     │  workers  │    │
 │   └───────────┘     └───────────┘     └───────────┘    │
 └────────────────────────────────────────────────────────┘
```

---

## ✨ Key Capabilities

### 1. Ultra-Low Latency 10G Wire Protocol
Over a 10 GbE local network, network latency drops to **0.05–0.1 ms** with **~1.1–1.2 GB/s throughput** — indistinguishable from local NVMe storage. `prod-code` uses a tuned TCP streaming protocol with `TCP_NODELAY`, binary frame headers, and connection reuse.

### 2. Dual-Surface API: LSP for Humans, MCP for Agents
* **For Editors**: Drops directly into Cursor, VS Code, or Neovim as an ordinary language server speaking JSON-RPC LSP over `stdio`.
* **For AI Coding Agents**: Exposes clean, structured Model Context Protocol (MCP) endpoints (`code_definition`, `code_references`, `code_outline`, `code_diagnostics`, `code_type_at`). No parsing multi-megabyte JSON-RPC streams in agent loops.

### 3. Single-Owner Direct-Edit Fast Path
For ephemeral Git worktrees used by autonomous agents:
* Bypasses costly overlay crate cones and global database invalidation locks.
* Unsaved buffer edits write directly into in-memory base Salsa inputs.
* Benchmarked under 15 concurrent agent sessions: cuts query latency from **71s down to 12s**.

### 4. Transparent Bi-directional Path Translation
Your client talks about `/Users/me/workspace/repo/src/main.rs`.
The remote daemon maps it to `/srv/prod-code/workspaces/repo/src/main.rs`.
All response URIs, diagnostics, and symbol definitions are translated back into local client paths seamlessly.

### 5. Multi-Server Homelab / Cloud Clustering
Deploy across multiple machines on your 10G network:
* Consistent workspace hashing pins repositories to dedicated memory nodes.
* Procedural macro execution offloaded into an isolated worker pool.

---

## 📦 Workspace Layout

```text
prod-code/
├── Cargo.toml                  # Workspace definition
├── README.md                   # Project overview & architecture
├── ROADMAP.md                  # 5-phase engineering plan
├── LICENSE-MIT                 # MIT License
├── LICENSE-APACHE              # Apache 2.0 License
├── crates/
│   ├── prod-code-protocol/     # Binary wire framing, handshake & path translation
│   ├── prod-code-client/       # Ultra-thin CLI bridge (stdio LSP -> 10G TCP)
│   ├── prod-code-gateway/      # Daemon gateway, multi-tenant session dispatcher
│   ├── prod-code-engine-rust/  # In-memory Rust engine (ra_ap_ide::AnalysisHost)
│   ├── prod-code-engine-go/    # Managed gopls worker pool with shared caches
│   └── prod-code-mcp/          # Model Context Protocol (MCP) server for agents
└── bench/                      # Agent fleet mass load testing harness
```

---

## 🚀 Quick Start (Phase 1 Preview)

### Build from Source
```bash
git clone https://github.com/alex09x/prod-code.git
cd prod-code
cargo build --release
```

### 1. Launch the Server Daemon (on server)
```bash
# Bind to 10G interface
./target/release/prod-code-server --bind 0.0.0.0:9400 --storage /srv/prod-code/workspaces
```

### 2. Connect from Laptop / Agent Workstation
```bash
# Configure endpoint
export PROD_CODE_REMOTE=192.0.2.10:9400

# Check connectivity
./target/release/prod-code status

# Run as drop-in language server in your editor
./target/release/prod-code lsp

# Or run as an MCP server for AI coding agents
./target/release/prod-code mcp
```

---

## 🗺️ Roadmap & Milestones

See [**`ROADMAP.md`**](ROADMAP.md) for the active engineering plan:
* **Phase 1**: Wire Protocol, 10G TCP Streaming & Path Translation.
* **Phase 2**: In-Memory Rust Engine Core (`ra_ap_ide::AnalysisHost` + Direct-Edits).
* **Phase 3**: Polyglot Hub (Managed Go `gopls` pool + Python/TS adapters).
* **Phase 4**: Dual-Surface Gateway (LSP Multiplexer + Native Agent MCP Server).
* **Phase 5**: Multi-Node Clustering, Sharding & 50-Worker Fleet Stress Verification.

---

## 👤 Author

**Alex** ([@alex09x](https://github.com/alex09x)) — [alex@prod.codes](mailto:alex@prod.codes)

---

## 📄 License

Dual-licensed under either of:
* Apache License, Version 2.0 ([`LICENSE-APACHE`](LICENSE-APACHE))
* MIT license ([`LICENSE-MIT`](LICENSE-MIT))

at your option.

