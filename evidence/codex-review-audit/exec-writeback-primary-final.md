# Exec writeback integration and bounded snapshot correction

Candidate `0f496f920829d768d4f35be64d045cf74ee06951` was reviewed on main
`b131b3b7655089defb73180a19e13328d25cd867`. The prior independent aggregate review
identified file-to-directory loss; the candidate fixes that transition, including
conflicting ancestors, symlinks and subsequent batches. All historical evidence is retained.

Primary review found an additional compatibility regression: a single ignored
sparse file larger than the 256 MiB supplementary budget stopped both public runners
before an ExecRequest. A focused regression compiled and failed for that reason:
exit 101, 0 passed, 1 failed, 19 filtered. Both raw and polyglot runners recorded
execution_started=false and the snapshot byte-limit error. It is not a compilation
or environment failure. The after run passed the same test in both runner modes.

Optional outputs that exceed the byte budget or cannot be read now remain unknown;
they do not block unrelated command execution or outputs. Exceeding the metadata
inventory bound or encountering a traversal error leaves uncaptured output identity
unverified, while known source identities remain usable. Such unverified writebacks
are kept as they are and named in the CLI/MCP result. They are never treated as absent.
The existing size read cap remains; no unbounded allocation is introduced.

The former test requiring whole-command failure was updated to assert the stronger
boundary: an unchanged oversized file is preserved, while a small new output arrives.
Another test deletes the oversized file after the explicit request barrier and verifies
that returned bytes do not resurrect it. A 10,001-output metadata control verifies that
known source updates and a proven parent-file-to-child transition still apply, while
an unverified new output is retained. These are real filesystem observations with a
fake wire peer. No arbitrary sleeps or actual deployment are involved.

Final formatting of the seven primary-modified Rust files exited 0. Other candidate
source/fixture bytes are unchanged, as recorded in exec-writeback-primary-hashes.json.
Final scoped Clippy exited 0 with 103 MCP, 4 client-library and 22 client-binary existing
warnings. Two warnings are on unchanged lines 65 and 108 of commands/exec.rs; no changed
line diagnostics were observed. Strict -D warnings is not claimed clean.

One final compile/test gate on Linux x86_64:

```sh
cargo test -p prod-code-mcp -p prod-code-client --lib --test exec_writeback_preservation --test exec_writeback_warning -- --exact controls::formatter_and_same_os_no_change_controls controls::uncertain_tokens_literals_and_non_rust_changes_are_not_proven_layout directories::ignored_directories_have_initial_identity_in_both_runners directories::large_old_outputs_can_shrink_or_be_deleted_in_both_runners directories::unknown_link_outputs_are_not_mistaken_for_new_paths directories::oversized_output_stays_protected_without_blocking_execution ignored::ignored_outputs_update_and_preserve_local_mutations_in_both_runners ignored::excluded_node_caches_do_not_exhaust_writeback_snapshot platform::cfg_macro_and_nested_predicates_warn platform::deleted_or_rewritten_away_platform_evidence_warns platform::argument_reordering_and_tuple_comma_are_semantic platform::c_control_flow_brace_removal_is_semantic preflight::large_ignored_input_does_not_prevent_execution_or_resurrect_local_deletion preflight::metadata_limit_preserves_known_source_updates_and_keeps_uncertain_outputs preservation::local_deletion_rename_and_same_mtime_edit_survive_both_runners rendering::mcp_renders_pre_write_and_nested_cfg_warnings roots::symlink_root_preserves_writeback_identity_in_both_runners transitions::file_to_directory_preserves_children_in_both_runners transitions::file_to_directory_refreshes_identity_for_later_batches transitions::file_to_directory_keeps_conflicting_ancestors_in_both_runners transitions::file_to_directory_keeps_symlink_ancestors_in_both_runners exec::tests::files_rewritten_on_another_platform_are_named_when_they_depend_on_it exec::tests::a_formatter_s_rewrite_is_layout_and_an_edit_is_not exec::runner_tests::run_remote_streams_output_and_writes_back_changed_files exec::runner_tests::run_remote_refuses_an_empty_command exec::runner_tests::run_remote_names_what_is_unknown_when_the_stream_breaks exec::runner_tests::run_polyglot_remote_forwards_the_stream_and_writes_back_changed_files exec::runner_tests::run_polyglot_remote_names_what_is_unknown_when_the_stream_breaks exec::progress_tests::exec_reports_probe_wait_and_silent_execution_then_returns_exit exec::progress_tests::exec_preflight_identifies_local_sync_lock_wait cli_renders_deleted_and_rewritten_platform_warnings
```

```text
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 50 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.20s
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 1405 filtered out; finished in 11.06s
test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.60s
```

31 selected tests passed: 21 preservation scenarios, 9 affected unit tests and one
real CLI process test. Unrelated tests were filtered. Command wall time was 40.4s.
The CLI process talks to the real wire fixture; OS/platform values are simulated,
so this does not establish a live cross-platform compiler result. No broad suite,
benchmark, infrastructure restart, dependency install or deployment ran.

The new snapshot is not a filesystem transaction against uncooperative concurrent
writers. Its tested contract compares captured local identities before applying
returned files, preserves observed local changes, and refuses uncertain paths.
Warning suppression remains intentionally conservative for syntax transformations
beyond ordered whitespace, even when a formatter command was requested.
