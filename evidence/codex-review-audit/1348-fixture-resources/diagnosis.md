# Fixture resource diagnosis for issue #1348

Base: `4ab3fa1610b61e6ebb4f7e0f0ec94d2745fed6e1`. The candidate
`3690e4f895194e9e901e280e1ca507edab481f41` differs from that base only in the
two retained parent failure evidence files. No production code is changed here.

The observed failure is EMFILE during operations that need another descriptor;
the expected result is all 55 semantic tests passing with default concurrency
and the existing soft nofile limit of 1024. Competing explanations were temporary
concurrent fixture demand, accumulation beyond fixture lifetime, and analyzer
child leakage. Raising the limit or serializing tests would not distinguish them.

## Bounded observations before edits

`observe.py` launches the stated cargo command in the foreground through the
existing Linux remote, samples only that cargo process's preservation-test child,
and records descriptor kinds, socket states, empty inotify handles and that
process's actual limits. It does not enumerate unrelated processes. TCP tables
are filtered by the test child's owned socket inodes; no addresses or paths are
recorded in descriptor snapshots. Samples are not an atomic census or an absolute
peak; descriptor and TCP state enumeration can race.

- `ordinary-before.json`: the smallest previously failing test passes alone, but
  reaches 108 sampled descriptors, including 16 listeners, 32 established TCP
  sockets and 16 inotify handles, 15 already without watches. Its source contains
  four source cases times four modes, each creating a fresh root and endpoint.
- `target-before.json` and `target-before.txt`: the complete unchanged target
  reproduces 54/55 with EMFILE while launching a returned program, sampled peak
  1004 descriptors under soft 1024/hard 1000000. At that sample, 121 inotify
  handles have no watches and 118 TCP sockets are in CLOSE_WAIT (hex 08).
  This is cumulative retained ownership, not simply 55 simultaneous live fixtures.

## Ownership and causal boundary

`crates/prod-code-testkit/src/scripted_gateway.rs:56` moves the listener into an
unjoined Tokio accept task. Each accepted socket belongs to a spawned `serve`
task. Dropping the returned address-holder does not stop these tasks; in these
tests they end when their owning test runtime shuts down. The doc's process
lifetime wording does not imply that a test runtime survives process-wide.

`crates/prod-code-mcp/src/session/pool.rs:26` owns sessions in a process-wide map;
keys distinguish both root and endpoint. The client endpoints survive test
runtime shutdown in that map. `crates/prod-code-mcp/src/watch.rs:42` owns watcher
registrations in another process-wide map; deleting a temporary tree removes its
watches without dropping the stored watcher. Thus empty inotify handles and
CLOSE_WAIT client sockets have source-confirmed owners. We do not assign every
epoll/eventfd to a particular task from type counts alone.

The analyzer helper already kills and waits for its child on Drop. The isolated
ordinary-generics test never invokes that helper and still accumulates descriptors;
analyzer children are therefore not required to produce the observed growth.
There is no evidence here authorizing an analyzer lifecycle rewrite.

## Repair and preservation

Create each source/script workspace and gateway once, outside apply/force loops,
then write the exact original main file before each mode. Distinct source cases
and distinct scripted semantic answers still get distinct fixtures. This changes
fixture multiplicity from one per mode to one per case; it does not serialize,
ignore, filter or weaken tests. See
`crates/prod-code-mcp/tests/rust_method_preservation/declaration_identity.rs:189`
and the helper contract at
`crates/prod-code-mcp/tests/rust_method_preservation/support.rs:18`.

All assertions, source scenarios, real compiler/runtime checks, and apply/force
combinations remain. No new semantic corpus or Cargo changes were added. Existing
production retention policy is unchanged; a general eviction/teardown API would
be a separate production scope, not part of this target repair.

## Verification and limits

- Focused causal comparison: `ordinary-after.json` has four listeners instead of
  sixteen and terminal exit 0. Sampled descriptor peak is 24. Inotify availability
  differs between the two focused runs (zero observed after), so the full numeric
  drop is not attributed solely to the edit; listener multiplicity directly tracks
  the source change.
- One scoped remote rustfmt application: exit 0, exact file list in `format.json`.
- One scoped non-deny Clippy: exit 0, `cargo clippy -p prod-code-mcp --test
  rust_method_preservation`. The log reports 103 library warnings; no test-target
  diagnostics. This is not a warnings-as-errors claim.
- The first final observer failed during a /proc teardown race. Its raw cargo
  output and sanitized error are retained in `acceptance-observer-failed.txt` and
  `tool-failures.json`; that invocation is not claimed as a passing gate. Its
  exact target was confirmed no longer live before another test command. The
  observer was repaired to catch teardown errors and join its child even on failure.
- Final complete target: terminal cargo and remote command exit 0, 55 passed,
  zero failed/ignored/filtered, default concurrency, 7.80 seconds. See
  `acceptance.txt:61` and `acceptance.json`. Sampled peak 633 descriptors with
  inotify present (95 handles), under the unchanged 1024/1000000 limits. The wrapper
  executes exactly `cargo test -p prod-code-mcp --test rust_method_preservation`;
  it sets no resource limits or test thread options. No successful unchanged gate
  was repeated for reassurance.
- `preservation.json`: identical 55-test identity sets before and after; 214
  inherited evidence files verify against `inherited-evidence.sha256`.
- `source-before.sha256` preserves starting source identities;
  `source-final.sha256` records the final 12 changed Rust files. A separate
  `git diff --exit-code` confirms unchanged production sources, testkit, analyzer
  helper, root manifest and lockfile. Parent retains aggregate semantic review
  and integration ownership; a passing target is not approval to integrate.

No restart, configuration change, installation, deployment, publication, messages,
other-worktree work, delegation or commit was performed. Exact failed discovery
commands and bounded fallbacks are retained in `tool-failures.json`.
