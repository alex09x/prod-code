/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Isolated procedural macro worker farm and sandboxing for rust-analyzer.
//!
//! Replaces unconstrained, per-workspace `proc_macro_srv` process spawning with:
//! 1. A shared node-level worker farm (`ProcMacroWorkerFarm`) that bounds total
//!    worker processes across all loaded workspaces on the gateway node.
//! 2. OS-level process sandboxing (`ProcMacroSandbox`):
//!    - Virtual address space limits (`RLIMIT_AS` via `ulimit -v`) to prevent
//!      runaway macro expansion or memory leaks from exhausting host RAM.
//!    - Core dump suppression (`RLIMIT_CORE = 0`) to prevent leaking process memory.
//!    - Open file descriptor bounds (`RLIMIT_NOFILE`).
//!    - Nice priority lowering (+10) so macro expansion does not starve LSP queries.
//!    - Sensitive cluster secret and token scrubbing (`unset PROD_CODE_*`, `AWS_*`, `GITHUB_*`, etc.).
//!    - Isolated scratch temporary directory (`TMPDIR`).

pub mod pool;
pub mod sandbox;

#[cfg(test)]
mod tests;

pub use pool::{FarmMetrics, ProcMacroFarmPermit, ProcMacroWorkerFarm, shared};
pub use sandbox::{ensure_secure_farm_dir, prepare_sandboxed_srv};
