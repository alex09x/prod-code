/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod edits;
pub mod find;
pub mod format;

pub use edits::build_decl_edits;
pub use find::{find_polyglot_declaration, find_polyglot_declaration_with_jsx};
pub use format::format_polyglot_param;
