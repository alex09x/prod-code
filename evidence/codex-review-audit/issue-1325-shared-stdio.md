# Issue 1325: shared standard-stream review repair

Pinned acceptance base: `e74b152a0cc216daaf7f1c51fcaea89761f84b11`.
Repair starts from retained attempt `371aad53`.
The original deterministic pending-read red regression and original source hashes remain
in `issue-1325-stdin-reload.md` and `issue-1325-stdin-reload-hashes.json`.
This record supplements that evidence; it does not claim a fresh execution of the
original Tokio blocking-worker race.

All execution below was on a Linux x86_64 build node. `<build-node>` and `<worktree>`
replace private endpoints and paths only. No restart, deploy, install, publication,
external message, or delegation was performed. The source manifest alongside this record
captures the acceptance base, retained attempt, and final SHA-256 of each relevant source.

## Failure and distinguishing observation

The retained Unix adapter enabled O_NONBLOCK on a duplicate of stdin. If stdout shares
the same open-file description, that changes stdout too. Production Tokio stdout uses
blocking writes, so changing that description can produce WouldBlock under backpressure.
The expected behavior is cancellation-safe input while output retains its incoming flags.
The alias hypothesis predicts an observable stdout/stdin flag change even before output
is full; unchanged flags on a duplicated socket would contradict it.

Before production edits, extended the real-process stdin regression to duplicate one
socket endpoint into both child stdin and stdout. After opening production stdin, the
child asserts that the socket flags remain unchanged. This deterministically failed.

Command (exit 101):

```text
prod-code exec --remote <build-node> --timeout-secs 900 -- cargo test -p prod-code-mcp --lib -- server::reload_process_tests::server_reload_real_stdin
```

Sanitized observed output:

```text
Finished `test` profile [unoptimized + debuginfo] target(s) in 34.12s
test server::reload_process_tests::server_reload_real_stdin ... FAILED
socket: control handshake timed out
child stderr: assertion `left == right` failed: socket stdin changed shared stdout flags
  left: 2050
 right: 2
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1403 filtered out; finished in 10.00s
[prod-code exec] exit 101 in 45.1s (server 44.2s, cpu 45.6s user 7.3s sys, peak 2756 MB) on node <build-node>, copy <worktree> (linux x86_64)
```

The parent timeout was cleanup after the child's flag assertion, not the race oracle.
The production input adapter at this red run had SHA-256
`44074ca1ffabf74cada49764b3d82312d85814d9a3639195ba3fa3377ae8d00d`.
The red process test had SHA-256
`aaa6b1c22ed49759671325d51372ecfb0786a11d15b71f5cd4640759b592ef1c`.

## Repair and exercised behavior

`crates/prod-code-mcp/src/server/stdin_unix.rs:137` uses recv(MSG_DONTWAIT) for sockets.
This is nonblocking for that read alone; socket flags never change on open, drop,
successful exec or failed exec. AsyncFd still supplies readiness, Pending has no private
worker, and socket idle reload remains available. stdout and stderr aliases retain their
existing blocking behavior.

`crates/prod-code-mcp/src/server/stdin_unix.rs:83` retains the asynchronous-worker
ownership policy for regular files and aliased non-socket inputs. The output metadata
comparison at line 121 deliberately accepts false positives: two separately opened
descriptions of the same object also take the conservative path. An unreadable/closed
output descriptor also selects this fallback instead of mutating uncertain shared flags.
Unaliased pollable pipes keep the original cancellation-safe readiness path.

`crates/prod-code-mcp/src/server/reload_process_tests.rs:161` assigns real socket duplicates
to child stdin/stdout. Its socket and socket-failed-exec modes reuse the explicit Pending
and gated-exec protocol, verify unchanged flags, successful ping results, exactly one
response ID, EOF termination, announcement counts and continuation after failed exec.
The successful socket mode actually execs the child and serves the queued bytes using
run_stdio_mcp_server. Pipe, partial-frame, file, device EOF and pending-worker tests remain.

`crates/prod-code-mcp/src/server/reload_shared_stdio_tests.rs:22` opens production input
on a shared socket, checks stdout flags, and polls input Pending. It fills the actual
output socket with per-call nonblocking sends until WouldBlock, while the parent has not
read any stdout. It polls the existing reload drain with production Tokio stdout and a
1 MiB response, then lets the parent read through an explicit control message. The parent
checks all filler bytes and the complete response payload before checking child success.
Timeouts only bound failures; there are no sleeps or timing races used as the oracle.
The response task gets one scheduler yield; the test does not instrument Tokio's worker
internals or claim to measure the precise time at which that worker enters write().

No request timeout, frame parser, reload deadline or response-delivery implementation was
changed by this repair. All changed source modules remain below 400 lines.

## Checks

Focused development command (exit 0, before final formatting):

```text
prod-code exec --remote <build-node> --timeout-secs 900 -- cargo test -p prod-code-mcp --lib -- server::reload_process_tests server::reload_shared_stdio_tests
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 1402 filtered out; finished in 0.04s
[prod-code exec] exit 0 in 9.7s (server 9.3s, cpu 16.3s user 2.8s sys, peak 2065 MB) on node <build-node>, copy <worktree> (linux x86_64)
```

Scoped formatting, once on the completed repair (exit 0):

```text
prod-code exec --remote <build-node> --timeout-secs 900 -- rustfmt --edition 2024 --config skip_children=true crates/prod-code-mcp/src/server/mod.rs crates/prod-code-mcp/src/server/stdin_unix.rs crates/prod-code-mcp/src/server/reload_process_tests.rs crates/prod-code-mcp/src/server/reload_shared_stdio_tests.rs
[prod-code exec] 3 file(s) changed by the command written back: crates/prod-code-mcp/src/server/reload_process_tests.rs, crates/prod-code-mcp/src/server/reload_shared_stdio_tests.rs, crates/prod-code-mcp/src/server/stdin_unix.rs
[prod-code exec] exit 0 in 0.4s (server 0.1s, cpu 0.0s user 0.0s sys, peak 40 MB) on node <build-node>, copy <worktree> (linux x86_64)
```

Strict lint, once (exit 101, not a passing gate):

```text
prod-code exec --remote <build-node> --timeout-secs 900 -- cargo clippy -p prod-code-mcp --lib --tests -- -D warnings
error: this `if` statement can be collapsed
  --> crates/prod-code-mcp/src/config/mod.rs:52:9
error: could not compile `prod-code-mcp` (lib) due to 103 previous errors
error: could not compile `prod-code-mcp` (lib test) due to 107 previous errors
[prod-code exec] exit 101 in 23.0s (server 22.7s, cpu 30.5s user 4.1s sys, peak 1885 MB) on node <build-node>, copy <worktree> (linux x86_64)
```

Read both existing Cargo fingerprint output files via remote Python without rerunning
lint: 212 error records including summaries. Only two primary server-file diagnostics
occurred, both the unchanged failover.rs:131 collapsible-if. No primary diagnostics
referenced changed files. Counts match the retained first-attempt lint evidence.

Required final acceptance and sole final compile/test gate (exit 0):

```text
prod-code exec --remote <build-node> --timeout-secs 900 -- cargo test -p prod-code-mcp --lib -- server::reload server::transport_serve_tests
Finished `test` profile [unoptimized + debuginfo] target(s) in 5.97s
running 14 tests
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 1391 filtered out; finished in 2.21s
[prod-code exec] exit 0 in 8.6s (server 8.3s, cpu 8.2s user 1.8s sys, peak 1953 MB) on node <build-node>, copy <worktree> (linux x86_64)
```

This includes both new process test modules, the deterministic private-byte model, and
the existing drain deadline, partial-frame, exactly-once ID and transport tests.
No source changed after final acceptance. No full workspace suite or standalone build ran.

## Failed discovery call

Before any source read in this compound command, the hook rejected:

```text
git diff --stat e74b152a && sed -n '1,240p' crates/prod-code-mcp/src/server/stdin_unix.rs && sed -n '1,200p' crates/prod-code-mcp/src/server/stdin.rs && rg --files evidence/codex-review-audit
```

Exact error prefix: `Command blocked by PreToolUse hook: Supported-language source
discovery goes through prod-code: grep, rg, find and recursive scans over source are
blocked.` Bounded fallback removed rg, listed only the named evidence directory, read
explicit slices, and used prod-code outline for source navigation. Nothing was published.

## Limits

Executed platform: Linux x86_64; pipes, regular-file stdin, /dev/null and duplicated Unix
socket standard streams. Socket flag preservation, successful/failed process replacement,
and full-response output under kernel backpressure were observed. macOS/kqueue and other
Unix targets were not executed; they use the same socket per-call flag and readiness APIs.
Non-Unix retains the existing explicitly unsupported exec/hot-reload policy, with normal
Tokio stdin/EOF service; no Windows build or runtime verification occurred.

Aliased terminals/FIFOs take the conservative worker path; no terminal/FIFO alias runtime
test was executed. An outstanding read can delay idle replacement indefinitely on those
inputs, as on reactor-rejected devices. This preserves byte ownership and output flags;
it does not disable pipe/socket idle reload or extend the active-call drain deadline.
Already-nonblocking output supplied by a caller keeps its incoming flags; this task does
not add a readiness writer for that pre-existing configuration.
