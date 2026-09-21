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
| `cargo clippy --workspace --all-targets` on a 32-core node | 2.5 s |
| `cargo test --workspace` | 46 s |
| first load of a Rust workspace (build scripts, proc macros) | ~45 s, once per worktree |

## Install

```sh
# the client (one binary; the gateway is the same binary's sibling)
cargo install --path crates/prod-code-client        # or take a release binary

# register it once; every repository then has the tools
claude mcp add --scope user prod-code -e PROD_CODE_REMOTE=192.0.2.10:9400 -- prod-code mcp
codex  mcp add prod-code --env PROD_CODE_REMOTE=192.0.2.10:9400 -- prod-code mcp
```

For Antigravity/Gemini, add the same command to `~/.gemini/config/mcp_config.json`. On a node:

```sh
prod-code-server --bind 0.0.0.0:9400 --storage /srv/prod-code/workspaces \
  --advertise <this host>:9400 --peers <other nodes>
```

The MCP server tells the agent how to work at `initialize`, and reloads itself when the
binary is replaced, so a running session picks up new tools without a restart.

## What it gives an agent

Twenty-nine tools, all of them answered by the node that holds the workspace.

**Find code**

| tool | what it does |
|---|---|
| `code_search` | find code by what it does, ranked declarations with the doc comment that matched |
| `code_symbols` | workspace symbol index by name, fuzzy, analyzer-backed |
| `code_definition` · `code_references` | where a symbol is defined; every use of it |
| `code_callers` · `code_callees` · `code_implementations` | call hierarchy both ways; implementations of a trait or interface |
| `code_outline` | a file's declarations with their kinds and lines |
| `code_source` | read std, registry and SDK sources that live only on the node |

**Understand it without reading everything**

| tool | what it does |
|---|---|
| `code_slice` | only the code a symbol depends on, typically 90–96% smaller than its files |
| `code_hover` · `code_type_at` | signature, type and docs |
| `code_impact` | blast radius of a change: the functions it touches, their callers, the tests that cover them |
| `code_dead_code` | unreferenced functions, methods and types |

**Change it safely**

| tool | what it does |
|---|---|
| `code_validate_edit` · `code_validate_edits` | analyzer diagnostics for proposed file contents, nothing written; several files judged together, with a warning when an edit removes a symbol another file still uses |
| `code_diagnostics` | diagnostics for a file, in memory, without a build |
| `code_rename` · `code_safe_delete` | semantic rename across the workspace; delete only when nothing references it |
| `code_assists` · `code_assist` | the analyzer's code actions and compiler fix-its, applied to the checkout |
| `code_shadow_run` | run a command once per candidate fix, each in a private shadow of the workspace, and take the winner's diff |

**Run it**

| tool | what it does |
|---|---|
| `code_check` · `code_lint` · `code_test` | build, lint and test on the node with parsed diagnostics; `path` narrows to one crate, package or directory |
| `code_exec` | any command in the workspace copy; formatters, generators and lockfiles are written back |
| `code_diagnose_failure` | run the tests and, for each failure, the failing site, its callers and what changed |

**Operate it**

| tool | what it does |
|---|---|
| `code_status` · `code_sync` | gateway health, engines, loaded workspaces; a manual push (the watcher does this for you) |

Every position tool also takes `symbol` instead of a file and a position, so an agent never
has to grep for a line number:

```json
{ "name": "code_callers", "arguments": { "symbol": "Metrics::record" } }
```

The same surface exists as a CLI for humans and scripts: `prod-code search | slice | hover |
def | refs | callers | callees | impls | symbols | validate | diagnostics | check | lint | test |
exec | impact | diagnose | rename | assists | assist | safe-delete | dead-code | shadow-run |
source | status | cluster | metrics`.

## Three things worth seeing

**Ask a question in words.** The gateway indexes every declaration with the doc comment above
it and ranks them against your question. Lexical, not embeddings: a question sharing no words
with the code or its comments finds nothing.

```
$ prod-code search "how do we decide which node runs a workspace"
10 hit(s) in 16 ms (1010 declarations, 40 files)

 1. [function] pick_node  crates/prod-code-mcp/src/cluster.rs:107
    Chooses the gateway for `workspace_name` among `nodes`: the remembered placement when it
    is still one of the nodes, alive and able to serve `engine`, otherwise the quietest node
```

**Read a symbol without reading its files.** `code_slice` follows the analyzer's own edges
from a declaration and returns what it depends on.

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

**Builds and tests run on the node**, in the workspace's own `target/`, which stays warm
between runs. Files a command changes are written back into your checkout.

**Several nodes.** Gateways gossip every 5 s; a client needs one seed address. A checkout is
placed on the node that already holds it, otherwise on the quietest one that serves its
language. `--engines rust,go` restricts a node to what it should serve, so a macOS node can be
Swift-only and placement never sends Rust work to a workstation.

```
$ prod-code cluster
⚡ prod-code cluster (5 node(s))
192.0.2.11:9400   UP   load 0.10/cpu (32 cpus)   workspaces 0   rss 10 MB
                  engines: rust, go, cpp, python, typescript
192.0.2.20:9400   UP   load 2.96/cpu (24 cpus)   workspaces 0   rss 14 MB
                  engines: swift
```

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

`code_search` and `code_slice` work on all six. Tooling is detected from the checkout (lock
files, `package.json`, `pyproject`, `CMakeLists`…). Dependencies live on the node: run
`prod-code exec -- bun install`, `-- uv sync` or `-- npm ci` once per checkout and the
language servers and test runners use them.

## Per-repository options (`prod-code.toml`)

Put a `prod-code.toml` at the checkout root to tune how the gateway analyses it. It is synced
like any manifest and read when the workspace is loaded:

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

## Operating it

**Usage metrics.** Every query, command and sync round on a node is one event (which agent,
from which host, against which workspace and method, how long, whether it worked), appended
to `<storage>/../metrics/events-YYYY-MM-DD.jsonl`. `prod-code metrics [--since SECS]`
aggregates them across the cluster with p50 and p95.

**macOS nodes.** `scripts/deploy-mac-node.sh <host> <advertise> <peers>` builds or installs
the gateway, signs it with the identity in `PROD_CODE_SIGN_IDENTITY` under a stable bundle
identifier (so macOS remembers the Local Network grant instead of prompting after every
redeploy), writes the launchd agent with `--engines` (`PROD_CODE_ENGINES`, default `swift`)
and reloads it.

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

**Alex** ([@alex09x](https://github.com/alex09x)) — [alex@prod.codes](mailto:alex@prod.codes)

## License

Dual-licensed under either of:
* Apache License, Version 2.0 ([`LICENSE-APACHE`](LICENSE-APACHE))
* MIT license ([`LICENSE-MIT`](LICENSE-MIT))

at your option.
