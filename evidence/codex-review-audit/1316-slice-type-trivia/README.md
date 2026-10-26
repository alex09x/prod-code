# Issue #1316 — slice element token trivia follow-up

Acceptance base: `ec35bc59b2a3d5b38ff0f448c805757f691d1ea0`.
This repair started from the clean preceding attempt at `d0d4a0d7`.
All preceding evidence, including `1316-type-token-boundary/before.txt` with
actual public conversion changing runtime from `1` to `2`, remains unchanged.
That earlier public evidence is distinct from the parent's rustc-only probe.

## Observed failure and repair

Before changing production code, `before-repaired.txt` reproduces the reported
false rejection through public `convert_to_method`: the slice fixture compiles,
prints `1`, then its comment-separated nested generic closers cause the receiver
to be rejected. The same run freshly confirms the original `dyn Trait` versus
`dynTrait` fixture refuses all four preview/application and normal/force modes,
preserves every workspace byte, and continues to print `1`.

The cause was treating every bracket group as an expression. The predicted
failure was unequal punctuation jointness for `>>` versus `> >` in slice element
types; successful conversion before the repair would contradict that diagnosis.

`crates/prod-code-mcp/src/to_method/helpers.rs:133` now tracks the current
delimiter and inherited expression/macro context. Slice and array brackets
start with an element type; an array's `;` switches only that group to expression
context. Closing it restores the enclosing context. Macro bodies, including
nested brackets, keep punctuation jointness. Token spellings, meaningful
identifier/literal boundaries, fallible scanning, and pre-write refusal are
unchanged. No caller or owner protection was relaxed.

`crates/prod-code-mcp/tests/rust_method_preservation/type_identity.rs:127`
exercises actual public preview/application, each with and without force, for
slice element generic closers and nested references separated by comments.
Original and returned source both compile and print `1`. Preview leaves source,
manifest and sentinel bytes unchanged; application matches the returned source
and preserves the other files. The existing public macro refusal regression
also covers meaningful punctuation inside nested square brackets.

`type_identity.rs:145` directly checks the public receiver matcher for array
element trivia, restoration of type context after an array, and significant
macro punctuation in array lengths with both parenthesis and bracket delimiters.
These matcher assertions do not claim full public array conversion coverage.

## Verification and limits

`targeted-repaired.txt` records five passing focused tests with exact original,
refused and converted source, rustc exits, and runtime output. The final
`acceptance.txt` records the exact required Cargo target on a Linux x86_64 build
node: 43 passed, zero failed, terminal exit 0. It includes the restored exact
receiver caller conversion and all inherited owner/capture/recursion/lifetime/
cursor/comment/overlap/dispatch controls. The acceptance run follows formatting;
there was no overlapping final build/check/test command.

`format.txt` records scoped rustfmt, terminal exit 0. `lint.txt` records scoped
crate lint, terminal exit 1: 210 errors. `lint-comparison.txt` confirms all 175
displayed diagnostics equal those of the preceding attempt after removing line
and column shifts. The CLI omits 35 bodies, so those bodies were not compared.
Lint is not reported as passing.

`before.txt` retains a test-harness lookup failure before conversion; the harness
was corrected before obtaining the actual public reproduction. `targeted.txt`
retains intermediate full-conversion array fixtures that compiled but hit the
existing impl-header lookup limitation at semicolons before receiver matching.
Those fixtures were replaced by direct matcher assertions rather than expanding
this repair into unrelated header parsing. Exact sanitized failed commands and
errors are retained in `commands-and-failures.txt`. The repaired focused command
also included three already-green identity tests; this was a redundant rerun,
not additional coverage. No unchanged focused check was repeated after it passed.

Identity remains conservative lexical comparison, not semantic equivalence or
macro expansion. Existing raw/plain alias, unusual numeric spelling, doc-comment
attribute and impl-header limitations from the preceding report remain. Gateway
responses are scripted; compilation/runtime execution is real. All source
modules changed in this follow-up retain their attribution and are below 400
lines. No dependencies, polyglot edits, installs, publications, messages,
delegation, other worktrees, infrastructure changes or broad suites were added.
Managed task integration owns the result commit; this worker made no commit.

Logs replace the endpoint, remote checkout and temporary fixture directory with
`<build-node>`, `<remote-workspace>` and `<fixture-workspace>`. Fixture source and
runtime values are otherwise exact. `hashes.sha256` covers the final source and
new evidence files except itself; preceding manifests remain unchanged records
of their earlier source state.
