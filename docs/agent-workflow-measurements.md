# Agent workflow prototype: observed results

These are acceptance observations for #1795, not a general productivity or speed
claim. The implementation is experimental and the delivery PRs remain drafts.

## Code state and conditions

The measured local integration commit was
`8c8739e7ec91086dc513389a113a73c9ca4dc124`: guarded workflows at `380c516a`
combined with the non-Git session refresh fix at `13929404` (#1798).
The executor and existing gateway were Linux x86-64. The gateway ran `0.3.48`.
Fixtures were fresh non-Git directories containing synthetic Rust or Python
source. No gateway restart or index/cache deletion was required.

```sh
prod-code exec --remote "$BUILD_NODE" \
  --env PROD_CODE_WORKFLOW_TEST_REMOTE="$GATEWAY" -- \
  cargo test -p prod-code-mcp --test workflow_live -- --ignored --nocapture
```

All three explicitly selected scenarios passed. Standalone operations ran first,
so the following workflow reused a warmer analyzer. This ordering does not
isolate composition as the cause of any latency difference.

## Correctness and tool calls

| Scenario | Standalone calls | Workflow calls | Observed result |
| --- | ---: | ---: | --- |
| Rust symbol search, definition and dependency slice | 3 | 1, containing 3 operations | Definition and slice text matched exactly; helper included |
| Python definition and hover | 2 | 1 | Both returned texts matched exactly |
| Ambiguous Rust symbol | Separate recovery would require another call | 1 | Lookup error retained, success branch skipped, candidates returned |
| Python preview, apply and requested type check | Not timed as a baseline | 1 per authorized change | Valid change passed; invalid return type failed verification after applied effects |

The last case also asserted the changed file remained on disk after failed
verification. One call does not imply rollback of earlier successful steps.
The ambiguity scenario tests explicit recovery, not automatic selection of the
intended candidate.

## Output size

For the Rust comparison, standalone text totalled 823 bytes, and full standalone
MCP results totalled 1142 bytes. The workflow report was 1122 text bytes and
1261 bytes as a full MCP result. It reduced calls but increased returned bytes
in this small fixture. No context-compression claim follows from it.

Python standalone text totalled 139 bytes; the full workflow MCP result was
494 bytes. The test did not record full standalone MCP bytes for that case, so
these two values are not a like-for-like wire-size comparison.

`structured:true` was not requested. That opt-in duplicates the full JSON report
in `structuredContent`; the default uses only the text report.

## Latency limitations and stronger baselines

Standalone-first observed times were 3288 ms versus 35 ms for Rust, and 555 ms
versus 16 ms for Python. These cold/warm observations are not evidence that
composition accelerated the underlying operations. The sync fix benefits both
interfaces and must not be attributed to workflow orchestration.

A direct `code_slice` already includes the seed body and is a one-call baseline
when a dependency slice alone answers the task. The three-call Rust scenario
does not establish that three calls are necessary for that narrower question.
Existing standalone tools remain useful and available.

Further evaluation should compare equivalent requested context, include this
stronger baseline, balance execution order after explicit warm-up, and record
correctness, full model-visible bytes and calls independently of latency. Real
agent success, cold/unavailable index behavior and broad language coverage are
not established by three synthetic scenarios.
