<div align="center">

# ⚡ `prod-code`
### Remote code intelligence for fleets of coding agents

[![GitHub release](https://img.shields.io/github/v/release/alex09x/prod-code?color=blue&style=flat-square)](https://github.com/alex09x/prod-code/releases)
[![License](https://img.shields.io/badge/license-MIT%20%2F%20Apache--2.0-blue?style=flat-square)](LICENSE-MIT)
[![Rust](https://img.shields.io/badge/rust-2024_edition-dea584?style=flat-square&logo=rust)]()
[![Protocol](https://img.shields.io/badge/protocol-LSP_%2B_native_MCP-purple?style=flat-square)]()

<p align="center">
  <b>The analyzer, the builds and the tests run on a LAN node. The laptop edits.</b><br>
  Your checkout is mirrored to a gateway that keeps a warm analyzer for it (rust-analyzer in
  process; gopls, clangd, TypeScript, pyright, sourcekit-lsp as children) and runs commands
  there. Agents reach it as MCP tools, editors as a drop-in language server.
</p>

</div>

---

## Why

A language server was designed for one human typing in one checkout. What runs on a machine
now is a fleet: a resident agent in the main checkout, workers in their own worktrees, each
asking the analyzer the kind of question a human asks once a minute and asking it ten times a
second. Every worktree brings its own analyzer and its own `cargo test`, and they share the
cores with your editor.

`prod-code` moves all of it one hop away. Measured on this repository, with nothing compiled
on the laptop:

| | |
|---|---|
| hover on a warm workspace | 1–4 ms |
| tool call over a persistent session | ~10 ms |
| a one-shot CLI command from the laptop, end to end (`prod-code hover …`) | ~0.1 s |
| `cargo clippy --workspace --all-targets` on a 32-core node, one file changed | 1 s |
| `cargo test --workspace` — 565 tests | 5 min |
| the same without the suite that starts real gateways | 56 s |
| first load of a Rust workspace (build scripts, proc macros) | ~45 s, once per worktree |

These are recorded workloads; current test counts and timings are listed in each pull request.
The live suite starts real gateways and drives their language servers. Unit and scripted-server
tests complement those scenarios. The required coverage floor is 80% of regions per source file
(`python3 scripts/coverage.py --min 80`); missing measurements fail the gate.

## Architecture

```mermaid
flowchart TD
    subgraph ClientHost["Developer Workstation (Laptop / Desktop)"]
        Agent["AI Coding Agents\n(Claude Code, Codex, Antigravity)"]
        Editor["Code Editors\n(Zed, VS Code, Neovim)"]
        CLI["prod-code CLI"]

        Agent -->|"Native MCP (67 tools)"| MCPBridge["prod-code mcp"]
        Editor -->|"LSP protocol"| LSPBridge["prod-code lsp"]
        CLI -->|"Commands / Queries"| ClientCore["prod-code client"]

        MCPBridge & LSPBridge & ClientCore --> SyncEngine["Fast Mirror & Sync Engine\n(Base64 diffs, watermarks, local cache)"]
    end

    SyncEngine -->|"10 GbE LAN / Gossip (:9400)"| GatewayCluster

    subgraph GatewayCluster["Production Cluster (9 nodes · 355 CPU cores)"]
        subgraph LinuxAarch64["128-Core aarch64 (Ampere Altra)"]
            NodeRama["Linux aarch64 build node (128 cpus)\nRust, Go, C++, Python, TS, Java, Kotlin"]
        end

        subgraph LinuxX86["High-Throughput x86_64 Nodes (192 cpus)"]
            NodeRam11["Zen 5 build node (32 cpus)"]
            NodeRamX["Zen 4 build node + telemetry (32 cpus)"]
            NodeRam9["Polyglot build node (32 cpus)"]
            NodeZFS["Storage & build nodes (96 cpus)"]
        end

        subgraph MacNodes["Apple Silicon macOS Nodes (35 cpus)"]
            NodeStudio["macOS build node (24 cpus) · Swift, Go"]
            NodeMacbook["macOS build node (11 cpus) · Swift"]
        end

        subgraph GatewayInternals["Gateway Architecture (per node)"]
            Router["Placement & Gossip Router"] --> Workspaces["Per-Worktree Mirrors\n(<repo>--wt-<hash>)"]
            Workspaces --> PrimarySalsa["Primary Salsa DB\n(rust-analyzer in-proc; child LSPs)"]
            Workspaces --> ShadowValidation["Isolated Shadow Engines\n(RAM-disk validation without cache poison)"]
            Workspaces --> RemoteExec["Remote Builds, Lints & Tests\n(cargo, go, pytest, swift, etc.)"]
            GatewayInternals --> Telemetry["Prometheus Scrape (:9401)\n& Pushgateway (:9091)"]
        end
    end

    Telemetry --> Monitoring["Prometheus + VictoriaMetrics + Grafana"]
```

`prod-code` separates the lightweight developer interface from heavy language intelligence and compilation:

1. **Mirroring over Mounting**: Workspaces synchronize fast Base64 diffs over 10 GbE LAN instead of network-mounting filesystems (NFS/SSHFS), keeping filesystem operations local and fast.
2. **One Analyzer per Worktree**: Salsa databases and child language servers run isolated per worktree (`<repo>--wt-<hash>`), eliminating cache contamination between concurrent agent sessions.
3. **Dual-Engine Shadow Validation**: Code edits are verified on dedicated shadow engines in memory before writing to disk, ensuring speculative proposals cannot invalidate warm primary caches.
4. **Polyglot Cluster Gossip**: Nodes discover each other via UDP gossip on `:9401`, evaluate load and memory pressure, and route requests to capable nodes (including automatic rerouting of Swift subprojects to macOS nodes).

## Requirements

Building `prod-code` and running its development checks require Rust 1.95.0 or newer. With
rustup, install the supported minimum and the check components with:

```sh
rustup toolchain install 1.95.0 --profile minimal -c rustfmt -c clippy
```

Run commands with `cargo +1.95.0` to use that toolchain without changing your default.

## Install

### Package Managers & Direct Download

Choose your preferred installation method for the `prod-code` CLI:

```sh
# Homebrew (macOS & Linux)
brew tap alex09x/tap
brew install prod-code

# Direct shell installer (detects OS and architecture)
curl -fsSL https://prod.codes/install.sh | sh

# Debian / Ubuntu (.deb)
curl -fsSLO https://github.com/alex09x/prod-code/releases/latest/download/prod-code_amd64.deb
sudo dpkg -i prod-code_amd64.deb

# RHEL / Fedora / Rocky Linux (.rpm)
sudo rpm -i https://github.com/alex09x/prod-code/releases/latest/download/prod-code.x86_64.rpm

# Arch Linux (.pkg.tar.zst)
sudo pacman -U https://github.com/alex09x/prod-code/releases/latest/download/prod-code-x86_64.pkg.tar.zst

# Build from source
cargo install --path crates/prod-code-client
```

Native notarized macOS installers (`.pkg` and `.dmg`) and standalone binaries for `x86_64` and `aarch64` (Linux & macOS) are available on [GitHub Releases](https://github.com/alex09x/prod-code/releases/latest).

### In-Place Self-Updater

Keep your client installation up to date with a single command:

```sh
prod-code update --check   # check for newer releases on GitHub
prod-code update           # download, verify SHA-256 integrity, and replace binary in-place
```

When `prod-code` is updated, running agent sessions automatically pick up new MCP tools without requiring an IDE or agent restart (dynamic re-exec on binary change via MCP `notifications/tools/list_changed`).

### Registering with AI Coding Agents

Register `prod-code` once; every repository accessed by your agent gains remote code intelligence tools:

```sh
# Claude Code
claude mcp add --scope user prod-code -e PROD_CODE_REMOTE=192.0.2.10:9400 -- prod-code mcp

# OpenAI Codex CLI
codex mcp add prod-code --env PROD_CODE_REMOTE=192.0.2.10:9400 -- prod-code mcp
```

For **Google Antigravity** / **Gemini CLI**, add the server definition to `~/.gemini/config/mcp_config.json`:

```json
{
  "mcpServers": {
    "prod-code": {
      "command": "prod-code",
      "args": ["mcp"],
      "env": {
        "PROD_CODE_REMOTE": "192.0.2.10:9400"
      }
    }
  }
}
```

### Running the Gateway Daemon

On a Linux or macOS build node:

```sh
# Start gateway server directly
prod-code-server --bind 0.0.0.0:9400 --storage /srv/prod-code/workspaces \
  --advertise 192.0.2.10:9400 --peers 192.0.2.11:9400,192.0.2.12:9400

# Or run with declarative gateway.toml
prod-code-server --config /etc/prod-code/gateway.toml
```

## In an editor

`prod-code lsp` is an editor's language server. The language's own server (rust-analyzer,
gopls, clangd, basedpyright, the TypeScript server, sourcekit-lsp) runs on the node, started
for the editor's session in the node's copy of the checkout, and the bridge carries the
protocol both ways:

- The editor's settings, rust-analyzer's check on save and the server's protocol extensions
  work as they do with a local server.
- A save reaches the node before the server hears of it.
- Files the server points at that exist only on the node (the standard library, dependency
  caches, generated files) are mirrored read-only under the user's cache directory.
- `--language` picks the server for one language of a mixed checkout.

For Zed there is an extension in [`editors/zed`](editors/zed/README.md). Or point Zed's own
rust-analyzer at `prod-code lsp --language rust`, which keeps everything Zed wires to
rust-analyzer.

## What it gives an agent

Tools use the node that holds the workspace. The tables describe current source; check
[CHANGELOG.md](CHANGELOG.md) for the version that contains a change.

**Find code**

| tool | what it does |
|---|---|
| `code_search` | find code by natural language query; ranked declarations with matching doc comments (dense embeddings fused with lexical ranking) |
| `code_symbols` | workspace symbol index by name, fuzzy matching, analyzer-backed |
| `code_definition` · `code_references` | exact symbol definition location; every reference and usage across the workspace |
| `code_callers` · `code_callees` · `code_implementations` · `code_supertypes` | call hierarchy in both directions as a tree to requested depth; trait/interface implementations, and supertypes or required traits |
| `code_outline` | declaration outline with kinds and line numbers for a single file or every source file in a directory |
| `code_source` | read stdlib, crate/package registry and SDK sources that live cached only on the gateway node |
| `code_structural_search` | AST pattern search across polyglot languages using metavariables (`$name`) without brittle regex |
| `code_find_duplicates` | clone and duplicate block detection across workspace files using Type-1 and Type-2 AST hashing |

**Understand it without reading everything**

| tool | what it does |
|---|---|
| `code_slice` | bounded transitive traversal of declarations referenced by a symbol; depth and byte budgets limit result size |
| `code_hover` · `code_type_at` | signature, precise inferred type, documentation, and markdown hover information |
| `code_impact` | blast radius of uncommitted changes: changed functions, reachable callers, and candidate tests |
| `code_dependencies` | workspace dependency graph analysis, direct and transitive dependencies, coupling metrics, and cycle detection |
| `code_dead_code` | reference-based unreferenced candidate declarations, marking unverified and protected symbols |
| `code_prune_orphans` | eligible dead-code candidates removed with safe delete and analyzer validation; unverified and protected symbols remain |

**Change it safely**

Most custom planners below target Rust. The parameter, move, extract-function, wrap-return, and OOP refactoring tools also support Go, C/C++,
TypeScript/JavaScript, Python, and Swift for their documented input shapes. Rename and server
code actions depend on the selected language server. See [ROADMAP-AUDIT.md](ROADMAP-AUDIT.md)
for missing capabilities and [ROADMAP.md](ROADMAP.md) for individual restrictions.

Analyzer diagnostics do not include every compiler or borrow-checker check. Tools with
`verify: "compile"` can additionally run a shadow compiler check; semantic refusals still apply.
A Rust file reported as `unlinked-file` is unchecked and cannot pass edit validation. New
modules must be checked with their module declarations; new Cargo targets need a workspace
reload or an explicit compiler check that includes them. Missing or malformed diagnostic
reports also fail validation. Manifest and lockfile proposals need
`code_shadow_run` with the appropriate parser or build command; JSON manifests, Markdown documentation,
and SVG vector graphics are validated directly through built-in syntax parsers.

| tool | what it does |
|---|---|
| `code_validate_edit` · `code_validate_edits` | in-memory analyzer diagnostics and syntax validation for proposed contents across one or many files before writing; detects broken references and schema violations |
| `code_diagnostics` | diagnostics for a file, in memory, without running a full build |
| `code_rename` · `code_safe_delete` | semantic rename across the workspace (a field with its accessors, with `accessors`); delete declaration only when zero references exist, or parameter with arguments |
| `code_change_signature` | Rust signature changes; Go reorder/removal through gopls and typed literal additions to ordinary functions with compiler checks |
| `code_move` | a declaration moved to another module across languages, with imports it takes and leaves behind |
| `code_move_method` · `code_move_module` | a method moved to the type of one of its parameters; a whole module moved to another parent with all referencing paths |
| `code_extract_function` | a selection extracted into a named function across languages, replacing identical duplicates across files |
| `code_introduce_parameter_object` | parameters bundled into a struct, dataclass, or object across callers and body uses |
| `code_extract_parameter` · `code_inline_parameter` | promote an expression to a parameter; inline a uniform constant parameter into the body |
| `code_migrate_type` | change a declared type, listing all incompatible sites before execution, with automatic `.into()` insertion |
| `code_generify` | turn a parameter's concrete type into a bounded generic type parameter, verifying callers against the new signature |
| `code_invert_boolean` | invert a boolean predicate, variable, or field, rewriting all conditions with identical runtime semantics |
| `code_make_static` · `code_convert_to_method` | turn a method that never uses `self` into an associated function; turn an associated function into a method |
| `code_extract_delegate` | move cohesive fields and methods to a helper delegate struct, updating accessors and instantiations |
| `code_extract_trait` · `code_extract_interface` | extract inherent methods into a trait or interface definition across languages with caller imports |
| `code_loop_to_iterator` | transform accumulator loops into idiomatic functional iterator chains across languages |
| `code_introduce_variable` | bind repeated expression once (`let w1 = expr;`) and replace occurrences safely |
| `code_wrap_return` | wrap return type in `Option`, `Result`, or `Promise`, adding `?` or `await` at propagating callers |
| `code_extract_field` · `code_encapsulate_field` | turn an expression into a struct field; encapsulate a public field with getters/setters |
| `code_schema_rename` | rename a database/serialization schema field across multi-language models |
| `code_assists` · `code_assist` | list and apply analyzer code actions, quick fixes, and compiler suggestions directly to checkout |
| `code_codemod` | structural search and replace on syntax trees (`pattern ==>> replacement`), as diff or applied |
| `code_generate_fixture` | synthesize valid typed test fixtures and builders from struct definitions with default fallbacks |
| `code_propose_expression` | synthesize valid in-scope expressions evaluating to a requested target type |
| `code_pull_up` · `code_push_down` | pull members up to a superclass/trait or push members down to subclasses/implementors |
| `code_replace_constructor_with_factory` | replace direct constructor calls with named static factory methods |
| `code_replace_constructor_with_builder` | transform multi-parameter constructors into fluent builder patterns |
| `code_replace_inheritance_with_delegation` | refactor class inheritance hierarchies into composition and delegation |
| `code_replace_conditional_with_polymorphism` | replace `switch`/`match`/`if-else` conditionals with polymorphic dispatch |
| `code_shadow_run` | test multiple hypotheses in isolated RAM-disk workspaces concurrently and return the winning diff |

**Run it**

| tool | what it does |
|---|---|
| `code_check` · `code_lint` · `code_test` · `code_benchmarks` | compile check, lint, test, and benchmark on the node with parsed diagnostics; `path` narrows scope; `fix: true` applies compiler fixes |
| `code_exec` | execute any build or toolchain command in remote workspace copy; modified files, formatters, and lockfiles are written back |
| `code_diagnose_failure` | run tests and generate diagnostic failure dossiers with sites, callers, changes, and printed assertion operands |

**Operate it**

| tool | what it does |
|---|---|
| `code_status` · `code_sync` | gateway health, registered engines, loaded workspaces, and running builds/tests; manual delta sync push or pull |
| `code_report_issue` | report a bug or missing feature directly to GitHub with sanitized diagnostics and automated issue deduplication |

Every position tool also takes `symbol` instead of a file and a position, so an agent never
has to grep for a line number:

```json
{ "name": "code_callers", "arguments": { "symbol": "Metrics::record" } }
```

The same surface exists as a CLI for humans and scripts: `prod-code search | slice | codemod |
fixture | change-signature | schema-rename | migrate-type | move | move-method | move-module |
parameter-object | extract-parameter | extract-field | encapsulate-field | wrap-return | make-static |
convert-to-method | inline-parameter | introduce-variable | extract-trait | extract-interface |
extract-delegate | pull-up | push-down | replace-constructor-with-factory |
replace-constructor-with-builder | replace-inheritance-with-delegation |
replace-conditional-with-polymorphism | prune | loop-to-iterator | invert-boolean | generify | hover |
def | refs | callers | callees | impls | supertypes | symbols | outline | validate | diagnostics |
check | lint | test | benchmarks | exec | impact | diagnose | rename | assists | assist | safe-delete |
dead-code | dependencies | duplicates | structural-search | propose-expression | shadow-run | source |
status | cluster | resolve | sync | pull | update | package | cert | metrics`. The position commands take
`--symbol NAME` instead of a file and a position, `prod-code symbols <name>` finds a declaration by
name, and `prod-code validate FILE --from NEW --with OTHER=NEW2` checks a multi-file change in one overlay.

## Three things worth seeing

**Ask a question in words.** The gateway indexes every declaration with the doc comment above
it and ranks them against your question. When embeddings are available, dense ranking is
fused with lexical ranking; otherwise the response identifies lexical-only search.

```
$ prod-code search "how do we decide which node runs a workspace"
10 hit(s) in 16 ms (1010 declarations, 40 files)

 1. [function] pick_node  crates/prod-code-mcp/src/cluster.rs:107
    Chooses the gateway for `workspace_name` among `nodes`: the remembered placement when it
    is still one of the nodes, alive and able to serve `engine`, otherwise the quietest node
```

**Read a symbol without reading its files.** `code_slice` follows the analyzer's own edges
from a declaration within the requested budgets. It returns whole declarations, without
intra-function data-flow analysis.

```
$ prod-code slice crates/prod-code-gateway/src/shadow.rs --line 469 --depth 1
slice of `run_shadow`: 10 item(s), 11474 bytes from 284105 bytes of source (96% smaller)
```

**Try three fixes at once.** Each candidate runs in an overlay of the workspace mounted at
the workspace's own path, so the warm `target/` stays valid and nothing touches your checkout.

```
$ prod-code shadow-run spec.json -- cargo test
  plus      exit 0     in 0.4s  2 passed, 0 failed  <- winner
  clamp     exit 0     in 0.4s  2 passed, 0 failed
  baseline  exit 101   in 0.0s  0 passed, 2 failed
  mul       exit 101   in 0.4s  0 passed, 2 failed
```

## How it works

**The checkout is mirrored, not mounted.** First contact sends a manifest of paths, sizes and
hashes; the gateway seeds a new worktree from the origin repository's copy and asks only for
what is missing (0.4 s instead of ~7 s for a 10 MB repository). After that the client keeps a
watermark per node and sends a diff. Content travels base64, which took a 10.8 MB first sync
from 8.1 s to 0.76 s. The MCP server watches the tree and syncs only when something changed.

**One analyzer per worktree, never shared.** Salsa holds one state of one workspace keyed by
file path, so two worktrees served from one database answer each other's questions. Every git
worktree gets its own server workspace and its own database, named `<repo>--wt-<hash>`. The
cost is the first load; the engine then stays resident until it has been idle for thirty
minutes (`--idle-evict-secs`), and worktree directories are pruned after seven days
(`--prune-worktree-days`).

**Proposals are checked on a second analyzer.** Validating an edit opens the proposed text in the
analyzer and closes it again; in the analyzer every other query uses, a proposal that changes
what a widely imported file declares made the next query re-infer the crate (21 s for a
`references` after a dry run). Validation sessions run on a second engine for the same
workspace, fed by every sync like the first, so a dry run never reaches anyone else's query. It
costs the memory of one more database per workspace that has been validated.

**Builds and tests run on the node**, in the workspace's own `target/`, which stays warm
between runs. Files a command changes are written back into your checkout.

**Several nodes.** Gateways gossip every 5 s; a client needs one seed address. A checkout is
placed on the node that already holds it, otherwise on the quietest one that serves its
language. `--engines rust,go` restricts a node to what it should serve, so a macOS node can be
Swift-only and placement never sends Rust work to a workstation. A request that names a path in a
nested project of another language — `code_test {path: "swift"}` in a Rust repository with a
SwiftPM package under `swift/` — goes to a node that serves that language, placed under its own
key so the checkout's own placement stays. Nodes given with `--remote` on the command line are
used as given, without asking the cluster.

```
$ prod-code cluster
⚡ prod-code cluster (9 node(s) · 355 cpus)
192.0.2.10:9400   UP   load 0.05/cpu (128 cpus)  workspaces 3   rss 482 MB  [linux/aarch64]
                  engines: rust, go, cpp, python, typescript, java, kotlin, csharp, scala, zig, nim, d, php, ruby, dart
192.0.2.11:9400   UP   load 0.12/cpu (32 cpus)   workspaces 2   rss 310 MB  [linux/x86_64 Zen5]
                  engines: rust, go, cpp, python, typescript
192.0.2.12:9400   UP   load 0.08/cpu (32 cpus)   workspaces 1   rss 280 MB  [linux/x86_64 Zen4]
                  engines: rust, go, cpp, python, typescript
192.0.2.13:9400   UP   load 0.15/cpu (32 cpus)   workspaces 4   rss 415 MB  [linux/x86_64]
                  engines: rust, go, cpp, python, typescript, java, kotlin, csharp, scala, zig, nim, d, php, ruby, dart
192.0.2.14:9400   UP   load 0.04/cpu (32 cpus)   workspaces 1   rss 195 MB  [linux/x86_64]
                  engines: rust, go, cpp, python, typescript
192.0.2.15:9400   UP   load 0.06/cpu (32 cpus)   workspaces 0   rss 160 MB  [linux/x86_64]
                  engines: rust, go, cpp, python, typescript
192.0.2.16:9400   UP   load 0.07/cpu (32 cpus)   workspaces 0   rss 155 MB  [linux/x86_64]
                  engines: rust, go, cpp, python, typescript
192.0.2.17:9400   UP   load 0.18/cpu (24 cpus)   workspaces 2   rss 290 MB  [darwin/arm64 Mac Studio]
                  engines: swift, go, rust, typescript
192.0.2.18:9400   UP   load 0.22/cpu (11 cpus)   workspaces 1   rss 210 MB  [darwin/arm64 MacBook Pro]
                  engines: swift
```

## What each language gets

| | Rust | Go | C/C++ | TypeScript/JavaScript | Python | Swift |
|---|---|---|---|---|---|---|
| engine | rust-analyzer in-process | gopls | clangd | TypeScript 7 native LSP | basedpyright | sourcekit-lsp (macOS node) |
| hover / def / refs / symbols / callers / callees / impls | yes | yes | yes | yes | yes | yes |
| rename | yes (+ module files) | yes | yes | yes | yes | yes |
| server code actions | yes | server-dependent | server-dependent | server-dependent | server-dependent | server-dependent |
| safe-delete | yes | - | - | - | - | - |
| custom signature changes | yes | named-parameter permutations | - | - | - | - |
| extract parameter / parameter object | supported shapes | supported shapes | supported shapes | supported shapes | supported shapes | supported shapes |
| check | cargo check | go build | cmake / meson / make | tsc (bunx / pnpm / yarn / npx) | basedpyright (uv / .venv aware) | swift build / xcodebuild |
| lint | clippy | go vet | - | eslint / biome | ruff | - |
| test | cargo test | go test | ctest / meson test | vitest / jest / bun test / mocha | pytest / unittest | swift test / xcodebuild test |

`code_search` and `code_slice` work on all six. Tooling is detected from the checkout (lock
files, `package.json`, `pyproject`, `CMakeLists`…). Dependencies live on the node: run
`prod-code exec -- bun install`, `-- uv sync` or `-- npm ci` once per checkout and the
language servers and test runners use them.

## Configuration

### Gateway Server (`gateway.toml`)

The gateway daemon discovers its configuration file automatically from the following locations (highest to lowest priority):

1. Explicit CLI argument: `--config /path/to/gateway.toml`
2. System configuration: `/etc/prod-code/gateway.toml`
3. User XDG configuration: `~/.config/prod-code/gateway.toml`
4. Home directory: `~/.prod-code/gateway.toml`
5. Current working directory: `./gateway.toml`

CLI flags and environment variables always override values specified in `gateway.toml`.

```toml
[server]
# Network bind address for RPC connections (default: 0.0.0.0:9400)
bind = "0.0.0.0:9400"

# Optional Unix domain socket path
# socket_path = "/run/prod-code/gateway.sock"

# Workspace storage root directory on server
storage = "/srv/prod-code/workspaces"

# Advertised host:port for gossip peers and clients
advertise = "192.0.2.10:9400"

# Cluster peer addresses for gossip discovery
peers = "192.0.2.11:9400,192.0.2.12:9400"

# Languages served (empty array serves all installed language engines)
engines = ["rust", "go", "cpp", "python", "typescript"]

# Unload idle engines after this many seconds (0 disables, default: 1800)
idle_evict_secs = 1800

# Retain git worktree mirrors for this many days (default: 7)
prune_worktree_days = 7

# RAM-disk build cache (/dev/shm) for high-throughput compilation
build_cache_ram = false

[metrics]
enabled = true
snapshot_interval_secs = 30
inventory_interval_secs = 3600

[metrics.retention]
retention_days = 14
max_storage_bytes = 524288000   # 500 MB budget for metrics logs

[metrics.prometheus]
enabled = true
mode = "scrape"                 # "scrape" (pull HTTP endpoint), "push" (Pushgateway), or "both"
listen = "0.0.0.0:9401"         # Serves GET /metrics and GET /health
# push_url = "http://192.0.2.20:9091"
push_interval_secs = 15
job = "prod-code"
```

### Per-Repository Options (`prod-code.toml`)

Put a `prod-code.toml` at the checkout root to tune how the gateway analyses your repository. It is mirrored like any manifest and evaluated when the workspace loads:

```toml
[rust]
features = "all"            # or ["feat-a", "feat-b"]; default: the crate's default features
no_default_features = false
all_targets = true          # tests, benches, and examples are analysed (default: true)
sysroot = true              # standard library from rust-src (default: true)
```

Use `features = "all"` when the same module tree is compiled into several crates behind feature flags: rust-analyzer attaches each file to one crate, and a module behind a disabled feature is otherwise dead there.

### Environment Variables

The client and gateway respect the following environment variables:

| Variable | Scope | Description |
|---|---|---|
| `PROD_CODE_REMOTE` | Client | Remote gateway address(es) (`host:port[,host:port...]`), defaults to `127.0.0.1:9400`. Rendezvous hashing picks the target node with automatic failover. |
| `PROD_CODE_AUTH_TOKEN` | Client / Gateway | Shared bearer authentication token for secure RPC connections across untrusted networks. |
| `PROD_CODE_ENGINES` | Gateway | Comma-separated list of engines the node is permitted to serve (e.g. `swift` on macOS nodes, `rust,go` on Linux). |
| `PROD_CODE_SIGN_IDENTITY` | Gateway (macOS) | Developer ID / ad-hoc signing identity for macOS gateway daemons to preserve Local Network permissions across updates. |
| `PROD_CODE_LOCAL` | Client | When set to `1` or `true`, forces local execution mode, bypassing remote gateways. |

## Operating & Fleet Management

**Automated Cluster Rollout.** Deploy zero-downtime updates across multi-node clusters using our rollout scripts:

```sh
# Rolling upgrade across all nodes with canary staging and parallel downloads
python3 scripts/cluster-rollout.py --release 0.3.29

# Or run the shell orchestrator
scripts/cluster-rollout.sh 0.3.29
```

The rollout script verifies SHA-256 package checksums, upgrades a canary node, verifies health checks (`GET http://<node>:9401/health`), and performs a sequential rolling restart across remaining nodes.

**Prometheus & VictoriaMetrics Telemetry.** Gateways export native metrics for Grafana dashboards:
- **HTTP Scrape Endpoint**: `http://<node>:9401/metrics` exposes Prometheus exposition format metrics (CPU, RSS memory, cache hit rates, query latencies, active Salsa engines).
- **Health Probes**: `http://<node>:9401/health` returns `200 OK` when the node is ready to accept workspace traffic.
- **Pushgateway**: Configure `mode = "push"` or `mode = "both"` in `gateway.toml` to stream telemetry directly to a central Prometheus Pushgateway.
- **Event Audit Ledger**: Every query, command, and sync round appends an audit event to `<storage>/../metrics/events-YYYY-MM-DD.jsonl`. Analyze aggregate latency and usage across the cluster with `prod-code metrics [--since SECS]`.

**macOS Nodes.** `scripts/deploy-mac-node.sh <host> <advertise> <peers>` builds or installs the gateway, signs it with `PROD_CODE_SIGN_IDENTITY` under a stable bundle identifier (so macOS remembers the Local Network permission across updates), writes the launchd agent with `--engines` (`PROD_CODE_ENGINES`, default `swift`), and reloads the service.

## Field Evaluations & Benchmarks

We evaluate `prod-code` against prominent real-world open-source repositories using our standardized [67-tool evaluation protocol](docs/eval-protocol.md) across cluster nodes (0% CPU on developer laptop):

| Repository | Stack | Tested Tools | Cold Sync | LAN Latency | Deep-Dive Report |
|---|---|---|---|---|---|
| [`BurntSushi/ripgrep`](https://github.com/BurntSushi/ripgrep) | Rust (32-core node) | 67 / 67 tools (AST search, slicing, 30 refactorings, RAM validation) | 45.4 KB (27 files) | 1.02 ms RTT | [Can 67 AST Tools Break Ripgrep? →](https://prod.codes/blog/can-67-ast-tools-break-ripgrep/) |
| [`quickwit-oss/tantivy`](https://github.com/quickwit-oss/tantivy) | Rust (32-core node) | 67 / 67 tools (10-crate DAG, 18-file rename, clone detection, AST search) | 6.77 MB (602 files) | 0.96 ms RTT | [7,156 Cyclic Module Paths in Tantivy →](https://prod.codes/blog/7156-cyclic-module-paths-in-tantivy/) |
| [`tokio-rs/tokio`](https://github.com/tokio-rs/tokio) | Rust (32-core node) | 67 / 67 tools (10-crate circular DAG, AST slicing, Issue #736 fix, Criterion bench) | 6.11 MB (882 files) | 0.63 ms RTT | [Six Circular Dependencies in Tokio →](https://prod.codes/blog/six-circular-dependencies-in-tokio/) |
| [`pola-rs/polars`](https://github.com/pola-rs/polars) | Rust (32-core node) | 67 / 67 tools (33-crate DAG, Issue #737 fix, AST slicing, RAM validation) | 29.6 MB (3,426 files) | 0.56 ms RTT | [Half a Million Lines of Arrow →](https://prod.codes/blog/half-a-million-lines-of-arrow-what-67-ast-analyzers-found-inside-polars/) |
| [`astral-sh/uv`](https://github.com/astral-sh/uv) | Rust (32-core node) | 67 / 67 tools (74-crate DAG, 347 clones, Issue #738 fix, AST slicing) | 36.5 MB (1,787 files) | 0.50 ms RTT | [347 Code Clones Inside uv →](https://prod.codes/blog/347-code-clones-and-a-circular-crate-inside-uv/) |
| [`bevyengine/bevy`](https://github.com/bevyengine/bevy) | Rust (32-core node) | 67 / 67 tools (97-crate DAG, 204 clones, Issue #739 fix, AST slicing, 28 traits on Entity) | 81.0 MB (3,044 files) | 0.61 ms RTT | [204 Code Clones and Two Self-Loops in Bevy →](https://prod.codes/blog/204-code-clones-and-two-self-loops-inside-bevy/) |
| [`prometheus/prometheus`](https://github.com/prometheus/prometheus) | Go (32-core node) | 67 / 67 tools (84-package DAG, 1,485 error checks, Issue #740 fix, 99% slicing) | 385K lines (736 files) | 0.86 ms RTT | [1,485 Error Checks and Zero Import Cycles in Prometheus →](https://prod.codes/blog/1485-error-checks-and-zero-cycles-inside-prometheus/) |
| [`kubernetes/kubernetes`](https://github.com/kubernetes/kubernetes) | Go (32-core node) | 67 / 67 tools (779-pkg DAG, 900+ clones, Issues #741 & #742 fixes, full Go refactoring suite) | 5.38M lines (17,823 files) | 0.74 ms RTT | [5.3 Million Lines of Go in Kubernetes →](https://prod.codes/blog/5-million-lines-of-go-inside-kubernetes/) |
| [`etcd-io/etcd`](https://github.com/etcd-io/etcd) | Go (32-core node) | 67 / 67 tools (1,094-pkg DAG, zero cycles, 445 error returns, MoveLeader slice, interface guards) | 220K lines (1,094 files) | 0.65 ms RTT | [etcd Under the Microscope →](https://prod.codes/blog/etcd-under-the-microscope-67-ast-tools/) |
| [`django/django`](https://github.com/django/django) | Python (32-core node) | 67 / 67 tools (25 circular checks, 150+ clones, basedpyright diagnostics, full Python refactoring) | 527K lines (2,932 files) | 0.65 ms RTT | [Half a Million Lines of Python Inside Django →](https://prod.codes/blog/half-a-million-lines-of-python-inside-django/) |
| [`psf/black`](https://github.com/psf/black) | Python (32-core node) | 67 / 67 tools (concurrency cycle, 10-line version clones, 12-callsite parameter bundling, 99.8% slice) | 135K lines (358 files) | 0.93 ms RTT | [Black Under the Microscope →](https://prod.codes/blog/black-under-the-microscope-67-ast-tools/) |
| [`tiangolo/fastapi`](https://github.com/tiangolo/fastapi) | Python (32-core node) | 67 / 67 tools (135 OpenAPI clones, 66 error guards, 99.8% dependency slice, parameter bundling) | 118K lines (1,166 files) | 0.93 ms RTT | [FastAPI Under the Microscope →](https://prod.codes/blog/fastapi-under-the-microscope-67-ast-tools/) |
| [`pallets/flask`](https://github.com/pallets/flask) | Python (32-core node) | 67 / 67 tools (blueprint clones, 159 error guards, 98.6% dispatch slice, dataclass bundling) | 18K lines (83 files) | 0.65 ms RTT | [Flask Under the Microscope →](https://prod.codes/blog/flask-under-the-microscope-67-ast-tools/) |
| [`moby/moby`](https://github.com/moby/moby) | Go (32-core node) | 67 / 67 tools (224 varint clones, 858 error guards, 99.9% ContainerStart slice, interface safety refusal) | 388K lines (2,269 files) | 0.65 ms RTT | [Moby Under the Microscope →](https://prod.codes/blog/moby-under-the-microscope-67-ast-tools/) |
| [`microsoft/TypeScript`](https://github.com/microsoft/TypeScript) | TypeScript (32-core node) | 67 / 67 tools (264K decls, 17 AST factory clones, 99.9% createSourceFile slice, interface synthesis) | 31,433 files | 0.65 ms RTT | [TypeScript Under the Microscope →](https://prod.codes/blog/typescript-under-the-microscope-67-ast-tools/) |
| [`facebook/react`](https://github.com/facebook/react) | JavaScript/TS (32-core node) | 67 / 67 tools (Fiber work loop slice, 18 test clones, 156 error throws, compiler generic guardrails) | 745K lines (4,445 files) | 0.65 ms RTT | [React Under the Microscope →](https://prod.codes/blog/react-under-the-microscope-67-ast-tools/) |
| [`vercel/next.js`](https://github.com/vercel/next.js) | TypeScript/JS (32-core node) | 67 / 67 tools (SSR render slice, 484 clone groups, 1,254 error assertions, cross-file boolean inversion) | 1.29M lines (24,309 files) | 0.65 ms RTT | [Next.js Under the Microscope →](https://prod.codes/blog/nextjs-under-the-microscope-67-ast-tools/) |
| [`expressjs/express`](https://github.com/expressjs/express) | JavaScript (32-core node) | 67 / 67 tools (95% app.handle slice, 10 TypeError sites, Clone Group #150, dynamic receiver guardrails) | 21.5K lines (201 files) | 0.65 ms RTT | [Express Under the Microscope →](https://prod.codes/blog/express-under-the-microscope-67-ast-tools/) |
| [`redis/redis`](https://github.com/redis/redis) | C (32-core node) | 67 / 67 tools (event loop slice to epoll_wait, 454 error reply sites, Clone Group #382, non-call guards) | 212K lines (219 files) | 0.65 ms RTT | [Redis Under the Microscope →](https://prod.codes/blog/redis-under-the-microscope-67-ast-tools/) |
| [`duckdb/duckdb`](https://github.com/duckdb/duckdb) | C++ (32-core node) | 67 / 67 tools (QueryResult slice, 2,277 InternalException sites, 5,500+ clone groups, Catch2 guards) | 681K lines (3,273 files) | 0.65 ms RTT | [DuckDB Under the Microscope →](https://prod.codes/blog/duckdb-under-the-microscope-67-ast-tools/) |
| [`git/git`](https://github.com/git/git) | C (32-core node) | 67 / 67 tools (setup_git_directory in 316 ms, 4,657 error exit points, Clone Group #199, multi-file codemods) | 443K lines (986 files) | 0.65 ms RTT | [Git Under the Microscope →](https://prod.codes/blog/git-under-the-microscope-67-ast-tools/) |
| [`curl/curl`](https://github.com/curl/curl) | C (32-core node) | 67 / 67 tools (Curl_easy centrality 4.83, 1,924 error/telemetry points, Clone Group #780, parameterized codemod) | 200K lines (1,048 files) | 0.61 ms RTT | [curl Under the Microscope →](https://prod.codes/blog/curl-under-the-microscope-67-ast-tools/) |
| [`apple/swift-algorithms`](https://github.com/apple/swift-algorithms) | Swift (macOS node) | 67 / 67 tools (Chain2Sequence graph, 81 bounds invariants, Clone Group #314, remote test offload) | 13.7K lines (56 files) | 5.44 ms RTT | [Swift Algorithms Under the Microscope →](https://prod.codes/blog/swift-algorithms-under-the-microscope-67-ast-tools/) |
| [`apple/swift-argument-parser`](https://github.com/apple/swift-argument-parser) | Swift (macOS node) | 67 / 67 tools (ParsableCommand in-degree 296, 64 error egress points, property wrapper clones, 81 tests offloaded) | 30.0K lines (121 files) | 5.44 ms RTT | [Swift Argument Parser Under the Microscope →](https://prod.codes/blog/swift-argument-parser-under-the-microscope-67-ast-tools/) |
| [`Alamofire/Alamofire`](https://github.com/Alamofire/Alamofire) | Swift (macOS node) | 67 / 67 tools (SessionDelegate multiplexer, 92 error boundaries, 20 clone groups, remote test offload) | 39.2K lines (102 files) | 5.44 ms RTT | [Alamofire Under the Microscope →](https://prod.codes/blog/alamofire-under-the-microscope-67-ast-tools/) |
| [`apple/swift-collections`](https://github.com/apple/swift-collections) | Swift (macOS node) | 67 / 67 tools (Deque ring buffer, 1,279 boundary invariants, Clone Group #1165, 130 tests offloaded) | 71.0K lines (391 files) | 5.44 ms RTT | [Swift Collections Under the Microscope →](https://prod.codes/blog/swift-collections-under-the-microscope-67-ast-tools/) |
| [`google/guava`](https://github.com/google/guava) | Java (32-core node) | 67 / 67 tools (795K lines, 33-occurrence clone analysis, 3,279 files, 545.82 ms search, refactoring guardrails) | 795K lines (3,279 files) | 0.65 ms RTT | [Guava Under the Microscope →](https://prod.codes/blog/guava-under-the-microscope-67-ast-tools/) |
| [`spring-projects/spring-boot`](https://github.com/spring-projects/spring-boot) | Java (32-core node) | 67 / 67 tools (880K lines, 448 modules, 2,790 deps DAG, 55 autoconfig clones, 982 AST matches, cluster JDTLS 0 errors) | 880K lines (8,696 files) | 0.65 ms RTT | [Spring Boot Under the Microscope →](https://prod.codes/blog/spring-boot-under-the-microscope-67-ast-tools/) |
| [`apache/kafka`](https://github.com/apache/kafka) | Java/Scala (32-core node) | 67 / 67 tools (1.7M lines, 64-module DAG, 125-occurrence equals() clones, 847 AST matches, cluster JDTLS 0 errors) | 1.72M lines (6,475 files) | 0.49 ms RTT | [Kafka Under the Microscope →](https://prod.codes/blog/kafka-under-the-microscope-67-ast-tools/) |
| [`netty/netty`](https://github.com/netty/netty) | Java (32-core node) | 67 / 67 tools (652K lines, 61 modules, all→bom cycle, 510 Huffman clones, multi-site refactoring, cluster JDTLS 0 errors) | 652K lines (3,595 files) | 3.43 ms RTT | [Netty Under the Microscope →](https://prod.codes/blog/netty-under-the-microscope-67-ast-tools/) |
| [`ktorio/ktor`](https://github.com/ktorio/ktor) | Kotlin (32-core node) | 67 / 67 tools (296K lines, 134 Gradle modules, 100 test cycles, 74 coroutine clones, cluster kotlin-language-server 0 errors) | 296K lines (2,423 files) | 1.97 ms RTT | [Ktor Under the Microscope →](https://prod.codes/blog/ktor-under-the-microscope-67-ast-tools/) |
| [`Kotlin/kotlinx.coroutines`](https://github.com/Kotlin/kotlinx.coroutines) | Kotlin (32-core node) | 67 / 67 tools (113K lines, 25 modules clean DAG, 32 TestBase clones, 72 AST matches, cluster kotlin-language-server 0 errors) | 113K lines (1,039 files) | 1.97 ms RTT | [Kotlinx.coroutines Under the Microscope →](https://prod.codes/blog/kotlinx-coroutines-under-the-microscope-67-ast-tools/) |
| [`detekt/detekt`](https://github.com/detekt/detekt) | Kotlin (32-core node) | 67 / 67 tools (132K lines, 41 modules, 4 test cycles, 288 lint clones, 58 AST matches, cluster kotlin-language-server 0 errors) | 132K lines (1,105 files) | 1.97 ms RTT | [Detekt Under the Microscope →](https://prod.codes/blog/detekt-under-the-microscope-67-ast-tools/) |
| [`arrow-kt/arrow`](https://github.com/arrow-kt/arrow) | Kotlin (32-core node) | 67 / 67 tools (73K lines, 38 modules, 5 core cycles, 27 Either clones, Raise DSL search, cluster kotlin-language-server 0 errors) | 72.9K lines (757 files) | 1.97 ms RTT | [Arrow Under the Microscope →](https://prod.codes/blog/arrow-under-the-microscope-67-ast-tools/) |
| [`dotnet/aspnetcore`](https://github.com/dotnet/aspnetcore) | C# (32-core node) | 67 / 67 tools (1.76M lines, 625 projects, 7,427 deps, 20 clone groups, 4K+ guard modernizations, cluster OmniSharp 0 errors) | 1.76M lines (10,685 files) | 1.55 ms RTT | [ASP.NET Core Under the Microscope →](https://prod.codes/blog/aspnetcore-under-the-microscope-67-ast-tools/) |
| [`dotnet/efcore`](https://github.com/dotnet/efcore) | C# (32-core node) | 67 / 67 tools (1.75M lines, 58 projects clean DAG, 20 clone groups, 1.4K guard search, cluster OmniSharp 0 errors) | 1.75M lines (5,778 files) | 0.55 ms RTT | [EF Core Under the Microscope →](https://prod.codes/blog/efcore-under-the-microscope-67-ast-tools/) |
| [`dotnet/roslyn`](https://github.com/dotnet/roslyn) | C# (32-core node) | 67 / 67 tools (6.5M lines, 394 projects, 2,103 deps, 1.8% clones, 15.3K invariants, cluster OmniSharp 0 errors) | 6.50M lines (18,176 files) | 0.55 ms RTT | [Roslyn Under the Microscope →](https://prod.codes/blog/roslyn-under-the-microscope-67-ast-tools/) |
| [`dotnet/BenchmarkDotNet`](https://github.com/dotnet/BenchmarkDotNet) | C# (32-core node) | 67 / 67 tools (121K lines, 29 projects clean DAG, 20 clone groups, 1.1K benchmarks, cluster OmniSharp 0 errors) | 121K lines (1,175 files) | 0.58 ms RTT | [BenchmarkDotNet Under the Microscope →](https://prod.codes/blog/benchmarkdotnet-under-the-microscope-67-ast-tools/) |
| [`apache/spark`](https://github.com/apache/spark) | Scala (32-core node) | 67 / 67 tools (2.05M lines, 52 modules clean DAG, 0.7% clones, 40K assertions, cluster Metals 0 errors) | 2.05M lines (6,448 files) | 0.95 ms RTT | [Spark Under the Microscope →](https://prod.codes/blog/spark-under-the-microscope-67-ast-tools/) |
| [`apache/incubator-pekko`](https://github.com/apache/incubator-pekko) | Scala (32-core node) | 67 / 67 tools (838K lines, 41 modules clean DAG, 3.5% clones, 435 preconditions, cluster Metals 0 errors) | 838K lines (3,384 files) | 0.95 ms RTT | [Pekko Under the Microscope →](https://prod.codes/blog/pekko-under-the-microscope-67-ast-tools/) |
| [`playframework/playframework`](https://github.com/playframework/playframework) | Scala (32-core node) | 67 / 67 tools (222K lines, 44 modules clean DAG, 0.9% clones, 133 preconditions, cluster Metals 0 errors) | 222K lines (1,636 files) | 0.95 ms RTT | [Play Under the Microscope →](https://prod.codes/blog/playframework-under-the-microscope-67-ast-tools/) |
| [`scalameta/scalameta`](https://github.com/scalameta/scalameta) | Scala (32-core node) | 67 / 67 tools (106K lines, 24 modules clean DAG, 2.7% clones, 202 @ast nodes, cluster Metals 0 errors) | 106K lines (722 files) | 0.95 ms RTT | [Scalameta Under the Microscope →](https://prod.codes/blog/scalameta-under-the-microscope-67-ast-tools/) |
| [`ziglang/zig`](https://github.com/ziglang/zig) | Zig (32-core node) | 67 / 67 tools (1.36M lines, 2,052 codegen clones, 480 compiler errdefers, 4.2K asserts, cluster ZLS 0 errors) | 1.36M lines (2,950 files) | 0.44 ms RTT | [Zig Under the Microscope →](https://prod.codes/blog/zig-under-the-microscope-67-ast-tools/) |
| [`ghostty-org/ghostty`](https://github.com/ghostty-org/ghostty) | Zig (32-core node) | 67 / 67 tools (363K lines, 107 VT clones, 8.8K defers, 766 asserts, cluster ZLS 0 errors) | 363K lines (807 files) | 0.44 ms RTT | [Ghostty Under the Microscope →](https://prod.codes/blog/ghostty-under-the-microscope-67-ast-tools/) |
| [`tigerbeetle/tigerbeetle`](https://github.com/tigerbeetle/tigerbeetle) | Zig (32-core node) | 67 / 67 tools (157K lines, 22 VSR frames, 8,251 asserts, 0 runtime allocs, cluster ZLS 0 errors) | 157K lines (244 files) | 0.25 ms RTT | [TigerBeetle Under the Microscope →](https://prod.codes/blog/tigerbeetle-under-the-microscope-67-ast-tools/) |
| [`zigtools/zls`](https://github.com/zigtools/zls) | Zig (32-core node) | 67 / 67 tool calls attempted; Zig dependency graph unsupported (56K lines, 5 completion clones, 86 errdefers, in-memory ZLS pre-flight) | 56K lines (90 files) | 0.46 ms RTT | [ZLS Under the Microscope →](https://prod.codes/blog/zls-under-the-microscope-67-ast-tools/) |
| [`nim-lang/Nim`](https://github.com/nim-lang/Nim) | Nim (32-core node) | 67 / 67 tool calls attempted; Nim dependency graph unsupported (461K lines, allocator test clones, 1.8K asserts, 94% AST slice reduction) | 461K lines (3,923 files) | 0.68 ms RTT | [Nim Under the Microscope →](https://prod.codes/blog/nim-under-the-microscope-67-ast-tools/) |
| [`status-im/nimbus-eth2`](https://github.com/status-im/nimbus-eth2) | Nim (32-core node) | 67 / 67 tool calls attempted; Nim dependency graph unsupported (180K lines, hard-fork clones, 585 Result types, zero-exception safety) | 180K lines (379 files) | 0.63 ms RTT | [Nimbus Under the Microscope →](https://prod.codes/blog/nimbus-under-the-microscope-67-ast-tools/) |
| [`nim-lang/nimble`](https://github.com/nim-lang/nimble) | Nim (32-core node) | 67 / 67 tool calls attempted; Nim dependency graph unsupported; 6-line clone threshold is nonstandard (39K lines, SAT solver & PubGrub, 136 test fixtures, compiler AST parser) | 38.9K lines (135 files) | 0.58 ms RTT | [Nimble Under the Microscope →](https://prod.codes/blog/nimble-under-the-microscope-67-ast-tools/) |
| [`dlang/dmd`](https://github.com/dlang/dmd) | D (32-core node) | 67 / 67 tool calls attempted; D dependency graph and extract-function unsupported (976K lines, ImportC C11 AST parser, 12K invariants, CTFE interpreter) | 976K lines (5,579 files) | 0.58 ms RTT | [DMD Under the Microscope →](https://prod.codes/blog/dmd-under-the-microscope-67-ast-tools/) |
| [`dlang/phobos`](https://github.com/dlang/phobos) | D (32-core node) | 67 / 67 tool calls attempted; D dependency graph and extract-function unsupported (366K lines, range pipeline architecture, 33K asserts, 5.6K unittests) | 366K lines (186 files) | 0.58 ms RTT | [Phobos Under the Microscope →](https://prod.codes/blog/phobos-under-the-microscope-67-ast-tools/) |
| [`vibe-d/vibe.d`](https://github.com/vibe-d/vibe.d) | D (32-core node) | 67 / 67 tool calls attempted; D dependency graph and extract-function unsupported (30K lines, compile-time REST interface generator, 1.4K asserts) | 30.2K lines (108 files) | 0.58 ms RTT | [Vibe.d Under the Microscope →](https://prod.codes/blog/vibed-under-the-microscope-67-ast-tools/) |
| [`laravel/framework`](https://github.com/laravel/framework) | PHP (32-core node) | 67 / 67 tool calls attempted; PHP dependency graph unsupported (573K lines, IoC container reflection, 34K test asserts, Eloquent ORM) | 573K lines (3,120 files) | 0.58 ms RTT | [Laravel Under the Microscope →](https://prod.codes/blog/laravel-under-the-microscope-67-ast-tools/) |
| [`symfony/symfony`](https://github.com/symfony/symfony) | PHP (32-core node) | 67 / 67 tool calls attempted; PHP dependency graph unsupported (2.20M lines, compiled DI container passes, 47.9K test asserts) | 2.20M lines (11,999 files) | 0.58 ms RTT | [Symfony Under the Microscope →](https://prod.codes/blog/symfony-under-the-microscope-67-ast-tools/) |
| [`composer/composer`](https://github.com/composer/composer) | PHP (32-core node) | 67 / 67 tool calls attempted; PHP dependency graph unsupported (140K lines, Boolean SAT solver, 2.5K test asserts, 620 exceptions) | 139.7K lines (634 files) | 0.58 ms RTT | [Composer Under the Microscope →](https://prod.codes/blog/composer-under-the-microscope-67-ast-tools/) |
| [`WordPress/WordPress`](https://github.com/WordPress/WordPress) | PHP (32-core node) | 67 / 67 tool calls attempted; PHP dependency graph unsupported (674K lines, WP_Hook event dispatcher, 2.4K hook points, 3.7K error guards) | 673.9K lines (1,899 files) | 0.58 ms RTT | [WordPress Under the Microscope →](https://prod.codes/blog/wordpress-under-the-microscope-67-ast-tools/) |
| [`rails/rails`](https://github.com/rails/rails) | Ruby (32-core node) | 67 / 67 tool calls attempted; Ruby dependency graph and extract-function unsupported (573K lines, Active Record Relation AST, 60K test asserts, 1.4K exceptions) | 573.5K lines (3,478 files) | 0.58 ms RTT | [Rails Under the Microscope →](https://prod.codes/blog/rails-under-the-microscope-67-ast-tools/) |
| [`jekyll/jekyll`](https://github.com/jekyll/jekyll) | Ruby (32-core node) | 67 / 67 tool calls attempted; Ruby dependency graph and extract-function unsupported (23K lines, 6-stage compilation pipeline, 1.3K test asserts) | 22.8K lines (162 files) | 0.58 ms RTT | [Jekyll Under the Microscope →](https://prod.codes/blog/jekyll-under-the-microscope-67-ast-tools/) |
| [`mastodon/mastodon`](https://github.com/mastodon/mastodon) | Ruby (32-core node) | 67 / 67 tool calls attempted; Ruby dependency graph and extract-function unsupported (190K lines, ActivityPub JSON-LD verification, 8.2K RSpec asserts) | 189.7K lines (3,278 files) | 0.58 ms RTT | [Mastodon Under the Microscope →](https://prod.codes/blog/mastodon-under-the-microscope-67-ast-tools/) |
| [`Homebrew/brew`](https://github.com/Homebrew/brew) | Ruby (32-core node) | 67 / 67 tool calls attempted; Ruby dependency graph and extract-function unsupported (371K lines, Sorbet typed strict, 12.4K test asserts, 2.2K exceptions) | 371.1K lines (2,379 files) | 0.58 ms RTT | [Homebrew Under the Microscope →](https://prod.codes/blog/homebrew-under-the-microscope-67-ast-tools/) |
| [`flutter/flutter`](https://github.com/flutter/flutter) | Dart (32-core node) | 67 / 67 tool calls attempted; Dart dependency graph and extract-function unsupported (2.60M lines, 10-phase render pipeline, 112.7K test asserts, 3.8K errors) | 2.60M lines (6,554 files) | 0.30 ms RTT | [Flutter Under the Microscope →](https://prod.codes/blog/flutter-under-the-microscope-67-ast-tools/) |
| [`felangel/bloc`](https://github.com/felangel/bloc) | Dart (32-core node) | 67 / 67 tool calls attempted; Dart dependency graph and extract-function unsupported (46K lines, reactive event transformers, 1.6K test asserts, 72 errors) | 45.7K lines (616 files) | 0.35 ms RTT | [Bloc Under the Microscope →](https://prod.codes/blog/bloc-under-the-microscope-67-ast-tools/) |
| [`cfug/dio`](https://github.com/cfug/dio) | Dart (32-core node) | 67 tool calls attempted; dependency graph and extract-function unsupported (23K lines, FIFO interceptor chain, 845 test asserts, 138 exceptions) | 22.8K lines (156 files) | 0.32 ms RTT | [Dio Under the Microscope →](https://prod.codes/blog/dio-under-the-microscope-67-ast-tools/) |
| [`dart-lang/sdk`](https://github.com/dart-lang/sdk) | Dart (32-core node) | 67 / 67 tool calls attempted; Dart dependency graph and extract-function unsupported (5.74M lines, Kernel AST CFE compiler, 33.0K test asserts, 8.7K invariants) | 5.74M lines (25,732 files) | 0.35 ms RTT | [Dart SDK Under the Microscope →](https://prod.codes/blog/dart-sdk-under-the-microscope-67-ast-tools/) |
| [`neovim/neovim`](https://github.com/neovim/neovim) | Lua (32-core node) | 67 / 67 tool calls attempted; Lua dependency graph and extract-function unsupported (448K lines, built-in LSP client, 18.4K test asserts, 1.3K invariants) | 448.2K lines (854 files) | 0.35 ms RTT | [Neovim Under the Microscope →](https://prod.codes/blog/neovim-under-the-microscope-67-ast-tools/) |
| [`openresty/lua-nginx-module`](https://github.com/openresty/lua-nginx-module) | C/Perl (32-core node) | 67 tool calls attempted; C/Perl dependency graph and extract-function unsupported (224K lines, cosockets & execution contexts, 6.8K test asserts, 827 errors) | 224.7K lines (434 files) | 0.35 ms RTT | [OpenResty NGINX Module Under the Microscope →](https://prod.codes/blog/lua-nginx-module-under-the-microscope-67-ast-tools/) |
| [`luau-lang/luau`](https://github.com/luau-lang/luau) | C++/Luau (32-core node) | 67 tool calls attempted; dependency graph and extract-function unsupported (534K lines, constraint solver & native codegen, 16.3K asserts, 1.8K invariants) | 534.2K lines (1,533 files) | 0.35 ms RTT | [Luau Under the Microscope →](https://prod.codes/blog/luau-under-the-microscope-67-ast-tools/) |
| [`elixir-lang/elixir`](https://github.com/elixir-lang/elixir) | Elixir (32-core node) | 67 / 67 tool calls attempted; Elixir dependency graph and extract-function unsupported (390K lines, quoted-AST traversal, 18.0K test asserts, 633 exceptions) | 390.1K lines (780 files) | 0.35 ms RTT | [Elixir Under the Microscope →](https://prod.codes/blog/elixir-under-the-microscope-67-ast-tools/) |
| [`phoenixframework/phoenix`](https://github.com/phoenixframework/phoenix) | Elixir (32-core node) | 67 / 67 tool calls attempted; Elixir dependency graph and extract-function unsupported (101K lines, generated route matching and socket routing, 3.2K test asserts, 198 exceptions) | 101.0K lines (508 files) | 0.35 ms RTT | [Phoenix Under the Microscope →](https://prod.codes/blog/phoenix-under-the-microscope-67-ast-tools/) |
| [`phoenixframework/phoenix_live_view`](https://github.com/phoenixframework/phoenix_live_view) | Elixir (32-core node) | 67 / 67 tool calls attempted; Elixir dependency graph and extract-function unsupported (142K lines, HEEx template compiler & diff engine, 3.5K test asserts, 215 exceptions) | 142.2K lines (430 files) | 0.35 ms RTT | [Phoenix LiveView Under the Microscope →](https://prod.codes/blog/phoenix-live-view-under-the-microscope-67-ast-tools/) |
| [`dashbitco/broadway`](https://github.com/dashbitco/broadway) | Elixir (32-core node) | 67 / 67 tool calls attempted; Elixir dependency graph and extract-function unsupported (10.3K lines, GenStage backpressure & dynamic batching, 435 test asserts, 18 exceptions) | 10.3K lines (46 files) | 0.35 ms RTT | [Broadway Under the Microscope →](https://prod.codes/blog/broadway-under-the-microscope-67-ast-tools/) |












## Reading more

The design decisions, with the measurements behind them, are written up as a series:
[**prod.codes/blog/series/prod-code**](https://prod.codes/blog/series/prod-code/) — why the
laptop is the wrong place for the analyzer, why worktrees cannot share a database, how the
mirror stays current, why tools take a symbol instead of a position, validating an edit before
writing it, where the milliseconds went, and running candidate fixes in overlay shadows.

In this repository: [`ROADMAP.md`](ROADMAP.md) for what is built and what is not,
[`CHANGELOG.md`](CHANGELOG.md) for what each release changed, and
[`CONTRIBUTING.md`](CONTRIBUTING.md) for how a change gets made here (an issue and a pull
request with the commands that reproduce it; checks run on the build nodes, there is no
hosted CI).

## Author

[**Alexander Panasenko**](https://prod.codes/about/) ([@alex09x](https://github.com/alex09x)) — [alex@prod.codes](mailto:alex@prod.codes)

## Citation

If you use `prod-code` in academic work or research, please cite the archived release (v0.3.18):

```bibtex
@software{panasenko_2026_prodcode,
  author       = {Panasenko, Alexander},
  title        = {prod-code: Remote code intelligence for AI coding agents},
  month        = sep,
  year         = 2026,
  publisher    = {Zenodo},
  version      = {0.3.18},
  doi          = {10.5281/zenodo.23028285},
  url          = {https://doi.org/10.5281/zenodo.23028285}
}
```

See [CITATION.cff](CITATION.cff) for complete citation metadata.

## License

Dual-licensed under either of:
* Apache License, Version 2.0 ([`LICENSE-APACHE`](LICENSE-APACHE))
* MIT license ([`LICENSE-MIT`](LICENSE-MIT))

at your option.
