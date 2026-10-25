# Issue 1330: module-boundary repair and fresh final verification

This retry retains the prior route/placement/error-chain repair and fixes the
review's remaining module-scope leak. These records supersede the prior attempt's
final test count. The prior attempt's referenced `.log` files are not tracked;
this directory preserves this attempt's actual captured output in `.txt` files.

## Reproduction and demonstrated cause

Before production edits, the new regression created a portable Go module at
`tools/plain` containing an independent Apple-only cgo module at
`tools/plain/apple`, inside a Rust checkout. The unchanged cgo directory walk
descended through the child's `go.mod`. Routing the portable parent therefore
required macOS, even with a responding Linux Go node. Expected: route the parent
to Linux, while direct child requests still require macOS.

`before.txt` records the exact sanitized command/output and observed exit 101:
seven inherited regressions passed; the new parent-module regression failed
with `no reachable gateway runs macOS`. Only regression tests had changed.
`source-hashes.json` records the pinned base, inherited attempt, and final source
hashes; the inherited cgo helper equals the pinned helper before this repair.

The repair excludes descendant directories containing their own `go.mod` from
the cgo scan. The selected scan root is still inspected. Regular subpackages
without `go.mod` remain included, as exercised by the nested cgo regressions.
The new regression checks portable parent directories/files through compatible
default and fallback routes, including Linux as the only available Go node.
It also checks direct Apple-child routing through both selection branches.

## Execution and simulation boundaries

Compilation and unit execution ran on a Linux build node through `prod-code`.
`<linux-build-node>`, `<remote-checkout>`, and `<assigned-worktree>` replace private
addresses/paths in captured records. The macOS/Linux platform strings, engines,
pressure, and placements are simulated responses from ephemeral loopback fake
endpoints. They are not evidence of execution on a live macOS node. Tests use
mutex barriers and request counters; the new regression introduces no sleeps.
No real placement/configuration changes, restarts, deployments, publications,
dependencies, installations, messages, or child workers were used.

## Final checks and limitations

`format.txt`: scoped remote rustfmt, observed exit 0, after the coherent repair.
All seven changed source modules are at most 303 lines, with attribution headers.
`after.txt`: required cluster unit gate, observed exit 0: **46 passed**, including
all eight issue regressions. Existing engine refusal, reported-pressure,
placement, ordinary Go, Swift, and nested-project cases also ran. No broader
suite or overlapping build gate was run, and successful checks were not repeated.

`lint.txt`: exact JSON output from scoped strict crate lint, CLI exit 1 and
compiler exit 101. All 210 structured diagnostics name 53 files, all byte-identical
to the pinned base; none names a changed source file. The complete structured
diagnostics are retained, unlike the prior attempt's truncated text display.
`lint-baseline-files.json` records these comparisons. Strict lint did **not** pass;
no pristine-base rerun or unrelated lint repair was performed. This comparison
does not establish that the unchanged files' diagnostics are independent of every
possible cross-file interaction. No warning suppression was added.

`tool-failures.txt` preserves the blocked discovery call and its bounded fallback.
The expected red reproduction and strict lint failure are retained separately.
