# Working on prod-code

These rules apply to every agent session in this repository. `CONTRIBUTING.md` is the
full version; this is the part that is not optional.

## Process

- Work is tracked on GitHub. Before writing code: find or open the issue. After: open a
  pull request from a branch, filled in with the template (Problem, Change,
  Measurements, Checks, `Closes #N`). No direct commits or pushes to `main`.
- Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D
  warnings` and the tests for the touched crates before the PR, on a build node when one
  is available rather than on the developer's machine.
- Merge only after CI is green; squash-merge, delete the branch.
- Measurements back every performance or correctness claim: before/after, hardware and
  workload named generically.

## Privacy

This repository is public. Never write private IP addresses, host names, home
directory paths, credentials, or the names of internal tools and knowledge bases into
code comments, commits, issues or pull requests. Describe nodes as "a Linux build
node", "a macOS node", "the developer workstation".

## Authorship

Commits and PRs carry no generated co-authorship trailers or "generated with" lines.
