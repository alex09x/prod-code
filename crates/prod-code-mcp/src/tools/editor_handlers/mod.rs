/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Code editing and navigation handlers: rename, typecheck, code assists, and safe delete.

pub(crate) mod assists;
pub(crate) mod check;
pub(crate) mod rename;
pub(crate) mod safe_delete;

pub(crate) use assists::*;
pub(crate) use check::*;
pub(crate) use rename::*;
pub(crate) use safe_delete::*;
