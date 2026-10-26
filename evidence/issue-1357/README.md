# Native Rust method navigation repair (#1357)

Pinned base: `7503df742797f6e1a26dc8cc3ab7f610a20d2e17`.
The failed baseline remains unchanged in
`evidence/issue-1316/native-consumer/README.md`, `nominal-final.txt` and
`nominal/semantic-replies.json`: both preview and apply converted **0/2** calls,
and the actual impl-owner type-definition request returned **-32601**.
Its complete original/returned compiler and runtime evidence is retained there.

## Demonstrated cause and repair

The pinned analyzer has `Analysis::goto_type_definition(FilePosition)`. The
native editor adapter and gateway did not expose it. The repair registers
`textDocument/typeDefinition` and advertises its capability, executes it through
the existing bounded native snapshot/session/worktree preparation, validates file
URIs and exact UTF-16 positions, and maps every native target's selection range
and view-aware path to an LSP Location. Malformed requests return -32602; empty
and multiple results remain empty and multiple. Numeric/string request IDs are
preserved, and notifications/null IDs produce no replies.

Real control probes establish the relevant query semantics: at the nominal
**declaration** name (zero-based 0:7), type-definition returns `[]`; at the actual
**impl owner** (1:5), it returns the declaration at 0:7 with selection end 0:8.
Definition at the impl owner resolves to the same declaration start. Therefore
the consumer's existing impl-owner type-definition request is correct and no
change to its identity acquisition or method planner is needed.

The focused nominal run proves both public preview and apply convert **2/2**
calls to `a.f()` and `b.f()`. Each proposed dot call has a real private-validation
definition reply identifying the converted declaration. The original and both
returned sources are compiled with the original rustc command and print `7 41`.
Public requests/replies, separate control probes, exact programs, full compiler
stderr/stdout and runtime exits/stdout/stderr are in `native-consumer/nominal/`.

## Controls and preservation

The dedicated target retains all original assertions and both macro cases.
Its additional navigation case checks capability advertisement, string/integer
IDs, notification handling, missing/invalid/foreign document URIs, fractional,
negative, overflowing and out-of-document coordinates, split UTF-16 pairs,
CRLF, same-named distinct types, empty primitive types and multiple generic type
candidates. It checks base, attached worktree, private validation overlay and
unchanged sibling sessions, returning to the proposal after switching views.
Actual control traffic is retained in `native-consumer/navigation/replies.json`.

The 55 semantic preservation tests, their sources and fixtures, the transport
fixture, and source-native extension implementations remain unchanged. Existing
deref/ambiguous-owner qualification and capture refusal (including force) are
preserved. There are no dependency, production cache, other planner, service,
deployment or installed binary changes.

## Verification

`run-final.py` is the one final logical gate, stopping on the first failure:

```
cargo test -p prod-code-mcp --test rust_method_preservation
cargo test -p prod-code-gateway --test rust_method_native
```

Run it using `prod-code exec --remote <build-node> --timeout-secs 900 -- python3
evidence/issue-1357/run-final.py`. All compilation/testing runs on a Linux x86_64
build node with original default test concurrency. Scoped formatting and
non-denying clippy precede this gate. Logs retain complete terminal output and
exit status; only private checkout prefixes/node addresses are normalized.
`source-hashes.json` identifies final relevant production/harness sources and
`preserved-input-hashes.json` compares original retained inputs.

Final gate: **exit 0**, **55/55** preservation tests and **4/4** native tests.
`rust_method_preservation.txt`, `rust_method_native.txt`, `gate-results.json` and
`acceptance.txt` retain the actual commands, complete terminal output and exits.
`format.txt` and `lint.txt` each record exit 0; scoped non-denying clippy reports
the same 56 existing gateway-library warnings recorded by the baseline.
`preservation.txt` records the clean diff against the pinned base for the entire
MCP crate, original fixtures/tests, native extensions, dependencies and baseline.
All 23 original input hashes match their retained originals.

After the gate, the evidence driver's self-hash path was made repository-relative
(Python gives `__file__` an absolute path), and generated-program source hashes
were added. The manifest was refreshed without repeating either successful test
command, after comparing every production/test source hash with the executed
gate's manifest. Gate commands, production sources and test sources are unchanged
by these metadata corrections. `source-hashes.json` contains 97 source records;
`artifact-hashes.json` also identifies the full evidence snapshot.
