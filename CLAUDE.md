# Working on prod-code

These rules apply to every agent session in this repository. `CONTRIBUTING.md` is the
full version; this is the part that is not optional.

## Process

- Work is tracked on GitHub. Before writing code: find or open the issue, with the exact
  command that reproduces the problem and its current output. After: open a pull request
  from a branch, filled in with the template (Problem, Change, Reproduce and verify,
  Measurements, Checks, `Closes #N`). No direct commits or pushes to `main`.
- There is no hosted CI. Run `cargo fmt --all -- --check`, `cargo clippy --workspace
  --all-targets -- -D warnings` and the tests for the touched crates on a build node
  (`prod-code exec -- ...` or the MCP `code_check` / `code_lint` / `code_test`), never on
  the developer's machine, and paste what they printed into the PR.
- Every PR shows how to repeat the result: commands, output before, output after, node
  kind. Performance and correctness claims come with before/after numbers.
- Merge only with recorded checks; squash-merge, delete the branch.

## Privacy

This repository is public. Never write private IP addresses, host names, home
directory paths, credentials, or the names of internal tools and knowledge bases into
code comments, commits, issues or pull requests. Describe nodes as "a Linux build
node", "a macOS node", "the developer workstation".

## Authorship

Commits and PRs carry no generated co-authorship trailers or "generated with" lines.
