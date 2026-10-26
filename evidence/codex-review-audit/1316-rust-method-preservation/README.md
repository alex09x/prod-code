# Rust method preservation — issue #1316

Base: `4f2943bc1bcf6312aa81b393d97827cb5cca449b`. All execution used a Linux x86_64 build node through the public prod-code CLI. No dependency, polyglot implementation, other-worktree, publication or merge changes. No child workers. The result is left uncommitted for task integration.

## Reviewed original comments

All seven original comments were read with `gh api repos/alex09x/prod-code/pulls/comments/ID`:

- https://github.com/alex09x/prod-code/pull/126#discussion_r4079089180 — deref-wrapper dispatch
- https://github.com/alex09x/prod-code/pull/126#discussion_r4079089186 — complete self-type identity
- https://github.com/alex09x/prod-code/pull/197#discussion_r4087082287 — receiver-move literal data
- https://github.com/alex09x/prod-code/pull/197#discussion_r4087082300 — explicit reference lifetimes
- https://github.com/alex09x/prod-code/pull/199#discussion_r4087257719 — associated recursion
- https://github.com/alex09x/prod-code/pull/199#discussion_r4087257728 — associated-move literal data
- https://github.com/alex09x/prod-code/pull/211#discussion_r4088035140 — selected binding cursor

## Observed failure and acceptance

Before production edits, the new public planning/apply fixtures reproduced seven failures (3 passed, 7 failed; exit 101). `before.txt` preserves the exact available source/output with environment paths sanitized. The wrapper program compiled and returned 99 after transformation instead of 7. The associated literal program compiled and printed `Order self tax` instead of `Self self tax`. Receiver movement corrupted nested receiver ownership. Explicit lifetimes were refused, the earlier module word stole the binding cursor, and recursive references were skipped (only one of three references accounted for). The combined wrong-owner test stopped on its first, path-identity failure; its generic-argument variant was subsequently exercised by the passing run, rather than separately measured before the fix.

Hypotheses and distinguishing observations: a base-name comparison predicts accepting a different module or generic instantiation; complete spelling contradicts that acceptance. A dot-call rewrite predicts dispatch to the wrapper's inherent method even with zero diagnostics; runtime 99 confirms this. Raw identifier scans predict changed literal bytes and nested owners; the emitted source and runtime confirm both. The declaration cursor fixture answers only the exact selected binding position, distinguishing it from the earlier module word. Associated recursion must target the destination after removal; both its reference count and executable output check that boundary.

The final required command passed (12 tests, zero failures):

`prod-code exec --remote <linux-build-node> --timeout-secs 900 -- cargo test -p prod-code-mcp --test rust_method_preservation`

`acceptance.txt` is the final command's terminal output. `after-runtime.txt` records the preceding focused run with `-- --nocapture`, including exact fixture source, rustc results and executable output. The semantic code state was the same except for final formatting and removal of an unused base-name helper introduced by the changed call path. The final command recompiled that final state.

The passing fixtures cover qualified wrapper dispatch (7), exact ordinary/generic/path owners (7), wrong path/generic rejection under force, ordinary/raw/byte/raw-byte literals including embedded quotes, comments, nested owners and destination shadow bindings, explicit reference lifetimes (3), selected-binding cursors (3), recursion through `Self::f`, `OldType::f` and function values (5), and cross-module recursion from the destination perspective (5). Every successful apply has an actual rustc/runtime check. Previews and failed/unsupported reference controls assert unchanged source, manifest and sentinel bytes. Effectful-call refusal preserves all bytes; force constructs and executes the reversed-order call, observing `tax`, `order`, `3` rather than `order`, `tax`, `3`.

## Existing affected fixtures and final gates

- `prod-code exec --remote <linux-build-node> --timeout-secs 900 -- cargo test -p prod-code-mcp --test orchestration an_associated_function_ -- --nocapture`: exit 0, 2 passed, 60 filtered out. See `orchestration-associated.txt`.
- `prod-code exec --remote <linux-build-node> --timeout-secs 900 -- cargo test -p prod-code-mcp --test orchestration a_method_moves_to_the_type_of_its_parameter -- --nocapture`: exit 0, 1 passed, 61 filtered out. See `orchestration-receiver.txt`.
- Final formatting: one remote `rustfmt --edition 2024 --config skip_children=true` invocation over the twelve changed Rust files: exit 0. The exact file list is in `commands-and-failures.txt`; terminal metadata is in `format.txt`.
- `prod-code exec --remote <linux-build-node> --timeout-secs 900 -- cargo fmt --all -- --check`: exit 1 on unrelated existing formatting. Full available output is in `format-check.txt`. No unrelated files were reformatted.
- `prod-code lint --remote <linux-build-node> --path crates/prod-code-mcp --timeout-secs 900`: CLI exit 1, underlying clippy exit 101, 210 errors. See `lint.txt`. It reports existing diagnostics across untouched code; the displayed diagnostic in the edited helper file is the unchanged `receiver_argument` explicit lifetime (base line 186, final line 233). The CLI truncates diagnostics, so this is not evidence of a clean lint result or an exhaustive lint audit.

The orchestration runs preceded final formatting and unused-helper removal; their unchanged behavior is reused rather than rerun. Only these three orchestration tests ran, not the complete target. No broad tests or extra build gates ran.

## Limits and final hashes

Qualified calls intentionally remain qualified unless dispatch equivalence can be established; conversion still changes the declaration and resolved receiver uses. Full type spelling comparison is conservative about aliases and alternate equivalent paths. Receiver-method recursion and receiver method values remain unsupported and are reported rather than forced through. Associated angle-qualified paths such as `<Order>::f` are explicitly unaccounted and block writes even with force. No claim is made about these unsupported transformations.

The tests use cursor-specific ScriptedGateway protocol fixtures, plus real rustc and executable results on the build node. They are not a live rust-analyzer end-to-end run. No polyglot implementation was edited or tested. Existing unrelated format/lint failures remain. The edited production files and all new test source files are at most 400 lines; the existing orchestration file remains in its established structure.

Environment paths, temporary fixture directory names, the remote node address and private-address fixture values in unrelated formatting diffs are redacted in captured output; task fixture source and runtime data are preserved. `hashes.sha256` records SHA-256 hashes of every final changed/new file except the manifest itself; it includes this audit and all captured logs. No result commit was created by this worker.
