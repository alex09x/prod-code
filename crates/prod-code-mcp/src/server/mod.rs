/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod dispatch;
pub mod failover;
pub mod rebalance;
pub mod transport;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod transport_tests;

pub use dispatch::{MCP_TOOL_CALL_TIMEOUT, handle_mcp_request, resolve_tool_call_timeout};
pub use failover::{is_retryable_connection_error, rediscover_node};
pub use transport::{run_stdio_mcp_server, serve_mcp_requests};
