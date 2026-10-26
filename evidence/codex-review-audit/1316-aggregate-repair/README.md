# Issue 1316: complete Rust method preservation repairs

Pinned base: `1fadae441d74e155d219bef7829817c8f5c59575`.
Execution node: Linux x86_64. Commands and terminal statuses are in `commands.json`.
Node addresses, remote checkout paths and randomized fixture directories are normalized
in the logs; Rust source and program stdout are retained verbatim.

## Observed before production edits

`before-owner.txt` runs the public associated-function move for `r#impl`, `r#trait`
and `r#mod` struct literals and a genuine nested `impl Inner<[u8; 1 << 2]>`.
Every original compiles and prints `8`. Ordinary/forced preview/apply all compile,
but print `0` for the raw literals and `16` for the nested owner (16 bad results).
This turns both source-derived review predictions into runtime observations.

`before.txt` also records zero caller conversions for two exact-owner calls, with
runtime `3 7` preserved. Its first owner fixture hit a separate item-removal panic
because of its one-line layout. The multiline, global-struct fixture in
`before-owner.txt` corrected that probe before production edits. Both attempts remain.

`controls-before.txt` records four passing controls: qualified Wrapper dispatch,
failed-reference byte preservation, unsupported-reference refusal, and ordinary
effectful-call refusal with explicit forced construction.

## Repairs and focused evidence

The owner scanners share `Word` token identity and its `owner_end` decision across
rewriting, ownership validation and implicit-capture checks. Raw keywords cannot
open owners. Both type/header scanners use `TypeDelimiters` to distinguish an
array's element type from its length expression. Genuine nested owners keep Self.

Caller conversion requires the full receiver type, one matching nominal definition,
and resolution of the proposed dot call to the converted method in an unsaved
analyzer overlay. Definition failure, an uncertain receiver, or a different target
retains the qualified call, including under force. The bounded receiver proof
supports bindings and built-in slice/array indexing by an integer literal.

`acceptance.txt` records all 36 preservation tests passing. The new fixtures exercise
ordinary/forced preview/apply, both move entrypoints, raw literals that hide implicit
captures, exact-owner call conversion, Wrapper/different-identity/generic uncertainty,
and failed or conflicting overlay definitions. Every emitted program is compiled
and executed by real rustc. The exact-owner positive fixture converts two calls and
keeps `3 7`; the trait control demonstrates blind dot dispatch changing `3` to `99`,
while the guarded transformation keeps `3`.

The LSP responses are scripted, including type/definition identity and proposed-call
definition responses. This is semantic-response fixture evidence plus real compiler
and runtime evidence, rather than a live-analyzer end-to-end claim.

`orchestration.txt` records the one affected positive fixture passing, with two
conversions (`c.bump(2)` and `items[0].bump(3)`), qualified recursion and a function
value retained. `format-check.txt` records scoped rustfmt check exit 0.

## Strict lint limit and retained failures

`lint-final.txt` records strict clippy exit 101 with 103 diagnostics. Every primary
diagnostic source line is present unchanged in the pinned baseline; the comparison
is in `lint-baseline-comparison.json`. This is a failed strict lint gate, not a clean
lint result. The unchanged helper lifetime warning is included in that limit.

`lint-initial.txt` and `lint-overlay.txt` retain earlier diagnostics; the changed
context branch and new needless borrow were repaired. `acceptance-initial.txt`
retains 35 passes plus the trait fixture selecting its first trait declaration;
the fixture now explicitly selects the inherent function. `orchestration-initial.txt`
retains the missing macro-qualification compile error, fixed in the scoped retry.
`tool-failures.txt` preserves navigation/edit-tool failures and bounded fallbacks.

`source-sha256.json` hashes pinned and final source files. `runtime-sha256.json`
indexes exact fixture source/stdout hashes and log lines. `artifact-sha256.json`
hashes this evidence set. Earlier tracked audit evidence has no diff against HEAD.
