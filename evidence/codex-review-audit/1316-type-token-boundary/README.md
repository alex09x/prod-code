# Issue #1316 — complete type token boundaries

Acceptance base: `ec35bc59b2a3d5b38ff0f448c805757f691d1ea0`.
The assigned worktree started clean. All preceding audit artifacts and the
independent approval reports remain unchanged. No manual commit was made;
managed task integration owns the result commit.

## Public reproduction before production edits

`before.txt` is fresh public `convert_to_method` evidence, not the parent's
earlier rustc-only prediction. The exact requested one-line fixture compiles
and prints `1`. Normal/forced preview and application all accept the receiver,
compile the actual returned source, and print `2`. The qualified caller remains
valid by reference unsizing. The focused regression exits 101 after collecting
all four failures. Both original and actual transformed sources are retained.

The observed source failure was concatenation of distinct token sequences:
`S<dyn Trait>` and `S<dynTrait>`. Hypothesis: losing this boundary authorizes the
wrong declaration even without any caller conversion. Contradicting evidence
would have been a refused declaration or unchanged runtime. The scripted
gateway supplies binding/call references and empty diagnostics, with no nominal
identity or overlay-dispatch proof. Public conversion changes runtime anyway.

## Repair

`crates/prod-code-mcp/src/to_method/helpers.rs:107` reuses the existing Rust
fixture lexer and compares token spellings with significant jointness. It
preserves literal prefixes/suffixes, Unicode continuations, numeric exponent
boundaries and macro punctuation, without concatenating identifiers. Ordinary
comments and whitespace are trivia; nested generic closers and nested
references can be split in type grammar. Macro/expression groups retain
punctuation jointness. Lexical failures and doc-comment attributes fail closed.

`helpers.rs:72` uses comment blanking only to locate the self type and returns
the original slice, so meaningful comments cannot disappear before comparison.
`helpers.rs:57` refuses a different receiver before any planned write, including
forced application. `crates/prod-code-mcp/src/to_method/dispatch.rs:261` uses the
same fallible token identity and does not equate two failed scans.

Public regression entry points are in
`crates/prod-code-mcp/tests/rust_method_preservation/type_identity.rs:43`, `:81`
and `:108`. They retain normal/forced preview/application, byte equality of
source/manifest/sentinel on refusal, and actual rustc/runtime execution. The
literal prefix, raw-byte prefix, literal suffix and punctuation cases use type
macros to produce different complete instantiated types. Positive controls
cover trait objects with nested comments, generic owners, function-pointer
types, and whitespace between nested generic closers.

## Verification and limitations

`targeted.txt` retains an intermediate failure: the initial const-macro fixture
hit the existing impl-header lookup limit before receiver comparison; a generic
impl following a semicolon on the same line also missed impl lookup. The
fixtures were repaired to use type macros and an item newline, without changing
that unrelated scanner. `targeted-repaired.txt` records 3 passing focused tests
before the final nested-generic positive control was added.

`format.txt` records one changed-file rustfmt run, exit 0. `lint.txt` records the
scoped crate lint, exit 1, reporting 210 errors. `lint-comparison.txt` confirms all
175 displayed diagnostics match the retained baseline after removing line/column
shifts. The CLI omits 35 diagnostic bodies; no claim is made about those bodies.
The lint failure is not reported as a clean check.

`acceptance.txt` records the final formatted source state on a Linux x86_64
build node, exit 0: 41 passed, 0 failed. The requested Cargo target is unchanged;
the remote `RUST_TEST_NOCAPTURE=1` setting only retains successful test output.
The exact new original/refused/converted sources, rustc exits and runtime output
are included. The original dynamic fixture now refuses all four modes, retains
all workspace bytes and still prints `1`. Equivalent trait-object conversion
prints `2`; the other positive type controls print `7`. The restored
`aggregate_exact_receiver_calls_convert_with_semantic_identity` case and every
inherited owner/capture/recursion/lifetime/cursor/comment/overlap/dispatch
control pass in the same run.

This is conservative lexical identity, not a semantic type-equivalence checker
or macro expander. Raw/plain identifier aliases and unusual numeric/punctuation
spellings may be refused rather than proven equivalent. Doc-comment attributes
inside types are conservatively refused. Gateway responses in the public tests
are scripted; rustc and runtime execution are real. Existing impl-header parsing
limits remain. No generic parser corpus, dependencies, polyglot changes,
installations, publications, messages, other worktrees, delegation, detached
providers, infrastructure changes, or broad test suites were introduced.

`commands-and-failures.txt` records exact sanitized commands and navigation
failures. `hashes.sha256` covers all changed/new deliverable files except itself.
All changed source modules have attribution headers and remain below 400 lines.
