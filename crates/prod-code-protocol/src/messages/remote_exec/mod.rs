/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod argv;
pub mod cargo;
pub mod go;
pub mod types;

pub use argv::build_argv;
pub use cargo::parse_cargo_json_event;
pub use go::parse_go_test_json_event;
pub use types::{
    RemoteExecCommand, RemoteExecDiagnostic, RemoteExecFormat, RemoteExecLanguage,
    RemoteExecRequest, RemoteExecResult, RemoteExecSpan, RemoteExecStream, RemoteExecTestEvent,
};
