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

## Evaluation Workflow for Each Project

For every target repository:
1. **Target Selection**: Pure native language (no external C/C++ or system headers), high adoption, rich architecture.
2. **Environment**: Execute on `booster` (`192.168.2.168:9400`). Keep local client at 0% CPU.
3. **Execution**: Step through all 9 suites (67 tools). Record exact latency, memory, AST matches, and edge cases.
4. **Issue Filing**: If any tool gives an unexpected result, file an issue via `code_report_issue` / `prod-code report-issue`.
5. **Publication**: Produce an in-depth article in `prod.codes/src/content/blog/` documenting all 9 suites with real outputs and metrics. Do NOT put star counts in article titles.
