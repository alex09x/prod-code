# Working on prod-code

These rules apply to every agent session in this repository. `CONTRIBUTING.md` is the
full version; this is the part that is not optional.

## Process

- Work is tracked on GitHub. Before writing code: find or open the issue, with the exact
  command that reproduces the problem and its current output. After: open a pull request
  from a branch, filled in with the template (Problem, Change, Reproduce and verify,
  Measurements, Checks, `Closes #N`). No direct commits or pushes to `main`.
- Label every issue and PR when you open it: one type (`bug`, `enhancement`,
  `documentation`, `perf`; `test` or `release` for PRs that are only that) and the areas it
  touches (`gateway`, `client`, `mcp`, `cluster`, `worktree`, `infra`, `test`). A PR takes
  the labels of the issue it closes. `report-issue --label X` / `code_report_issue {labels}`.
- There is no hosted CI. Run `cargo fmt --all -- --check`, `cargo clippy --workspace
  --all-targets -- -D warnings` and the tests for the touched crates on a build node
  (`prod-code exec -- ...` or the MCP `code_check` / `code_lint` / `code_test`), never on
  the developer's machine, and paste what they printed into the PR.
- Every PR shows how to repeat the result: commands, output before, output after, node
  kind. Performance and correctness claims come with before/after numbers.
- Merge only with recorded checks; squash-merge, delete the branch.

## Use prod-code on prod-code

This repository is worked on with the tool it builds, and only with it. That is how the tool
gets tested on real work, and how its bugs are found before a user finds them.

- **Navigate** with `code_definition`, `code_references`, `code_callers`, `code_callees`,
  `code_outline`, `code_symbols` and `code_hover` (or `prod-code def|refs|callers|outline|
  symbols|hover`). Do not grep for code structure or line numbers; grep is for prose.
- **Change code** with the refactoring tools where one fits: `code_rename`, `code_move`,
  `code_change_signature`, `code_extract_parameter`, `code_extract_field`,
  `code_encapsulate_field`, `code_introduce_parameter_object`, `code_assist`. Hand-written code
  goes through `code_validate_edit` / `code_validate_edits` with the complete new text before
  it is written. No scripted text surgery (sed, python) on source files.
- **Build, test and lint** with `code_check`, `code_test`, `code_lint` and `code_exec`, which
  run on a build node.
- **When the tool is wrong, slow, confusing or missing something**, that is an issue. Open it
  with the command that shows the problem and its output, then fix it like any other change.
  A workaround is not the fix.

## Privacy

This repository is public. Never write private IP addresses, host names, home
directory paths, credentials, or the names of internal tools and knowledge bases into
code comments, commits, issues or pull requests. Describe nodes as "a Linux build
node", "a macOS node", "the developer workstation".

## Authorship

Commits and PRs carry no generated co-authorship trailers or "generated with" lines.
