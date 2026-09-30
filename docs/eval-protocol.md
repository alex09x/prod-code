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








