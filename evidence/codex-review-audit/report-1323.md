# Report privacy, type labels, and roadmap guidance — issue 1323

Historical evidence from attempt commit `92369c40fe649098c851d417a15d23f2419165ab`.
Its final hashes and checks describe that earlier state. The punctuation-colon
follow-up and current final hashes are recorded in `report-1323-punctuation.md`
and `report-1323-punctuation-source-hashes.tsv`.

Base: `e74b152a0cc216daaf7f1c51fcaea89761f84b11`. Node kind: Linux x86_64.
Checks were observed to terminal completion in the assigned checkout on 2026-10-25.
The tree was clean before work. No issue was published, no dependencies changed,
and no background worker or delegation was used. Result commit is managed by the caller.

## Sources and observed cause

Read issue 1323 using `gh issue view 1323 --repo alex09x/prod-code --json title,body,labels`.
Read the original Codex comments using
`gh api repos/alex09x/prod-code/pulls/comments/4100229821 --jq '{path,line,body}'`,
then the same command for comments `4102250606` and `4113176130`.
All three review-comment reads succeeded. The findings describe the same three defects.

Baseline `report/scrub.rs` called only `replace_ipv4`. Baseline `issue_labels`
normalized and deduplicated labels, then supplied a default type without counting
distinct types. CLI labels help, the MCP description and labels property, and
initialization instructions did not advertise the already accepted roadmap marker.
Both the CLI handler and MCP dispatcher forward requests to `report::report`.

Hypotheses before editing: IPv6 literals survive the public draft boundary because
only IPv4 is recognized; conflicting normalized types reach fake gh because they
are never counted. A scrubbed IPv6 draft or a pre-gh refusal on baseline would
contradict those hypotheses. The focused failures below supported both causes.

## Commands and measured results

Privacy convention: `<build-node>` below replaces only the actual `--remote`
operand. Every other argument is recorded as executed. Node addresses, node
checkout paths, and process IDs are omitted from output excerpts. The numeric
fixtures in the tests are invented; none names a real endpoint or hostname.

### Before production edits

Added focused regression tests and registered their module before changing any
production behavior. The scrubber, label helper and all guidance surfaces still
matched the base hashes in `report-1323-source-hashes.tsv`; only test code and
test-module registration differed from the base during reproduction.

Command (also the required final acceptance command):

```text
prod-code exec --remote <build-node> --timeout-secs 900 -- cargo test -p prod-code-mcp -p prod-code-client --lib --bins -- report:: report_issue_help
```

Observed exit **101**, remote test build 39.37s:

```text
bench library: test result: ok. 3 passed; 0 failed
client binary: test result: FAILED. 2 passed; 1 failed
report_issue_help_advertises_roadmap_and_parses_labels ... FAILED
report labels help omits roadmap
```

Cargo stopped at the client binary, so MCP reproductions were then run explicitly:

```text
prod-code exec --remote <build-node> --timeout-secs 900 -- cargo test -p prod-code-mcp --lib -- report::regression_tests
```

Observed exit **101**, remote test build 26.34s:

```text
test result: FAILED. 0 passed; 5 failed; 1401 filtered out
distinct_normalized_types_are_refused_but_duplicates_and_markers_work: multiple types accepted
ipv6_scrubbing_preserves_syntax_and_safe_controls: IPv6 must be removed
report_issue_help_schema_and_agent_guidance_advertise_roadmap: report guidance omits roadmap
ipv6_is_scrubbed_at_draft_and_fake_gh_boundaries: draft title leaked IPv6
multiple_types_fail_before_any_gh_invocation: report accepted multiple types
```

### Completed change: one scoped formatting pass

```text
prod-code exec --remote <build-node> --timeout-secs 900 -- rustfmt --edition 2024 --config skip_children=true crates/prod-code-client/src/cli/commands.rs crates/prod-code-client/src/commands/ops/report.rs crates/prod-code-client/src/commands/ops/report_tests.rs crates/prod-code-mcp/src/protocol.rs crates/prod-code-mcp/src/report/mod.rs crates/prod-code-mcp/src/report/ipv6.rs crates/prod-code-mcp/src/report/regression_tests.rs crates/prod-code-mcp/src/report/scrub.rs crates/prod-code-mcp/src/report/tests.rs crates/prod-code-mcp/src/report/types.rs crates/prod-code-mcp/src/tools/schemas/utility.rs
```

Observed exit **0**. Six files formatted and pulled back. `skip_children=true`
kept formatting inside the explicit changed-file list.

### Completed change: one scoped strict lint pass

```text
prod-code exec --remote <build-node> --timeout-secs 900 -- cargo clippy -p prod-code-mcp -p prod-code-client --lib --bins --no-deps -- -D warnings
```

Observed exit **101**, 17.8s wall time:

```text
error: this `if` statement can be collapsed
  --> crates/prod-code-mcp/src/config/mod.rs:52:9
error: this function has too many arguments (9/7)
  --> crates/prod-code-mcp/src/diagnostics/stream/feed.rs:31:5
error: call to `std::mem::drop` with a value that does not implement `Drop`
  --> crates/prod-code-mcp/src/dossier/locations.rs:126:5
error: could not compile `prod-code-mcp` (lib) due to 103 previous errors
```

Those example files are unchanged from base. Other visible baseline categories
include needless range loops, unnecessary map_or, needless lifetimes, identical
branches, and collapsible matches. Terminal output was truncated (1,873 lines,
19,626 original tokens), so this is not a complete inventory or proof that every
diagnostic is baseline. MCP lint failure prevented claiming a client lint pass.
No lint suppression, unrelated cleanup, baseline rebuild, or repeated lint run.

### Final focused compile/test gate

```text
prod-code exec --remote <build-node> --timeout-secs 900 -- cargo test -p prod-code-mcp -p prod-code-client --lib --bins -- report:: report_issue_help
```

Observed terminal exit **0**, remote build 9.81s, overall 10.6s:

```text
bench library: test result: ok. 3 passed; 0 failed; 47 filtered out
client binary: test result: ok. 4 passed; 0 failed; 110 filtered out
MCP library: test result: ok. 37 passed; 0 failed; 1370 filtered out
```

All six new MCP regressions and both new client report tests passed. The exact
requested filter also selected existing unrelated modules named `report`; no
additional broad suite or separate build was run. Existing report privacy tests
and positive IPv4/home/hostname controls passed. `git diff --check` returned 0.

## Coverage and limits

- `report/ipv6.rs` parses literals with `std::net::Ipv6Addr`, before IPv4 scrubbing.
  Tests cover bracketed ports, compressed/expanded/bare IPv6, link-local zones,
  URI-encoded zone delimiters, global and IPv4-mapped literals, Unicode context,
  and surrounding brackets, ports, URLs and punctuation.
- Loopback, unspecified, documentation (`2001:db8::/32`) and mapped safe IPv4
  controls remain intact. Ordinary Rust paths and colon text are not masked.
  A token syntactically valid as both an all-hex Rust path and IPv6 is treated as
  IPv6; address parsing cannot infer the author's meaning from that token alone.
- Draft tests supply synthetic `StatusResponse` metadata for platform, version,
  commit and engines. The real environment formatter output then travels through
  `report` dry runs and fake-gh search/title/stdin boundaries, together with title,
  body and private_ref. No live remote status response is needed for these checks.
- Fake gh is the existing shell stand-in: it logs arguments, captures stdin and
  prints a fixed URL. No real GitHub create command runs. MCP uses dry runs only;
  its optional status lookup is directed at loopback port zero.
- All ordered distinct type pairs are refused. Normalization, duplicate types,
  default bug, areas and roadmap remain accepted. Every force/dry-run combination
  rejects conflicting types without a fake-gh invocation. CLI and MCP request
  forwarding are exercised, along with all four requested guidance strings.
- New IPv6 module is 76 lines, new regression module 247 lines; both carry the
  existing attribution header. The pre-existing large CLI enum receives only
  its requested help-line edit, preserving CLI layout.
- No entire-suite tests, installations, infrastructure changes, issue/PR writes,
  or external messages. Strict lint is a recorded limitation, not a passing check.

## Tool failures and bounded fallbacks

Preserved unsuccessful discovery commands and their errors:

1. `gh api repos/alex09x/prod-code/issues/comments/4100229821 --jq .body`
   (also IDs 4102250606 and 4113176130): `gh: Not Found (HTTP 404)`.
   Correct pull-review comment endpoint succeeded as recorded above.
2. `prod-code outline crates/mcp/src/report`: `cannot outline: file does not exist`
   with suggestion `crates/prod-code-mcp/src/report/mod.rs`.
   `prod-code grep 'ReportIssue|AGENT_INSTRUCTIONS' --path crates/client/src`:
   `search path does not exist` for that checkout-relative path.
3. `prod-code outline crates/prod-code-mcp/src/report` using the default remote:
   no output after repeated bounded waits; cancelled to terminal exit 130 before
   any other source/test command. Subsequent explicit-node grep and file outlines
   succeeded. No automatic public issue was filed, per task's publication ban.
4. `prod-code grep 'Ipv6Addr|is_documentation|report::report|report_issue_help|mod tests' crates/prod-code-mcp/src/report crates/prod-code-client/src/cli --remote <build-node>`:
   exit 2, `unexpected argument 'crates/prod-code-client/src/cli' found`.
   Retried with the single supported `crates` path; exit 0.
5. A proposed `rg --files crates/prod-code-mcp/src/report` discovery batch was
   blocked before execution: `Supported-language source discovery goes through
   prod-code`. Removed that discovery command; used explicit bounded file reads
   and successful prod-code grep/outlines. No shell source mutation was used.

No command started by this worker remains running.
