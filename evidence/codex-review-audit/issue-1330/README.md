# Issue 1330: route and placement repair

Base: `65c6e5a62e69fafae17820ae9548925b74711187`.
All commands ran from the assigned worktree. No publication, deployment, restart,
configuration change, real placement mutation, dependency change, or child worker
was performed. The delivery service owns the result commit.

## Execution and simulation boundaries

Rust compilation and unit execution ran on a real Linux build node through
`prod-code exec`. `<linux-build-node>`, `<remote-checkout>`, and
`<assigned-worktree>` replace private addresses and absolute paths in the exact
captured command/output records. Loopback addresses in assertion output belong to
ephemeral fake endpoints inside the unit-test process.

The macOS/Linux platform strings, engines, resources, and placement answers are
simulated protocol metadata, not evidence of execution on a live macOS node.
The returning Linux alternative is modeled by enabling its Go engine under a
mutex between awaited selections. The mutex is the response-state barrier.
Rendezvous keys deterministically query the Mac first; request counts prove the
stale recommendation was exercised. No new test uses sleeps. OS probe failures
use a bound, non-listening socket that keeps its port reserved throughout.

## Fresh reproduction before production edits

- `before-initial.log`: exit 101, four combined regression tests failed.
- `before-separated.log`: exit 101, six separated cases failed, exposing both
  default and fallback OS loss, remembered and recommended Mac preference loss,
  and single/multiple-node probe-context loss.
- `before-fallback.log`: exit 101, strengthened fallback fixture with a Rust-only
  default plus a separate Linux Go node selected Linux for macOS-only cgo.

Only test fixtures changed during these reproductions. The recorded pre-edit
production hashes match the pinned base. `source-hashes.json` also records the
last pre-production test snapshot hash and each final source hash. The test file
was subsequently extended and split; its final hash therefore differs.

## Repair and final results

`routing.rs:122` resolves the selected Go module from the actual requested path,
scans cgo only for Go work, and carries its OS through default and fallback
selection. `selection.rs:66` probes compatible, unpressured non-Mac alternatives;
both remembered and recommended placements apply that preference. Candidate
capacity filtering precedes Mac preference. `pick.rs:248` and `pick.rs:273`
preserve probe errors as contextual error chains, including the original I/O type.

`after-development.log` records six passing tests and a test-only failure: its
key did not guarantee the stale recommendation endpoint was queried first.
The deterministic rendezvous-key correction is recorded in
`after-placement.log`: exit 0, two placement regressions passed.

`after-final.log` is the required final gate: exit 0, **45 cluster tests passed**,
including all seven new regressions. Existing sole-node engine refusal,
pressure/rebalance checks, ordinary routing, and placement tests also passed.
No broader test suite or additional compile gate was run.

## Formatting and strict lint limitation

`format.log`: scoped remote rustfmt completed with exit 0. All changed source
modules are below 400 lines; counts and content hashes are recorded.
`git diff --check` also completed with exit 0 after formatting.

`lint.log`: the crate-name path was rejected before lint ran (exit 1).
`lint-scoped.log`: the confirmed crate directory invoked
`cargo clippy -p prod-code-mcp --all-targets --message-format=json -- -D warnings`.
The CLI exited 1, reporting compiler exit 101 and 210 diagnostics.
All 53 visible diagnostic files are byte-identical to the pinned base; none is
in the changed cluster files. `lint-baseline-files.json` contains those hashes.
The CLI omits 35 diagnostics, so this is not a claim that strict lint passed or
that the complete hidden diagnostic set was audited. Baseline lint was not rerun,
warnings were not suppressed, and unrelated source was not changed.

Other failed discovery commands and bounded fallbacks are preserved in
`tool-failures.txt`. There are no performance or live-node routing claims here.
