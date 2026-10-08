/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Workspace symbol resolution, search across projects, and LSP coordinate translation.

pub mod across_projects;
pub mod alias;
pub mod matching;
pub mod nested_projects;
pub mod resolve;
pub mod search;
pub mod sources;
pub mod type_members;
pub mod types;
pub mod unindexed;

pub use resolve::resolve_symbol;
pub use search::workspace_symbol_search;
pub use types::SymbolHit;

#[allow(unused_imports)]
pub(crate) use across_projects::*;
#[allow(unused_imports)]
pub(crate) use alias::*;
#[allow(unused_imports)]
pub(crate) use matching::*;
#[allow(unused_imports)]
pub(crate) use nested_projects::*;
#[allow(unused_imports)]
pub(crate) use search::*;
#[allow(unused_imports)]
pub(crate) use sources::*;
#[allow(unused_imports)]
pub(crate) use type_members::*;
#[allow(unused_imports)]
pub(crate) use types::*;
#[allow(unused_imports)]
pub(crate) use unindexed::*;
