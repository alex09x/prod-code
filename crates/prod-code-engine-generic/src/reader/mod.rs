/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod frame;
pub mod loop_task;

pub(crate) use frame::{spawn_stderr_reader, write_frame_until};
pub(crate) use loop_task::spawn_reader_loop;
