# Contributing

prod-code is developed in the open, and every change goes through the same path,
whether a person or an agent writes it. There is no hosted CI: checks and benchmarks run on
the project's build nodes, which are far faster than a shared runner, and the pull request
records what was run and what it showed.

## One unit of work = one issue + one pull request

1. **Open an issue first**, and make it reproducible. An issue states the problem with the
   exact command that shows it, the output it gives today, and the output that would be
   right. "Hover is slow" is not an issue; this is:

   ```text
   $ PROD_CODE_TIMING=1 prod-code hover src/lib.rs 130 8
   [timing] total=129.7ms connect=1.0ms preflight_sync=116.6ms handshake=0.7ms ...
   ```
   > the pre-flight sync spends 116 ms in three git subprocesses for a query the gateway
   > answers in 3.7 ms; expected: one git status, under 40 ms.

   Roadmap phases are tracked as `roadmap` issues; concrete work gets its own issue that
   references the phase.
2. **Branch from `main`**: `feat/<topic>`, `fix/<topic>`, `perf/<topic>`, `docs/<topic>`.
   Nothing is committed to `main` directly.
3. **Follow the Development validation policy on a build node**: during development,
   run only new or directly affected tests; do not run broad workspace suites, formatters,
   or linters in the edit loop. Immediately before committing, run formatting and lint
   checks once on the completed change (`cargo fmt --all -- --check`, `code_lint`), then
   ONE final build/test gate for the changed deliverable (`code_check` / `code_test`) on a
   build node, never locally. For scripts, run their own focused checks and tests. Exercise
   the scenario from the issue, against a running gateway when it involves one.
4. **Open the pull request** with the template. The PR must let a reader repeat what you
   did: the commands you ran, their output before and after, on which kind of node
   (described generically: "32-core Linux node", "developer workstation"). Before/after
   numbers are required for anything about latency, throughput, memory or correctness rates.
   Reference the issue with `Closes #<n>`.
5. **Merge**: squash-merge once the checks in the PR are recorded and reviewed; delete the
   branch. The squash commit message is the PR title plus its Problem/Change summary.
6. **Ship the knowledge with the code**: update `ROADMAP.md` when a phase item lands,
   `CHANGELOG.md` for user-visible changes, and the CLI/MCP help texts.

## What counts as finished

Before implementation, state the observable acceptance criteria in the issue: the supported
languages and input shapes, the expected output, and the command that decides whether it works.
Keep the original requirement visible when only part of it ships. A roadmap parent stays `[~]`
while any required child or language remains unimplemented or unverified; a Rust implementation
does not complete the corresponding Go, JavaScript, Python, C++, or Swift work.

Each completed item needs a source location, a regression or integration test, and recorded
results for its acceptance scenario. Distinguish these states in the issue or PR:

- **Implemented**: the code and its public entry points exist.
- **Verified**: the stated checks passed on the exact proposed revision, with the results recorded.
- **Released**: a published version contains the change.
- **Deployed**: the running service or installed client was checked after installation.

A merged PR proves neither release nor deployment. An unavailable engine, skipped test,
timeout, empty report, or missing measurement is a verification gap, not a passing result.
Record the blocker and leave the affected acceptance criterion open.

For bug fixes, demonstrate that a focused regression fails on the base revision for the
reported reason and passes with the fix. A compilation or environment failure before the
assertion does not reproduce the bug. For language-server adapters and refactorings, include a
real-server scenario in each newly supported language: a mock returning a requested answer
cannot establish the server's coordinate encoding, reference completeness, or edit behavior.
Check refusal cases as well as successful edits, and verify that a failed write preserves all
original paths and contents.

Record the exact command, revision, exit status, test counts (including ignored tests), and
relevant output. Preserve the status of the checked command when filtering its output; the
success of `tail`, `grep`, or a logging step does not mean the test passed. Compare performance
on the same build profile, workload, cache state, and node class. Report cold and warm runs
separately, including errors and timeouts rather than percentiles of successes alone.

Remote checks have an execution identity as well as a command. When a tool returns a running
session or job handle, retain it and resume that execution until its terminal status is known.
A quiet or yielded command is still running; do not launch the same check again merely because
its handle or output was lost. First inspect the owned execution. If its result cannot be
recovered, record that evidence gap and establish that it has finished before a replacement run.

Use a bounded remote timeout and cancel only the exact execution or process group owned by
that check. Do not use process-name patterns, `pkill` or `killall` to clean up a shared build
node. Keep complete command output with its revision and final status; tail excerpts are useful
for progress but cannot establish full test counts. A callback or interrupted client is not
proof that the remote process finished or was canceled. Reuse completed verification when the
source is unchanged; repeat a check only after a relevant change, failure or unresolved concern.

Test fixtures that spawn child processes own each child immediately after spawn, before any
readiness wait or output wait. Cleanup must be bounded and target only the exact owned child or
process group. On failed readiness, timeout, panic or cancellation, reap the child and descendants
and retire reader tasks. A leader exit does not prove the fixture is gone when descendants can keep
stdout or stderr open.

A released dependency bump remains a version/lockfile change followed by the consumer's
existing checks. Do not add tests of the dependency's own algorithms or an extra audit unless
the requested work includes that scope.

## Labels

Every issue and every pull request is labelled when it is opened, not later:

- **One type**: `bug`, `enhancement`, `documentation` or `perf` (latency, throughput, memory).
  Pull requests that only add tests carry `test`; a release carries `release`.
- **The areas it touches**, one or more: `gateway` (the server daemon and its engines),
  `client` (the CLI, sync and the `lsp` bridge), `mcp` (the MCP tools), `cluster` (placement
  and gossip across nodes), `worktree` (per-worktree copies and isolation), `infra` (node setup,
  deploy scripts, the coverage gate), `test` (tests and coverage).
- `roadmap` marks a roadmap epic, which concrete issues reference.

A pull request carries the labels of the issue it closes. `prod-code report-issue --label <name>`
(repeat it) and `code_report_issue {labels: [...]}` label the issue they file, with `bug` when no
type is given; by hand it is `gh issue create --label bug --label gateway` and
`gh pr create --label ...`.

## Reproduction commands that belong in issues and PRs

```sh
# Per-phase timing of one query
PROD_CODE_TIMING=1 prod-code hover <file> <line> <col>

# Correctness across diverged worktrees, with latency percentiles
prod-code divergent-bench --base-repo <repo> --persistent --workers 24 --queries-per-worker 50

# Sustained load with pipelined persistent sessions
prod-code bench --workspaces <checkout> --concurrency 16 --depth 4 --duration-secs 10

# Node health and where a checkout is placed
prod-code status
prod-code cluster

# Gateway log on a node (systemd user unit)
journalctl --user -u prod-code-gateway -o short-precise --since "-10min"
```

## What never goes into the repository, issues or PRs

- Addresses, host names, paths or names of private machines and networks. Describe
  hardware generically ("a 32-core Linux node", "a macOS node", "the developer's
  workstation").
- Credentials, tokens, keys, or files that contain them.
- Internal tools and knowledge bases that are not part of this project.
- Generated co-authorship or "generated with" lines in commits or PR descriptions.

## Style

- Rust 2024 edition, `rustfmt` defaults, clippy clean with `-D warnings`.
- Errors carry context (`anyhow::Context`); logs use `tracing` with structured fields.
- A change that alters behaviour comes with a test that fails without it.
- Comments explain why, not what; keep them short.

## Coverage

The required floor is 80% of regions per Rust source file. The gate runs on a build node
like everything else; the result belongs to the revision it measured:

```sh
prod-code exec --timeout-secs 1800 --no-pull -- python3 scripts/coverage.py --min 80
```

It names each file under the bar and how many regions it is short. There are no exemptions;
if a new file cannot reach the bar, that is worth a sentence in the pull request rather than an
entry in `EXEMPT`.

Coverage is evidence only for files actually measured in the report. Missing or malformed
measurements must fail the gate; an existing source file absent from the report is not a file
with no executable code. A zero-region result must be explicitly present in the report. Run
`python3 -m unittest discover -s scripts -p test_coverage.py` through remote execution when
changing the coverage gate. Coverage percentages supplement the behavioral and real-server
checks above; they do not replace them.

Most orchestration tests do not need a live analyzer: `crates/prod-code-testkit` stands up a gateway that
answers each LSP method from a closure, and `crates/prod-code-mcp/tests/orchestration.rs` is
the worked example. The exception is `crates/prod-code-gateway/tests/live.rs`, which starts the
real daemon and drives it against real language servers. These tests complement the mock
tests by proving that real workspaces load and the adapter speaks the server's protocol.
