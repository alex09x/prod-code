# Issue #1316 — owner-header review follow-up

Effective acceptance base: `61107a98e959d8d6517a1d67d5f84abcc5adc166`.
This attempt started clean at retained candidate
`87631a285ad715c98af00b61fc48a4b105355887`, which already repairs the three
original remaining regressions. Earlier audit artifacts remain unchanged.
Reviewer probes were treated as historical until these public reproductions ran.

## Fresh public evidence before production edits

`before.txt` records a focused `regressions` filter (exit 101, 10 passed,
1 failed). It reruns the original three public regressions successfully:
nested impl Trait retains runtime 8, negation/never blocks retain literal data,
and the mutable destination binding queries (4,21), not the `mut` keyword at
(4,19), and retains runtime 6. The inherited failures remain in
`../1316-final-regressions/before-confirmed.txt`.

The new const-owner test fails: originals compile and print 8; normal/forced
preview and application compile but print 16. The independent Inner impl's Self
was rewritten to Order. The matrix covers `2 > 1`, `1 < 2`, a right shift, and
nested braces. Preview checks retain source, manifest and sentinel bytes.

The initial macro fixture defined its macro inside the moved body, triggering
the existing declaration guard before reaching the invocation. Moving that
definition outside the body exposed a fixture-ordering compiler failure because
the destination impl was inserted before the macro. `before-macro.txt` preserves
the compiler rejection. No compiler/runtime assertions were removed.

With the definition placed before both types, `before-macro-confirmed.txt`
records the actual semantic failure (exit 101): originals compile and print 0;
preview and application compile and print 8, in both normal and forced modes.
The forwarding macro accepts declaration input preceded by a colon, an arrow,
or a function-shaped prefix. All three forms bypassed the ownership guard.

## Repair and final evidence

`crates/prod-code-mcp/src/move_method/syntax.rs:224` skips braced const arguments
as balanced opaque-aware blocks. Expression comparisons and shifts cannot alter
the surrounding owner header's generic delimiter stack.

`crates/prod-code-mcp/src/move_method/context.rs:47` keeps macro token trees
outside inferred Rust type/header context. Macro colons, arrows and fn tokens
cannot authorize impl Trait classification. Both the owner/capture scanners
and the rewriting scanner use this shared context decision.

The public regressions are in
`crates/prod-code-mcp/tests/rust_method_preservation/owner_header_regressions.rs:14`
and `:41`. They use cursor-specific scripted gateway responses and actual rustc
and runtime execution, rather than a live analyzer. This is a bounded lexical
repair, not a Rust parser or macro expander.

`acceptance.txt` records the final formatted source state on a Linux x86_64
build node: terminal exit 0, 26 passed, 0 failed. Successful-test stdout is
enabled only to retain exact original/preview/applied sources and runtime
output; the Cargo test target is unchanged. Const-owner moves retain 8, macro
owners refuse all twelve mode/prefix combinations and preserve all workspace
bytes, and the original three regressions and full inherited controls pass.
No unrelated target or broad suite ran.

Changed-file rustfmt exited 0. Workspace format check exited 1; its normalized
diagnostic bodies match inherited evidence. Crate lint exited 1 and reports
210 errors, with all 175 displayed diagnostics identical to inherited evidence;
35 bodies are omitted by the CLI. See `verification-comparison.txt`. Neither
failed check is reported as clean or waived.

`commands-and-failures.txt` records exact sanitized commands and tool failures.
`hashes.sha256` covers every changed/new file relative to the acceptance base,
excluding itself. Source modules remain below 400 lines. No polyglot changes,
dependencies, installs, external publication, merge, other-worktree edits,
delegation, background providers or manual commits occurred. Task integration
owns the result commit.
