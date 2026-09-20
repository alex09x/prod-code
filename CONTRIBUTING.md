# Contributing

prod-code is developed in the open, and every change goes through the same path,
whether a person or an agent writes it. There is no hosted CI: checks and benchmarks run on
the project's build nodes, which are far faster than a shared runner, and the pull request
records what was run and what it showed.

## One unit of work = one issue + one pull request

1. **Open an issue first**, and make it reproducible. An issue states the problem with the
   exact command that shows it, the output it gives today, and the output that would be
   right. "Hover is slow" is not an issue; this is:

   ```text
   $ PROD_CODE_TIMING=1 prod-code hover src/lib.rs 130 8
   [timing] total=129.7ms connect=1.0ms preflight_sync=116.6ms handshake=0.7ms ...
   ```
   > the pre-flight sync spends 116 ms in three git subprocesses for a query the gateway
   > answers in 3.7 ms; expected: one git status, under 40 ms.

   Roadmap phases are tracked as `roadmap` issues; concrete work gets its own issue that
   references the phase.
2. **Branch from `main`**: `feat/<topic>`, `fix/<topic>`, `perf/<topic>`, `docs/<topic>`.
   Nothing is committed to `main` directly.
3. **Run the checks on a build node before opening the PR**: `cargo fmt --all -- --check`,
   `cargo clippy --workspace --all-targets -- -D warnings`, the tests of the crates you
   touched, and the scenario from the issue against a running gateway.
4. **Open the pull request** with the template. The PR must let a reader repeat what you
   did: the commands you ran, their output before and after, on which kind of node
   (described generically: "32-core Linux node", "developer workstation"). Before/after
   numbers are required for anything about latency, throughput, memory or correctness rates.
   Reference the issue with `Closes #<n>`.
5. **Merge**: squash-merge once the checks in the PR are recorded and reviewed; delete the
   branch. The squash commit message is the PR title plus its Problem/Change summary.
6. **Ship the knowledge with the code**: update `ROADMAP.md` when a phase item lands,
   `CHANGELOG.md` for user-visible changes, and the CLI/MCP help texts.

## Reproduction commands that belong in issues and PRs

```sh
# Per-phase timing of one query
PROD_CODE_TIMING=1 prod-code hover <file> <line> <col>

# Correctness across diverged worktrees, with latency percentiles
prod-code divergent-bench --base-repo <repo> --persistent --workers 24 --queries-per-worker 50

# Sustained load with pipelined persistent sessions
prod-code bench --workspaces <checkout> --concurrency 16 --depth 4 --duration-secs 10

# Node health and where a checkout is placed
prod-code status
prod-code cluster

# Gateway log on a node (systemd user unit)
journalctl --user -u prod-code-gateway -o short-precise --since "-10min"
```

## What never goes into the repository, issues or PRs

- Addresses, host names, paths or names of private machines and networks. Describe
  hardware generically ("a 32-core Linux node", "a macOS node", "the developer's
  workstation").
- Credentials, tokens, keys, or files that contain them.
- Internal tools and knowledge bases that are not part of this project.
- Generated co-authorship or "generated with" lines in commits or PR descriptions.

## Style

- Rust 2024 edition, `rustfmt` defaults, clippy clean with `-D warnings`.
- Errors carry context (`anyhow::Context`); logs use `tracing` with structured fields.
- A change that alters behaviour comes with a test that fails without it.
- Comments explain why, not what; keep them short.
