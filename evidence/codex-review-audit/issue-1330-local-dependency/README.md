# Issue 1330: selected-module local dependency ownership

Base: `f6063f7399342f220c93a2013e8e99be82749633`. Earlier audit directories are
preserved. This repair addresses `local_dependency_os_constraint_lost` without
removing the selected-module boundary.

## Reproduction

Before production edits, the new fixture required `example.org/apple`, replaced
it with `./apple`, and imported it. The child had its own real `go.mod` and a Go
source file with an unconditional `libproc.h` cgo preamble. An independent child
module was present too. The detector returned `None`, and public checkout routing
selected the simulated Linux Go gateway instead of the simulated macOS gateway.

`before.txt` preserves the sanitized exact command and output: exit 101,
two failures (detector and public routing), one passing independent-child test.
Production source was unchanged from the pinned base for that run. Additional
fixtures were added afterwards; `source-hashes.json` records base/final production
hashes, final fixture hashes, and the actual log hashes, rather than claiming
the expanded final fixture was the red-run fixture.

## Repair and boundaries

`crates/prod-code-mcp/src/sync/engine/cgo.rs:58` retains module boundaries and
queues explicitly reached local dependency roots under their selected owner.
`crates/prod-code-mcp/src/sync/engine/local_go.rs:63` follows imports using the
longest required module path. Single and block require/replace declarations,
unversioned and matching version-specific replacements, quoted paths, ordinary
and block imports, aliases, blank imports and raw import strings are supported.
The existing Go `opaque_end` lexer handles comments and literals; comment or
string examples cannot invent imports.

Only the selected main module's replacement directives apply. Reached local
modules can contribute requirements and imports, with deduplication for cycles.
Independent children, unused replacements, mismatched versions and prefix-only
import lookalikes remain excluded. Canonical containment rejects external path
and symlink escapes. The detector's provided root is the containment boundary:
a wrapper can reach a contained sibling; a selected module cannot read a sibling
outside its provided scan root. This is source-based ownership inference, not
Go's full version/dependency solver, package selection, or module cache discovery.

Existing limits of 4,000 Go files and 1,000 directories remain shared across the
walk. Additional limits are 128 reached local modules per owner, 1,024 manifest
records, and 1 MiB per inspected manifest/source. No subprocess, network access,
dependency installation or toolchain discovery was added to startup detection.
Every changed source module has its attribution header and is below 400 lines.

## Verification

- `format.txt`: scoped remote rustfmt, observed terminal exit 0.
- `after.txt`: required remote `cargo test -p prod-code-mcp --lib cluster::`,
  observed terminal exit 0; 58 passed, 0 failed. Seven new tests cover detection,
  checkout/default/fallback routing, absent-Mac refusal, independent children,
  syntax, transitive requirements, longest import ownership and containment.
  Inherited wrapper/go.work, selected nested-module, required-engine,
  remembered-Mac alternatives, OS probe error detail and pressure checks passed.
- `lint.txt`: strict crate lint did not pass: CLI exit 1, compiler exit 101,
  210 diagnostics across 53 files. `lint-baseline-files.json` confirms every
  named file is byte-identical to the pinned base. No diagnostic names a changed
  source file. This is not a claim that strict lint passed or a pristine-base
  rerun proved every cross-file interaction independent of this repair.
- `tool-failures.txt`: exact sanitized failed discovery calls and bounded
  fallbacks. No background command remained running at completion.

Compilation and test execution were real execution on a Linux build node.
OS/platform, engine availability, placement and pressure responses came from
deterministic loopback fake gateways; they do not prove real macOS execution.
The fixtures are actual temporary manifests and source files. Private addresses
and checkout paths are replaced by descriptive placeholders in command records.
No real topology/configuration changes, restarts, deployment, publication,
messages, delegation, dependencies, installation or broad tests were performed.
