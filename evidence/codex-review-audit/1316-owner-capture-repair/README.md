# Issue #1316 — nested owner and implicit capture repair

Effective acceptance base: `4f2943bc1bcf6312aa81b393d97827cb5cca449b`.
Inherited implementation commit: `07385118f924e09afdc683b4efedc1faab610ff2`.
This audit supersedes the inherited final-state claims and hashes. Historical
evidence is retained in the two sibling audit directories.

All seven linked original Codex comments were read through the public GitHub API,
with their complete bodies fetched using `--jq .body`: 4079089180, 4079089186,
4087082287, 4087082300, 4087257719, 4087257728, 4088035140. Changes remain limited
to the Rust method transformation implementation, directly affected fixtures and
audit evidence. No polyglot source, dependency, installation, publication, merge,
other worktree, child agent or background worker was involved.

## Reproduce before production edits

The review supplied two hypotheses. First, treating a comparison inside a nested
impl body as a generic opener would lose its owner span and rewrite its `Self`.
If false, the preview would retain runtime 8. Second, leaving an implicit format
capture untouched would bind it to the destination receiver. If false, the
preview would still print `Order(2)`.

`before.txt` records the actual sources, successful rustc exits and runtime
counterexamples through the public planning API: nested-owner output changed
8 -> 16; receiver format output changed `Order(2)` -> `Tax(3)`. Both tests failed
before production edits (terminal exit 101). Compiler acceptance alone therefore
does not prove equivalence. Paths, private addresses and environment details are
sanitized; fixture source bytes and runtime values are retained.

## Repairs and controls

- `crates/prod-code-mcp/src/move_method/syntax.rs:228` switches to lexical brace
  matching after a nested owner's body begins. Expression comparisons, shifts
  and generic arguments no longer influence the body's closing brace. The
  existing opaque-token scanner still excludes literal and comment braces.
- `crates/prod-code-mcp/src/move_method/captures.rs:16` refuses macro literals
  with possible implicit captures of the old receiver or selected destination
  binding before constructing edits. It decodes ordinary string escapes for
  inspection, accepts raw strings, respects escaped format braces, and skips
  independent nested owners. Literal bytes are never rewritten by this guard.
  Normal planning, normal application, forced planning and forced application
  all refuse the unsupported capture; unrelated files remain byte-identical.
- `crates/prod-code-mcp/tests/rust_method_preservation/owner_capture.rs:14`
  verifies runtime 8 -> 8 through preview and apply for both associated and
  receiver moves. The capture refusal matrix starts at line 56 and includes
  ordinary/raw captures, an escaped opening brace, a Unicode-escaped binding
  name, and destination-parameter captures. The positive case at line 104
  preserves positional formatting, plain and byte literal data, escaped braces
  and a nested owner's independent implicit receiver capture.

`after.txt` contains the complete sources, rustc exits and runtime values for
these three focused tests (terminal exit 0). Original, preview and applied
positive outputs are identical. Formatting was the only later source change.

The inherited owner/path/generic, deref-wrapper dispatch, literal/comment,
nested-binding, recursion/function-value, explicit-lifetime, binding-cursor,
failed-reference, unsupported-reference and effectful-force controls remain in
the acceptance target. None were weakened. The earlier directly affected
orchestration evidence remains historical; those tests were not rerun on this
repair. No entire orchestration target or unrelated test suite ran.

## Final gates and limitations

Changed-file rustfmt exited 0 (`format.txt`). The mandatory workspace formatting
check exited 1 (`format-check.txt`); its diagnostic bodies exactly match the
inherited check after path/address normalization. Crate lint exited 1 (underlying
clippy 101), reporting 210 errors; all 175 displayed diagnostics exactly match
the inherited evidence. The CLI omits 35 diagnostics, so exhaustive clean lint
is not claimed. `verification-comparison.txt` records these comparisons.

After formatting and lint, the exact required acceptance command exited 0 on a
Linux x86_64 build node: 21 passed, 0 failed (`acceptance.txt`). This was the one
final compile/test gate. Commands and discovery failures are recorded in
`commands-and-failures.txt`; unchanged successful checks were not repeated.

Capture refusal is deliberately conservative: any macro string resembling a
capture of a changing binding is refused without macro expansion proof, even
if a custom macro treats it as data or an explicit named argument supplies it.
Positional formatting, escaped braces and ordinary non-macro literal data remain
supported. Independent nested owners retain their own captures. The fixtures
use cursor-specific scripted protocol answers, not a live analyzer; actual
rustc and runtime checks are independent of scripted empty diagnostics. Other
inherited limitations documented in the sibling audit still apply.

Changed/new production and focused test source remains at most 400 lines per
file. No commit was created by this repair; task integration owns the result.
`hashes.sha256` covers every changed/new file against the effective base,
including inherited evidence and this audit, except this manifest itself.
