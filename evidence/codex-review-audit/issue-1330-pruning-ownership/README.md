# Issue 1330: pruning, applicable sources and selected-module ownership

Pinned acceptance base: `f6063f7399342f220c93a2013e8e99be82749633`.
Repair input: `e2578113ec9038ff22a5fff5d51563a9b9ab1ecc`.
All earlier evidence directories are preserved unchanged. This pass repairs the
three blocking findings from the previous semantic review; the retained records
also contain the original local-replacement boundary reproduction.

## Before and after

`before.txt` records the exact sanitized focused command and terminal exit 101:
all three new regressions failed before production edits. The joint assertions
cover detector output, public checkout placement, compatible-default routing and
Linux-only routing with real contained go.mod/source fixtures:

- Modern Go incorrectly loaded a non-root leaf's requirements, upgraded the
  imported Apple module to a portable replacement and permitted Linux placement.
- An underscore-prefixed file's import incorrectly scanned the Apple dependency
  and rejected Linux placement.
- A selected-module import was incorrectly attributed to a shorter required
  parent path, scanning its unused Apple-only replacement and rejecting Linux.

`source-hashes.json` records red-run production and fixture hashes, final source
hashes and evidence hashes. Final fixtures were formatted and the self-import
case was moved into a command package so both root and subpackage self-imports
are acyclic. The final fixture is not claimed byte-identical to the red fixture.

## Repair

- `crates/prod-code-mcp/src/sync/engine/local_go.rs:40` retains module and Go
  directives. At line 86, modern graph roots contribute immediate requirements;
  legacy roots and their transitive dependencies retain full manifest closure.
  A pruned and an unpruned visit remain distinct and bounded. At line 153, import
  ownership includes the selected module before choosing the longest prefix.
- `crates/prod-code-mcp/src/sync/engine/cgo.rs:142` filters ignored filenames and
  recognized OS/architecture suffixes before source imports expand dependencies.
  Linux architecture support matches the existing build-expression predicate.
- `crates/prod-code-mcp/src/cluster/tests/route_local_deps_pruning.rs:14` covers
  Go 1.17/1.24 pruning, Go 1.16 closure, legacy dependency closure and explicit
  leaf roots; line 60 covers ignored/Windows/other OS and unsupported architecture
  files plus applicable filename controls; line 99 covers own-module ownership
  and genuinely more-specific required modules. All use the existing public
  routing helper and deterministic fake gateways.

Canonical containment, replacement precedence, main-module replacement ownership,
independent module boundaries and byte/module/directory/file budgets remain.
No external dependency trees, network discovery or toolchain queries were added
to startup. This is bounded contained dependency inspection, not a full Go solver.
Changed source files retain attribution headers and have 362, 340, 256 and 130
lines, respectively.

## Verification

- `format.txt`: one scoped remote rustfmt invocation, terminal exit 0.
- `lint.txt`: one strict crate lint, CLI exit 1/compiler exit 101; exactly the
  retained 210 diagnostics, with none in changed files. All 53 diagnostic files
  are byte-identical to the pinned base (`lint-baseline-files.json`). Lint did
  not pass; no unrelated cleanup or baseline rerun was performed.
- `after.txt`: exact required remote cluster unit acceptance, terminal exit 0;
  69 passed, 0 failed, 1,363 filtered out. Existing single/block syntax, wrapper
  and go.work, selected nested-module OS constraints, engine checks, compatible
  defaults, remembered-Mac alternatives, OS probe-error details and pressure
  controls remain passing. This was the single final compile/test gate.
- `tool-failures.txt`: exact rejected discovery command/error and bounded fallback.

Compilation, formatting, lint and Rust tests ran on a real Linux build node.
Gateway OS/status/engine responses were simulated loopback fixtures; no real
macOS execution is claimed. Private endpoint/copy arguments are replaced with
descriptive placeholders in recorded commands/output. No topology, runtime or
configuration changes, installs, restarts, deployments, publication, messages,
delegation or unrelated tests occurred. All started commands reached completion.
