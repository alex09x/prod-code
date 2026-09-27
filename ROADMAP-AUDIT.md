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
| 1.2 Paths | Structured URI/path translation in the protocol, gateway and MCP sync code preserves source and edit text, with encoded file URIs and path-component boundaries (#438). | The #425 repair covers ordered edits, UTF-16 columns, rollback and containment after directory moves; preview still flags unmodeled resource operations. |
| 1.3 Editor client | `prod-code lsp` bridges the editor to a remote language server and exits unsuccessfully on disconnect. | Recovery is an editor restart, not in-process replay of LSP state. Zero-allocation hot paths are not established by the recorded evidence. |
| 1.4 Gateway | Session registry, command execution and JSON status in `gateway/src/lib.rs`. | The #433 repair reserves memory for concurrent new engines; this is admission control, not a hard limit on later analyzer growth. |
| 2.1 Rust database | In-process analyzer and per-workspace resident databases in `engine-rust/src/lib.rs`. | Distinct worktrees retain distinct databases; memory is not a single shared dependency database. |
| 2.2 Direct edits | Single-owner editing and divergent-worktree benchmark exist. | The original cold workload had 32 timeouts (#408). CPU budgeting and a correctly seeded benchmark are addressed in #432; it is not a repeat of that larger workload. |
| 2.3 File IDs | Explicit file-ID mask and edition-aware conversion in the Rust engine. | Existing tests cover the implementation; this audit did not exhaust all upstream analyzer ID states. |
| 2.4 Observability | Request timing, host memory/disk snapshots and pressure logging. | The observed OOM motivated reservation-based admission (#433); production rollout and monitoring remain separate from passing injected-pressure tests. |
| 3.1 Detection | Manifest and nested-project detection in `gateway/src/detect.rs` and MCP sync. Relative session hints resolve against the supplied checkout (#488). | Framework support still depends on the installed server, project configuration and build index. |
| 3.2 Go | Supervised gopls adapter and shared toolchain caches. | Named-parameter signature permutations and provably-unused removals use real gopls (#448); additions and type/result/modifier changes remain open. Server-provided assists are not a port of the custom Rust refactoring catalog. |
| 3.3 LSP adapters | Generic subprocess lifecycle, request timeouts and exit detection. | Exit detection is the implemented alternative to periodic health pings. |
| 3.4 C/C++ | clangd, independent compilation databases and ccache. | Shared PCH and a cross-worktree clangd index remain absent. |
| 3.5 TS/JS | TypeScript server, per-copy seeded packages and shared package-download cache. | Package trees are copied; pre-resolved shared type declarations and broad framework latency targets remain unverified. |
| 3.6 Python | basedpyright and copied virtual environments with corrected paths. Validation retains one restored document identity across baseline/proposal sessions and isolates proposals on a session-serialized validation server (#466). | Copied environments are not shared mutable environments; semantic completeness follows the server. |
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
| 6.3 Isolation | Worktree copies and process supervision. | The #426 repair keeps sccache compilation client-side in the shadow and refuses incompatible logging/distributed settings. The #440 repair serializes in-place runs per workspace and reports incomplete rollback; arbitrary external build daemons still need their own isolation contract. |
| 6.4 Build tools | Language-specific check, lint and test dispatch in `mcp/src/verify.rs`. | The 1–3 second target is workload-dependent, not a universal guarantee. |
| 7.1 Refactoring | Rust custom planners plus language-server actions; both parameter operations also cover JavaScript and the other listed languages. | Most custom planners remain Rust-specific. Go signature reorder/removal is verified through CLI/MCP and real gopls (#448), with proof of unused parameters and safe dropped arguments; additions, modifiers and broader signature changes remain open. JavaScript parameter objects are added in #428. The #425 repair makes application transactional; #442 guards Rust signature effects, while #436/#441 preserve parameter-object evaluation and destruction order with explicit refusals for uncertain cases. The #446 repair blocks writes on failed/malformed/unreadable or unmatched required references, even with `force` and compiler verification; individual signature occurrences are reconciled, including ordinary block comments relocated by the analyzer (#472). The #456 conversion uses UTF-16 source columns and refuses split-surrogate, out-of-line and CRLF-splitting planner positions; the native editor clamps to valid boundaries. |
| 7.2 Fixes | Server code actions and compiler/linter fix modes. | The proposed `code_quickfix` spelling is not a separate tool; use the shipped assist/check/lint surfaces. |
| 7.3 Slicing | Bounded declaration traversal in `mcp/src/slice.rs`; #457 distinguishes missing evidence from depth/byte/declaration bounds and checks URI and actual UTF-16 source coordinates, verified through a real engine. | No intra-function program-dependence/data-flow slicing or proof of a minimal complete slice. |
| 7.4 Shadows | Overlay hypotheses and a serialized fallback in `gateway/src/shadow.rs`. | Real sccache/build-script regressions cover #426. Regressions for #440 cover failed staging, cancellation, metadata and symlink containment during rollback, including created directories; unsupported environments must not count as a passing integration scenario. |
| 7.5 Graphs | Callers, callees, implementations and supertypes. | Traversal is bounded and server-dependent; the advertised sub-5 ms whole-workspace target is not established for arbitrary repositories. |
| 7.6 Schemas | Analyzer-assisted cross-language rename with text handling for schema formats. | This coordinates known references and schema names; it does not infer every external API consumer. |
| 7.7 Validation | Analyzer overlays, multi-file proposals and optional compiler verification. Unlinked Rust files (#467) and malformed required diagnostic reports (#470) fail edit validation; non-source proposals get an actionable refusal (#465). | Analyzer acceptance is not full compiler/borrow-checker proof; new Cargo targets need inclusion in an actual compiler check or an analyzer reload. Missing or stale push publications fail instead of becoming clean reports (#471), and numbered publications must exactly match the current text; unversioned ones retain only arrival-order evidence. Generic pulls require a full report without an error envelope (#479). Derive artifacts are separately labeled and excluded from the error count (#159; #424 confirmed this existing behavior). |
| 8.1 Impact | Changed ranges, incoming-call traversal and test selection in `mcp/src/impact.rs`. | The #434 repair includes direct test edits and falls back to a full suite on deleted/binary files, unreadable symbols, incomplete hierarchy queries or depth truncation. Selection still depends on the language server's semantic reachability. |
| 8.2 Diagnosis | Failure text, source sites, caller context, diffs and ranked suspects in `mcp/src/dossier.rs`. | Structured printed assertion evidence now covers Rust equality macros and supported Node assert output (#445), with per-failure attribution and raw excerpts. Unsupported/ambiguous formats remain text; there is no debugger-style runtime capture. |
| 8.3 Dependencies | Read-only dependency/SDK source retrieval with gateway root restrictions. | This audit did not rerun every SDK/toolchain combination. |
| 8.4 Search | Lexical and optional dense ranking in `gateway/src/search.rs` and `embed.rs`. | The measured ranking is imperfect; typed graph fusion and universal sub-10 ms latency are not established. |
| 8.5 Fixtures | Rust value generation with explicit fallback types in `mcp/src/fixture.rs`; typed builder previews for named structs in `mcp/src/fixture/builder.rs` (#459, #497), including ordinary lifetime, type and const parameters with bounds and defaults, through CLI and MCP. | Type resolution refuses ambiguous declarations (#461); separately indexed re-exports require a declaring-file hint. Builders require every field, verify names and generated code, and write no files. `Self`-dependent bounds, macro-expanded generic syntax, nontrivial const expressions, conditional shapes, factory-call rewriting, general mocks, randomized generation and other-language tools remain open. |
| 8.6 Pruning | Reference-count candidates and analyzer safe-delete in `dead_code.rs` and `prune.rs`. | This is not entry-point reachability. The #435 repair marks failed and malformed reference/symbol queries unverified and keeps them during pruning; attribute-only references remain a documented limitation. |
| 8.7 Codemods | Rust structural search/replace through the analyzer. | Other-language codemods and the broad sub-second migration target remain open. |

Paths shortened above are under `crates/prod-code-*`. The main integration evidence lives in
the gateway live tests, MCP orchestration/analysis tests, native parameter tests and client CLI
tests. Unit and mock coverage complements real-server checks; it does not replace them.

## Checkbox reconciliation

The following 14 checkboxes changed from `[x]` to `[~]` (#480). Their delivered behavior and recorded
evidence remain above; the original mechanism or required scope remains open.

- **1.1**: length framing, compatibility defaults and loopback TCP do not implement NUL framing,
  negotiated capabilities/version, or Unix-socket/named-pipe transport.
- **1.3**: editor restart is not in-process reconnect, and the record does not establish
  zero-allocation hot paths.
- **3.3**: exit detection and request timeouts are not the specified periodic health ping.
- **3.6**: copied virtual environments and bundled typeshed are not a shared virtual-environment
  stub cache.
- **5.1**: client-directed placement avoids, rather than implements, the gateway dispatcher and
  `WireMessage::Redirect` mechanism.
- **5.2**: seed-address gossip/cache discovery is a substitute for, not an implementation of,
  DNS/mDNS/SRV publication.
- **5.4**: per-workspace out-of-process macro servers are not a sandboxed shared worker farm.
- **6.3**: seeded per-worktree artifacts and per-user caches are not a shared read-only dependency
  artifact cache.
- **7.1 `extract_function`**: token-matched copies, with optional other-file search, are not
  automatic workspace-wide structural duplicate detection.
- **7.1 `extract_trait` / `extract_interface`**: imports for moved methods do not update caller
  type annotations, and generic implementations are refused.
- **7.1 `loop_to_iterator`**: the three documented accumulator shapes and assists do not cover
  the promised general `for`/`while` iterator/fold transformation.
- **7.4**: overlayfs shadows live beside storage by default and may fall back in place; RAM storage
  is optional, not the specified in-memory RAM workspace mechanism.
- **7.5**: `code_dead_code` counts references; it is not whole-program reachability.
- **7.6**: planning and direct all-or-rollback writes are not an atomic multi-repository
  `WorkspaceEdit`.

Other completed checkboxes remain unchanged when their stated language/input restrictions have
attributable implementation and verification evidence; this reconciliation does not turn a lack
of a fresh audit rerun into a failure.

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
