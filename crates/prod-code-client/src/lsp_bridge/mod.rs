/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod bridge;
pub mod encoding;
pub mod reconnect;
pub mod reconnect_handler;
pub mod session;
pub mod state;
pub mod sync;
pub mod transport;

pub use bridge::run_lsp_bridge;
pub use session::resolve_redirect_target;
pub use transport::refuse_lsp;
