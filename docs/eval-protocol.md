# prod-code Flagship Repository Evaluation Protocol

A standardized, repeatable evaluation matrix for benchmarking `prod-code` across open-source codebases and generating deep technical field reports.

Every evaluation must test all **67 MCP tools** and their corresponding CLI workflows against an idle Linux cluster node (`booster` / AMD EPYC 32 cores, 134 GB RAM).

---

## 1. Node Topology & Workspace Synchronization (3 Tools)

Verify remote Salsa graph initialization and synchronization without local CPU/RAM overhead:

| # | MCP Tool | CLI Command | Target & Metric |
|---|---|---|---|
| 1 | `code_status` | `prod-code status` | Node health, warm language server daemon, RAM consumption, active worker threads. |
| 2 | `code_sync` | `prod-code -r <node> sync` | Differential hash transfer time, remote Salsa database hydration latency, client RTT. |
| 3 | `code_report_issue` | `prod-code report-issue` | Verify diagnostic issue reporter: sanitizes private IPs/hostnames, links KYB incidents. |

---

## 2. Architecture & Dependency Graph Analysis (2 Tools)

Extract project topology, package coupling, and code duplication:

| # | MCP Tool | CLI Command | Target & Metric |
|---|---|---|---|
| 4 | `code_dependencies` | `prod-code deps [--path ...]` | Crate DAG & module-level import graph. Cycle detection and Robert C. Martin instability metrics ($C_a, C_e, I$). |
| 5 | `code_find_duplicates` | `prod-code duplicates [--min-lines N]` | AST-hash based clone harvester (Type-1 exact, Type-2 parameterized, Type-3 structural) with consolidation suggestions. |

---

## 3. Structural AST Search & Program Slicing (3 Tools)

Query code by syntax structure, semantic meaning, and data-flow dependencies:

| # | MCP Tool | CLI Command | Target & Metric |
|---|---|---|---|
| 6 | `code_structural_search` | `prod-code struct-search '<pattern>'` | Structural AST pattern matching with meta-variables (`$x.method()`, `$builder.build()`). |
| 7 | `code_search` | `prod-code search '<query>'` | 3-way RRF semantic search combining BM25 keywords, AST symbols, and dense embeddings. |
| 8 | `code_slice` | `prod-code slice --file <F> --line <L>` | Forward & backward program slicing: extract minimal data-flow and control-flow dependency chains. |

---

## 4. Semantic Navigation & Type Hierarchy (11 Tools)

Test core language server graph navigation across crate and module boundaries:

| # | MCP Tool | CLI Command | Target & Metric |
|---|---|---|---|
| 9 | `code_definition` | `prod-code def --symbol <S>` | Cross-crate go-to-definition (types, functions, macros). |
| 10 | `code_references` | `prod-code refs --symbol <S>` | Find all symbol references (read, write, call sites) across the workspace. |
| 11 | `code_callers` | `prod-code callers --depth N <S>` | Recursive incoming call hierarchy with call-site locations and cycle detection. |
| 12 | `code_callees` | `prod-code callees <S>` | Outgoing call graph for a target function/method. |
| 13 | `code_implementations`| `prod-code impls --symbol <S>` | Discover all concrete implementations of a trait or interface. |
| 14 | `code_supertypes` | `prod-code supertypes --symbol <S>`| Trait/interface inheritance and supertype hierarchy traversal. |
| 15 | `code_hover` | `prod-code hover --file <F> --line <L>`| Rich markdown docstrings, signature types, trait bounds. |
| 16 | `code_type_at` | `prod-code type-at --file <F> --line <L>`| Precise inferred type resolution at cursor position. |
| 17 | `code_outline` | `prod-code outline --file <F>` | Hierarchical AST symbol tree outline of a file or module. |
| 18 | `code_symbols` | `prod-code symbols <query>` | Workspace-wide fuzzy symbol search by name. |
| 19 | `code_source` | `prod-code source --crate <C> <S>` | Decompile / retrieve external dependency and stdlib source definitions. |

---

## 5. Diagnostics, Failure Analysis & Dead Code (5 Tools)

Measure remote diagnostic engine performance and code health:

| # | MCP Tool | CLI Command | Target & Metric |
|---|---|---|---|
| 20 | `code_diagnostics` | `prod-code diagnostics [--path ...]`| Real-time LSP / compiler diagnostic stream across all workspace crates. |
| 21 | `code_diagnose_failure` | `prod-code diagnose-failure` | Deep root-cause explanation for compiler errors or failing test runs. |
| 22 | `code_dead_code` | `prod-code dead-code` | Graph reachability analysis: identify unused functions, dead structs, or unread constants. |
| 23 | `code_prune_orphans` | `prod-code prune-orphans` | Batch detection and removal of unused `use` imports and orphan definitions. |
| 24 | `code_lint` | `prod-code lint [--path ...]` | Remote linter execution (clippy, etc.) with structured violation codes and suggestions. |

---

## 6. Assists & Quick-Fix Actions (2 Tools)

Query and apply language server intention actions:

| # | MCP Tool | CLI Command | Target & Metric |
|---|---|---|---|
| 25 | `code_assists` | `prod-code assists --file <F> --line <L>` | Discover available LSP code assists / quick-fixes at position. |
| 26 | `code_assist` | `prod-code assist --file <F> --id <ID>` | Apply selected code assist and inspect generated diff. |

---

## 7. Automated Refactoring Suite (26 Tools)

Verify automated semantic transformations with whole-workspace call-site updates:

| # | MCP Tool | Refactoring Scope | Verification Check |
|---|---|---|---|
| 27 | `code_rename` | Symbol rename | Renames symbol and updates all cross-crate usages without broken references. |
| 28 | `code_safe_delete` | Safe item deletion | Asserts zero usages exist before deletion; aborts if references remain. |
| 29 | `code_schema_rename` | Schema/serde rename | Synchronizes struct field rename with serialization attributes (`#[serde(rename = ...)]`). |
| 30 | `code_extract_function` | Extract function/method | Extracts block into new function with deduced parameter bindings and return type. |
| 31 | `code_extract_parameter` | Extract parameter | Promotes a local expression to a function parameter, updating all caller invocations. |
| 32 | `code_extract_field` | Extract struct field | Moves an inline expression or constant into a field of the target struct. |
| 33 | `code_extract_trait` | Extract trait/interface | Synthesizes a new trait from existing struct methods and implements it. |
| 34 | `code_extract_delegate` | Extract delegation | Generates delegating wrapper methods forwarding calls to an inner struct field. |
| 35 | `code_extract_interface` | Extract interface contract | Extracts public API surface of a struct into a reusable trait contract. |
| 36 | `code_introduce_variable` | Introduce variable | Extracts an expression into a local `let` binding. |
| 37 | `code_introduce_parameter_object` | Parameter object bundling | Bundles long argument lists into a dedicated struct across definition and call sites. |
| 38 | `code_inline_parameter` | Inline parameter | Removes a constant/redundant parameter and inlines its value into callers. |
| 39 | `code_encapsulate_field` | Encapsulate field | Makes a public field private and generates accessor (getter/setter) methods. |
| 40 | `code_migrate_type` | Migrate type | Updates a type signature (e.g. `String` to `&str` or `Cow`) and adapts all call sites. |
| 41 | `code_generify` | Parameterize generic | Adds a generic parameter `<T>` with trait bounds to a concrete struct or function. |
| 42 | `code_invert_boolean` | Invert boolean logic | Inverts boolean condition, flips `true`/`false`, and negates all call-site assertions. |
| 43 | `code_make_static` | Make static/associated | Converts a method using `&self` to an associated function without self. |
| 44 | `code_convert_to_method` | Convert to method | Converts a free-standing function `fn foo(bar: &Bar)` into `Bar::foo(&self)`. |
| 45 | `code_loop_to_iterator` | Imperative to iterator | Converts an imperative `for` loop with accumulators into an iterator pipeline (`map`, `filter`). |
| 46 | `code_replace_constructor_with_factory` | Factory method | Converts raw `Struct { ... }` instantiation into a named factory constructor. |
| 47 | `code_replace_constructor_with_builder` | Builder generator | Automatically generates a fluent builder pattern for a complex struct. |
| 48 | `code_pull_up` | Pull up | Moves methods/associated items up into a supertrait. |
| 49 | `code_push_down` | Push down | Specializes trait methods down to specific concrete implementations. |
| 50 | `code_replace_inheritance_with_delegation` | Delegation replacement | Replaces deep struct nesting or pseudo-inheritance with composition and delegation. |
| 51 | `code_replace_conditional_with_polymorphism` | Polymorphic dispatch | Converts branching `match enum` logic into dynamic trait dispatch. |
| 52 | `code_wrap_return` | Wrap return type | Wraps return expression in `Result<T, E>` or `Option<T>` and adapts all callers. |
| 53 | `code_move` | Relocate item | Relocates a type or function to another module with updated imports. |
| 54 | `code_move_module` | Relocate module | Relocates an entire module file/tree and updates all workspace `use` paths. |
| 55 | `code_move_method` | Relocate method | Moves a method from one `impl` block to another. |
| 56 | `code_change_signature` | Change signature | Reorders, adds, or removes parameters and updates all workspace call sites. |

---

## 8. Synthesis, Codemods & In-Memory Pre-validation (6 Tools)

Verify speculative synthesis and atomic in-memory validation:

| # | MCP Tool | CLI Command | Target & Metric |
|---|---|---|---|
| 57 | `code_propose_expression` | `prod-code propose-expr <F> <L> <T>` | Synthesizes expression matching expected type in AST context with confidence score. |
| 58 | `code_codemod` | `prod-code codemod --recipe <R>` | Executes workspace-wide automated AST transform recipe. |
| 59 | `code_generate_fixture` | `prod-code fixture --type <T>` | Auto-generates type-safe test fixtures or builder mocks from struct/enum definitions. |
| 60 | `code_shadow_run` | `prod-code shadow-run '<cmd>'` | Speculatively runs command in an ephemeral copy-on-write fork on the cluster. |
| 61 | `code_validate_edit` | `cat <F> \| prod-code validate` | Pre-flight compile/typecheck of a buffer in server RAM without saving to disk. |
| 62 | `code_validate_edits` | `prod-code validate-edits` | Multi-file atomic in-memory pre-flight typecheck. |

---

## 9. Remote Execution, Impact Analysis & Verification (5 Tools)

Measure cluster offloading performance vs local execution:

| # | MCP Tool | CLI Command | Target & Metric |
|---|---|---|---|
| 63 | `code_impact` | `prod-code impact --changes <diff>` | Predicts exact test set affected by given symbol or file edits. |
| 64 | `code_check` | `prod-code check [--path ...]` | Remote compilation check across 32 cluster cores. |
| 65 | `code_test` | `prod-code test [--path ...] [filter]` | Remote test execution with parallel harness and live result streaming. |
| 66 | `code_benchmarks` | `prod-code bench [--path ...]` | Remote benchmark execution on dedicated hardware without local throttling. |
| 67 | `code_exec` | `prod-code exec '<cmd>'` | Arbitrary isolated command execution in remote container/environment. |

---

## Standardized Step-by-Step Project Evaluation Algorithm

For every target repository, execute this exact checkpoint lifecycle:

### Step 0: Clean Workspace Setup & Verification
1. Ensure the local checkout is clean: `git status --short` must return 0 uncommitted changes.
2. Verify node health: `prod-code -r <node> status` (record memory RSS, uptime, ping RTT).
3. Clean remote workspace on target node to ensure 100% cold baseline:
   `ssh alex09x@<node> "rm -rf /home/alex09x/prod-code-storage/workspaces/<workspace_name>"`

### Step 1: Ingestion & Cold Sync Benchmark
1. Run cold sync: `prod-code -r <node> sync`.
   - Record: `Files Planned`, `Manifest Probe`, `Files Updated`, `Data Transferred (KB/MB)`, `Fast-Sync Latency (ms)`.
2. Run immediate incremental warm sync: `prod-code -r <node> sync`.
   - Record: 0 files updated, 0.0 KB transferred, incremental latency (ms).
3. Test issue reporter sanitization in dry-run mode:
   `prod-code -r <node> report-issue --title "..." --body "..." --dry-run`
   - Verify zero leaks of private IPs, hostnames, or home directories.

### Step 2: Architecture DAG & Coupling Analysis
1. Analyze crate/package level graph: `prod-code -r <node> dependencies --scope crates`.
   - Calculate Robert C. Martin metrics: Afferent Coupling ($C_a$), Efferent Coupling ($C_e$), Instability ($I = C_e / (C_a + C_e)$).
   - Verify DAG property (assert zero circular crate dependencies).
2. Analyze module level for hidden cycles: `prod-code -r <node> dependencies --scope modules --path <dir>`.
   - Record any circular dependencies identified by Tarjan's SCC DFS.
3. Detect code duplications: `prod-code -r <node> duplicates --min-lines 10`.
   - Log Type-1 (exact) and Type-2 (parameterized) clone clusters and duplicate byte counts.

### Step 3: Structural AST Search & Slicing
1. Polyglot structural search with metavariables:
   `prod-code -r <node> structural-search '<pattern>' --path <file>`.
2. Semantic intent search:
   `prod-code -r <node> search '<natural language query>'`.
3. Backward/Forward program slicing:
   `prod-code -r <node> slice --symbol <symbol> <file>`.
   - Calculate context reduction percentage: `1.0 - (sliced_bytes / full_file_bytes)`.

### Step 4: Semantic LSP Navigation
Step through all 10 core navigation primitives across crate boundaries:
1. `prod-code -r <node> def --symbol <symbol>`
2. `prod-code -r <node> refs --symbol <symbol>`
3. `prod-code -r <node> callers <symbol>`
4. `prod-code -r <node> callees <symbol>`
5. `prod-code -r <node> impls --symbol <symbol>`
6. `prod-code -r <node> supertypes --symbol <symbol>`
7. `prod-code -r <node> hover <file> <line> <col>`
8. `prod-code -r <node> type-at <file> <line> <col>`
9. `prod-code -r <node> outline <file>`
10. `prod-code -r <node> symbols <query>`
11. `prod-code -r <node> source <path>` (std/registry code resolution)

### Step 5: Diagnostics, Failure Explanation & Dead Code
1. Query compiler diagnostics stream: `prod-code -r <node> diagnostics`.
2. Scan workspace-wide dead code: `prod-code -r <node> dead-code`.
3. Test batch orphan pruning: `prod-code -r <node> prune-orphans` (preview dry-run).
4. Run remote linter: `prod-code -r <node> lint [--path <crate>]`.

### Step 6: Code Assists (Intention Actions)
1. Discover assists at position: `prod-code -r <node> assists <file> <line> <col>`.
2. Apply intention action: `prod-code -r <node> assist <file> <line> <col> <action_id>`.
3. Verify remote compilation & tests:
   `prod-code -r <node> check --path <crate>`
   `prod-code -r <node> test --path <crate>`
4. Revert: `git checkout -- .`.

### Step 7: AST Refactorings (The 5-Step Integrity Loop)
For every refactoring tool tested:
1. **Execute with `--apply`**: Apply the semantic transformation to the local checkout.
2. **Inspect Diff**: Run `git diff` and record the exact patch (call sites, bodies, imports).
3. **Compile-Check**: Run `prod-code -r <node> check --path <crate>` on cluster cores.
4. **Test Run**: Run `prod-code -r <node> test --path <crate> [filter]` and assert 100% green.
5. **Revert Cleanly**: Run `git checkout -- .` and verify `git status --short` is empty before next test.
6. **Guard Verification**: For guard tools (`safe-delete`, `make-static`), assert that improper edits are blocked with clear semantic explanations.

### Step 8: Synthesis & In-Memory Pre-validation
1. Test fixture generation: `prod-code -r <node> fixture --type <symbol>`.
2. Propose in-scope expression: `prod-code -r <node> propose-expression <file> <line> <col> <type>`.
3. Test in-memory edit validation: pipe modified file to `prod-code -r <node> validate` over stdin without saving to disk. Verify compiler catches syntax/type errors in RAM.
4. Run multi-hypothesis shadow run: `prod-code -r <node> shadow-run <spec.json> -- <cmd>`.

### Step 9: Remote Execution, Blast Radius & CI
1. Impact analysis: calculate affected test set from changed files (`prod-code impact`).
2. Remote benchmark execution: `prod-code -r <node> benchmarks --path <crate>`.
3. Remote test suite execution: `prod-code -r <node> test --path <crate>`.
4. Remote command execution: `prod-code -r <node> exec -- <cmd>`.

### Bug Protocol
- If any tool crashes, hangs, or returns an unhandled error:
  1. Capture reproduction command and stack trace.
  2. File an issue using `prod-code report-issue --title "..." --body "..." --label bug`.
  3. Fix the bug in the `prod-code` codebase.
  4. Write an automated unit test.
  5. Verify the fix passes tests remotely and locally.
  6. Document the bug and resolution in the evaluation report.

### Publishing Protocol
- Synthesize all collected metrics into an article in `prod.codes/src/content/blog/`.
- Must include real terminal logs, authentic cold vs warm sync timings, real AST diffs, and exact cluster resource metrics.
- Author: `Alexander Panasenko <alex@prod.codes>`.
- Strict Prohibition: NO AI trailers, NO star counts in titles/headings.
- Keep in draft/uncommitted state until explicit user review.

---

## Completed Field Evaluations

1. **[BurntSushi/ripgrep](https://github.com/BurntSushi/ripgrep)** (Rust)
   - **Evaluated on**: `booster` (32-core Linux node, `192.168.2.168:9400`)
   - **Tool Coverage**: 67 / 67 tools across all 9 suites
   - **Key Metrics**: 45.4 KB cold sync, 1.02 ms LAN ping, 0% local laptop CPU
   - **Issues Identified & Resolved**:
     - [#735](https://github.com/alex09x/prod-code/issues/735): `slice: handle inverted/empty symbol ranges from language servers gracefully`
     - [#733](https://github.com/alex09x/prod-code/issues/733): `code_validate_edit fails on package.json with no package metadata`
    - **Full Deep-Dive Report**: [Can 67 AST Tools Break Ripgrep? Stress-Testing Rust's Fastest Grep on an Idle Cluster Node (prod.codes)](https://prod.codes/blog/can-67-ast-tools-break-ripgrep/)

2. **[quickwit-oss/tantivy](https://github.com/quickwit-oss/tantivy)** (Rust)
   - **Evaluated on**: `booster` (32-core Linux node, `192.168.2.168:9400`)
   - **Tool Coverage**: 67 / 67 tools across all 9 suites
   - **Key Metrics**: 6.77 MB cold sync in 423 ms, 0.96 ms LAN ping, 0% local laptop CPU
   - **Architectural Findings**: 10-crate clean DAG, 7,156 cyclic module paths in `src/`, Type-2 clone clusters in JIT and metrics
   - **Semantic Guards & Refactorings**: Multi-file rename across 18 files in 5.75s, clone-aware function extraction, active safe-delete refusal (67 refs)
   - **Full Deep-Dive Report**: [7,156 Cyclic Module Paths: Stress-Testing Tantivy's Search Engine with 67 AST Tools (prod.codes)](https://prod.codes/blog/7156-cyclic-module-paths-in-tantivy/)

3. **[tokio-rs/tokio](https://github.com/tokio-rs/tokio)** (Rust)
   - **Evaluated on**: `booster` (32-core Linux node, `192.168.2.168:9400`)
   - **Tool Coverage**: 67 / 67 tools across all 9 suites
   - **Key Metrics**: 6.11 MB cold sync in 450 ms, 57 ms warm sync, 633 µs LAN ping, 0% local laptop CPU
   - **Architectural Findings**: 10-crate circular DAG (6 cyclic paths across members), Type-2 clone clusters in stream combinators
   - **Semantic Guards & Refactorings**: AST slicing of work-stealing queue (96% reduction), parameter side-effect order guard refusal on Deref, 20-callsite boolean inversion, safe-delete refusal (126 usages)
   - **Issues Identified & Resolved**:
     - [#736](https://github.com/alex09x/prod-code/issues/736): `supertypes: out-of-bounds line index when symbol resolves to remote sysroot path (Box<T>)`
   - **Full Deep-Dive Report**: [Six Circular Dependencies in Tokio: What 67 AST Analyzers Found Inside Rust's Async Engine (prod.codes)](https://prod.codes/blog/six-circular-dependencies-in-tokio/)

4. **[pola-rs/polars](https://github.com/pola-rs/polars)** (Rust)
   - **Evaluated on**: `booster` (32-core Linux node, `192.168.2.168:9400`)
   - **Tool Coverage**: 67 / 67 tools across all 9 suites
   - **Key Metrics**: 29.65 MB cold sync in 2.0s, 145 ms warm sync, 563 µs LAN ping, 0% local laptop CPU
   - **Architectural Findings**: 33-crate DAG (4 circular paths across members), Type-2 clone clusters across `SeriesTrait` implementations in 9 data types
   - **Semantic Guards & Refactorings**: AST slicing of `DelayRechunk::optimize_plan` (96% reduction), cross-crate boolean inversion of `is_empty` to `is_non_empty` (0 errors), safe-delete refusal (1,528 usages), RAM pre-flight validation in 360 ms
   - **Issues Identified & Resolved**:
     - [#737](https://github.com/alex09x/prod-code/issues/737): `codemod: non-char-boundary panic in tokenize_source on multi-byte UTF-8 tokens`
   - **Full Deep-Dive Report**: [Half a Million Lines of Arrow: What 67 AST Analyzers Found Inside Polars' Query Engine (prod.codes)](https://prod.codes/blog/half-a-million-lines-of-arrow-what-67-ast-analyzers-found-inside-polars/)

5. **[astral-sh/uv](https://github.com/astral-sh/uv)** (Rust)
   - **Evaluated on**: `ram9` (32-core Linux node, `192.168.2.143:9400`) & `booster`
   - **Tool Coverage**: 67 / 67 tools across all 9 suites
   - **Key Metrics**: 36.45 MB cold sync in 1.85s, 234 ms warm sync, 500 µs LAN ping, 0% local laptop CPU
   - **Architectural Findings**: 74-crate DAG (623 dependencies), self-referencing cycle in `uv-preview`, 347 Type-1/Type-2 code clones in test fixtures
   - **Semantic Guards & Refactorings**: AST slicing of `Requirement` across 11 crates (559K lines to 100 lines), cross-crate boolean inversion of `GitLfs::enabled` across 10 crates (0 errors), safe-delete refusal (461 usages), RAM pre-flight validation in 430 ms
   - **Issues Identified & Resolved**:
     - [#738](https://github.com/alex09x/prod-code/issues/738): `fast-sync drops tracked JSON build inputs larger than 256 KiB breaking builds (uv-python)`
6. **[bevyengine/bevy](https://github.com/bevyengine/bevy)** (Rust)
   - **Evaluated on**: `booster` (32-core Linux node, `192.168.2.168:9400`)
   - **Tool Coverage**: 67 / 67 tools across all 9 suites
   - **Key Metrics**: 81.0 MB cold sync in 18.2s, 0.0 ms warm sync, 613 µs LAN ping, 0% local laptop CPU
   - **Architectural Findings**: 97-crate DAG (708 dependencies), 2 circular dev-dependency loops (`bevy_math -> bevy_math`, `bevy_remote -> bevy_remote`), 8-module mutual circular import knot in `bevy_ecs`, 204 Type-1 and 633 Type-2 code clones
   - **Semantic Guards & Refactorings**: Cross-crate AST slicing of `Entity` & `App` (99% reduction), multi-callsite rename in 2.57s with full test pass, safe-delete refusal (3,160 usages), RAM pre-flight validation in 10.66s, shadow runs in 6.4s
   - **Issues Identified & Resolved**:
     - [#739](https://github.com/alex09x/prod-code/issues/739): `fast-sync drops Cargo examples and tests in directories named state or data`
   - **Full Deep-Dive Report**: [204 Code Clones and Two Self-Loops: Dissecting Bevy's 97-Crate Engine with 67 AST Tools (prod.codes)](https://prod.codes/blog/204-code-clones-and-two-self-loops-inside-bevy/)
7. **[prometheus/prometheus](https://github.com/prometheus/prometheus)** (Go)
   - **Evaluated on**: `ram9` (32-core Linux node, `192.168.2.143:9400`)
   - **Tool Coverage**: 67 / 67 tools across all 9 suites
   - **Key Metrics**: 385,511 lines of Go across 736 files, 14.1s cold sync into `gopls` in RAM, 865 µs LAN ping, 0% local laptop CPU
   - **Architectural Findings**: 84-package DAG (524 dependencies), 0 circular package cycles, core foundations `discovery/targetgroup` ($C_a=35$) and `model/labels` ($C_a=32$) with 0.000 instability, 40 Protobuf Varint clone clusters
   - **Semantic Guards & Refactorings**: Structural search captured 1,485 `if err != nil` patterns in 3.71s, cross-package AST slicing of `promql.Engine` (99% reduction to 2.4 KB), safe-delete refusal (9 call sites of `contextDone`), RAM compiler shadow validation caught type error in 1.8s
   - **Issues Identified & Resolved**:
     - [#740](https://github.com/alex09x/prod-code/issues/740): `dependencies: unbounded circular dependency stream and loose Go import segment matching`
   - **Full Deep-Dive Report**: [1,485 Error Checks and Zero Import Cycles: Dissecting Prometheus's 84-Package Engine with 67 AST Tools (prod.codes)](https://prod.codes/blog/1485-error-checks-and-zero-cycles-inside-prometheus/)

8. **[kubernetes/kubernetes](https://github.com/kubernetes/kubernetes)** (Go)
   - **Evaluated on**: `ram9` (32-core Linux node, `192.168.2.143:9400`)
   - **Tool Coverage**: 67 / 67 tools across all 9 suites
   - **Key Metrics**: 5,384,262 lines of Go across 17,823 files (31,351 total files), 779 internal packages, 737 µs LAN ping, 0% local laptop CPU
   - **Architectural Findings**: 779-package DAG (3,639 dependencies), core foundations `pkg/features` ($C_a=154$), `pkg/api/legacyscheme` ($C_a=138$), and `pkg/apis/core` ($C_a=137$), 900+ Type-2 clone groups across scheduler plugins
   - **Semantic Guards & Refactorings**: Transitive AST slicing of `v1.Pod` (99.4% reduction to 1.42s), structural search captured 737 error checks in 160 files in `pkg/kubelet` in 1.86s, interface extraction of 22 methods from `CycleState` into `CycleStateManager`, safe-delete refusal (2 callers), type-invalidation caught in memory in 3.61s
   - **Issues Identified & Resolved**:
     - [#741](https://github.com/alex09x/prod-code/issues/741): `compile_go_shadow: go test -c -o collides on packages with identical base names in multi-package modules`
     - [#742](https://github.com/alex09x/prod-code/issues/742): `invert_boolean: find_polyglot_predicate_declaration matches call site before function declaration`
   - **Full Deep-Dive Report**: [5.3 Million Lines of Go and 779 Packages: Dissecting Kubernetes with 67 AST Tools (prod.codes)](https://prod.codes/blog/5-million-lines-of-go-inside-kubernetes/)

9. **[django/django](https://github.com/django/django)** (Python)
   - **Evaluated on**: `ram9` (32-core Linux node, `192.168.2.143:9400`)
   - **Tool Coverage**: 67 / 67 tools across all 9 suites
   - **Key Metrics**: 526,995 lines of Python across 2,932 files, 654 µs LAN ping, 0% local laptop CPU
   - **Architectural Findings**: 112 modules in `django/core` (832 dependencies), 25 circular module import paths detected in `django::core::checks`, foundational modules `django::core::exceptions` ($C_a=11, I=0.00$), 150+ Type-2 clone groups across serializers, command parsers, and cache backends
   - **Semantic Guards & Refactorings**: Transitive AST slicing of `BaseHandler.resolve_request` (99.97% reduction to 160 lines in 0.12s), structural search captured 38 error guards in `django/core` in 99.70 ms, parameter object bundling on `func_supports_parameter` -> `FuncParamSpec` (0 type errors), boolean inversion of `is_module_level_function` with proven safety refusal on passed-as-value functions, in-memory pre-validation caught injected attribute typo in 2.07s
   - **Full Deep-Dive Report**: [Half a Million Lines of Python and 25 Import Cycles: Dissecting Django with 67 AST Tools (prod.codes)](https://prod.codes/blog/half-a-million-lines-of-python-inside-django/)

10. **[psf/black](https://github.com/psf/black)** (Python)
    - **Evaluated on**: `booster` (32-core Linux node, `192.168.2.168:9400`) & `ram9`
    - **Tool Coverage**: 67 / 67 tools across all 9 suites
    - **Key Metrics**: 135,022 lines of Python across 358 files, 930 µs LAN ping, 0% local laptop CPU
    - **Architectural Findings**: Import cycle in `src/black/__init__.py` via `concurrency.py`, 10-line Type-2 clone blocks across 7 Python version feature tables in `src/black/mode.py`
    - **Semantic Guards & Refactorings**: Transitive AST slicing of `can_be_split` (99.8% reduction to 240 lines), structural search captured 35 guard-and-raise statements in 8 files in 119.90 ms, 3-way RRF semantic search ranked delimiter split implementations in 8 ms across 3,133 declarations, parameter object bundling on `assert_equivalent(src, dst)` -> `@dataclass CodePair` across 12 call sites in 4 files, boolean inversion on `can_be_split` with proven safety refusal on non-call imports, in-memory shadow pre-validation caught injected attribute error in 0.68s
    - **Full Deep-Dive Report**: [Black Under the Microscope: What 67 AST Tools Found Inside Python's Uncompromising Formatter (prod.codes)](https://prod.codes/blog/black-under-the-microscope-67-ast-tools/)

11. **[tiangolo/fastapi](https://github.com/tiangolo/fastapi)** (Python)
    - **Evaluated on**: `booster` (32-core Linux node, `192.168.2.168:9400`) & `ram9`
    - **Tool Coverage**: 67 / 67 tools across all 9 suites
    - **Key Metrics**: 118,093 lines of Python across 1,166 files, 930 µs LAN ping, 0% local laptop CPU
    - **Architectural Findings**: 135 occurrences of OpenAPI schema response clones in `tests/test_include_router_defaults_overrides.py`, `fastapi/routing.py` APIRouter hierarchy with 866 variables
    - **Semantic Guards & Refactorings**: Transitive AST slicing of `solve_dependencies` (99.8% reduction to 260 lines), structural search captured 66 raise statements in 16 files in 241.47 ms, 3-way RRF semantic search located dependency solving in 55 ms across 6,216 declarations, parameter object bundling on `create_model_field` -> `@dataclass FieldSpec` across 5 call sites in 3 files, boolean inversion on `is_body_allowed_for_status_code` with 2 import reference safety refusals, function extraction safety refusal on classmethod calling `self`, in-memory shadow pre-validation caught injected attribute error in 0.52s
    - **Full Deep-Dive Report**: [FastAPI Under the Microscope: What 67 AST Tools Found Inside Python's Modern Async Framework (prod.codes)](https://prod.codes/blog/fastapi-under-the-microscope-67-ast-tools/)

12. **[pallets/flask](https://github.com/pallets/flask)** (Python)
    - **Evaluated on**: `booster` (32-core Linux node, `192.168.2.168:9400`) & `ram9`
    - **Tool Coverage**: 67 / 67 tools across all 9 suites
    - **Key Metrics**: 18,345 lines of Python across 83 files (1,658 declarations), 650 µs LAN ping, 0% local laptop CPU
    - **Architectural Findings**: Code clone clusters between `src/flask/blueprints.py` (lines 81-94) and `src/flask/app.py` (lines 392-405) on `send_static_file` and cache expiration
    - **Semantic Guards & Refactorings**: Transitive AST slicing of `Flask.full_dispatch_request` (98.6% reduction from 18K to 250 lines in 250 ms), structural search captured 159 exception raises across 33 files in 255.96 ms (166 total direct raises across 33 files), 3-way RRF semantic search ranked request dispatching in 10 ms across 1,658 declarations, parameter object bundling on `flash(message, category)` -> `@dataclass FlashMessage` with default argument preservation, boolean inversion on `AppContext.has_request` with proven safety refusal on 6 property value references across 4 files, semantic indentation verification on function extraction, in-memory shadow pre-validation caught 6 syntax errors in 0.22s
    - **Full Deep-Dive Report**: [Flask Under the Microscope: What 67 AST Tools Found Inside Python's Iconic Microframework (prod.codes)](https://prod.codes/blog/flask-under-the-microscope-67-ast-tools/)

13. **[etcd-io/etcd](https://github.com/etcd-io/etcd)** (Go)
    - **Evaluated on**: `booster` (32-core Linux node, `192.168.2.168:9400`) & `ram9`
    - **Tool Coverage**: 67 / 67 tools across all 9 suites
    - **Key Metrics**: 219,982 lines of Go across 1,094 source files (11,320 declarations), 650 µs LAN ping, 0% local laptop CPU
    - **Architectural Findings**: 1,094-package DAG (3,420 dependencies), zero circular dependencies, verified unidirectional layering (`api`, `pkg`, `client`, `server/storage`, `server/etcdserver`)
    - **Semantic Guards & Refactorings**: Transitive AST slicing of `(*EtcdServer).MoveLeader` (99.9% reduction from 220K lines to 200 lines in 180 ms), structural search captured 445 error returns across 132 files in 1,226 ms, 3-way RRF semantic search located Raft election handlers in 87 ms across 11,320 declarations, parameter object bundling on `MoveLeader` with compiler safety refusal catching cross-package `LeaderTransferrer` interface mismatch and unexported structs, boolean inversion on `isLeader` with 7 non-call value reference safety refusals, in-memory shadow pre-validation caught syntax error in 0.57s
    - **Full Deep-Dive Report**: [etcd Under the Microscope: What 67 AST Tools Found Inside Cloud-Native Consensus (prod.codes)](https://prod.codes/blog/etcd-under-the-microscope-67-ast-tools/)
14. **[moby/moby](https://github.com/moby/moby)** (Go)
    - **Evaluated on**: `booster` (32-core Linux node, `192.168.2.168:9400`) & `ram9`
    - **Tool Coverage**: 67 / 67 tools across all 9 suites
    - **Key Metrics**: 387,617 lines of Go across 2,269 source files (20,508 declarations), 650 µs LAN ping, 0% local laptop CPU
    - **Architectural Findings**: 2,269-file DAG, zero circular package dependencies, 224 occurrences of an identical 14-line Protobuf varint decoding loop across BuildKit and fsutil
    - **Semantic Guards & Refactorings**: Transitive AST slicing of `ContainerStart` (99.9% reduction from 387K lines to 250 lines in 190 ms), structural search captured 858 error returns across 285 files in 1.9s, 3-way RRF semantic search located container start and attach streams in 212 ms across 20,508 declarations, boolean inversion on `IsRunning` across 19 files with gopls compiler safety refusal protecting plugin executor interfaces, in-memory shadow pre-validation in 2.00s
    - **Full Deep-Dive Report**: [Moby Under the Microscope: What 67 AST Tools Found Inside Docker's Engine (prod.codes)](https://prod.codes/blog/moby-under-the-microscope-67-ast-tools/)
15. **[microsoft/TypeScript](https://github.com/microsoft/TypeScript)** (TypeScript)
    - **Evaluated on**: `booster` (32-core Linux node, `192.168.2.168:9400`) & `ram9`
    - **Tool Coverage**: 67 / 67 tools across all 9 suites
    - **Key Metrics**: 264,411 declarations across 31,433 source files, 650 µs LAN ping, 0% local laptop CPU
    - **Architectural Findings**: Clean modular DAG across 116 packages, 17 occurrences of an identical 10-line modifier traversal generator loop in `packages/typescript/src/ast/factory.generated.ts` (Group #1480), and 332 occurrences across test baselines
    - **Semantic Guards & Refactorings**: Transitive AST slicing of `createSourceFile` (99.9% reduction from 264K declarations to ~120 lines across 3 files), structural search captured 134 error guards matching `throw new Error($$$)` across 18 files in 721.72 ms, 3-way RRF semantic search located `createSourceFile` in 2,088 ms across 264,411 declarations, boolean inversion on `hasProperty` safely halted by compiler refusal due to 10 non-call value references in namespace imports, parameter object bundling on `hasProperty` generated `export interface HasPropertyArgs` with call sites rewritten to `{ map: member, key: "kind" }` and 0 errors, in-memory shadow pre-validation in 5.24s
    - **Full Deep-Dive Report**: [TypeScript Under the Microscope: What 67 AST Tools Found Inside the Compiler (prod.codes)](https://prod.codes/blog/typescript-under-the-microscope-67-ast-tools/)
16. **[facebook/react](https://github.com/facebook/react)** (JavaScript / Flow / TypeScript)
    - **Evaluated on**: `booster` (32-core Linux node, `192.168.2.168:9400`) & `ram9`
    - **Tool Coverage**: 67 / 67 tools across all 9 suites
    - **Key Metrics**: 115,541 declarations across 4,578 source files (745,980 lines), 650 µs LAN ping, 0% local laptop CPU
    - **Architectural Findings**: Clean 167-module DAG in reconciler, 18 occurrences of an identical 12-line test harness class in `ReactFragment-test.js` (Group #328), and 16 occurrences of a 12-line transition tracing callback in `ReactTransitionTracing-test.js` (Group #17)
    - **Semantic Guards & Refactorings**: Transitive AST slicing of `scheduleUpdateOnFiber` in `ReactFiberWorkLoop.js` (99.9% reduction from 745K lines to Fiber/FiberRoot/lanes data structures), structural search captured 156 error guards matching `throw Error($$$)` across 59 files in 1,901 ms, 3-way RRF semantic search located `scheduleUpdateOnFiber` in 341 ms (dense cosine similarity 0.839), parameter object bundling on `retainWhere<T>` in `babel-plugin-react-compiler` safely rejected by language engine because the synthesized interface was missing generic `<T>` scope (`Cannot find name 'T' [2304]`), remote in-memory shadow pre-validation in 840 ms
    - **Full Deep-Dive Report**: [React Under the Microscope: What 67 AST Tools Found Inside the UI Engine (prod.codes)](https://prod.codes/blog/react-under-the-microscope-67-ast-tools/)
17. **[vercel/next.js](https://github.com/vercel/next.js)** (TypeScript / JavaScript)
    - **Evaluated on**: `booster` (32-core Linux node, `192.168.2.168:9400`) & `ram9`
    - **Tool Coverage**: 67 / 67 tools across all 9 suites
    - **Key Metrics**: 229,297 declarations across 24,309 source files (1,293,158 lines), 650 µs LAN ping, 0% local laptop CPU
    - **Architectural Findings**: 24,309-file multi-package DAG, 13 occurrences of an identical 12-line immediate test helper in `test/e2e/app-dir/actions/fast-set-immediate.external.test.ts:90-101` (Clone Group #484)
    - **Semantic Guards & Refactorings**: Transitive AST slicing of `renderToHTMLImpl` in `packages/next/src/server/render.tsx:457` (isolated Document shell, React Fizz streaming, tracing spans, and HTML postProcess), structural search captured 1,254 error assertions matching `throw new Error($$$)` across 306 files in 10,378 ms, semantic search located `renderToHTML` entrypoints in 1,572 ms across 229,297 declarations, AST boolean inversion on `isResSent` -> `isResPending` accurately updated 24 lines across 5 files flipping 5 call-site negations and simplifying pre-existing `!`, in-memory pre-flight validation caught 4 syntax errors in 4.35s while filtering 9 pre-existing diagnostics without touching disk
    - **Full Deep-Dive Report**: [Next.js Under the Microscope: What 67 AST Tools Found Inside the React Framework (prod.codes)](https://prod.codes/blog/nextjs-under-the-microscope-67-ast-tools/)
18. **[expressjs/express](https://github.com/expressjs/express)** (JavaScript)
    - **Evaluated on**: `booster` (32-core Linux node, `192.168.2.168:9400`) & `ram9`
    - **Tool Coverage**: 67 / 67 tools across all 9 suites
    - **Key Metrics**: 6,037 declarations across 201 source files (21,492 lines), 650 µs LAN ping, 0% local laptop CPU
    - **Architectural Findings**: Prototype delegation chains, Clone Group #150 identified identical 8-line middleware mock fixtures across 6 test suites (`test/express.json.js`, `test/res.sendFile.js`, etc.)
    - **Semantic Guards & Refactorings**: Transitive AST slicing of `app.handle` in `lib/application.js:152` (95% reduction from 13.9KB to 711 bytes isolating handler binding, headers, and error logger), structural search captured 10 type assertion sites matching `throw new TypeError($$$)` across 4 files in 37.56 ms, semantic search located `this.router.handle` and `app.handle` in 22 ms across 6,037 declarations, AST boolean inversion on `app.enabled` safely refused mutation due to 22 dynamic value references in untyped JS where `this.enabled` is accessed as a property, remote in-memory pre-flight validation caught invalid JavaScript variable syntax in 0.20s
    - **Full Deep-Dive Report**: [Express Under the Microscope: What 67 AST Tools Found Inside the Node.js Backbone (prod.codes)](https://prod.codes/blog/express-under-the-microscope-67-ast-tools/)
19. **[redis/redis](https://github.com/redis/redis)** (C)
    - **Evaluated on**: `booster` (32-core Linux node, `192.168.2.168:9400`) & `ram9`
    - **Tool Coverage**: 67 / 67 tools across all 9 suites
    - **Key Metrics**: 76,566 declarations across 844 files (211,529 lines in `src/`), 650 µs LAN ping, 0% local laptop CPU
    - **Architectural Findings**: Multiplexed event architecture, Clone Group #382 identified 5 identical 12-line reply parser validation blocks in `deps/hiredis/hiredis.c:190-296`
    - **Semantic Guards & Refactorings**: Transitive AST slicing of `aeProcessEvents` in `src/ae.c:365` (isolated event dispatch dependencies, traversing to `aeEventLoop` in `src/ae.h` and `aeApiPoll` epoll_wait in `src/ae_epoll.c:89`), structural search captured 454 error reply sites matching `addReplyError($$$)` across 38 files in 4,538 ms, semantic search located `aeProcessEvents` in 196 ms (dense cosine similarity 0.852), AST boolean inversion on `stringmatch` -> `stringmismatch` updated 26 lines across 4 files and safely halted on 1 non-call reference in `src/debug.c:1080` (`stringmatch-test`), remote in-memory pre-flight validation under clangd caught 3 C compilation errors in 0.52s
    - **Full Deep-Dive Report**: [Redis Under the Microscope: What 67 AST Tools Found Inside the In-Memory Engine (prod.codes)](https://prod.codes/blog/redis-under-the-microscope-67-ast-tools/)
20. **[duckdb/duckdb](https://github.com/duckdb/duckdb)** (C++)
    - **Evaluated on**: `booster` (32-core Linux node, `192.168.2.168:9400`) & `ram9`
    - **Tool Coverage**: 67 / 67 tools across all 9 suites
    - **Key Metrics**: 350,004 declarations across 4,971 files (681,473 lines in `src/`), 650 µs LAN ping, 0% local laptop CPU
    - **Architectural Findings**: Columnar vectorized execution architecture, 5,500+ clone groups across third-party generators (Group #1518 with 33 occurrences and Group #2227 with 25 occurrences in `third_party/utf8proc`)
    - **Semantic Guards & Refactorings**: Transitive AST slicing of `Connection::Query` in `src/main/connection.cpp:81` (traversed query execution spine into `QueryResult` data chunk streaming contracts in `src/include/duckdb/main/query_result.hpp`), structural search captured 2,277 exception invariants matching `throw InternalException($$$)` across 693 files in 18.8s, semantic search located `class ClientContext` in 4,172 ms with typed graph centrality of 5.23 (in-degree 3,671), AST boolean inversion on `StringUtil::EndsWith` safely halted due to cross-class overload collision with `Identifier::EndsWith` and 2 non-call value references in Catch2 test matchers, remote in-memory pre-flight validation under clangd caught undeclared types in 2.82s
    - **Full Deep-Dive Report**: [DuckDB Under the Microscope: What 67 AST Tools Found Inside the Analytical Engine (prod.codes)](https://prod.codes/blog/duckdb-under-the-microscope-67-ast-tools/)

21. **[git/git](https://github.com/git/git)** (C)
    - **Evaluated on**: `booster` (32-core Linux node, `192.168.2.168:9400`) & `ram9`
    - **Tool Coverage**: 67 / 67 tools across all 9 suites
    - **Key Metrics**: 107,779 declarations across 1,007 files (443,306 lines of C), 650 µs LAN ping, 0% local laptop CPU
    - **Architectural Findings**: Monolithic C DAG (`gitcore`), Clone Group #199 identified 4 occurrences of a 12-line filter situation dispatch block in `list-objects-filter.c:81-412`, Clone Group #173 identified repeated reftable block iterator loops in unit tests
    - **Semantic Guards & Refactorings**: Semantic search located `setup_git_directory` entry points in 316 ms across 107,779 declarations with dense cosine similarity 0.901-0.904, structural search captured 3,001 fatal `die($$$)` exit sites in 7.46s and 1,656 recoverable `error($$$)` sites in 5.15s (4,657 total exit boundaries across the C codebase), parameterized AST codemod updated 6 lines across 3 files in disjoint subsystems (`apply.c`, `builtin/unpack-file.c`, `xdiff-interface.c`) binding `$arg` metavariables and wrapping in gettext macros `_()` without disk mutations
    - **Full Deep-Dive Report**: [Git Under the Microscope: What 67 AST Tools Found Inside the Core VCS (prod.codes)](https://prod.codes/blog/git-under-the-microscope-67-ast-tools/)

22. **[curl/curl](https://github.com/curl/curl)** (C)
    - **Evaluated on**: `booster` (32-core Linux node, `192.168.2.168:9400`) & `ram9`
    - **Tool Coverage**: 67 / 67 tools across all 9 suites
    - **Key Metrics**: 70,295 declarations across 1,101 files (200,796 lines of C), 610 µs LAN ping, 0% local laptop CPU
    - **Architectural Findings**: Core transfer architecture centered on `Curl_easy` struct handle (in-degree 1,573, graph centrality 4.83), Clone Group #780 identified 17 identical occurrences of global initialization logic across `tests/libtest/lib*.c`, Clone Group #2287 identified 16 occurrences of execution/cleanup scaffolds
    - **Semantic Guards & Refactorings**: Semantic search located `curl_easy_perform` entry points in 328 ms across 70,295 declarations, structural search captured 1,134 failure points matching `failf($$$)` in 3.50s and 790 protocol trace points matching `infof($$$)` in 2.74s (1,924 total telemetry and failure egress points across the C codebase), parameterized AST codemod updated proxy error descriptions across 4 lines in `lib/cf-h1-proxy.c` and bound local ephemeral port telemetry via `$p` metavariable without touching disk
    - **Full Deep-Dive Report**: [curl Under the Microscope: What 67 AST Tools Found Inside the Ubiquitous Transfer Engine (prod.codes)](https://prod.codes/blog/curl-under-the-microscope-67-ast-tools/)

23. **[apple/swift-algorithms](https://github.com/apple/swift-algorithms)** (Swift)
    - **Evaluated on**: macOS node (`192.168.2.40:9400`, Apple Silicon)
    - **Tool Coverage**: 67 / 67 tools across all 9 suites
    - **Key Metrics**: 1,735 declarations across 57 files (13,722 lines of Swift), 5.44 ms LAN ping, 0% local laptop CPU
    - **Architectural Findings**: Generic collection adapter architecture centered on progressive protocol extensions (`Sequence`, `Collection`, `BidirectionalCollection`, `RandomAccessCollection`), Clone Group #314 surfaced 6 identical occurrences of `offsetForward` bounds checking across distinct collection adapters, Clone Group #11 surfaced 3 identical `offsetBackward` implementations
    - **Semantic Guards & Refactorings**: Semantic search located `Chain2Sequence` and protocol conformances in 13 ms across 1,735 declarations, structural search captured 45 release-mode bounds preconditions in 57.45 ms and 36 debug assertions in 57.57 ms (81 total bounds invariants across collection indexers), structural codemod transformed internal error descriptions across 24 lines in 5 files (`Chain.swift`, `FlattenCollection.swift`, `Intersperse.swift`, `Product.swift`, `Windows.swift`), remote Apple Silicon test offloading executed `swift test --filter ChainTests` passing 5/5 tests in 10.3s without local battery consumption
    - **Full Deep-Dive Report**: [Swift Algorithms Under the Microscope: What 67 AST Tools Found Inside Apple's Sequence Engine (prod.codes)](https://prod.codes/blog/swift-algorithms-under-the-microscope-67-ast-tools/)

24. **[apple/swift-argument-parser](https://github.com/apple/swift-argument-parser)** (Swift)
    - **Evaluated on**: macOS node (`192.168.2.40:9400`, Apple Silicon)
    - **Tool Coverage**: 67 / 67 tools across all 9 suites
    - **Key Metrics**: 3,724 declarations across 170 files (30,001 lines of Swift), 5.44 ms LAN ping, 0% local laptop CPU
    - **Architectural Findings**: CLI command dispatch hierarchy centered on `ParsableCommand` protocol (in-degree 296, graph centrality 4.04), Clone Group #417 and #511 identified identical value unwrapping and `configurationFailure` boilerplate across all 5 core property wrappers (`@Argument`, `@Option`, `@OptionGroup`, `@ParentCommand`, `@Flag`)
    - **Semantic Guards & Refactorings**: Semantic search located `ParsableCommand` and `AsyncParsableCommand` in 19 ms across 3,724 declarations, structural search captured 46 parser failures (`throw ParserError.$$$`) in 80.97 ms and 18 validation failures (`throw ValidationError($$$)`) in 44.52 ms (64 total error exit points across the CLI tree), parameterized AST codemod updated 42 lines across 9 files binding `$msg` metavariable cleanly into `ValidationError("DoccReference: " + $msg)`, remote Apple Silicon test offloading executed `swift test --filter HelpGenerationTests` passing 81/81 tests in 14.2s without local CPU consumption
    - **Full Deep-Dive Report**: [Swift Argument Parser Under the Microscope: What 67 AST Tools Found Inside Apple's CLI Framework (prod.codes)](https://prod.codes/blog/swift-argument-parser-under-the-microscope-67-ast-tools/)

25. **[Alamofire/Alamofire](https://github.com/Alamofire/Alamofire)** (Swift)
    - **Evaluated on**: macOS node (`192.168.2.40:9400`, Apple Silicon)
    - **Tool Coverage**: 67 / 67 tools across all 9 suites
    - **Key Metrics**: 7,652 declarations across 102 files (39,248 lines of Swift), 5.44 ms LAN ping, 0% local laptop CPU
    - **Architectural Findings**: Core networking pipeline centered on `SessionDelegate` multiplexer and `AFError` typed domain (in-degree 472, centrality 3.31), Clone Group #471 surfaced 25 occurrences of response expectation fulfillment boilerplate across 6 test files, Clone Group #2051 surfaced 10 occurrences of WebSocket disconnect handlers
    - **Semantic Guards & Refactorings**: Semantic search located `SessionDelegate::urlSession` and `AFError` in 13-16 ms across 7,652 declarations, structural search captured 46 typed library errors (`throw AFError.$$$`) in 220.95 ms, 92 total throw statements (`throw $$$`) in 185.35 ms, and 7 invalid URL conversions (`throw AFError.invalidURL($$$)`) in 61.92 ms, parameterized AST codemod updated 14 lines across 2 files binding `$u` URL metavariables without touching disk, remote Apple Silicon test offloading executed 60 tests across `HTTPHeadersTests`, `ParameterEncodingTestCase`, and `RetryPolicyTestCase` passing in under 8s with 0% local CPU, dead-code scanner isolated 11 unreferenced example symbols in 11.35s
    - **Full Deep-Dive Report**: [Alamofire Under the Microscope: What 67 AST Tools Found Inside the Swift Networking Engine (prod.codes)](https://prod.codes/blog/alamofire-under-the-microscope-67-ast-tools/)

26. **[apple/swift-collections](https://github.com/apple/swift-collections)** (Swift)
    - **Evaluated on**: macOS node (`192.168.2.40:9400`, Apple Silicon)
    - **Tool Coverage**: 67 / 67 tools across all 9 suites
    - **Key Metrics**: 10,710 declarations across 441 files (70,979 lines of Swift), 5.44 ms LAN ping, 0% local laptop CPU
    - **Architectural Findings**: Core algorithmic collection architecture centered on `Deque` ring buffers (in-degree 270, graph centrality 4.00) and `OrderedSet` / `OrderedDictionary` hash trees (in-degree 396 and 229, centralities 4.18 and 3.92), Clone Group #1165 identified 11 occurrences of nested tree-node traversal scaffolding across persistent hash tree tests
    - **Semantic Guards & Refactorings**: Semantic search located `Deque`, `OrderedSet`, and `OrderedDictionary` in 19-100 ms across 10,710 declarations, structural search captured 548 release preconditions (`precondition($$$)`) in 481.67 ms, 731 debug assertions (`assert($$$)`) in 540.70 ms (1,279 total boundary invariants), and 15 precondition traps (`preconditionFailure($$$)`) in 89.64 ms, parameterized AST codemod updated 30 lines across 10 files spanning `BitCollections`, `HashTreeCollections`, and `OrderedCollections` binding `$msg` without touching disk, remote Apple Silicon test offloading executed 130 tests across `DequeTests` and `OrderedSetTests` passing in under 27s with 0% local CPU, dead-code scanner isolated 33 unreferenced benchmark harnesses in 25.65s
    - **Full Deep-Dive Report**: [Swift Collections Under the Microscope: What 67 AST Tools Found Inside Apple's Data Structure Engine (prod.codes)](https://prod.codes/blog/swift-collections-under-the-microscope-67-ast-tools/)

27. **[google/guava](https://github.com/google/guava)** (Java)
    - **Evaluated on**: 32-core remote cluster node (`192.168.2.143:9400` / `192.168.2.190:9400`)
    - **Tool Coverage**: 67 / 67 tools across all 9 suites
    - **Key Metrics**: 795,118 lines of Java across 3,279 files, 0.65 ms LAN ping, 0% local laptop CPU
    - **Architectural Findings**: Core collections and concurrency abstractions evaluated; clone analysis identified 33-occurrence clone patterns across concurrent hash multiset and table implementations
    - **Semantic Guards & Refactorings**: Structural search scanned 3,279 files in 545.82 ms finding 1,440 precondition sites (`Preconditions.checkNotNull($$$)`), parameterized AST codemod tested dry-run transformations across 454 files, refactoring engine validated method extraction, signature modifications, and type migrations
    - **Full Deep-Dive Report**: [Guava Under the Microscope: What 67 AST Tools Found Inside Google's Core Java Libraries (prod.codes)](https://prod.codes/blog/guava-under-the-microscope-67-ast-tools/)

28. **[spring-projects/spring-boot](https://github.com/spring-projects/spring-boot)** (Java)
    - **Evaluated on**: 32-core remote cluster node (`192.168.2.143:9400` / `192.168.2.190:9400`)
    - **Tool Coverage**: 67 / 67 tools across all 9 suites
    - **Key Metrics**: 880,132 lines of Java across 8,696 files (448 Gradle modules, 2,790 dependencies), 0.65 ms LAN ping, 0% local laptop CPU
    - **Architectural Findings**: 448-module dependency graph analyzed with `prod-code dependencies`, revealing a completely acyclic DAG (0 cycles). Foundational `starter:spring-boot-starter` has highest afferent coupling (Ca=283, Ce=3, instability 0.01), followed by `core:spring-boot` (Ca=164, Ce=1). Clone analysis (`prod-code duplicates`) scanned 9,192 files (908,509 lines) and isolated 20 clone groups, including 55 occurrences of `isEnabled()` property accessors across autoconfiguration classes, confirming intentional POJO decoupling over fragile base classes.
    - **Semantic Guards & Refactorings**: Structural search scanned 8,897 files in 3,075 ms isolating 982 assertions matching `Assert.notNull($A, $B)` across 398 files. Parameterized AST codemod tested `Assert.notNull($A, $B) ==>> Objects.requireNonNull($A, $B)` yielding 1,965 changed lines across 398 files in dry run. AST refactoring evaluated `prod-code extract-function` on `SpringApplicationShutdownHook.java`, capturing method parameters and instance receiver, verified by remote Eclipse JDTLS with 0 errors.
    - **Full Deep-Dive Report**: [Spring Boot Under the Microscope: What 67 Remote AST Tools Found Inside the Enterprise Java Standard (prod.codes)](https://prod.codes/blog/spring-boot-under-the-microscope-67-ast-tools/)

29. **[apache/kafka](https://github.com/apache/kafka)** (Java/Scala)
    - **Evaluated on**: 32-core remote cluster node (`192.168.2.143:9400` / `192.168.2.190:9400`)
    - **Tool Coverage**: 67 / 67 tools across all 9 suites
    - **Key Metrics**: 1,717,457 lines across 6,218 Java and 257 Scala files (64 modules, 82 dependencies), 0.49 ms LAN ping, 0% local laptop CPU
    - **Architectural Findings**: 64-module centralized Gradle build analyzed with `prod-code dependencies`, revealing a completely acyclic DAG (0 circular dependencies). Protocol wire client `clients` sits at the root layer (Ca=6, Ce=0, instability 0.00). Clone analysis (`prod-code duplicates`) scanned 6,653 files (1,748,112 lines) and surfaced 20 clone groups, including Clone Group #60505 (125 occurrences of `equals(Object o)` across protocol and administrative records).
    - **Semantic Guards & Refactorings**: Structural search scanned 6,653 files in 4,299.86 ms isolating 847 assertion sites matching `Objects.requireNonNull($A, $B)` across 231 files. Parameterized AST codemod tested dry-run migration to lazy supplier closures (`Objects.requireNonNull($A, () -> $B)`) with 1,738 changed lines across 231 files. AST refactoring evaluated `prod-code extract-function` on `Metadata.java` (`validateLeaderEpoch`), verified with 0 analyzer errors by cluster Eclipse JDTLS in 49 ms warm.
    - **Full Deep-Dive Report**: [Kafka Under the Microscope: What 67 Remote AST Tools Found Inside the Distributed Event Core (prod.codes)](https://prod.codes/blog/kafka-under-the-microscope-67-ast-tools/)

30. **[netty/netty](https://github.com/netty/netty)** (Java)
    - **Evaluated on**: 32-core remote cluster node (`192.168.2.143:9400` / `192.168.2.190:9400`)
    - **Tool Coverage**: 67 / 67 tools across all 9 suites
    - **Key Metrics**: 652,153 lines across 3,595 Java source files (4,210 total files tracked, 61 Maven modules, 441 inter-module dependencies), 3.43 ms LAN ping, 0% local laptop CPU
    - **Architectural Findings**: 61-module Maven hierarchy analyzed with `prod-code dependencies`, uncovering an `all -> bom -> all` packaging circular dependency between shaded jar and BOM modules, while foundational core `common` (Ca=45, Ce=3, instability 0.06), `transport` (Ca=44, Ce=4, instability 0.08), and `buffer` (Ca=42, Ce=4, instability 0.09) form a clean, highly stable DAG. Clone analysis (`prod-code duplicates`) scanned 3,614 files (663,381 lines) and isolated 20 clone groups, notably Clone Group #7325 with 510 occurrences of parameterized bitwise operations in unrolled HPACK and QPACK Huffman lookup state machines across `QpackHuffmanDecoder.java` and `HpackHuffmanDecoder.java`.
    - **Semantic Guards & Refactorings**: Structural search scanned 3,633 files in 1,575.32 ms finding 574 precondition sites matching `ObjectUtil.checkNotNull($A, $B)` across 310 files. Parameterized AST codemod tested migration to standard JDK invariants (`ObjectUtil.checkNotNull($A, $B) ==>> Objects.requireNonNull($A, $B)`) with 1,149 changed lines across 310 files in dry run. AST refactoring evaluated `prod-code extract-function` on `AbstractByteBuf.java` (`validateBounds`), automatically deduplicating across both `setBytes` and `writeBytes` (line 1,100), verified with 0 analyzer errors by cluster Eclipse JDTLS.
    - **Full Deep-Dive Report**: [Netty Under the Microscope: What 67 Remote AST Tools Found Inside Java's High-Throughput I/O Engine (prod.codes)](https://prod.codes/blog/netty-under-the-microscope-67-ast-tools/)

31. **[ktorio/ktor](https://github.com/ktorio/ktor)** (Kotlin)
    - **Evaluated on**: 32-core remote cluster node (`192.168.2.143:9400` / `192.168.2.190:9400`)
    - **Tool Coverage**: 67 / 67 tools across all 9 suites
    - **Key Metrics**: 295,995 lines of Kotlin across 2,423 source files (3,132 total files tracked, 134 Gradle subprojects, 458 inter-module dependencies), 1.97 ms LAN ping, 0% local laptop CPU
    - **Architectural Findings**: 134-module Gradle Kotlin DSL build analyzed with `prod-code dependencies`, parsing unary plus subproject definitions and modern Gradle TypeSafe Project Accessors. The engine detected 100 circular dependency paths centered around test fixtures cross-referencing concrete engines (`ktor-test-base` <-> `ktor-client-cio`), while foundational core `ktor-serialization` (Ca=28, Ce=2, instability 0.07) and `ktor-utils` (Ca=15, Ce=1, instability 0.06) anchor the graph with zero-instability stability. Clone analysis (`prod-code duplicates`) scanned 2,605 files (303,134 lines) and isolated 20 clone groups, notably Clone Group #10619 with 74 occurrences of coroutine channel feeding pipelines across `EngineTestBase.kt` and `ClientTestBase.kt`.
    - **Semantic Guards & Refactorings**: Structural search scanned 2,617 files in 1,033.58 ms finding 218 precondition sites matching `require($A) { $B }` across 111 files. Parameterized AST codemod tested dry-run migration to state invariant assertions (`require($A) { $B } ==>> check($A) { $B }`) with 650 changed lines across 111 files. AST refactoring evaluated `prod-code extract-function` on `Crypto.kt` (`parseHexDigit`), extracting `s[srcIdx].toString().toInt(16)` into a private function while preserving surrounding bitwise shift operations, verified with 0 analyzer errors by cluster `kotlin-language-server`.
    - **Full Deep-Dive Report**: [Ktor Under the Microscope: What 67 Remote AST Tools Found Inside Kotlin's Asynchronous Web Framework (prod.codes)](https://prod.codes/blog/ktor-under-the-microscope-67-ast-tools/)

32. **[Kotlin/kotlinx.coroutines](https://github.com/Kotlin/kotlinx.coroutines)** (Kotlin)
    - **Evaluated on**: 32-core remote cluster node (`192.168.2.143:9400` / `192.168.2.190:9400`)
    - **Tool Coverage**: 67 / 67 tools across all 9 suites
    - **Key Metrics**: 113,379 lines of Kotlin across 1,039 source files (1,337 total files tracked, 25 Gradle subprojects, 22 inter-module dependencies), 1.97 ms LAN ping, 0% local laptop CPU
    - **Architectural Findings**: 25-module Gradle build analyzed with `prod-code dependencies`, discovering submodules via custom JetBrains `module("...")` helper DSL in `settings.gradle.kts`. The graph forms a clean directed acyclic graph (0 circular dependencies), anchored by foundational core `kotlinx-coroutines-core` (Ca=5, Ce=0, instability 0.00) and `kotlinx-coroutines-debug` (Ca=5, Ce=0, instability 0.00). Clone analysis (`prod-code duplicates`) scanned 1,105 files (116,834 lines) and isolated 20 clone groups, notably Clone Group #1534 with 32 occurrences of test dispatcher execution and lifecycle completion scaffolding across `TestBase.common.kt` and `Tasks.kt`.
    - **Semantic Guards & Refactorings**: Structural search scanned 1,105 files in 320.22 ms finding 72 precondition sites matching `check($A) { $B }` across 42 files. Parameterized AST codemod tested dry-run migration to defensive argument preconditions (`check($A) { $B } ==>> require($A) { $B }`) with 192 changed lines across 42 files. AST refactoring evaluated `prod-code extract-function` on `JobSupport.kt` (`isStateCancelled`), extracting state check expression into a private function, verified with 0 analyzer errors by cluster `kotlin-language-server`.
    - **Full Deep-Dive Report**: [Kotlinx.coroutines Under the Microscope: What 67 Remote AST Tools Found Inside Kotlin's Concurrency Runtime (prod.codes)](https://prod.codes/blog/kotlinx-coroutines-under-the-microscope-67-ast-tools/)

33. **[detekt/detekt](https://github.com/detekt/detekt)** (Kotlin)
    - **Evaluated on**: 32-core remote cluster node (`192.168.2.143:9400` / `192.168.2.190:9400`)
    - **Tool Coverage**: 67 / 67 tools across all 9 suites
    - **Key Metrics**: 132,028 lines of Kotlin across 1,105 source files (1,993 total files tracked, 41 Gradle subprojects, 232 inter-module dependencies), 1.97 ms LAN ping, 0% local laptop CPU
    - **Architectural Findings**: 41-module Gradle build analyzed with `prod-code dependencies`, uncovering 4 circular dependency paths strictly confined to test fixtures (`detekt-api` <-> `detekt-test` <-> `detekt-test-utils`), while foundational production modules `detekt-psi-utils` (Ca=14, Ce=4, instability 0.22) and `detekt-utils` (Ca=6, Ce=0, instability 0.00) enforce clean unidirectional layers. Clone analysis (`prod-code duplicates`) scanned 1,175 files (135,761 lines) and isolated 20 clone groups with 9.4% duplication, notably Clone Group #2627 with 288 occurrences of parameterized test harness boilerplate across `detekt-rules-*` test specifications.
    - **Semantic Guards & Refactorings**: Structural search scanned 1,179 files in 267.38 ms finding 58 precondition sites matching `require($A) { $B }` across 31 files. Parameterized AST codemod tested dry-run migration to defensive state invariants (`require($A) { $B } ==>> check($A) { $B }`) with 154 changed lines across 31 files. AST refactoring evaluated `prod-code extract-function` on `BaselineHandler.kt` (`recordIssueByCurrentType`), extracting XML state handling branch into a private helper function, verified with 0 analyzer errors by cluster `kotlin-language-server`.
    - **Full Deep-Dive Report**: [Detekt Under the Microscope: What 67 Remote AST Tools Found Inside Kotlin's Static Code Analyzer (prod.codes)](https://prod.codes/blog/detekt-under-the-microscope-67-ast-tools/)

34. **[arrow-kt/arrow](https://github.com/arrow-kt/arrow)** (Kotlin)
    - **Evaluated on**: 32-core remote cluster node (`192.168.2.143:9400` / `192.168.2.190:9400`)
    - **Tool Coverage**: 67 / 67 tools across all 9 suites
    - **Key Metrics**: 72,889 lines of Kotlin across 757 source files (985 total files tracked, 38 Gradle subprojects, 86 inter-module dependencies), 1.97 ms LAN ping, 0% local laptop CPU
    - **Architectural Findings**: 38-module Gradle build analyzed with `prod-code dependencies`, uncovering 5 circular dependency paths linking low-level concurrency and utility modules (`arrow-atomic` <-> `arrow-fx-coroutines` <-> `arrow-autoclose` <-> `arrow-exception-utils` <-> `arrow-core`) driven by multiplatform atomic reference requirements, while foundational production modules `arrow-core` (Ca=18, Ce=5, instability 0.22), `arrow-platform` (Ca=7, Ce=0, instability 0.00), and `arrow-optics` (Ca=6, Ce=1, instability 0.14) form clean unidirectional layers. Clone analysis (`prod-code duplicates`) scanned 804 files (74,888 lines) and isolated 20 clone groups with 2.8% duplication, notably Clone Group #2545 with 27 occurrences of parameterized Either monadic law tests and Clone Group #3117 with 14 occurrences of JVM high-arity iterator stepping sequences across `Sequence.kt`.
    - **Semantic Guards & Refactorings**: Structural search scanned 804 files in 273.96 ms finding 20 sites matching the signature Arrow Raise DSL construct `ensure($A) { $B }` across 14 files, and 20 precondition sites matching `require($A) { $B }` across 10 files in 191.74 ms. Parameterized AST codemod tested dry-run migration to state assertions (`require($A) { $B } ==>> check($A) { $B }`) with 58 changed lines across 10 files. AST refactoring evaluated `prod-code extract-function` on `Option.kt` (`wrapNullable`), extracting nullable conversion into a private helper function, verified with 0 analyzer errors by cluster `kotlin-language-server`.
    - **Full Deep-Dive Report**: [Arrow Under the Microscope: What 67 Remote AST Tools Found Inside Kotlin's Functional Core (prod.codes)](https://prod.codes/blog/arrow-under-the-microscope-67-ast-tools/)

35. **[dotnet/aspnetcore](https://github.com/dotnet/aspnetcore)** (C#)
    - **Evaluated on**: 32-core remote cluster node (`192.168.2.143:9400` / `192.168.2.190:9400`)
    - **Tool Coverage**: 67 / 67 tools across all 9 suites
    - **Key Metrics**: 1,757,169 lines of C# across 10,685 source files (17,658 total files tracked, 625 .NET projects, 7,427 inter-project dependencies), 1.55 ms LAN ping, 0% local laptop CPU
    - **Architectural Findings**: 625-project .NET solution analyzed with `prod-code dependencies`, parsing MSBuild project references across multi-target frameworks. Identified 100 circular dependency paths strictly confined to test harness cross-references and benchmark fixtures, while foundational core `Microsoft.AspNetCore.Http.Abstractions` (Ca=340, Ce=7, instability 0.02), `Microsoft.AspNetCore.Http.Features` (Ca=317, Ce=3, instability 0.01), and `Microsoft.AspNetCore.Http` (Ca=292, Ce=11, instability 0.04) establish an inward abstraction gravity DAG where over 50% of the entire repository depends on these zero-instability primitives. Clone analysis (`prod-code duplicates`) scanned 11,383 files (2,233,809 lines) and isolated 20 clone groups with 0.90% duplication, dominated by Roslyn Source Generator verification snapshots (`ValidatableInfoResolver.g.verified.cs`).
    - **Semantic Guards & Refactorings**: Structural search scanned 1,292 files in 5,971.27 ms finding 4,037 call sites matching `ArgumentNullException.ThrowIfNull($A)` across 1,292 files and 169 call sites matching `ArgumentException.ThrowIfNullOrEmpty($A)` across 81 files in 3,044.66 ms. Parameterized AST codemod tested dry-run migration to standard guards (`ArgumentException.ThrowIfNullOrEmpty($A) ==>> ArgumentNullException.ThrowIfNull($A)`) with 338 changed lines across 81 files. AST refactoring evaluated `prod-code extract-function` on `ParsingHelpers.cs` (`ResolveHeaderValue`), extracting conditional header resolution into a private static helper, verified with 0 analyzer errors by cluster OmniSharp.
    - **Full Deep-Dive Report**: [ASP.NET Core Under the Microscope: What 67 Remote AST Tools Found Inside Microsoft's Web Engine (prod.codes)](https://prod.codes/blog/aspnetcore-under-the-microscope-67-ast-tools/)

36. **[dotnet/efcore](https://github.com/dotnet/efcore)** (C#)
    - **Evaluated on**: 32-core remote cluster node (`192.168.2.143:9400` / `192.168.2.190:9400`)
    - **Tool Coverage**: 67 / 67 tools across all 9 suites
    - **Key Metrics**: 1,749,818 lines of C# across 5,778 source files (6,266 total files tracked, 58 .NET projects, 139 inter-project dependencies), 0.55 ms LAN ping, 0% local laptop CPU
    - **Architectural Findings**: 58-project .NET solution analyzed with `prod-code dependencies`, parsing MSBuild project references across multi-provider packages. Discovered zero circular dependencies (clean directed acyclic graph), anchored by foundational root abstractions `EFCore.Analyzers` (Ca=15, Ce=0, instability 0.00), `Microsoft.Data.Sqlite.Core` (Ca=9, Ce=0, instability 0.00), and `EFCore.Abstractions` (Ca=6, Ce=0, instability 0.00), feeding downward into core relational pipeline engines `EFCore` (Ca=9, Ce=2, instability 0.18) and `EFCore.Relational` (Ca=9, Ce=2, instability 0.18). Clone analysis (`prod-code duplicates`) scanned 5,779 files (1,750,227 lines) and isolated 20 clone groups with 2.0% duplication, dominated by Northwind data specification fixtures (`NorthwindData.Objects.cs`, 1,316 occurrences) and JSON SQL update assertions (`JsonUpdateSqlServerTest.cs`, 277 occurrences).
    - **Semantic Guards & Refactorings**: Structural search scanned 5,782 files finding 898 call sites matching `Check.NotNull($A)` across 151 files in 5,722.22 ms and 498 call sites matching `Check.NotEmpty($A)` across 106 files in 5,522.35 ms. Parameterized AST codemod tested dry-run BCL modernization (`Check.NotNull($A) ==>> ArgumentNullException.ThrowIfNull($A)`) with 1,792 changed lines across 151 files. AST refactoring evaluated `prod-code extract-function` on `src/Shared/Check.cs` (`IsWhiteSpaceSpan`), extracting span emptiness validation into a reusable static helper, verified with 0 analyzer errors by cluster OmniSharp.
    - **Full Deep-Dive Report**: [EF Core Under the Microscope: What 67 Remote AST Tools Found Inside .NET's Relational Engine (prod.codes)](https://prod.codes/blog/efcore-under-the-microscope-67-ast-tools/)

37. **[dotnet/roslyn](https://github.com/dotnet/roslyn)** (C#)
    - **Evaluated on**: 32-core remote cluster node (`192.168.2.143:9400` / `192.168.2.190:9400`)
    - **Tool Coverage**: 67 / 67 tools across all 9 suites
    - **Key Metrics**: 6,499,949 lines of C# across 18,176 source files (35,115 total files tracked, 394 .NET projects, 2,103 inter-project dependencies), 0.55 ms LAN ping, 0% local laptop CPU
    - **Architectural Findings**: 394-project self-hosting compiler platform analyzed with `prod-code dependencies`, parsing MSBuild project references across syntax, semantic, workspace, and IDE tiers. Identified 100 circular dependency paths centered around bootstrapping source generators (`CSharpSyntaxGenerator` <-> `Microsoft.CodeAnalysis.Analyzers` <-> `Microsoft.CodeAnalysis.CSharp`), while core compiler engines `Microsoft.CodeAnalysis.CSharp` (Ca=112, Ce=37, instability 0.25) and `Microsoft.CodeAnalysis` (Ca=105, Ce=36, instability 0.26) serve as the architectural gravity center for over 100 downstream projects. Clone analysis (`prod-code duplicates`) scanned 18,140 files (6,495,296 lines) and isolated 20 clone groups with 1.8% duplication, dominated by syntax token sequence assertions across parser regression test suites (Clone Group #258538, 1,500 occurrences).
    - **Semantic Guards & Refactorings**: Structural search scanned 18,177 files finding 14,101 invariant checks matching `Debug.Assert($A)` across 2,049 files in 68,787.39 ms and 1,208 checks matching `RoslynDebug.Assert($A)` across 230 files in 19,343.53 ms. Parameterized AST codemod tested dry-run assertion standardization (`RoslynDebug.Assert($A) ==>> Debug.Assert($A)`) with 2,418 changed lines across 230 files. AST refactoring evaluated `prod-code extract-function` on `src/Compilers/CSharp/Portable/Syntax/SyntaxFacts.cs` (`MatchesAlias`), extracting alias qualification check into a static helper, verified with 0 analyzer errors by cluster OmniSharp.
    - **Full Deep-Dive Report**: [Roslyn Under the Microscope: What 67 Remote AST Tools Found Inside the C# Compiler (prod.codes)](https://prod.codes/blog/roslyn-under-the-microscope-67-ast-tools/)

38. **[dotnet/BenchmarkDotNet](https://github.com/dotnet/BenchmarkDotNet)** (C#)
    - **Evaluated on**: 32-core remote cluster node (`192.168.2.143:9400` / `192.168.2.190:9400`)
    - **Tool Coverage**: 67 / 67 tools across all 9 suites
    - **Key Metrics**: 120,810 lines of C# across 1,175 source files (1,556 total files tracked, 29 .NET projects, 55 inter-project dependencies), 0.58 ms LAN ping, 0% local laptop CPU
    - **Architectural Findings**: 29-project benchmarking framework analyzed with `prod-code dependencies`, parsing MSBuild project references across runtime harnesses, diagnosers, and analyzers. Discovered zero circular dependencies (clean directed acyclic graph), anchored by core engine `BenchmarkDotNet` (Ca=22, Ce=2, instability 0.08) feeding into independent diagnoser modules (`BenchmarkDotNet.Diagnostics.Windows` Ca=5, `BenchmarkDotNet.Diagnostics.dotMemory` Ca=3, `BenchmarkDotNet.Diagnostics.dotTrace` Ca=3) and decoupled analyzers (`BenchmarkDotNet.Analyzers` Ca=3, Ce=0, instability 0.00). Clone analysis (`prod-code duplicates`) scanned 1,192 files (126,437 lines) and isolated 20 clone groups with 3.1% duplication, dominated by synthetic Roslyn diagnostic verification sequences (Clone Group #4278, 85 occurrences) and execution validator test fixtures (Clone Group #2974, 18 occurrences).
    - **Semantic Guards & Refactorings**: Structural search scanned 1,157 files finding 83 low-level I/O and hashing assertions matching `Debug.Assert($A)` across 12 files in 477.33 ms, 1,120 benchmark annotations matching `[Benchmark]` across 195 files in 1,262.90 ms, and 176 null comparisons matching `if ($A == null)` across 87 files in 565.32 ms. Parameterized AST codemod tested dry-run modernization to pattern matching (`if ($A == null) ==>> if ($A is null)`) with 352 changed lines across 87 files. AST refactoring evaluated `prod-code extract-function` on `src/BenchmarkDotNet/Analysers/ConclusionHelper.cs` (`FormatTitle`), extracting benchmark case title formatting into a static helper, verified with 0 analyzer errors by cluster OmniSharp.
    - **Full Deep-Dive Report**: [BenchmarkDotNet Under the Microscope: What 67 Remote AST Tools Found Inside the .NET Micro-Benchmarking Engine (prod.codes)](https://prod.codes/blog/benchmarkdotnet-under-the-microscope-67-ast-tools/)

39. **[apache/spark](https://github.com/apache/spark)** (Scala)
    - **Evaluated on**: 32-core remote cluster node (`192.168.2.143:9400` / `192.168.2.190:9400`)
    - **Tool Coverage**: 67 / 67 tools across all 9 suites
    - **Key Metrics**: 2,047,587 lines of Scala (and 207,298 lines of Java) across 6,448 source files (27,495 total files tracked, 52 Maven/SBT modules), 0.95 ms LAN ping, 0% local laptop CPU
    - **Architectural Findings**: 52-module distributed compute engine analyzed with `prod-code dependencies` across Maven and SBT hierarchies (`pom.xml` and `project/`). Discovered zero circular dependencies (clean directed acyclic graph), anchored by foundational core engine `core` (Ca=41, Ce=3, instability 0.07) feeding into relational Catalyst optimizer expressions `sql/catalyst` (Ca=15, Ce=2, instability 0.12), DataFrame execution platform `sql/core` (Ca=19, Ce=4, instability 0.17), and low-level off-heap memory primitives `common/unsafe` (Ca=8, Ce=1, instability 0.11) and `common/network-common` (Ca=7, Ce=0, instability 0.00). Clone analysis (`prod-code duplicates`) scanned 9,400 files (2,755,888 lines) and isolated 20 clone groups with an exceptionally lean 0.7% duplication ratio, dominated by network RPC/status message serialization boilerplate (Clone Group #29026, 558 occurrences) and high-arity UDF parameter pattern matching from UDF1 to UDF22 in `ToScalaUDF.scala` and `UdfUtils.scala` (Clone Group #6659, 52 occurrences).
    - **Semantic Guards & Refactorings**: Structural search scanned 2,490 files finding 40,444 invariant assertions matching `assert($A)` across 2,490 files in 130.88s and 2,678 precondition checks matching `require($A)` across 621 files in 42.20s. Parameterized AST codemod tested dry-run precondition modernization (`assert($A != null) ==>> require($A != null)`) with 876 changed lines across 165 files. AST refactoring evaluated `prod-code extract-function` on `core/src/main/scala/org/apache/spark/util/Utils.scala` (`elapsedMillis`), extracting execution duration calculation into a private helper, verified with 0 analyzer errors by cluster Scala Metals 1.6.9.
    - **Full Deep-Dive Report**: [Apache Spark Under the Microscope: What 67 Remote AST Tools Found Inside the 2-Million-Line Distributed Engine (prod.codes)](https://prod.codes/blog/spark-under-the-microscope-67-ast-tools/)


