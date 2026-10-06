/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod buffer;
pub mod client;
pub mod lsp;
pub mod stream;

#[cfg(test)]
mod tests;

pub use buffer::{
    KEEPALIVE_IDLE, KEEPALIVE_INTERVAL, KEEPALIVE_RETRIES, TCP_BUFFER_SIZE_ENV,
    TCP_RECV_BUFFER_ENV, TCP_SEND_BUFFER_ENV, parse_buffer_size, tune, tune_with_buffer_sizes,
};
pub use client::{
    BuiltClientTls, auth_token, clear_client_tls_cache, connect, connect_stream,
    connect_stream_with, connect_stream_with_client_config, connect_stream_with_tls, connect_with,
    default_client_tls_built, init_client_tls_from_env, set_default_client_tls,
    set_default_client_tls_built,
};
#[cfg(windows)]
pub use client::{connect_named_pipe, connect_named_pipe_with};
#[cfg(unix)]
pub use client::{connect_unix, connect_unix_with};
pub use lsp::read_lsp_frame;
pub use stream::{
    AUTH_TOKEN_ENV, AUTH_TOKEN_FILE_ENV, AUTH_TOKEN_VARS, AnyStream, SOCKET_PATH_ENV, ScrubSecrets,
};
