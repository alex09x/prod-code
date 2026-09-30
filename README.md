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

## Requirements

Building `prod-code` and running its development checks require Rust 1.95.0 or newer. With
rustup, install the supported minimum and the check components with:

```sh
rustup toolchain install 1.95.0 --profile minimal -c rustfmt -c clippy
```

Run commands with `cargo +1.95.0` to use that toolchain without changing your default.

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
| `code_search` | find code by what it does, ranked declarations with the doc comment that matched |
| `code_symbols` | workspace symbol index by name, fuzzy, analyzer-backed |
| `code_definition` · `code_references` | where a symbol is defined; every use of it |
| `code_callers` · `code_callees` · `code_implementations` · `code_supertypes` | call hierarchy both ways, as a tree to a depth; implementations of a trait or interface, and the traits a type implements or a trait requires |
| `code_outline` | a file's declarations with their kinds and lines, or those of every source file in a directory |
| `code_source` | read std, registry and SDK sources that live only on the node |

**Understand it without reading everything**

| tool | what it does |
|---|---|
| `code_slice` | bounded traversal of declarations referenced by a symbol; depth and byte budgets limit the result |
| `code_hover` · `code_type_at` | signature, type and docs |
| `code_impact` | changed functions, reachable callers and candidate tests; CI falls back to the full suite when the evidence is incomplete |
| `code_dead_code` | reference-based candidates, with failed or malformed queries marked unverified |
| `code_prune_orphans` | eligible dead-code candidates removed with safe delete and analyzer validation; unverified and protected symbols remain |

**Change it safely**

Most custom planners below target Rust. The two parameter tools also support Go, C/C++,
TypeScript/JavaScript, Python and Swift for their documented input shapes. Rename and server
code actions depend on the selected language server. See [ROADMAP-AUDIT.md](ROADMAP-AUDIT.md)
for missing capabilities and [ROADMAP.md](ROADMAP.md) for individual restrictions.

Analyzer diagnostics do not include every compiler or borrow-checker check. Tools with
`verify: "compile"` can additionally run a shadow compiler check; semantic refusals still apply.
A Rust file reported as `unlinked-file` is unchecked and cannot pass edit validation. New
modules must be checked with their module declarations; new Cargo targets need a workspace
reload or an explicit compiler check that includes them. Missing or malformed diagnostic
reports also fail validation. Manifest, lockfile and documentation proposals need
`shadow-run` with the appropriate parser or build command instead of source diagnostics.

| tool | what it does |
|---|---|
| `code_validate_edit` · `code_validate_edits` | analyzer diagnostics for proposed file contents, nothing written; several files judged together, with a warning when an edit removes a symbol another file still uses |
| `code_diagnostics` | diagnostics for a file, in memory, without a build |
| `code_rename` · `code_safe_delete` | semantic rename across the workspace (a field with its accessors, with `accessors`); delete only when nothing references it, or a parameter with its arguments |
| `code_change_signature` | Rust signature changes; Go reorder/removal through gopls and typed literal additions to ordinary functions. Additions preserve old argument order and require remote compiler verification |
| `code_move` | a declaration moved to another module, with the imports it takes and the imports it leaves behind |
| `code_move_method` · `code_move_module` | a method moved to the type of one of its parameters, or an associated function to another type; a whole module moved to another parent, with every path that names it |
| `code_extract_function` | a selection turned into a named function, and its duplicates and near-duplicates in this file and others replaced by calls |
| `code_introduce_parameter_object` | parameters bundled into the language's supported object form, updating supported body uses and calls; JavaScript uses plain objects |
| `code_extract_parameter` | an expression promoted to a parameter and supported callers updated, with structural and semantic refusal cases |
| `code_migrate_type` | a declared type changed, with every site that no longer fits listed before any of it is done; with `convert`, `.into()` written wherever the analyzer accepts it |
| `code_generify` | a parameter's concrete type turned into a bounded type parameter, every file that calls it type-checked against the new signature |
| `code_invert_boolean` | a predicate, a `bool` field or a `bool` variable renamed to its opposite, every read and write unchanged in effect |
| `code_make_static` | a method that never uses `self` turned into an associated function, every call site with it |
| `code_inline_parameter` | a parameter every caller passes the same constant for, moved into the body and out of every call |
| `code_extract_delegate` | fields and the methods that use only them moved into a helper type the struct holds, with every access and literal rewritten |
| `code_extract_trait` | selected Rust inherent methods moved into a trait with caller imports; preserves ordinary lifetime/type/const generics and bounds, refuses conditional or opaque-return shapes and Self-dependent impl bounds |
| `code_loop_to_iterator` | a loop that only sums, counts or pushes into an accumulator turned into an iterator chain, checked by the analyzer |
| `code_introduce_variable` | an expression bound once (`let w1 = w + 1;`) and every occurrence of it in the function replaced, refused when evaluating once would change what the code does |
| `code_convert_to_method` | an associated function turned into a method: its first parameter becomes `self`, `Type::f(&x, a)` becomes `x.f(a)` |
| `code_wrap_return` | a return type wrapped in `Option` or `Result`, `?` at every caller that can propagate, the rest named |
| `code_extract_field` | an expression in a method turned into a field of its type, initialised wherever the type is built |
| `code_encapsulate_field` | a public field made private, every read and write outside its file turned into a getter or setter call |
| `code_schema_rename` | one schema field renamed across every language that spells it differently, semantically per project |
| `code_assists` · `code_assist` | the analyzer's code actions and compiler fix-its, applied to the checkout |
| `code_codemod` | structural search and replace on the syntax tree (`pattern ==>> replacement`), as a diff or applied |
| `code_generate_fixture` | a Rust value with explicit default fallbacks, or `builder: true` for a typed builder preview of a named struct with ordinary lifetime/type/const parameters, defaults and bounds; verified by default, with no files written |
| `code_shadow_run` | run a command once per candidate fix, each in a private shadow of the workspace, and take the winner's diff |

**Run it**

| tool | what it does |
|---|---|
| `code_check` · `code_lint` · `code_test` · `code_benchmarks` | build, lint and test on the node with parsed diagnostics; `path` narrows to one crate, package or directory; `fix: true` applies the compiler's machine-applicable fixes and checks again (Rust) |
| `code_exec` | any command in the workspace copy; formatters, generators and lockfiles are written back |
| `code_diagnose_failure` | run tests and report up to ten failure dossiers with sites, callers, changes and supported printed assertion operands |

**Operate it**

| tool | what it does |
|---|---|
| `code_status` · `code_sync` | gateway health, engines, loaded workspaces and the builds and tests running on it; a manual push (the watcher does this for you) |

Every position tool also takes `symbol` instead of a file and a position, so an agent never
has to grep for a line number:

```json
{ "name": "code_callers", "arguments": { "symbol": "Metrics::record" } }
```

The same surface exists as a CLI for humans and scripts: `prod-code search | slice | codemod |
fixture | change-signature | schema-rename | migrate-type | move | parameter-object |
extract-parameter | extract-field | encapsulate-field | wrap-return | make-static | convert-to-method | inline-parameter | introduce-variable | extract-trait | extract-delegate | prune | loop-to-iterator | invert-boolean | generify | hover | def | refs | callers | callees |
impls | symbols | outline | validate | diagnostics | check | lint | test | exec | impact |
diagnose | rename | assists | assist | safe-delete | dead-code | shadow-run | source | status |
cluster | metrics`. The position commands take `--symbol NAME` instead of a file and a
position, `prod-code symbols <name>` finds a declaration by name, and `prod-code validate FILE
--from NEW --with OTHER=NEW2` checks a multi-file change in one overlay.

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
⚡ prod-code cluster (5 node(s))
192.0.2.11:9400   UP   load 0.10/cpu (32 cpus)   workspaces 0   rss 10 MB
                  engines: rust, go, cpp, python, typescript
192.0.2.20:9400   UP   load 2.96/cpu (24 cpus)   workspaces 0   rss 14 MB
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
