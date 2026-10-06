/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Cluster PKI, certificate authority, node certificates, pinning, and TLS bootstrap (Phase 5.6).

mod runner;
mod types;

#[cfg(test)]
mod tests;

pub use runner::run_cert;
pub use types::CertCommands;
