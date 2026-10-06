/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#![cfg(test)]

pub(crate) mod common;

mod cache_resilience;
mod cluster;
mod command_exec;
mod engine_state;
mod gossip_auth;
mod placement;
mod protocol;
mod refactoring;
mod seeding;
mod sync_limits;
mod sync_probe;
