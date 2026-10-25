# Issue 1330: preserve reachable lower-version requirement edges

Pinned acceptance base: `f6063f7399342f220c93a2013e8e99be82749633`.
Repair input: `3948c9d305525d4db6fa7ba5448f9b0cc515bb29`.
Earlier evidence is preserved, including the original local-replacement boundary
reproduction in `../issue-1330-local-dependency/before.txt`. This pass repairs the
remaining `required_version_graph_edges_lost` review finding.

## Observed failure and reproduction

Before production edits, `before.txt` records the exact sanitized focused command
and terminal exit 101: two new regressions failed and the modern pruning control
passed. The detector and public checkout/compatible-default/Linux-only routing
were exercised together using actual contained go.mod/source fixtures and the
existing deterministic fake gateways.

The root requires apple v1.0.0, shared v1.1.0 and bridge v1.0.0. Bridge requires
shared v1.0.0, whose distinct replacement manifest requires apple v1.1.0. The old
selected-version map discarded shared v1.0.0 before its manifest was read, so
the detector returned None and routing permitted Linux despite the imported,
selected Apple-only replacement. Reversing the replacement sources reproduced
the opposite error: the discarded older Apple-only source rejected Linux.
Both red assertions failed on their first (Go 1.16 main module) iteration.

Expected behavior is to retain lower-version graph edges when unpruned, then
scan imported sources using the highest selected version. A modern pruned bridge
must not expand that lower, unselected vertex's requirements. `source-hashes.json`
records pre-edit production and red fixture hashes. The final fixture differs
only through the one scoped rustfmt pass; it is not claimed byte-identical.

## Repair

- `crates/prod-code-mcp/src/sync/engine/local_go.rs:89` queues reachable
  module-version requirements independently of the selected-version map.
  Unpruned edges enqueue their exact required versions even when a higher version
  is already selected. Newly selected root versions remain eligible in pruned
  graphs. Contained manifest/mode visits are deduplicated, including wildcard
  replacements, and remain capped at 128 with existing byte/directive limits.
- `crates/prod-code-mcp/src/cluster/tests/route_local_deps_graph.rs:58` tests
  the lower-version edge under both a legacy main module and a legacy bridge.
  Line 68 reverses the replacement sources to verify that only the selected
  imported version supplies an OS constraint; unimported graph sources stay out.
  Line 78 controls modern pruning with both replacement source assignments.
- `crates/prod-code-mcp/src/cluster/tests/route_local_deps_review.rs:20`
  attaches the focused graph tests to the existing routing fixture module.

Independent module boundaries, containment checks and the selected-version
source scan are retained. No external trees, dependency downloads or toolchain
discovery were added to startup. This remains bounded local ownership inspection.
Changed source files retain attribution headers and have fewer than 400 lines.

## Verification

- `format.txt`: one scoped remote rustfmt pass, terminal exit 0.
- `lint.json`: one strict crate lint, CLI exit 1/compiler exit 101. The same
  210 diagnostics in the retained record affect 53 files, all byte-identical to
  the pinned base; none names a file changed in this repair. Lint did not pass.
  The record includes the retained diagnostic log's path and SHA-256. No baseline
  lint rerun or unrelated cleanup was performed.
- `after.txt`: the exact required remote cluster unit acceptance, terminal exit
  0; 78 passed, 0 failed, 1,363 filtered out. This was the single final test gate.
  All new graph cases and existing wrapper/go.work, nested selected-module OS,
  engine, compatible-default, remembered-Mac alternative, OS probe-error detail
  and pressure controls passed.
- `tool-failures.txt`: exact hook-rejected discovery command/error and bounded
  successful fallback. All started commands reached terminal completion.
- `source-hashes.json`: red/final source hashes, hashes of these records and
  verification that previously tracked audit evidence remains unchanged.

Formatting, lint, compilation and Rust tests executed on a real Linux build node.
Gateway OS/status/engine responses were simulated loopback fixtures. No real macOS
execution is claimed. Private endpoint/copy arguments are replaced with descriptive
placeholders in recorded commands/output. No real topology/configuration changes,
restarts, deployments, installs, publication, messages, delegation, detached work
or unrelated test suites were performed. Result integration is left to the caller.
