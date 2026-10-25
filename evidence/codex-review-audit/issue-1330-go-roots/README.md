# Issue 1330: workspace and wrapper root regression repair

This attempt preserves the inherited route, placement, and probe-error repairs,
and corrects the review's blocking cgo scan regression. Its final evidence
supersedes the earlier attempts' test counts for the resulting source state.

## Fresh reproduction

Before production edits, three new cluster regressions exercised Go workspace
roots, workspaces with a root module, and wrapper roots whose Go engine is
inferred from a child manifest. Each fixture has `apple/go.mod` and an
unconditional `libproc.h` cgo include. All three incorrectly placed the checkout
on simulated Linux instead of simulated macOS: 0 passed, 3 failed, exit 101.
The exact sanitized command and complete output are in `before.txt`.
`source-hashes.json` records production and test hashes at that reproduction,
the inherited HEAD, the pinned base, and all final changed source hashes.

## Repair and coverage

The scan now distinguishes a selected Go module from a wrapper root: wrappers
discover child modules before enforcing independent descendant-module exclusion.
Workspace roots retain the original recursive scan, including when `go.work`
sits beside `go.mod` or names a member nested under another member. This does
not introduce a new go.work parser or narrow the baseline workspace scan.
The existing engine gate still prevents Rust/non-Go checkout scans, and path
routing still derives its selected Go module from the requested path.

Five added cluster regressions exercise checkout placement and path routing,
compatible defaults and fallback selection, workspace member discovery, and a
wrapper's portable module containing an independent Apple-only child module.
The latter remains portable until the child itself is selected. The inherited
regressions retain ordinary Go, nested cgo, remembered Mac recovery, stale
placement rejection, resource pressure, engine refusal, and contextual probe
error-chain coverage. No production changes outside the cgo helper were needed
in this retry; the inherited routing changes remain part of the task's diff.

## Execution and simulation boundary

The commands compiled and executed unit tests on a Linux build node. Platform,
engine, pressure, and placement metadata came from ephemeral loopback fake
endpoints; these records do not demonstrate live macOS execution. Tests use
mutex barriers and captured placement requests, with no sleeping races added.
No real cluster placement, service, configuration, dependency, installation,
publication, message, other worktree, or worker process was changed or started.
Private addresses and paths are replaced by descriptive placeholders in logs.

## Final checks and limits

`format.txt`: scoped remote rustfmt, terminal exit 0. All changed source modules
retain attribution headers and are at most 306 lines.
`after.txt`: required final cluster unit gate, terminal exit 0: **51 passed**,
including all five new regressions and all eight inherited issue regressions.
This was the sole final compile/test gate; no broader suite was run.

`lint.txt`: strict crate lint, CLI exit 1 / compiler exit 101, **210 diagnostics**
in 53 files. None names a changed source file. `lint-baseline-files.json` confirms
all 53 diagnostic files are byte-identical to the pinned base. Strict lint did
not pass; no pristine-base rerun, unrelated repairs, or warning suppression was
performed. File equality does not prove independence from every possible
cross-file interaction. `tool-failures.txt` records the unsuccessful guessed
symbol lookup and its bounded read-only fallback.
