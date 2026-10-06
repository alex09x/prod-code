/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Path and URI translation between local client environments and remote server storage.
//!
//! An LSP message is translated as JSON, not as text: only a value that names a file under the
//! workspace root, at a whole path component, is mapped. Source text (`didOpen`, `didChange`, an
//! edit's `newText`) and documentation pass through as they were written (#438).

mod translator;
mod uri;

#[cfg(test)]
mod tests;

pub use translator::PathTranslator;
pub use uri::{file_uri, file_uri_path, map_lsp_locations, uri_or_path};
