# Contributing

prod-code is developed in the open, and every change goes through the same path,
whether a person or an agent writes it.

## One unit of work = one issue + one pull request

1. **Open an issue first.** State the problem with evidence: a measurement, a failing
   case, a log line. Roadmap phases are tracked as `roadmap` issues; concrete work gets
   its own issue that references the phase.
2. **Branch from `main`**: `feat/<topic>`, `fix/<topic>`, `perf/<topic>`, `ci/<topic>`,
   `docs/<topic>`. Nothing is committed to `main` directly.
3. **Run the checks before opening the PR**: `cargo fmt --all -- --check`,
   `cargo clippy --workspace --all-targets -- -D warnings`, and the tests of the crates
   you touched. Changes to the gateway or the sync path are also verified against a
   running gateway (say which scenario in the PR).
4. **Open the pull request** using the template: Problem, Change, Measurements, Checks,
   `Closes #<issue>`. Before/after numbers are required for anything about latency,
   throughput, memory or correctness rates, with the hardware and workload named.
5. **CI must be green**, then the PR is squash-merged and the branch deleted. The squash
   commit message is the PR title plus its Problem/Change summary.
6. **Ship the knowledge with the code**: update `ROADMAP.md` when a phase item lands,
   `CHANGELOG.md` for user-visible changes, and the CLI/MCP help texts.

## What never goes into the repository, issues or PRs

- Addresses, host names, paths or names of private machines and networks. Describe
  hardware generically ("a 32-core Linux node", "the developer's workstation").
- Credentials, tokens, keys, or files that contain them.
- Internal tools and knowledge bases that are not part of this project.
- Generated co-authorship or "generated with" lines in commits or PR descriptions.

## Style

- Rust 2024 edition, `rustfmt` defaults, clippy clean with `-D warnings`.
- Errors carry context (`anyhow::Context`); logs use `tracing` with structured fields.
- A change that alters behaviour comes with a test that fails without it.
- Comments explain why, not what; keep them short.
