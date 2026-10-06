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

pub(crate) mod fixtures;

mod in_place_concurrency_eval;
mod in_place_restore_eval;
mod isolation_eval;
mod overlay_exec;
mod roots;
mod sccache_alias_eval;
mod sccache_config_eval;
mod sccache_run_eval;
mod staging_eval;
mod tail_buffer_eval;
