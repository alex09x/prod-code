/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Files an editor is pointed at that exist only on the node (#333).

mod frames;
mod remote;

#[cfg(test)]
mod tests;

pub use frames::{
    engine_for_language, method_of, push_failed_warning, read_frame, refuse_session,
    startup_error_message, write_frame,
};
pub use remote::{RemoteFiles, default_cache, write_read_only};
