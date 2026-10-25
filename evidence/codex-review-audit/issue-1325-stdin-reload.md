# Issue 1325: stdin ownership at process replacement

Historical first-attempt record, retained with its original hashes. The shared-descriptor
review repair and current final verification are recorded in
`issue-1325-shared-stdio.md` and `issue-1325-shared-stdio-hashes.json`.

Base: `e74b152a0cc216daaf7f1c51fcaea89761f84b11`.
Issue: https://github.com/alex09x/prod-code/issues/1325
Original finding: https://github.com/alex09x/prod-code/pull/26#discussion_r4059399383
Dependency contract: https://docs.rs/tokio/latest/tokio/io/fn.stdin.html

The Tokio documentation confirms that stdin uses a separate blocking read which cannot
be cancelled. The original source used that reader, raced its `fill_buf` future against
reload, and authorized exec from empty outer buffers. Those buffers do not establish
ownership of bytes held by a pending worker. The original report was source analysis;
it was not an observed fresh execution of the Tokio worker race.

All build/test execution below used a Linux x86_64 build node. In command records,
`<build-node>` replaces only the private endpoint; `<worktree>` and `<skill-root>` replace
private filesystem prefixes. No service was restarted or deployed, no dependencies were
installed, and nothing was published. Source hashes, including unchanged request timeout
and exec helpers, are in `issue-1325-stdin-reload-hashes.json` alongside this record.

## Deterministic red regression, before production edits

Added `reload_pending_tests.rs` and its test-module declaration, with production transport
and reload code still at the pinned base. The test polls the server until an input read
has returned Pending while privately owning a complete ping frame, requests reload,
polls again, and asserts exec has not happened. Completion is explicitly released later.
There are no sleeps, worker-scheduling guesses, or DuplexStream input in this regression.
This is a deterministic model of worker ownership, not a claim that Tokio internals were
instrumented. The same test file SHA-256 in both the red and final green runs is:
`5e86b84bb4ca733c74adb0869e103c848f166666130a9b631bfab33f4f10f7ab`.

Command:

```text
prod-code exec --remote <build-node> --timeout-secs 900 -- cargo test -p prod-code-mcp --lib -- server::reload_pending_tests
```

Observed sanitized output (exit 101):

```text
running 1 test
test server::reload_pending_tests::server_reload_waits_for_privately_owned_pending_read ... FAILED
assertion `left == right` failed: exec would discard the worker's privately owned request
  left: 1
 right: 0
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1401 filtered out; finished in 0.00s
```

The distinguishing observation is one exec attempt before private read completion. If an
empty outer buffer proved OS ownership, that assertion could not have failed.

## Fix and ownership guarantees

- Production stdin on Unix duplicates fd 0 with close-on-exec ownership. Pollable pipes
  use AsyncFd and synchronous **nonblocking** reads. Pending means no private read worker
  exists; queued bytes remain in the OS for the replacement.
- Duplicated descriptors share status flags. Original flags are restored before exec,
  nonblocking mode is reinstated after failed exec, and original flags are restored on
  drop. A failed flag reinstatement makes subsequent reads fail instead of blocking the
  runtime unexpectedly. Standard streams remain inherited by the replacement.
- Redirected regular files use Tokio file workers rather than reactor registration or
  synchronous disk reads on the runtime. Devices rejected by the reactor also use file
  workers. The loop records Pending ownership across selection cancellation and waits
  for those reads to finish and their bytes to be processed before exec.
- Arbitrary generic AsyncRead transports use the conservative pending-read policy too.
  Only explicitly known cancellation-safe inputs may replace while a read is Pending.
  Existing DuplexStream reload fixtures explicitly identify their safe input.
- Existing frame buffering, EOF behavior, dispatch/cancellation, exactly-once active IDs,
  shared reload drain deadline, and failed-exec continuation remain in the existing loop.
  Request timeout semantics, including explicit long-running overrides, are unchanged.
- All changed source modules are at most 400 lines. The new ownership helper is in the
  stdin module; this keeps transport.rs at 400 without reorganizing unrelated behavior.

## Actual process/pipe and file evidence

`server::reload_process_tests::server_reload_real_stdin` runs isolated child processes in
four modes. A separate control socket reports that real stdin returned Pending and gates
the exec callback. The parent writes and flushes bytes into the OS stdin pipe before
releasing that gate. These are actual successful execs, not only simulated callbacks.

1. `idle-pipe`: stdin has returned Pending with no data. Reload reaches the exec gate
   without another request; bytes then queued in the pipe are answered by the replacement.
2. `partial`: the wrapper observes bytes before announcing Pending. The consumed partial
   frame must finish and ID 1 must be answered before exec; queued ID 2 is answered by the
   replacement. Responses are ordered and each ID occurs once.
3. `failed-exec`: uses a nonexistent executable and the production flag-restoring callback.
   Bytes queued before the exec attempt are answered once by the original process. EOF
   completes the loop and the child checks that the original stdin flags were restored.
4. `file`: stdin is an actual redirected regular file containing two requests. Reload
   before reading preserves its offset; the replacement's production stdin entry point
   answers both IDs once and terminates at EOF.

Successful replacements check that stdin did not inherit O_NONBLOCK. Every response ID
must have the successful empty ping result, not merely a matching error ID. Announcement
counts distinguish the one failed-exec attempt from successful replacement/resumption.
The replacement calls `run_stdio_mcp_server` itself. Control messages, rather than sleeps,
establish ordering. Timeouts only bound broken-test cleanup; they are not race evidence.

`server_reload_device_eof_and_regular_file_input` additionally reads `/dev/null` to EOF
and checks asynchronous regular-file input and unchanged file flags. The existing tests
exercise active-call completion before notification, bounded drain with retryable errors,
partial frames, numeric/string/null IDs once, cancelled calls, descriptor preservation,
transport errors, JSON-RPC framing, and response bounds.

## Focused verification and final source state

The first coherent implementation passed 13 tests (14.9s total). After formatting and
strengthening ping-result assertions, the same selection passed 13 tests (11.2s total).
Source review then extracted only the new read-tracking helper to meet the module limit;
that changed source state passed 13 tests (11.7s total). A final correction retained async
regular-file workers to avoid slow disk I/O blocking the drain clock. The exact final
acceptance command and output follow; no implementation changed after this final run.

```text
prod-code exec --remote <build-node> --timeout-secs 900 -- cargo test -p prod-code-mcp --lib -- server::reload server::transport_serve_tests
```

Observed sanitized final output (exit 0):

```text
Finished `test` profile [unoptimized + debuginfo] target(s) in 8.43s
running 13 tests
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 1391 filtered out; finished in 2.23s
[prod-code exec] exit 0 in 11.1s (server 10.7s, cpu 17.1s user 2.6s sys, peak 2032 MB) on node <build-node>, copy <worktree> (linux x86_64)
```

Final acceptance includes both new regression modules, the original reload tests, and
the requested transport_serve_tests. No full workspace suite, benchmark, or standalone
build was run. The final acceptance command compiled the changed deliverable.

Scoped formatting was run once for the eight changed Rust files using:

```text
prod-code exec --remote <build-node> --timeout-secs 900 -- rustfmt --edition 2024 --config skip_children=true crates/prod-code-mcp/src/server/mod.rs crates/prod-code-mcp/src/server/reload.rs crates/prod-code-mcp/src/server/transport.rs crates/prod-code-mcp/src/server/stdin.rs crates/prod-code-mcp/src/server/stdin_unix.rs crates/prod-code-mcp/src/server/reload_tests.rs crates/prod-code-mcp/src/server/reload_pending_tests.rs crates/prod-code-mcp/src/server/reload_process_tests.rs
```

Exit 0; only mod.rs and reload_process_tests.rs were reformatted. After the later scoped
helper/file correction, formatting of those changed scopes was checked without edits:

```text
prod-code exec --remote <build-node> --timeout-secs 60 -- rustfmt --check --edition 2024 --config skip_children=true crates/prod-code-mcp/src/server/stdin.rs crates/prod-code-mcp/src/server/stdin_unix.rs crates/prod-code-mcp/src/server/transport.rs crates/prod-code-mcp/src/server/reload_process_tests.rs
```

Exit 0 in 0.4s. `git diff --check` also returned zero.

## Strict lint failure (not a passing gate)

```text
prod-code exec --remote <build-node> --timeout-secs 900 -- cargo clippy -p prod-code-mcp --lib --tests -- -D warnings
```

Exit 101 (24.4s total), before the final helper/file correction. Exact diagnostic excerpts:

```text
error: this `if` statement can be collapsed
  --> crates/prod-code-mcp/src/config/mod.rs:52:9
error: could not compile `prod-code-mcp` (lib) due to 103 previous errors
error: could not compile `prod-code-mcp` (lib test) due to 107 previous errors
```

Read both existing Cargo fingerprint output files with a remote Python read-only command
instead of rerunning lint. There were 212 error records, including the two final summaries.
The only diagnostics with primary server-file spans were the pre-existing collapsible-if
in failover.rs:131 (duplicated for lib/lib-test). None referenced a changed file. Lint was
not repeated or represented as passing; no unrelated code was changed to satisfy it.

## Other failed tool calls and bounded fallbacks

Initial read-only discovery was rejected before execution:

```text
gh api repos/alex09x/prod-code/issues/1325 --jq '{title,body}' && gh api repos/alex09x/prod-code/issues/comments/4059399383 --jq .body && sed -n '1,130p' <skill-root>/remote-browser/SKILL.md && sed -n '1,120p' crates/prod-code-mcp/src/hot_reload.rs && rg -n '^tokio|features.*full' Cargo.toml
```

Exact error prefix: `Command blocked by PreToolUse hook: Supported-language source
discovery goes through prod-code: grep, rg, find and recursive scans over source are
blocked.` The fallback removed the text search and used explicit file slices.

The next compound read command successfully fetched issue 1325, then stopped at the
incorrect issue-comment endpoint (the later file reads in the chain did not execute):

```text
gh api repos/alex09x/prod-code/issues/1325 --jq '{title,body}' && gh api repos/alex09x/prod-code/issues/comments/4059399383 --jq .body && sed -n '1,130p' <skill-root>/remote-browser/SKILL.md && sed -n '70,118p' crates/prod-code-mcp/src/hot_reload.rs && sed -n '35,80p' Cargo.toml
gh: Not Found (HTTP 404)
{"message":"Not Found","documentation_url":"https://docs.github.com/rest/issues/comments#get-an-issue-comment","status":"404"}
```

The bounded fallback used `gh api repos/alex09x/prod-code/pulls/comments/4059399383
--jq '{path,line,body,html_url}'`, which succeeded and identified PR 26's original finding.
No issue, PR, comment, or other external message was written.

## Platform limits and remaining coverage

Measured: Linux x86_64, actual process stdin pipes, redirected regular files, /dev/null,
successful exec, failed exec, and the focused existing transport/drain tests.
Other Unix systems use the same AsyncFd path where their reactor accepts the descriptor;
their kqueue/device behavior has not been executed in this task. No Windows build or test
was run. Non-Unix production stdin continues through Tokio without hot reload, explicitly
warning that process replacement needs Unix exec (the existing exec helper is Unsupported
there). Normal stdin/EOF behavior on those targets is retained by design, not newly measured.

Regular files, reactor-rejected devices, and arbitrary generic inputs may delay replacement
until an outstanding worker read returns. Active calls still use the original fixed drain
deadline, so that safety wait does not grant new calls another execution budget. An indefinitely
blocked non-pollable device can therefore delay exec; pollable stdin pipes keep idle reload.
No claim is made that all Tokio stdin workers have become cancellable: production Unix pipes
no longer use such a worker, and fallback readers retain their bytes until completion.
