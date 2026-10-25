# Report issue 1323: punctuation-colon follow-up

Acceptance base: `e74b152a0cc216daaf7f1c51fcaea89761f84b11`.
Previous attempt: `92369c40fe649098c851d417a15d23f2419165ab`.
Observed on 2026-10-25, Linux x86_64 build node. The assigned tree was clean.
The caller manages the result commit; no commit, publication, delegation,
dependency change, installation or infrastructure change was performed here.

## Inspection and causal evidence

Read issue 1323 with `gh issue view 1323 --json title,body,comments` and each
original review using `gh api repos/alex09x/prod-code/pulls/comments/ID --jq
'{path,line,body}'`, substituting 4100229821, 4102250606 and 4113176130.
All four commands exited 0. Inspected the report helpers, CLI forwarding/help,
MCP forwarding/schema and agent guidance. Prior evidence is retained in
`report-1323.md`; its final-state claims belong to the previous attempt.

Measured cause: the scanner collected a maximal candidate including a trailing
colon. Parsing that candidate failed and the start-boundary guard prevented
retrying inside it. Hypothesis before editing: an invented unique-local literal
followed by `: connection refused` would survive scrubbing. Successful redaction
on the unchanged production scanner would contradict that hypothesis.

Added a focused direct regression and extended the existing draft/fake-gh test
before production edits. Observed failures confirmed both the scanner cause and
the leak through the draft boundary. The fix parses the full literal first,
then removes trailing punctuation colons only upon parse failure. This preserves
valid trailing compression. A following identifier prevents treating Rust `::`
as punctuation. No changes were needed to label validation or help guidance.

Current source locations:

- `crates/prod-code-mcp/src/report/ipv6.rs:46`: identifier boundary and parse fallback.
- `crates/prod-code-mcp/src/report/regression_tests.rs:59`: title, body, synthetic
  environment and private_ref through dry-run and fake-gh search/create/stdin.
- `crates/prod-code-mcp/src/report/regression_tests.rs:146`: exact punctuation,
  valid compressed, mapped, safe-address and Rust-path controls.

## Exact checks and observed output

Privacy convention: commands below retain their executed argv except that the
private `--remote` operand is replaced by `<build-node>`. Output excludes node
addresses, filesystem locations outside the repository, and process IDs.

Before production edits:

```text
prod-code exec --remote <build-node> --timeout-secs 900 -- cargo test -p prod-code-mcp --lib -- report::regression_tests::ipv6_
```

Terminal exit **101**, build 32.73s, wall 33.2s:

```text
ipv6_scrubbing_preserves_syntax_and_safe_controls ... ok
ipv6_followed_by_punctuation_colon_preserves_valid_compression ... FAILED
ipv6_is_scrubbed_at_draft_and_fake_gh_boundaries ... FAILED
IPv6 and punctuation must be parsed separately
draft title leaked IPv6
test result: FAILED. 1 passed; 2 failed; 1405 filtered out
```

After the complete coherent implementation, one scoped formatting pass:

```text
prod-code exec --remote <build-node> --timeout-secs 900 -- rustfmt --edition 2024 --config skip_children=true crates/prod-code-mcp/src/report/ipv6.rs crates/prod-code-mcp/src/report/regression_tests.rs
```

Terminal exit **0**, wall 0.4s; one test file formatted and pulled back.

One scoped strict lint pass (stdout/stderr captured for complete inspection):

```text
prod-code exec --remote <build-node> --timeout-secs 900 -- cargo clippy -p prod-code-mcp -p prod-code-client --lib --bins --no-deps -- -D warnings
```

Terminal exit **101**, wall 18.1s:

```text
error: this `if` statement can be collapsed
  --> crates/prod-code-mcp/src/config/mod.rs:52:9
error: this function has too many arguments (9/7)
  --> crates/prod-code-mcp/src/diagnostics/stream/feed.rs:31:5
error: could not compile `prod-code-mcp` (lib) due to 103 previous errors
```

All 2,033 log lines were captured. The complete 103-diagnostic inventory is
`report-1323-punctuation-lint.tsv`. Every diagnostic file was verified by Git
blob hash to be unchanged from the acceptance base; none names a changed report
file. This is a failed strict lint check, and it prevents claiming a client lint
pass. No lint suppressions, unrelated fixes, baseline rebuild or repeated lint.

One final focused compile/test gate, exactly the required acceptance selection:

```text
prod-code exec --remote <build-node> --timeout-secs 900 -- cargo test -p prod-code-mcp -p prod-code-client --lib --bins -- report:: report_issue_help
```

Terminal exit **0**, build 30.10s, wall 30.9s:

```text
bench library: test result: ok. 3 passed; 0 failed; 47 filtered out
client binary: test result: ok. 4 passed; 0 failed; 110 filtered out
MCP library: test result: ok. 38 passed; 0 failed; 1370 filtered out
```

The final selection passed the new punctuation regression, expanded fake-gh
boundary test, existing privacy controls, normalized-type rejection/deduplication,
default bug/area/roadmap behavior, shared CLI/MCP requests, CLI report_issue_help,
MCP description/labels schema and AGENT_INSTRUCTIONS assertions. The requested
filter also selects other existing modules named report. No broad suite or
overlapping build gate was run. `git diff --check` exited 0.

## Hashes, limits and tool failure

`report-1323-punctuation-source-hashes.tsv` records acceptance-base, prior-attempt
and current Git blob hashes for all eleven changed source files. Production
scanner code was unchanged from the prior-attempt hash during the failing run.
Current source is 90 lines for ipv6.rs and 277 for regression_tests.rs; both have
the existing project attribution header.

Fixtures are invented numerical addresses and synthetic zone IDs; no real
endpoint is included. Fake gh alone captures public create arguments/stdin.
Environment metadata is synthetic; no live status response was tested. The
existing IPv4, home, hostname, loopback/unspecified/documentation policy remains.
A token valid as both an all-hex Rust path and IPv6 remains inherently ambiguous.

One discovery batch was rejected before execution. Exact command:

```text
git diff --stat e74b152a0cc216daaf7f1c51fcaea89761f84b11 HEAD && rg --files crates/prod-code-mcp/src/report && sed -n '1,180p' evidence/codex-review-audit/report-1323.md && sed -n '1,180p' crates/prod-code-mcp/src/report/ipv6.rs && prod-code def --help
```

Verbatim hook error (before its repeated command):

```text
Command blocked by PreToolUse hook: Supported-language source discovery goes through prod-code: grep, rg, find and recursive scans over source are blocked. Text, strings, error messages and regexes: `prod-code grep PATTERN [PATH]` (MCP code_grep). Identifiers and structure: `prod-code def --symbol NAME`, `prod-code refs --symbol NAME`, `prod-code symbols NAME`, `prod-code outline PATH` (MCP code_definition, code_references, code_symbols, code_outline). Reading a file you name explicitly (Read, cat, sed -n) is allowed.
```

Removed that discovery command and used explicit bounded reads and successful
prod-code outline/grep calls.
No report was published for this tooling limitation, per the task's ban.
Every command started by this worker reached a terminal result.
