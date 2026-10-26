# Native convert-to-method consumer verification

Base: `0db89d215af8e7c8dca97eaa6e68dc0ba01d42e0`. Issue context: #1316;
the transport fixture is retained from #1352. This bounded assignment permits
test/evidence changes and explicitly excludes production repairs and publication.

The dedicated `rust_method_native` gateway integration target calls public
`prod_code_mcp::to_method::convert_to_method`. It adds three cases:

* Exact nominal `S` receiver, two direct calls through separate bindings: preview
  and apply must convert both calls to dot syntax, preserve output `7 41`, and
  have real validation definitions resolve to the converted declaration.
* `fixed!()` expands to `[u8; { let U = 1; U }]`, alongside unrelated method type
  parameter `U`: preview/apply must succeed, rename the parameter use, preserve
  the generic method, and preserve output `7`.
* `identity!(A)` actually captures a method type parameter in the receiver:
  preview/apply, with and without force, must refuse with the capture explanation
  and leave bytes unchanged. The original source prints `2`.

Every original and every successfully returned source is retained and compiled
using real `rustc --edition=2021 -C overflow-checks=yes --error-format=short`;
each executable is run and its complete stdout compared. Compiler/runtime
artifacts record actual exit status, stdout and stderr.

## Boundary and preservation

The existing fixture-local semaphore and joined Bridge transports are reused
unchanged. The shared-disk sync/handshake boundary is documented in
`evidence/issue-1352/consumer/README.md`: probes/deltas verify actual disk bytes;
handshakes register native session views; validation sessions use the production
private overlay; every semantic request goes through real `on_client_message`
and replies come exclusively from production dispatch. No ScriptedGateway data,
external rust-analyzer, detached provider, global serialization, raised limits,
dependency changes or service changes are used.

Each case retains `semantic-replies.json` with the public consumer's actual
requests, responses, session IDs and validation flags. `control-probes.json`
contains separately labelled native definition/hover/typeDefinition observations;
these are never provided to the public consumer as semantic input. Public method
coverage and dispatch identity assertions use only Bridge answers.

The previous 55 preservation tests/fixtures and six native endpoint/introduce
variable cases remain byte-identical to the assigned base. This task does not
rerun those already-recorded parent checks. Final hashes identify the new
harness, preserved inputs and generated evidence.

## Commands

`<build-node>` denotes the assigned Linux x86_64 node. Formatting is scoped to
the two new Rust files. Lint is non-denying and scoped to the dedicated target.
`format.txt`, `lint.txt` and `acceptance.txt` retain terminal output and exits;
only private node and remote checkout paths are normalized in command logs.
`tool-errors.txt` records the rejected read-only discovery command and fallback.

Final gate (with the original default test concurrency):

```
prod-code exec --remote <build-node> --timeout-secs 900 -- cargo test -p prod-code-gateway --test rust_method_native
```

## Observed result: blocked, not a completed nominal proof

The first command lost its output stream during dependency compilation and
returned client exit 1 with the remote result unknown (`acceptance-initial.txt`).
Status subsequently confirmed that this assigned command had stopped before the
same command was retried. `recovery-status.txt` redacts an unrelated command;
the node reported 99% memory use. No resource settings or services were changed.

The complete target then returned **101**, with two cases successful and one
failed (`acceptance.txt`, test runtime 173.72s). The macro-local-value case
successfully previewed/applied the receiver conversion and both returned sources
compiled and printed `7`. Actual expanded method-type capture refused all four
preview/apply/force combinations before writes, with the expected capture reason;
its original source compiled and printed `2`. These are case results within a
failed command, not an independently successful acceptance gate.

The exact nominal case returned successful preview/apply plans, renamed one
parameter use, reported no diagnostics, and preserved output `7 41` in real
compiler/runtime checks. However **both plans converted zero of the required two
direct calls**, retaining `S::f(&a)` and `S::f(&b)`. Both public owner-proof
requests at zero-based line 1, character 5 returned native JSON-RPC **-32601**:
`textDocument/typeDefinition is not supported by prod-code for Rust`.
`nominal/public-false.txt`, `nominal/public-true.txt` and
`nominal/semantic-replies.json` retain the exact operation and answers.

This prevents owner identity proof before hover or proposed dot-call definition
validation. Native dot-call dispatch identity is therefore **not established**.
The missing native typeDefinition capability and the consumer's required owner
proof are the concrete production boundary for the parent to repair. No endpoint,
MCP or other production code was changed, and the expected call count stays two.

After inspecting real traffic, one harness-only request assertion was corrected:
syntax-tree coverage belongs to macro cases, because the nominal operation never
requests it. Scoped format and non-denying lint were rerun on that completed
correction (`format-final.txt`, `lint-final.txt`, both exit 0; the library retains
56 pre-existing warnings). Only the failed nominal case was rerun:

```
prod-code exec --remote <build-node> --timeout-secs 900 -- cargo test -p prod-code-gateway --test rust_method_native exact_nominal_receiver_converts_direct_calls_with_native_dispatch_identity
```

It again returned **101**, zero converted calls versus two required, 54.00s test
runtime (`nominal-final.txt`). No successful macro case was redundantly rerun.
The previous complete-target nominal artifacts are preserved separately in
`full-target-nominal/`. `nominal/` contains final-state public plans/replies;
byte-identical source/compiler/runtime artifacts are reused. Every transport
unregistered before each assertion failure. All started commands have finished.

`preserved-input-hashes.json` confirms 23 existing test/fixture files are
byte-identical to the pinned base, including all preservation cases and the
previous six native cases. `source-hashes.json` records final harness and evidence
SHA-256 values. Acceptance remains failed; the parent must scope the production
repair before the required nominal behavior can be proven.
