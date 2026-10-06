/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub(crate) mod coalesce;
pub(crate) mod lifecycle;
pub(crate) mod session_view;
pub(crate) mod types;

pub use types::{RECLAIM_MIN_IDLE, WorkspaceManager};
