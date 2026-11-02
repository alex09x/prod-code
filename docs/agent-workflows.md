# Composable agent investigation (prototype)

`code_workflow` is an additional MCP entry point for related investigation steps.
It uses the same operations, gateway protocol and language engines as individual
tools. Existing tools remain available for a single lookup. The prototype does
not introduce a shell, JavaScript runtime or a new network protocol.

This documents the investigation slice of #1795. Explicit preview/apply and
verification are described in [guarded edits](agent-workflow-edits.md). A single
call is not a multi-file transaction, and the prototype does not claim that an
index or a series of reads is a consistent filesystem snapshot.

## Example

For common investigations, use a short form:

```json
{"symbol":"Cache::get","include":["dependencies","callers"]}
```

Or search for literal text and expand distinct enclosing symbol hints:

```json
{"grep":"cache miss","path":"src","limit":3}
```

The search adapter reuses existing grep enrichment (a heuristic, not a new AST
parser), deduplicates `(file, enclosing symbol)` hints and resolves each definition
separately. Ambiguous hints fail visibly. Matches without an enclosing symbol stay
in `unexpanded`; they are not silently counted as expanded definitions.
Raw grep columns are returned as one-based UTF-8 byte columns (`column_bytes` or
`match_column_bytes`), not LSP character positions. Expansion uses symbol hints
and file paths. Unexpanded excerpts are limited to 256 bytes each and an 8 KiB
total representation; omitted or shortened excerpts are marked explicitly.

The equivalent explicit form for symbol context is:

```json
{
  "outputs": ["body", "context"],
  "steps": [
    {"id": "target", "tool": "resolve", "args": {"symbol": "Cache::get"}},
    {"id": "body", "tool": "code_definition", "args": {
      "path": {"$ref": "target#/path"},
      "line": {"$ref": "target#/line"},
      "character": {"$ref": "target#/character"}
    }},
    {"id": "context", "tool": "code_slice", "args": {
      "path": {"$ref": "target#/path"},
      "line": {"$ref": "target#/line"},
      "character": {"$ref": "target#/character"},
      "depth": 1, "max_bytes": 6000
    }}
  ]
}
```

`resolve` reuses the existing unique-symbol resolver and refuses ties. `symbols`
returns `{items: [...], truncated, exhaustive: false}` with up to eight candidates.
Both return relative `path`, `line`, `character`, `name`, `kind` and `container`.
The candidate list is not proof that every possible declaration is indexed.
Regular `code_*` steps retain their arguments and return `{text: ...}`.
The default symbol request uses the dependency slice, which already includes
the seed body, avoiding a redundant standalone body query. Explicitly requesting
both sections can repeat the seed; span-based deduplication across arbitrary
operations is not implemented.

Exact reference objects use a prior step ID followed by `#` and an RFC 6901 JSON
pointer. Strings containing reference-like text are literal strings. Forward and
self references are rejected before execution. `for_each` takes an array or an
array reference; its arguments can use `$item#/path`. More than eight iterations
is an error, rather than silently choosing a subset.

An optional `when: {step: "target", status: "ok"}` gates a step. For an explicit
recovery path, use `on_error: "continue"` on the failing step and
`when: {step: "target", status: "error"}` on the recovery step. A failed step
does not produce a referenceable value. Without `on_error`, an error stops later
steps. No step or whole plan is automatically retried.
If an iteration fails, its completed items remain as `partial_value`, while the
step is marked as failed and cannot be used as a successful reference.

## Bounds and failure behavior

- One configured workspace or verified sibling Git worktree per plan. Inner
  steps cannot switch roots or use cross-workspace `also_in` references.
- Up to 16 steps and 32 operation calls, including iterations. Input is limited
  to 64 KiB; retained intermediate values to 1 MiB and each value to 256 KiB.
- `timeout_ms` defaults to 30 seconds and is capped at 60 seconds. Completed
  steps remain in the result if a later operation times out or fails.
  This bounds waiting for results, not CPU time in an already-started remote
  request or non-cancellable blocking worker.
- Only selected step values are shown (`outputs`, last step by default). All
  step statuses and the executed-call count remain visible. `max_bytes` bounds
  the JSON report; oversized values are omitted whole and `truncated` is set.
  Text always contains the JSON report. `structured:true` additionally returns
  the full `structuredContent` for programmatic consumers; it is off by default
  to avoid duplicating source in both channels. Transport bytes include framing
  and JSON string escaping, and must be measured separately from report bytes.
- Indexing, truncation and dependency-boundary warnings are retained per step even
  if its value is hidden by `outputs`, including warnings in completed iterations.
  At a tight output budget, `warning_steps` maps warning names to zero-based step
  indices instead of repeating warning names in each step. A successful execution
  does not certify index completeness. No `warm` state is inferred from silence.
- Status is `partial` if a step fails or output is omitted. Read-only plans do
  not promise stable line numbers across concurrent edits.
- Writes, shell commands and build/test operations are refused. The first
  prototype also refuses `build_index`, `apply`, `fix` and `run` step arguments.

## Evaluation and next decision

New unit tests exercise references, failure branches, iteration, time and context
budgets, confinement and registration. `workflow_live` is a separate explicitly
invoked test using a disposable Rust fixture and an existing gateway. It compares
standalone symbol lookup/definition/slice with the composed result, then checks
ambiguous-symbol recovery. It prints operation counts, timings and agent-visible
bytes. Baseline runs first; those timings are not a controlled speed benchmark.

Before extending the contract, compare both orders on equivalent real tasks,
including cold and unavailable analyzers, and inspect actual agent choices. Fewer
model round trips alone does not establish better accuracy or less total context.
Include the strongest existing baseline: direct `code_slice(symbol)` already
retrieves a seed and its dependencies in one call. Do not attribute a three-call
baseline's avoidable duplicate reads to an inherent limitation of existing tools.
Compare both text and full MCP-result bytes, including optional structured data.

Initial evaluation: the engine-level unit cases pass, and the live ambiguous-symbol
recovery case passed. The standalone definition call in the live comparison timed
out even with an explicit file and position; adding a fixture lockfile did not
resolve it. This remains tracked in #1796. The end-to-end comparison has not passed,
so no speed or context-reduction claim is made.

For mutation, inspect backend staging and writeback guarantees first. Named-symbol
edits must expose preview diffs, reject concurrent changes and report exactly what
was applied after a partial failure. Verification must remain explicitly scoped
and follow the repository's validation policy.

Background reading: [MCP structured tool results](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/docs/specification/2025-06-18/server/tools.mdx)
and [Anthropic's discussion of tool composition and context](https://www.anthropic.com/engineering/code-execution-with-mcp).
These motivate experiments; they do not establish performance of this prototype.
