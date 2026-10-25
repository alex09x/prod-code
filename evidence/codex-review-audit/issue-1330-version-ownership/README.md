# Issue 1330: requirement versions and dependency test ownership

Pinned acceptance base: `f6063f7399342f220c93a2013e8e99be82749633`.
Repair input: `3f4b719cc7e28a76173da8b8d93a672c450410b0`.
Earlier audit records, including the original local-replacement boundary reproduction,
remain unchanged. This pass repairs both findings from the preceding semantic review.

## Reproduction

`before.txt` records the exact sanitized focused command and observed terminal exit
101: five existing review tests passed and both new regressions failed before
production edits. Each failing assertion jointly captured the detector, public
checkout placement, compatible-default routing and Linux-only routing. Both
fixtures incorrectly reported an Apple-only dependency and rejected Linux:

- A Go 1.16 main module required a lower Apple version; its imported, contained
  bridge required a higher version whose exact replacement was portable. The
  first-write-wins requirement map selected the wrong replacement.
- A portable bridge dependency imported the Apple module only in its own tests.
  Those tests incorrectly expanded the selected main module's dependency imports.

`source-hashes.json` records the red-run production hashes, red fixture hash,
final source hashes and evidence hashes. The production hashes match the repair
input. The fixture was expanded after the red run, then formatted before final
validation; the final fixture is not claimed to be byte-identical to the red one.

## Repair and scope

`crates/prod-code-mcp/src/sync/engine/local_go.rs:66` resolves contained local
manifest requirements before source inspection can report an OS constraint.
Competing requirements merge by Go semantic version ordering, including numeric
components, prerelease identifiers, pseudo-version timestamps and build metadata.
The selected module's exact-version replacement still overrides its wildcard,
including remote overrides that are not fetched or inspected. Manifest discovery
and imported source discovery have separate bounded visited sets; only imports
authorize source scans. Unimported required manifests can contribute Go 1.16
requirements without their source introducing an OS constraint.

`crates/prod-code-mcp/src/sync/engine/cgo.rs:118` excludes dependency-owned tests
from import expansion. Selected-module test imports remain supported, and normal
dependency source imports still expand under the selected owner's replacements.
Directory visits retain both owner and selected/dependency status.

The existing canonical containment, independent child-module boundary, ignored
directories, byte/manifest/module limits and shared directory/file budgets remain.
No network or toolchain discovery was added to startup. This remains bounded
inspection of contained local replacements, not a full Go dependency solver.
Changed source files retain attribution headers and contain 323, 284 and 253 lines.

## Final verification

- `format.txt`: one scoped remote rustfmt invocation, terminal exit 0.
- `lint.txt`: single strict crate lint invocation, CLI exit 1/compiler exit 101.
  Its complete 210-diagnostic set is identical to the retained lint record; none
  names a changed file. `lint-baseline-files.json` verifies that all 53 diagnostic
  files are byte-identical to the pinned base. Strict lint did not pass; no broader
  cleanup or baseline rerun was performed.
- `after.txt`: exact requested remote cluster unit acceptance, terminal exit 0;
  66 passed, 0 failed, 1,363 filtered out. The three added tests cover competing
  versions with both replacement-tree assignments and both main/bridge version
  directions; transitive manifest closure before source scans; dependency tests
  at root/subpackage paths and ordinary dependency imports. Earlier single/block
  syntax, wrapper/go.work, nested selected-module OS constraints, compatible
  defaults, engine requirements, remembered-Mac alternatives, OS probe-error
  detail and pressure cases remain in the passing cluster acceptance.
- `tool-failures.txt`: exact hook-rejected discovery command, error and successful
  bounded fallback. All started commands reached terminal completion.

Compilation, formatting, lint and Rust tests executed on a real Linux build node.
Gateway status/platform, engines and OS probes were deterministic loopback fakes;
the fixtures were actual temporary contained go.mod/source files. No real macOS
execution is claimed. Private endpoint and remote-copy arguments are replaced
with descriptive placeholders in recorded commands/output. There were no topology
changes, config edits, installs, restarts, deployments, publication, messages,
delegation or unrelated test runs.
