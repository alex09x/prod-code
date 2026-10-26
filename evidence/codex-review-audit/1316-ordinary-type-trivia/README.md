# Issue #1316 — ordinary type punctuation trivia

Acceptance base: `ec35bc59b2a3d5b38ff0f448c805757f691d1ea0`.
This repair started from clean commit `3422bae793eb8392d8f5f9ffa67096a9aa69a6ab`.
All earlier evidence and independent approval reports remain byte-for-byte
unchanged. In particular, `1316-type-token-boundary/before.txt` retains the
original public converter's actual `1` to `2` runtime change, distinct from the
parent's rustc-only prediction. That reproduction was reused, not rerun or
relabeled as new evidence in this follow-up.

## Fresh public reproduction

Before editing production code, `before.txt` records a focused public
`convert_to_method` regression: `impl S<&'static u8>` with parameter
`c:&S< &'static u8>` compiles and prints `7`, but conversion rejects the receiver.
The command exits 101. The hypothesis was that jointness of the adjacent `<`
and `&` punctuation incorrectly participates in ordinary type identity. A
successful conversion before the repair would contradict that hypothesis.

## Repair and runtime evidence

`crates/prod-code-mcp/src/to_method/helpers.rs:131` distinguishes punctuation
consumed separately by ordinary type grammar from compound path separators,
function arrows and variadics. Macros and expressions still retain all
punctuation jointness. Token boundaries, literal prefixes/suffixes, numeric
boundaries, doc-comment refusal and complete instantiated-type comparison are
preserved. The pre-write receiver refusal and caller/owner dispatch protections
are unchanged.

`crates/prod-code-mcp/tests/rust_method_preservation/type_identity.rs:127`
adds five public positive cases: whitespace/comment gaps before references,
a pointer, a reference tuple element, and a function-pointer return reference.
All exercise preview and application, each with and without force. The original
and actual returned programs compile and print `7`. Preview preserves all
workspace bytes; application writes precisely the returned source and preserves
the manifest and sentinel. Qualified callers remain qualified where the public
converter requires coercion/dispatch preservation; these tests do not claim
their callers became method syntax.

`after.txt` retains an intermediate fixture failure: the pointer initializer
cast `&7` with inferred `i32` to `*const u8`. The bounded fixture repair adds
the explicit `u8` suffix (also made explicit in the function-pointer fixture).
`after-repaired.txt` records the corrected focused test, exit 0, including exact
sanitized original/converted source, compiler exits and runtime output.

## Final verification and limitations

`format.txt` records one scoped rustfmt invocation, exit 0. `lint.txt` records
one scoped crate lint invocation, exit 1, reporting 210 errors. All 175 displayed
diagnostic bodies match the preceding retained lint log after source-coordinate
normalization (`lint-comparison.txt`). The CLI omits 35 bodies; those were not
compared. Lint is not claimed to pass.

`acceptance.txt` records the exact required Cargo target, exit 0, on a Linux
x86_64 build node: 44 passed, zero failed. This is the final compile/test gate
after formatting and lint. It includes distinct `dyn Trait`/`dynTrait` refusal
with byte preservation in normal/force preview/application, macro literal and
punctuation refusal, equivalent trait-object/generic/slice conversion, restored
exact receiver caller conversion, Wrapper/trait dispatch and all inherited
owner/capture/recursion/lifetime/cursor/comment/overlap controls. There was no
additional build or broad suite.

Comparison remains conservative lexical identity, not semantic equivalence or
macro expansion. Existing impl-header parsing limits, raw/plain alias and
unusual spelling refusals remain. Gateway responses are scripted; compiler
and runtime execution are real. Changed source modules retain their attribution
headers and contain 334 and 218 lines, respectively. No dependencies, polyglot
edits, installs, publications, messages, delegation, other worktrees or
infrastructure changes were introduced. Managed task integration owns the
result commit; this worker made no commit.

Logs consistently replace the endpoint, remote checkout and temporary fixture
directory with `<build-node>`, `<remote-workspace>` and `<fixture-workspace>`.
Fixture source and runtime values are otherwise exact. Commands, tool failures,
terminal exits and bounded fallbacks are in `commands-and-failures.txt`.
`hashes.sha256` covers final changed source and new evidence (except itself);
`preserved-evidence.sha256` records unchanged preceding audit files.
