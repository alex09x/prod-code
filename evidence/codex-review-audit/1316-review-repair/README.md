# Issue #1316 — semantic review repair

Effective task base: `4f2943bc1bcf6312aa81b393d97827cb5cca449b`.
Inherited previous-attempt commit: `2de4a84dc4fe73c41feed8df99ae96398076b2e3`.
This audit supersedes the previous attempt's final-state claims and hashes; its
historical evidence remains in `../1316-rust-method-preservation/`.

All seven original Codex comments were re-read using `gh api` with `--jq .body`:
4079089180, 4079089186, 4087082287, 4087082300, 4087257719, 4087257728,
4088035140. The work remains limited to Rust conversion helpers/root modules,
method movement, directly affected tests and audit artifacts. No polyglot source,
dependencies, installations, other worktrees, delegation, publication or merge.

## Measured failures before production changes

The hypothesis for recursive shadowing was that a bare destination name resolves
through local imports. If false, the preview would still print 2. Instead both
programs compiled and output changed 2 -> 100. The scanner hypothesis was that an
`impl` token in macro arguments consumed source through the next semicolon. If
false, macro `Self` would retain its original owner and size. Instead both
associated programs compiled and output changed 8 -> 0. See `before-initial.txt`.

The first receiver probe used a unit destination and exposed a rustc error from
the unchanged `self.0`. Its fixture was then made type-correct on both sides
(destination `Tax(u32)`, original `Order(u64)`). The exact rerun demonstrated
runtime 10 -> 7; see `before-receiver-runtime.txt`. A related associated return
type `impl Iterator` also incorrectly introduced an owner span: both programs
compiled and output changed 8 -> 0, in `before-impl-trait-runtime.txt`.

Commands, exit statuses and discovery failures are in `commands-and-failures.txt`.
Each runtime log includes exact fixture source, rustc status and executable
stdout. Environment paths and private addresses are sanitized; fixture code and
values are retained. The first three before probes failed as expected (exit 101),
as did both subsequent one-test baseline probes.

## Repairs and behavioral proof

- Internal recursive references use the destination's crate-qualified module
  path, preventing local imports or type aliases from capturing its bare name.
  Both recursive calls and function-value references follow that path.
- Nested-owner scanning requires a complete owner-shaped span, does not cross a
  macro token tree's closing delimiter, and distinguishes return-position
  `impl Trait` (including intervening comments) from an owner declaration.
- Declaration-shaped macro tokens have ambiguous ownership without expansion.
  Both move entrypoints refuse them before planning or writing, even with force.
  Ordinary keyword/type macro arguments in parentheses, brackets and braces are
  still transformed. Existing nested real owners retain their own Self/receiver.
- One opaque-token helper handles ordinary/raw/byte/raw-byte data consistently
  during rewriting, owner validation and owner-span scanning.

`after-runtime.txt` records all 18 focused tests, including runtime 2 -> 2,
8 -> 8, 10 -> 10 and return-position impl Trait 8 -> 8. The six new review tests
exercise public planning/apply APIs, real rustc and execution; ambiguous-owner
refusals assert byte preservation under normal and forced application. The
existing twelve tests retain full owner/generic identity, wrapper dispatch,
literal/comment bytes, nested binding identity, lifetime/cursor handling,
recursion/function values, failed-reference refusal and effectful-force controls.

The two associated orchestration tests and one receiver orchestration test pass
on this semantic code state (logs alongside this file). No complete orchestration
target or other broad test suite ran. Only formatting changed Rust source after
these focused runs.

## Final gates and limits

Final changed-file rustfmt exited 0 (`format.txt`). One workspace format check
exited 1 on existing unrelated formatting (`format-check.txt`). One crate lint
exited 1, underlying clippy 101, with 210 errors (`lint.txt`), the same count as
the inherited attempt. Displayed diagnostics are in unchanged code; the edited
to_method helper diagnostic is its unchanged receiver_argument lifetime. The CLI
omits 35 diagnostics, so no exhaustive clean lint claim is made.

After final formatting/lint, the exact required acceptance command exited 0:

`prod-code exec --remote <linux-build-node> --timeout-secs 900 -- cargo test -p prod-code-mcp --test rust_method_preservation`

`acceptance.txt`: 18 passed, 0 failed, Linux x86_64 build node. This is the final
compile/test gate. The runtime log precedes final rustfmt; acceptance recompiles
the formatted state. Successful unchanged checks were not repeated for reassurance.

The protocol fixtures use cursor-specific ScriptedGateway answers, not a live
rust-analyzer session. The actual compiler/runtime checks are independent of its
empty diagnostic responses. Alias/alternate-path equivalence is conservatively
refused for receiver eligibility. Receiver-method recursion/values and unsupported
angle-qualified associated references remain explicitly accounted refusals.
Macro owner expansion is conservatively refused as described above. No polyglot
behavior was changed or verified. Existing unrelated lint/format failures remain.

All substantively edited production/new test source is <=400 lines; the existing
orchestration fixture retains its established structure. No result commit is
created here; task integration owns it. `hashes.sha256` covers all final changed
and new files against the effective base, except the manifest itself, including
this audit and historical evidence. Paths are repository-relative.
