/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod discovery;
pub mod installed;
mod presets;
pub mod types;

pub use discovery::{settings_for_section, venv_python, which_bin};
pub use types::{
    DEFAULT_HEALTH_PROBE_INTERVAL, DEFAULT_MAX_RETAINED_DOCUMENTS, DEFAULT_REQUEST_TIMEOUT,
    GenericLspConfig, HEALTH_PROBE_ID_PREFIX, HEALTH_PROBE_METHOD, MAX_IDLE_PROBE_TIMEOUTS,
};
