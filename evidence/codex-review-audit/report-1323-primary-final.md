# Report privacy and label integration

Candidate `8dea09616798143ecc363a923265310f2e4a80ae` was reviewed against current main
`65c6e5a62e69fafae17820ae9548925b74711187`. Nine of eleven source/test files are
unchanged. The primary added one compact-suffix regression and corrected the shared
IPv6 token boundary. All prior source, tests and evidence remain.

Observed failure: the new focused test failed before the production edit (exit 101,
0 passed, 1 failed). A numerically generated synthetic unique-local endpoint in
`peer:<synthetic-address>:timeout` remained visible instead of `peer:<node>:timeout`.
The first direct scrub assertion failed; later draft assertions were not reached
in that failing run. No real address was used or published.

The correction parses the candidate before the final punctuation colon when an
identifier suffix follows it. This handles suffixes beginning with hex characters
(`error`, `closed`) as well as `timeout`, numeric error text and underscore labels.
It retains the complete valid compressed IPv6 prefix and existing Rust path guards.
The focused test then passed (1 passed, 1409 filtered), including ten address/suffix
combinations through direct scrub and draft title/body plus Rust-path controls.

Final changed-source rustfmt: exit 0. Final scoped Clippy: exit 0, with 103 existing
MCP, 4 client-library and 22 client-binary warnings, none in changed files. Strict
warning-clean status is not claimed. No baseline cleanup or broad test rerun.

One final compile/test gate, Linux x86_64:

```sh
cargo test -p prod-code-mcp -p prod-code-client --lib --bins -- --exact report::tests::private_details_are_scrubbed_and_documentation_addresses_stay report::tests::a_draft_needs_a_searchable_title_and_a_body_and_gets_an_environment report::tests::labels_default_to_a_bug_allow_roadmap_and_refuse_unsupported_labels report::tests::a_report_is_filed_when_nothing_similar_is_open report::tests::a_similar_open_issue_stops_the_report_unless_forced report::tests::a_private_reference_is_named_in_the_issue report::tests::a_dry_run_sends_nothing_and_a_failing_gh_is_reported report::privacy_tests::report_private_names_are_refused_before_gh report::privacy_tests::report_lists_all_tokens_checks_titles_and_allows_neutral_reports report::privacy_tests::report_generic_file_names_do_not_refuse_ordinary_words report::privacy_tests::report_explicit_override_and_public_remote_keep_existing_behavior report::privacy_tests::report_collects_declarations_across_languages_without_an_analyzer report::privacy_tests::report_checks_private_reference_and_schema_explains_the_policy report::privacy_tests::report_directory_name_is_not_a_public_repository_identity report::privacy_tests::report_only_scans_tracked_sources_and_caps_individual_reads report::regression_tests::ipv6_scrubbing_preserves_syntax_and_safe_controls report::regression_tests::ipv6_compact_error_suffixes_are_scrubbed_in_drafts report::regression_tests::ipv6_is_scrubbed_at_draft_and_fake_gh_boundaries report::regression_tests::ipv6_followed_by_punctuation_colon_preserves_valid_compression report::regression_tests::ipv6_after_label_separator_preserves_rust_paths_and_safe_controls report::regression_tests::distinct_normalized_types_are_refused_but_duplicates_and_markers_work report::regression_tests::multiple_types_fail_before_any_gh_invocation report::regression_tests::report_issue_help_schema_and_agent_guidance_advertise_roadmap report::regression_tests::mcp_request_uses_shared_type_validation_and_scrubbing commands::ops::report::tests::report_issue_help_advertises_roadmap_and_parses_labels commands::ops::report::tests::report_private_names_override_is_explicit commands::ops::report::coverage_tests::report_issue_help_labels_reach_shared_validation commands::ops::report::coverage_tests::unplaced_checkout_still_validates_report_title
```

```text
benchmark library: 0 passed; 50 filtered out
client binary: 4 passed; 0 failed; 110 filtered out
MCP library: 24 passed; 0 failed; 1386 filtered out
command exit 0; wall time 29.6s
```

Existing report tests exercise synthetic environment metadata, shared CLI/MCP
validation, draft output and fake-gh arguments/stdin. They do not publish real
issues or establish live node metadata. IPv6 text that is also a valid all-hex
path is inherently ambiguous; the existing documented preservation policy for
Rust qualified paths and safe addresses remains covered. Final file hashes are
in `report-1323-primary-final-hashes.json`; no claim is made that an historical
failed strict-lint run passed.
