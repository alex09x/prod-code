/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

mod await_jsx;
mod await_operand;
mod await_regex;
mod await_template;
pub mod call_sites;
mod cpp_declarations;
pub mod declaration;
pub mod execute;
mod js_syntax;
mod numeric_literal;
mod semantic;
pub mod sources;
pub mod syntax;
pub mod types;

pub use execute::change_with;
pub use sources::{collect_workspace_sources, is_candidate_source_file};
pub use syntax::{is_import_or_export_context, is_in_comment};
pub use types::PolyglotDecl;
