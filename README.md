# prod-code: Remote Code Intelligence (RCI)

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE)
[![Status: Experimental](https://img.shields.io/badge/Status-Active%20Development-orange.svg)]()
[![Network: 10GbE Ready](https://img.shields.io/badge/Network-10GbE%20Optimized-green.svg)]()

**prod-code** is a distributed, polyglot code-intelligence daemon and gateway designed specifically for **high-throughput AI coding agent fleets**, multi-worktree parallelism, and remote cluster offloading over ultra-fast local networks (10 GbE LAN).

It untangles the bottleneck of modern language analysis by decoupling the developer's local workspace from the heavy, memory-hungry semantic engines (Rust Salsa DB, Go `gopls`, Pyright, etc.), moving the compute and 50–100+ GB memory footprints to dedicated server hardware while keeping client latency sub-millisecond.

---

## The Problem: Why Traditional Language Servers Fail at Agent Scale

Modern language servers (`rust-analyzer`, `gopls`, `pyright`, `tsserver`) were built under a single core assumption: **one human developer editing files sequentially inside a single desktop IDE**.

When scaling to **fleets of autonomous AI coding agents** (Claude, Codex, Agy, Cursor Agent) running concurrent tasks across dozens of Git worktrees, this assumption completely breaks:

1. **Catastrophic Memory & CPU Spikes**:
   - An in-memory Salsa database (`rust-analyzer`) or `gopls` instance for a large project easily eats **20–60+ GB of RAM**.
   - Running 10–20 parallel agent worktrees on a developer workstation or laptop leads to thermal throttling, out-of-memory kernel panics, and massive fan noise.
2. **Lock Contention on Overlay Rebuilds**:
   - Traditional servers rebuild structural overlay cones under a global database lock on every text change. Concurrent sessions queue behind each other, causing query latencies to spike from milliseconds to **70–120 seconds**.
3. **Fragmented Daemon Chaos**:
   - Managing separate local supervisors for Go, Rust, Python, and TypeScript creates fragile process trees that crash, leave orphaned sockets, or leak daemons during agent teardowns.
4. **Protocol Mismatch for AI Agents**:
   - Autonomous agents do not need bidirectional, stateful JSON-RPC LSP streams with document synchronization gymnastics. Agents need **fast, stateless, typed semantic answers**: *where is this symbol defined? what are its references? what is the function signature? give me diagnostics.*

---

## The Solution: The prod-code Architecture

`prod-code` splits code intelligence into a lightweight local client and a high-performance remote server cluster connected via 10 GbE LAN.

```
┌────────────────────────────────────────────────────────┐
│  Developer Laptop / Agent Fleet (MacBook / Worker Pods)│
│  • Ultra-lightweight `prod-code` client (< 15 MB)      │
│  • Local IDEs (Cursor, VS Code, Neovim) via stdio LSP  │
│  • Autonomous Agents (Claude, Codex, Agy) via MCP      │
│  Resource Usage: 0% CPU, < 10 MB RAM, completely cool  │
└───────────────────────────┬────────────────────────────┘
                            │ 10 GbE TCP / QUIC (~0.05–0.1 ms latency)
                            ▼
┌────────────────────────────────────────────────────────────────────────┐
│  Dedicated Compute Servers (128+ GB RAM, 32–64 Cores, Fast NVMe)      │
│                                                                        │
│  ┌──────────────────────────────────────────────────────────────────┐  │
│  │  Universal Remote Gateway (:9400)                                │  │
│  │  • Dual-Surface: Standard LSP + Native MCP for AI Agents         │  │
│  │  • Bi-directional Path Translation (/Users/... <-> /srv/...)     │  │
│  │  • Workspace Auto-Detection (Cargo.toml, go.mod, pyproject.toml) │  │
│  └──────────────────────────────────┬───────────────────────────────┘  │
│                                     │                                  │
│        ┌────────────────────────────┼────────────────────────────┐     │
│        ▼                            ▼                            ▼     │
│  ┌──────────────┐             ┌──────────────┐             ┌─────────┐ │
│  │  Rust Engine │             │  Go Engine   │             │ Python/ │ │
│  │  • Clean RA  │             │  • Supervised│             │ TS / JS │ │
│  │ AnalysisHost │             │    gopls pool│             │ Workers │ │
│  │ • Direct-edit│             │  • Shared    │             │         │ │
│  │   in memory  │             │   GOCACHE/mod│             │         │ │
│  └──────────────┘             └──────────────┘             └─────────┘ │
└────────────────────────────────────────────────────────────────────────┘
```

---

## Core Pillars

### 1. Ultra-Fast 10 GbE Network Transport
At 10 Gbps with ~0.08 ms round-trip time, network transfers operate at local memory bus and NVMe speeds (~1.1–1.2 GB/s). `prod-code` uses a specialized binary framing protocol with connection pooling, zero-copy forwarding, and optional TLS/mTLS authentication.

### 2. Dual-Surface API: LSP + Native MCP
* **For Editors (Cursor, VS Code, Neovim)**: Acts as a drop-in Language Server speaking standard JSON-RPC LSP over `stdio` via the thin client.
* **For AI Coding Agents**: Exposes first-class **Model Context Protocol (MCP)** tools. Agents query symbols directly (`definition`, `references`, `outline`, `diagnostics`) without parsing raw LSP envelopes.

### 3. Single-Owner Direct-Edit Fast Path
When an autonomous agent operates in a dedicated Git worktree, unsaved edits are applied directly to the in-memory database inputs without constructing costly structural overlay cones or triggering cascade invalidations for other sessions. This cuts p95 query latency under 15-worker load from **71s to 12s**.

### 4. Transparent Path Translation
The client works with local filesystem paths (e.g. `/Users/alex09x/Documents/workspace/my-app`). The remote daemon transparently maps them to the server-side workspace storage, returning all symbol locations and diagnostic file URIs translated back into local client paths.

### 5. Multi-Server Clustering & Sharding
For multi-server homelabs or rack setups, `prod-code` supports consistent workspace hashing:
* Heavy Rust workspaces with extensive crate graphs are pinned to high-memory nodes (128+ GB).
* Go and Python workloads are distributed across worker nodes.
* Procedural macro compilation and evaluation run in an isolated worker pool.

---

## Repository Structure

```text
prod-code/
├── Cargo.toml                  # Workspace manifest
├── README.md                   # Project overview & architecture
├── ROADMAP.md                  # Detailed phase-by-phase implementation plan
├── crates/
│   ├── prod-code-protocol/     # Wire framing, transport types, path translation
│   ├── prod-code-client/       # Ultra-thin CLI bridge (stdio LSP -> 10G TCP)
│   ├── prod-code-gateway/      # Daemon gateway, session router, LSP/MCP dispatch
│   ├── prod-code-engine-rust/  # In-memory Rust analysis (ra_ap_ide::AnalysisHost)
│   ├── prod-code-engine-go/    # Managed gopls worker pool with shared caches
│   └── prod-code-mcp/          # Native Model Context Protocol (MCP) server
└── bench/                      # Multi-session agent-fleet load testing harness
```

---

## Quick Start (Preview)

### Starting the Remote Daemon (on server)
```bash
# Start daemon listening on 10G interface
prod-code-server --bind 0.0.0.0:9400 --storage /srv/prod-code/workspaces
```

### Running the Client (on laptop / agent pod)
```bash
# Configure endpoint
export PROD_CODE_REMOTE=192.168.2.100:9400

# Use as drop-in LSP for your editor
prod-code lsp

# Or launch as an MCP server for Claude / Codex / Agy
prod-code mcp
```

---

## License

Dual-licensed under either of:
* Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
* MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.
