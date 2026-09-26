# Roadmap audit — 2026-09-26

Baseline: `bf71e7d` (v0.3.18). This is a comparison of the written requirements with the
implementation, not a certificate that every language and scenario was rerun. Historical
measurements remain attributed to their original revision in [ROADMAP.md](ROADMAP.md).
An existing implementation, a passing mock test, a release and a deployed service are
different kinds of evidence. New fixes are tracked by their issues and pull requests.

## Coverage of the roadmap

| Item | Implemented scope and evidence | Remaining requirement or verification limit |
|---|---|---|
| 1.1 Transport | Length-framed JSON and optional authentication in the protocol crate; `gateway/tests/e2e_phase1.rs` exercises transport. | Protocol fields have compatibility defaults; there is no negotiated feature set, NUL framing, Unix socket or named-pipe transport. |
| 1.2 Paths | Workspace and worktree path translation in the protocol, gateway and MCP sync code. | The #425 repair covers ordered edits, UTF-16 columns, rollback and containment after directory moves; preview still flags unmodeled resource operations. |
| 1.3 Editor client | `prod-code lsp` bridges the editor to a remote language server and exits unsuccessfully on disconnect. | Recovery is an editor restart, not in-process replay of LSP state. Zero-allocation hot paths are not established by the recorded evidence. |
| 1.4 Gateway | Session registry, command execution and JSON status in `gateway/src/lib.rs`. | The #433 repair reserves memory for concurrent new engines; this is admission control, not a hard limit on later analyzer growth. |
| 2.1 Rust database | In-process analyzer and per-workspace resident databases in `engine-rust/src/lib.rs`. | Distinct worktrees retain distinct databases; memory is not a single shared dependency database. |
| 2.2 Direct edits | Single-owner editing and divergent-worktree benchmark exist. | The original cold workload had 32 timeouts (#408). CPU budgeting and a correctly seeded benchmark are addressed in #432; it is not a repeat of that larger workload. |
| 2.3 File IDs | Explicit file-ID mask and edition-aware conversion in the Rust engine. | Existing tests cover the implementation; this audit did not exhaust all upstream analyzer ID states. |
| 2.4 Observability | Request timing, host memory/disk snapshots and pressure logging. | The observed OOM motivated reservation-based admission (#433); production rollout and monitoring remain separate from passing injected-pressure tests. |
| 3.1 Detection | Manifest and nested-project detection in `gateway/src/detect.rs` and MCP sync. | Framework support still depends on the installed server, project configuration and build index. |
| 3.2 Go | Supervised gopls adapter and shared toolchain caches. | Server-provided assists are not a port of the custom Rust refactoring catalog. |
| 3.3 LSP adapters | Generic subprocess lifecycle, request timeouts and exit detection. | Exit detection is the implemented alternative to periodic health pings. |
| 3.4 C/C++ | clangd, independent compilation databases and ccache. | Shared PCH and a cross-worktree clangd index remain absent. |
| 3.5 TS/JS | TypeScript server, per-copy seeded packages and shared package-download cache. | Package trees are copied; pre-resolved shared type declarations and broad framework latency targets remain unverified. |
| 3.6 Python | basedpyright and copied virtual environments with corrected paths. | Copied environments are not shared mutable environments; semantic completeness follows the server. |
| 3.7 Swift | sourcekit-lsp on the macOS node and per-project build indexing. | Cross-copy module cache remains absent; Linux Swift was not installed or tested. |
| 4.1 MCP | Native tool discovery and typed dispatch in `mcp/src/tools.rs`. | Public help must describe each tool's actual languages and refusal cases. |
| 4.2 Sync | Delta transfer, worktree seeding and persistent MCP sessions in `mcp/src/sync.rs` and `session.rs`. | The #430 repair gives each pooled session its own lock and bounds opening and complete query waits; CLI invocations still reconnect and timings depend on checkout size and cache state. |
| 5.1 Placement | Gossip-aware placement and remembered repository affinity. | The #433 repair also checks new engines for already-affined worktrees; warm sessions continue without a new reservation. |
| 5.2 Discovery | Seed-address discovery and cached gossip membership. | This is the documented replacement for DNS/SRV discovery, not an implementation of it. |
| 5.3 Pressure | Memory/disk thresholds influence placement and idle eviction. | The #433 repair checks host usage plus unsettled reservations against 85%, reclaims eligible idle engines and explains capacity refusals. Unknown host memory and underestimated later growth remain limitations. |
| 5.4 Macros | Build-script loading and out-of-process macro expansion. | Raw analyzer derive diagnostics can contain E0282 artifacts; existing #159 handling labels them in_derive and excludes them from the error count. #424 was closed after confirming that handling; compiler checks passed. |
| 5.5 Fleet tests | Persistent-session and divergent-worktree benchmarks. | Warm success does not erase cold timeouts; record both error counts and percentiles. |
| 6.1 Execution | Streamed commands, exit status, timeout and cancellation. | A command's exit status must survive any output filtering in the acceptance command. |
| 6.2 Caches | Persistent workspace artifacts and per-user package/compiler caches. | RAM-backed caches, pre-warmed Python bytecode and some language-specific shared caches remain open. sccache was observed in node Cargo configuration; the older blanket claim that it was absent is incorrect. |
| 6.3 Isolation | Worktree copies and process supervision. | Shared compiler daemons can escape a shadow mount namespace (#426). |
| 6.4 Build tools | Language-specific check, lint and test dispatch in `mcp/src/verify.rs`. | The 1–3 second target is workload-dependent, not a universal guarantee. |
| 7.1 Refactoring | Rust custom planners plus language-server actions; two custom parameter operations have broader support. | Most custom planners remain Rust-specific. JavaScript parameter objects were absent at baseline (#428). Evaluation order, failed references and transactional application require repairs (#425, #436). |
| 7.2 Fixes | Server code actions and compiler/linter fix modes. | The proposed `code_quickfix` spelling is not a separate tool; use the shipped assist/check/lint surfaces. |
| 7.3 Slicing | Bounded declaration traversal in `mcp/src/slice.rs`. | No intra-function program-dependence/data-flow slicing or proof of a minimal complete slice. |
| 7.4 Shadows | Overlay hypotheses and a serialized fallback in `gateway/src/shadow.rs`. | sccache namespace behavior needs the #426 repair; unsupported environments must not count as a passing integration scenario. |
| 7.5 Graphs | Callers, callees, implementations and supertypes. | Traversal is bounded and server-dependent; the advertised sub-5 ms whole-workspace target is not established for arbitrary repositories. |
| 7.6 Schemas | Analyzer-assisted cross-language rename with text handling for schema formats. | This coordinates known references and schema names; it does not infer every external API consumer. |
| 7.7 Validation | Analyzer overlays, multi-file proposals and optional compiler verification. | Analyzer acceptance is not full compiler/borrow-checker proof; derive artifacts are separately labeled and excluded from the error count (#159; #424 confirmed this existing behavior). |
| 8.1 Impact | Changed ranges, incoming-call traversal and test selection in `mcp/src/impact.rs`. | The #434 repair includes direct test edits and falls back to a full suite on deleted/binary files, unreadable symbols, incomplete hierarchy queries or depth truncation. Selection still depends on the language server's semantic reachability. |
| 8.2 Diagnosis | Failure text, source sites, caller context, diffs and ranked suspects in `mcp/src/dossier.rs`. | No debugger-style runtime capture or structured runtime-values field; assertion output is retained as text. |
| 8.3 Dependencies | Read-only dependency/SDK source retrieval with gateway root restrictions. | This audit did not rerun every SDK/toolchain combination. |
| 8.4 Search | Lexical and optional dense ranking in `gateway/src/search.rs` and `embed.rs`. | The measured ranking is imperfect; typed graph fusion and universal sub-10 ms latency are not established. |
| 8.5 Fixtures | Rust value generation with explicit fallback types in `mcp/src/fixture.rs`. | General mock/builder generation, randomized generation and equivalent other-language tools remain open. |
| 8.6 Pruning | Reference-count candidates and analyzer safe-delete in `dead_code.rs` and `prune.rs`. | This is not entry-point reachability. The #435 repair marks failed and malformed reference/symbol queries unverified and keeps them during pruning; attribute-only references remain a documented limitation. |
| 8.7 Codemods | Rust structural search/replace through the analyzer. | Other-language codemods and the broad sub-second migration target remain open. |

Paths shortened above are under `crates/prod-code-*`. The main integration evidence lives in
the gateway live tests, MCP orchestration/analysis tests, native parameter tests and client CLI
tests. Unit and mock coverage complements real-server checks; it does not replace them.

## Remaining feature scope

The custom Rust-only operations still need independently specified ports to Go, TypeScript,
JavaScript, Python, C/C++ and Swift where applicable: signature changes, safe cascading deletion,
item/module/method moves, parameter inlining, named extraction with duplicate handling, field and
interface extraction, delegation, encapsulation, receiver conversion, type migration, Boolean
inversion, generics and return wrapping. Server assists expose only what that server supports.

Even Rust does not implement every stronger original requirement: whole-program transitive type
migration, custom return envelopes, general factory/builder generation and data-flow slicing
remain distinct work. Inheritance operations apply to languages with inheritance, rather than
being complete merely because they do not apply to Rust.

Infrastructure requirements still open include shared clangd/PCH and Swift module caches,
pre-resolved type declarations and RAM-backed cache policy. Any shared cache must preserve
divergent-worktree isolation; copying a package tree and sharing a mutable tree are different
contracts.

## Development method corrections

- State language, input shape, expected result and executable acceptance before implementation.
- Keep unmet requirements visible and distinguish implemented, verified, released and deployed.
- Reproduce a bug at the assertion; a failed build or unavailable test prerequisite is not a red regression.
- Treat missing coverage, failed semantic queries and truncated analysis as missing evidence.
- Review worker diffs and real outputs. This audit rejected a superficial phase audit, rejected
  a timed-out audit, and required repairs to initially passing implementation candidates.
- Preserve process exit status when collecting logs; document ignored tests and rerun required
  integration scenarios with their prerequisites available.
- Compare equivalent benchmark setup, including whether the origin and copies are cold or warm.
- Keep released dependency bumps within version/lockfile changes and existing consumer checks.

The enforceable contribution and PR requirements are in [CONTRIBUTING.md](CONTRIBUTING.md).
The coverage-gate correction landed in [#431](https://github.com/alex09x/prod-code/pull/431):
26 tests passed, and four false-success CLI cases changed from exit 0 to exit 2.
