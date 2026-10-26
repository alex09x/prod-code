# Issue #1316 — remaining owner boundary review findings

Effective acceptance base: `61107a98e959d8d6517a1d67d5f84abcc5adc166`.
This continuation started clean at `1080f1184044d2db7d122c91d395596cab96a7be`.
The inherited repairs for nested impl Trait, unary negation/never types and
the selected mutable binding cursor remain intact, together with all seven
original issue fixes. All earlier audit artifacts and failed runs are preserved.

## Fresh public reproductions before production edits

`before.txt` records terminal exit 101: 11 passed, 2 failed. Prior reviewer
probes were treated as historical until this new execution. Public
`move_associated_function` previews and applications compile successfully but
change runtime from 8 to 16 after a struct-field comparison, and from 9 to 16
when the independent owner's generic argument contains `ty!(a > b)`.
Normal and forced modes both fail. Exact sanitized sources, rustc exit statuses
and actual runtime outputs are retained. Failures are at lines 1151–1156.

The same run freshly re-executes all three original regressions successfully:
nested opaque return types retain runtime 8; ordinary negation/never blocks
retain plain literal bytes; the mutable binding queries (4,21), rather than
the keyword at (4,19), and retains runtime 6. The original failing reproductions
remain in `../1316-final-regressions/before-confirmed.txt`.

## Bounded repairs

`crates/prod-code-mcp/src/move_method/context.rs:56` only lets a colon enable
type-header context within an existing function header or a standalone type
fragment. A struct-field initializer inside a body cannot enable that state,
so its comparison cannot leave a spurious unmatched generic delimiter that
hides a subsequent independent owner. Both rewrite and ownership scanners
continue using the same context decision.

`crates/prod-code-mcp/src/move_method/syntax.rs:196` uses the existing macro
context tracker while finding a complete nested owner. Macro argument token
trees are opaque to Rust generic delimiters, including parentheses, brackets,
braces and nested token trees. Unmatched closers outside macros still reject
incomplete declaration-shaped input. No parser dependency or macro expansion
was introduced.

The new public runtime matrix is in
`crates/prod-code-mcp/tests/rust_method_preservation/owner_boundary_regressions.rs:45`
and `:55`. It covers four initializer expressions and four macro token-tree
forms, each with normal/forced preview and application. Preview checks preserve
source, manifest and sentinel bytes. All compiler/runtime assertions remain.
Scripted gateways provide cursor-specific public protocol responses; the
compiler and generated programs are actually executed, not simulated.

## Final verification

`acceptance.txt:3375` records terminal exit 0: 28 passed, 0 failed, on a Linux
x86_64 build node. The final requested rust_method_preservation target preserves
runtime 8 for independent owners following field initializers and runtime 9
for owners with macro-produced generic arguments, across every matrix case.
All inherited normal moves, dispatch/type safety, literal/comment/raw data,
recursion, lifetime, failed-reference and effectful-call controls pass.
The environment flag only retains successful stdout; the Cargo target and
selection match acceptance. No unrelated test target ran.

Changed-file rustfmt exited 0. Workspace formatting exited 1; its diagnostic
bodies match inherited evidence. Crate lint exited 1 (clippy 101), reporting
210 errors; all 175 displayed diagnostic lines match inherited evidence.
The CLI omits 35 diagnostic bodies, so exhaustive clean lint is not claimed.
See `verification-comparison.txt`, `format-check.txt` and `lint.txt`.

`commands-and-failures.txt` records sanitized commands and availability errors.
`hashes.sha256` covers every changed/new file relative to the effective base,
excluding itself. Source modules remain below 400 lines. No polyglot sources,
dependencies, installs, publication, other worktrees, delegation, background
providers, commits or merges were involved. The task integrator owns the result
commit. This remains a bounded lexical repair, with conservative refusal of
ambiguous macro owners and captures.
