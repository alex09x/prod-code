/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Native Model Context Protocol (MCP) server for prod-code AI agent fleets.

pub mod call_tree;
pub mod caller_migration;
pub mod cluster;
pub mod codemod;
pub mod compile_check;
pub mod dataflow;
pub mod dead_code;
pub mod dependencies;
pub mod diagnostics;
pub mod dossier;
pub mod duplicates;
pub mod encapsulate_field;
pub mod exec;
pub mod expression_synthesis;
pub mod extract_delegate;
pub mod extract_field;
pub mod extract_function;
pub mod extract_function_polyglot;
pub mod extract_interface;
pub mod extract_parameter;
pub mod extract_trait;
pub mod fixit;
pub mod fixture;
pub mod generify;
pub mod hot_reload;
pub mod impact;
pub mod inline_parameter;
pub mod introduce_variable;
pub mod invert_boolean;
pub mod invert_value;
pub mod lang;
pub mod loop_to_iterator;
pub mod make_static;
pub mod markdown;
pub mod move_item;
pub mod move_method;
pub mod move_module;
pub mod move_polyglot;
pub mod parameter_object;
pub mod patch;
pub mod protocol;
pub mod prune;
pub mod pull_push;
pub mod reachability;
pub mod refactor;
pub mod remote_fs;
pub mod rename_accessors;
pub mod rename_mentions;
pub mod replace_conditional;
pub mod replace_constructor;
pub mod replace_inheritance;
pub mod report;
pub mod safe_delete_go;
pub mod safe_delete_typescript;
pub mod schema;
pub mod search;
pub mod server;
pub mod session;
pub mod shadow;
pub mod signature;
pub mod signature_go;
pub mod signature_polyglot;
pub mod slice;
pub mod supertypes;
pub mod sync;
pub mod to_method;
pub mod tools;
pub mod trait_param;
pub mod type_migration;
pub mod verify;
pub mod watch;
pub mod wrap_return;
pub mod xml_svg;

pub use protocol::{MCP_PROTOCOL_VERSION, SERVER_NAME, SERVER_VERSION};
pub use server::{
    MCP_TOOL_CALL_TIMEOUT, handle_mcp_request, is_retryable_connection_error, rediscover_node,
    resolve_tool_call_timeout, run_stdio_mcp_server, serve_mcp_requests,
};
pub use sync::scan_workspace_files;
pub use tools::{execute_tool, list_tools};
