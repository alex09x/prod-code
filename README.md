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
Your client talks about `/Users/alex09x/Documents/workspace/repo/src/main.rs`.
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
export PROD_CODE_REMOTE=192.168.2.100:9400

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

