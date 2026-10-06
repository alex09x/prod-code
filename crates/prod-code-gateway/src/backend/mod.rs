/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Managed language server backend workers (e.g. rust-analyzer on a Linux node) supervised by prod-code gateway.

pub mod health;
pub mod init;
pub mod probe;
pub mod reader;
pub mod request_history;
pub mod worker;
pub mod writer;

pub use worker::BackendWorker;
