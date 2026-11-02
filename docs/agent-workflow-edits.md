# Guarded text edits in agent workflows (development contract)

This extends the investigation prototype in #1795. It is not yet released.
Preview and apply remain operations of `code_workflow`, rather than additional
public MCP tools.

## Preview before deciding to write

```json
{
  "steps": [{
    "id": "proposal",
    "tool": "preview",
    "args": {"edits": [{
      "path": "src/lib.rs",
      "old": "pub fn answer() -> u32 { 1 }",
      "new": "pub fn answer() -> u32 { 2 }"
    }]}
  }]
}
```

Preview reads current text, requires an unambiguous old snippet, and returns a
diff plus prepared `files` containing relative paths, original SHA-256 values,
and the replacements. It does not write, compile or run tests. This first edit
core handles existing regular UTF-8 files, not creates, deletes or renames.

Review the preview in one call if a decision depends on seeing its diff. An
explicit combined call can preview and apply an already-authorized change, but
the internal preview is not a human approval checkpoint.

## Explicit apply and verification

```json
{
  "apply": true,
  "timeout_ms": 120000,
  "outputs": ["proposal", "write", "check"],
  "steps": [
    {"id": "proposal", "tool": "preview", "args": {"edits": [{
      "path": "crates/demo/src/lib.rs",
      "old": "pub fn answer() -> u32 { 1 }",
      "new": "pub fn answer() -> u32 { 2 }"
    }]}},
    {"id": "write", "tool": "apply", "args": {
      "files": {"$ref": "proposal#/files"}
    }},
    {"id": "check", "tool": "verify", "args": {
      "kind": "check", "path": "crates/demo/src/lib.rs", "timeout_secs": 60
    }}
  ]
}
```

In this Cargo example, `crates/demo` is a declared workspace member package.

`apply:true` only permits explicit apply or verification steps; it does not
schedule them. The whole plan's permission checks run before its first step.
These steps cannot use iteration or continue after an error. Ordinary read
steps keep their existing bounded iteration/recovery behavior.

For a separate preview call, copy its prepared `files` into the later apply
request. References address earlier steps within the current call, not a global
or persistent token store.

Apply revalidates all paths and expected file bytes before writing, under the
existing prod-code writer gates, then uses the journaled workspace-edit backend.
The preflight can refuse a stale preview before application starts. Each apply
batch is independent: earlier batches are not automatically undone if a later
operation or check fails.

The response retains per-step `effects` even when `outputs` hides that step's
value. Successful application reports an applied status and file count. Refusal
before application reports zero writes. An unconfirmed backend failure or
cancelled wait does not claim that changes were rolled back. Inspect the backend
error and current files before deciding how to proceed.

Fingerprints detect content already changed when checked. The writer gates
coordinate prod-code operations, not arbitrary external editors. An editor can
still change a path after the check; no universal compare-and-swap or crash-safe
multi-file transaction is promised. See [backend feasibility](workflow-staging-feasibility.md).

## Requested checks only

Verification accepts `check`, `lint` or `test` with an explicit project/file
`path`. `test` additionally requires a non-empty narrow `filter`. A Rust file
selects its project/package; it does not turn Cargo into a single-file checker.
The operation uses the existing verifier and preserves its failures, including
a Rust test filter matching no tests.

The scope must exist below the selected workspace root. Root aliases, missing
paths and escapes are refused. Literal verification arguments and scopes are
checked before the first step, including before any earlier apply. Values from
references are validated after resolution and again before starting the check.

Workflow preflight shares the ordinary verifier's filesystem-only command
planner. Rust and Go scopes that still select `--workspace` or `./...` are
refused, including container directories and root-level files the existing
verifier cannot narrow. Choose a member package/subdirectory instead. This is
also a current limitation for custom package layouts that cannot be narrowed.
Root Cargo package sources under `src`, `tests`, `examples` and `benches` select
that package explicitly, including when the workspace has sibling members.
Test runners that ignore the requested filter are refused before any earlier
apply; a non-empty filter string alone is not evidence of a narrow test run.
The final command is checked again during execution under the ordinary verifier
gates, so changed configuration cannot bypass the earlier planning check.

No verification runs merely because a preview or apply succeeded. Choose checks
according to the repository's validation policy; the workflow does not authorize
per-save builds or broader testing. Individual verification timeouts are bounded
to 120 seconds, and the overall write-enabled plan is capped at two minutes.
Remote work may still be running after a cancelled wait; a timed-out response
must not be read as successful verification or proof of rollback.
