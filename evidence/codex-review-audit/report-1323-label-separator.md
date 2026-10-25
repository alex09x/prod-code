# Issue 1323: compact label separator follow-up

Acceptance base: `e74b152a0cc216daaf7f1c51fcaea89761f84b11`.
Initial clean HEAD: `4e3920d8ca25ed3f561221657a5d5b0d7e60dcd3`.
Observed on 2026-10-25, Linux x86_64 build node. The caller manages the result
commit. Earlier evidence files describe historical states; this file and
`report-1323-label-separator-source-hashes.tsv` describe the final source here.

## Inspection and causal evidence

Read issue 1323 with:

```text
gh issue view 1323 --repo alex09x/prod-code --json title,body,labels
gh api repos/alex09x/prod-code/pulls/comments/4100229821 --jq '{path,line,body}'
gh api repos/alex09x/prod-code/pulls/comments/4102250606 --jq '{path,line,body}'
gh api repos/alex09x/prod-code/pulls/comments/4113176130 --jq '{path,line,body}'
```

All four reads exited 0. Inspected current report helpers and the exact CLI/MCP
guidance surfaces through prod-code definition, outline and grep commands, plus
bounded source reads. The prior commit already implements normalized type counts,
roadmap help and parsed IPv6 redaction. Their initial failing reproductions and
the punctuation follow-up are retained in `report-1323.md` and
`report-1323-punctuation.md`; those historical checks were not repeated unchanged.

Observed remaining cause before editing production: the scanner refuses to start
after any colon. A non-hex label followed by a single colon prevents parsing the
whole token, and that guard prevents parsing the address after the label.
Hypothesis: a compact `peer:` field containing a numerically invented unique-local
address survives direct scrubbing and draft creation. Correct redaction on the
unchanged scanner would contradict it. The two focused failures below confirmed
the cause before the production edit.

Changed the guard to allow a start after a single label colon, retaining the guard
after `::` Rust path separators. Parsing still uses `std::net::Ipv6Addr`; no
dependencies or blanket colon-text masking were introduced. The full literal is
still parsed before trying trailing punctuation, preserving valid compression.

Current source locations, confirmed by terminal outlines/grep:

- `crates/prod-code-mcp/src/report/ipv6.rs:16`: scanner and label boundary.
- `crates/prod-code-mcp/src/report/regression_tests.rs:59`: title, body, synthetic
  environment and private_ref through draft, dry run, fake-gh search/create/stdin.
- `crates/prod-code-mcp/src/report/regression_tests.rs:179`: compact-label
  formatting, safe-address controls and Rust paths with all-hex components.
- `crates/prod-code-mcp/src/report/regression_tests.rs:203`: distinct normalized
  types rejected; duplicate spelling/case, absent type, areas and roadmap accepted.
- `crates/prod-code-mcp/src/report/regression_tests.rs:226`: conflicting types
  rejected before any fake-gh invocation for both force and dry-run settings.
- `crates/prod-code-mcp/src/report/regression_tests.rs:259`: MCP description,
  labels property and AGENT_INSTRUCTIONS guidance assertions.
- `crates/prod-code-mcp/src/report/regression_tests.rs:281`: shared MCP request path.
- `crates/prod-code-client/src/commands/ops/report.rs:80`: CLI help test whose
  name includes `report_issue_help` for the required selection.

## Commands and measured results

Privacy convention: `<build-node>` replaces only the actual `--remote` operand.
All remaining arguments below are as executed. Excerpts omit private node
addresses, external filesystem paths and process IDs. Fixtures identify no actual
machine: IPv6 values are constructed numerically and zone IDs are synthetic.

### Before production edits

Added the compact-label direct regression and extended the existing public
boundary regression before changing the scanner. Production files matched the
initial HEAD at this point. The hash table records the failing-run state; its
regression test file already matches final, and production uses prior-attempt blobs.

```text
prod-code exec --remote <build-node> --timeout-secs 900 -- cargo test -p prod-code-mcp --lib -- report::regression_tests::ipv6_
```

Observed terminal exit **101**, build 36.96s, wall 38.2s:

```text
ipv6_followed_by_punctuation_colon_preserves_valid_compression ... ok
ipv6_scrubbing_preserves_syntax_and_safe_controls ... ok
ipv6_after_label_separator_preserves_rust_paths_and_safe_controls ... FAILED
ipv6_is_scrubbed_at_draft_and_fake_gh_boundaries ... FAILED
label separators must not hide IPv6
draft title leaked IPv6
test result: FAILED. 2 passed; 2 failed; 1405 filtered out
```

### Complete change: scoped formatting once

```text
prod-code exec --remote <build-node> --timeout-secs 900 -- rustfmt --edition 2024 --config skip_children=true crates/prod-code-mcp/src/report/ipv6.rs crates/prod-code-mcp/src/report/regression_tests.rs
```

Terminal exit **0**, wall 0.4s. One file formatted and pulled back (ipv6.rs).
No recursive formatting or unrelated source cleanup.

### Complete change: scoped strict lint once

```text
prod-code exec --remote <build-node> --timeout-secs 900 -- cargo clippy -p prod-code-mcp -p prod-code-client --lib --bins --no-deps -- -D warnings
```

Captured stdout/stderr in a temporary log, then returned the same command status.
Terminal exit **101**, wall 19.5s:

```text
error: this `if` statement can be collapsed
  --> crates/prod-code-mcp/src/config/mod.rs:52:9
error: this function has too many arguments (9/7)
  --> crates/prod-code-mcp/src/diagnostics/stream/feed.rs:31:5
error: could not compile `prod-code-mcp` (lib) due to 103 previous errors
```

All 2,033 log lines were captured. The 103 source diagnostics are recorded in
`report-1323-label-separator-lint.tsv`. Every diagnostic file was verified by Git
blob hash to be unchanged from the acceptance base. None is in a changed report
file. This remains a **failed strict lint gate**; MCP failure prevents claiming a
client lint pass. No suppression, unrelated fix, baseline rebuild or repeat lint.

### Final focused compile/test gate

```text
prod-code exec --remote <build-node> --timeout-secs 900 -- cargo test -p prod-code-mcp -p prod-code-client --lib --bins -- report:: report_issue_help
```

Observed terminal exit **0**, build 29.10s, wall 29.9s:

```text
bench library: test result: ok. 3 passed; 0 failed; 47 filtered out
client binary: test result: ok. 4 passed; 0 failed; 110 filtered out
MCP library: test result: ok. 39 passed; 0 failed; 1370 filtered out
```

All eight MCP regression tests and both client report_issue_help tests passed,
along with existing report privacy tests. The exact requested filter also selects
other existing modules named report; no broad suite or overlapping build gate ran.

## Coverage and limitations

- Numerically synthetic bracketed host/port, bare, compressed/expanded,
  link-local zones, URI-encoded zone delimiters, global and mapped IPv6 forms
  traverse direct and public fake-gh boundaries, including compact `peer:` fields.
- Loopback, unspecified, documentation and safe mapped IPv4 policy, existing IPv4
  scrubbing, home paths and hostname controls pass. The new direct test asserts
  exact label/port/punctuation preservation and retains ordinary colon text and
  Rust `::` paths. A token valid as both an all-hex path and IPv6 remains ambiguous.
- Environment metadata is synthetic; the actual environment formatter output is
  passed through the report boundary. No live status response was required. MCP
  dry runs direct optional status lookup to loopback port zero.
- Fake gh captures arguments and body stdin; no actual issue publication occurred.
  Label refusal happens in the common path before gh. CLI/MCP shared validation,
  duplicate normalization, default bug, area/roadmap and all four help surfaces pass.
- ipv6.rs is 94 lines and regression_tests.rs is 308 lines, both with existing
  attribution headers. No CLI layout redesign or unrelated changes.
- No dependency changes, installations, infrastructure changes, messages,
  publication, delegation or background providers. Result commit is left to caller.

## Tool failure and bounded fallback

One discovery batch was rejected before execution. Exact command:

```text
git diff --stat e74b152a0cc216daaf7f1c51fcaea89761f84b11 HEAD; rg --files evidence/codex-review-audit crates/prod-code-mcp/src/report; prod-code outline crates/prod-code-mcp/src/report/ipv6.rs
```

Verbatim hook error before the repeated command:

```text
Command blocked by PreToolUse hook: Supported-language source discovery goes through prod-code: grep, rg, find and recursive scans over source are blocked. Text, strings, error messages and regexes: `prod-code grep PATTERN [PATH]` (MCP code_grep). Identifiers and structure: `prod-code def --symbol NAME`, `prod-code refs --symbol NAME`, `prod-code symbols NAME`, `prod-code outline PATH` (MCP code_definition, code_references, code_symbols, code_outline). Reading a file you name explicitly (Read, cat, sed -n) is allowed..
```

Removed the blocked scan, listed only the evidence directory, and used successful
prod-code outlines/definitions/grep plus explicit bounded file reads. No public
tooling issue was filed under the task's publication ban. All commands started by
this worker reached terminal completion.
