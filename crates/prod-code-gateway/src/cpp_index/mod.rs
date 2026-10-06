/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Cross-worktree C/C++ engine support: clangd background index seeding, compilation
//! database relocation, and shared precompiled header (PCH) compiler caching (Roadmap 3.4).

pub mod relocate;
pub mod seed;
pub mod shard;

#[cfg(test)]
mod tests;

pub use relocate::{
    is_under_root, relocate_arg_token, relocate_command_string, relocate_compile_commands_content,
    relocate_path_or_uri,
};
pub use seed::{seed_clangd_index, seed_compile_commands, seed_cpp_worktree};
pub use shard::{
    ShardIdentity, clangd_path_digest, parse_shard_filename, relocate_shard,
    shard_filename_for_path,
};
