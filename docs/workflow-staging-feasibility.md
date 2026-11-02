# Workflow staging feasibility

This is a bounded backend feasibility study for the next slice of #1795, not a
delivered mutation tool. The executable cases live in
`crates/prod-code-mcp/tests/workflow_staging_feasibility.rs`.

## What the five cases demonstrate

1. `planned_texts` stages text replacement, creation and deletion in memory; the
   fixture's file bytes and directory tree remain unchanged. `patch::apply` also
   computes a proposed replacement without writing this fixture.
2. `apply_workspace_edit` applies an ordered batch with a text replacement,
   rename and file creation, returning the expected touched paths and contents.
3. A batch with a non-UTF-8 text target is rejected before changing an earlier
   target or unrelated content.
4. A regular file blocking creation of a parent directory causes an execution
   failure after an earlier write and rename. The error reports rollback, and
   the fixture's original file bytes and paths are restored.
5. An edit prepared from old content overwrites a subsequently saved external
   edit when applied through the current API without a content precondition.
   This is an interleaving reproducer, not a stress test of all concurrent saves.

These cases do not prove every resource operation, every filesystem failure or
crash recovery. No test here proves inter-process isolation or crash-safe
multi-file atomicity. Rollback can fail, and concurrent changes during rollback
require a separate preservation strategy.

The apply implementation also invalidates sync watermarks and call-hierarchy
cache in its production path. This study inspected that code, but does not claim
to test those effects: checking an initially empty cache would be vacuous.
Likewise, this suite does not establish symlink-confinement coverage or compiler
correctness of proposed changes.

## Reusable APIs and limits

- `refactor::edits::planned_texts` returns planned text and a resource-operation
  indicator. It stages content; it does not validate semantics or write files.
- `patch::apply` computes text proposals for unified diff hunks.
- `refactor::execute::apply_workspace_edit` performs preflight checks and uses
  the existing journal. Its errors distinguish a successful rollback from
  files it could not restore. That is recoverable application in tested cases,
  not an isolated database transaction.
- `ValidationProposal` and analyzer overlays may be useful for a later preview
  implementation. This suite does not exercise their semantic validation.
  Resource-operation and deletion effects need their own validation strategy.

## Proposed next contract

Keep preview and apply as operations within the same primary `code_workflow`
interface. Do not introduce extra public MCP tools just for this study.

A preview operation should return a diff, explicit target paths and fingerprints
of the bytes used to construct the proposal. It must retain resource-operation
metadata rather than treating an omitted/deleted path as an empty text file.

An explicitly requested apply operation should require a precondition for every
target (including expected absence for a new path), revalidate confinement, and
check current bytes under prod-code's existing writer gates before any write.
Conflicting preconditions must cause a clear refusal before application begins.
The result must identify applied paths, rollback completion and failures to
restore, rather than infer success from a one-call interface.

Content-hash checking is a proposed addition, not a capability proved by this
suite. It can detect changes already present when checked. Our writer locks
coordinate cooperative prod-code operations, not arbitrary editors. An external
process can still change a file between the check and replacement. Hash checks
alone do not eliminate that race, and ordinary advisory locks do not coordinate
editors that ignore them. The precise concurrency guarantee needs proof before
being advertised; do not promise universal compare-and-swap semantics.

Preview, apply and semantic validation must be separate explicit choices. A
later verification operation must run only the requested narrow check under
the repository's development validation policy. No build, test or broad lint
should be an implicit side effect of a preview.
