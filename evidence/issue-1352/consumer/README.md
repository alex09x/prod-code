# Native public introduce-variable compatibility proof

Immutable endpoint candidate: `5173c99935aa6fd9833cf71898778d3cc491a7db`.
This task changes test code only. The existing five endpoint cases and their
historical logs remain intact. The original endpoint baseline was JSON-RPC
`-32601`, **not** a silent timeout; see `../baseline.txt` and `../README.md`.

## Consumer boundary

The sixth case, `consumer::public_introduce_variable_preserves_native_rust_behavior`,
calls the public `prod_code_mcp::introduce_variable::introduce_variable` function.
It selects `w + 1` in one small executable Rust source, without ScriptedGateway,
fabricated semantic answers, or an external rust-analyzer process.

The transport-only adapter is `tests/common/native_rust/bridge.rs` in the gateway
crate. Its boundaries are explicit:

1. An ephemeral loopback TCP socket and `ProdCodeCodec` carry the normal public
   MCP client's wire messages. Client and server share the fixture's actual disk
   directory; this is a consumer/dispatch proof, not a remote filesystem-sync test.
2. Manifest probes compare every supplied file's length and FNV content hash to
   disk. Sync deltas compare content/deletion to disk. Acknowledgements report no
   missing files or writes because both peers already use those exact bytes.
3. Handshake negotiates the real protocol version, verifies the fixture root and
   Rust engine selection, and registers a real `WorkspaceManager` session view.
   The path translator is identity because both peers use the same real root.
   Ordinary sessions use the fixture engine. Validation-purpose sessions obtain
   the production `validation_view`, asserted distinct from the ordinary engine.
4. Every LSP message, including initialize, notifications, syntax, definition,
   hover, document symbols and diagnostics, calls `Session::send`, hence production
   `on_client_message`. RPC replies come exclusively from its output queue, with
   IDs checked. The adapter never supplies an LSP result itself.
5. Planning loads syntax in ordinary sessions. Definitions and hover use the public
   consumer's pooled ordinary session. Native definitions resolve both occurrence
   reads to the parameter at line 1, column 12 (one-based). Native hover at the
   parameter type identifies built-in `u32`. Proposed diagnostics use real validation
   sessions and return empty items. The public planner itself checks definition
   identity and built-in hover, in addition to the harness checking real answers.
6. `tokio::join!` drives client and server together, with no detached tasks. A stop
   channel closes persistent pooled transports; every connection unregisters its
   session view before the bridge returns. The existing fixture-local lock bounds
   engine/sysroot memory. No global serial flags or resource-limit changes are used.

## Positive and refusal results

`original.rs` and `returned.rs` retain the exact source pair. Preview replaces two
occurrences with `cached` and leaves original disk bytes intact. Apply produces
the identical source and writes precisely the returned bytes. Both return no
diagnostics. Both originals and returned sources are separately compiled with
`rustc --edition=2021 -C overflow-checks=yes --error-format=short` and executed on
the Linux build node. Each compiler exits 0; each executable exits 0, prints
`2\n16\n202\n` for inputs 0, 7, 100, and has empty stderr. The test compares the
complete runtime result, checks the exact transformation, and requires success.

The same original source/selection with requested name `w` refuses as "not fresh"
for preview, apply and forced apply. Each mode asserts byte-for-byte unchanged
disk source. This control precedes, and does not substitute for, the supported
positive operation. There is no new semantic corpus.

## Commands and evidence

`$BUILD_NODE` below denotes the assigned Linux node, whose private address is
omitted. All compilation, formatting and linting run through `prod-code exec`.
Terminal transcripts normalize only node/build-home/build-checkout and random
temporary directory paths. Semantic payloads and source bytes are retained.

Development command:

```text
prod-code exec --remote "$BUILD_NODE" --timeout-secs 900 --env NATIVE_CONSUMER_EVIDENCE=evidence/issue-1352/consumer -- cargo test -p prod-code-gateway --test native_rust_extensions public_introduce_variable_preserves_native_rust_behavior -- --nocapture
```

Exit 0: one passed, five filtered out, 61.85 seconds test runtime, 108.1 seconds
server runtime (`development.txt`). Cargo runs the test from its crate directory;
the six generated source/compiler/runtime artifacts were moved to this directory
after collection. Compiler and runtime files retain actual output/exits. No
production defect was exposed; production code and Cargo manifests are unchanged.

Scoped formatting command (exit 0; `format.txt`):

```text
prod-code exec --remote "$BUILD_NODE" --timeout-secs 900 -- rustfmt --edition 2024 --config skip_children=true crates/prod-code-gateway/tests/native_rust_extensions.rs crates/prod-code-gateway/tests/common/native_rust.rs crates/prod-code-gateway/tests/common/native_rust/bridge.rs crates/prod-code-gateway/tests/native_rust_extensions/consumer.rs
```

Non-denying lint (exit 0; `lint.txt`):

```text
prod-code exec --remote "$BUILD_NODE" --timeout-secs 900 -- cargo clippy -p prod-code-gateway --test native_rust_extensions --no-deps
```

The gateway library reports 56 existing warnings; no warning points at the new
test files. Server runtime 22.4 seconds. No unrelated warning cleanup was made.

Exact final acceptance command (exit 0; `acceptance.txt`):

```text
prod-code exec --remote "$BUILD_NODE" --timeout-secs 900 -- cargo test -p prod-code-gateway --test native_rust_extensions
```

All six cases passed, zero failed, default concurrency, 210.82 seconds test
runtime, 213.8 seconds server runtime, Linux x86_64. This was the single final
compile/test gate following formatting and lint. No unchanged successful check
was repeated. `source-hashes.json` identifies the final harness and exact source
pair using SHA-256. `tool-errors.txt` retains three recovered read-only discovery
errors; none affected test execution. The full development transcript retains
the actual requests/replies and each handshake/sync/unregister boundary.

No daemon was deployed, restarted or installed; no root dependency was added.
