/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[path = "exec_writeback_preservation/controls.rs"]
mod controls;
#[path = "exec_writeback_preservation/directories.rs"]
mod directories;
#[path = "exec_writeback_preservation/ignored.rs"]
mod ignored;
#[path = "exec_writeback_preservation/platform.rs"]
mod platform;
#[path = "exec_writeback_preservation/preservation.rs"]
mod preservation;
#[path = "exec_writeback_preservation/rendering.rs"]
mod rendering;
#[cfg(unix)]
#[path = "exec_writeback_preservation/roots.rs"]
mod roots;
#[path = "exec_writeback_preservation/support.rs"]
mod support;

#[path = "exec_writeback_preservation/preflight.rs"]
mod preflight;
#[path = "exec_writeback_preservation/transitions.rs"]
mod transitions;
