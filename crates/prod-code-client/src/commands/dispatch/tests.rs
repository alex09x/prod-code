/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `dispatch` routes a command to its group, and the `grep` arm resolves its search path
//! before it asks anything of the node: every case here is refused or answered locally, so
//! the gateway address is one that nothing listens on.

use super::*;
use crate::cli::Cli;
use crate::workspace::find_workspace_root;
use clap::Parser;
use std::path::Path;

/// Runs `work` on a thread with room for the command enum: a debug build of the argument
/// parser and of the dispatch futures does not fit the 2 MiB of a test thread.
pub(crate) fn on_big_stack<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(work)
        .expect("spawn")
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

pub(crate) fn parse(args: &[&str]) -> Commands {
    let argv = std::iter::once("prod-code").chain(args.iter().copied());
    Cli::try_parse_from(argv)
        .expect("the command line parses")
        .command
        .expect("a subcommand")
}

fn context(cwd_root: Option<&'static Path>) -> DispatchContext<'static> {
    DispatchContext {
        remote: "127.0.0.1:9".parse().expect("address"),
        remotes: &[],
        placement_key: "scratch",
        cwd_engine: None,
        lsp_engine: None,
        cwd_root,
    }
}

/// The message a command is refused with, whole chain included, given the workspace root
/// the dispatcher was started in (none when it could not be found).
pub(crate) fn refusal_in(cwd_root: Option<&'static Path>, args: &[&str]) -> String {
    let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
    on_big_stack(move || {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let err = runtime
            .block_on(dispatch(parse(&args), context(cwd_root)))
            .expect_err("the command is refused");
        format!("{err:#}")
    })
}

pub(crate) fn refusal(args: &[&str]) -> String {
    refusal_in(None, args)
}

/// Runs a command that is answered from the checkout alone, and says whether it succeeded.
pub(crate) fn answered_locally(args: &[&str]) -> anyhow::Result<()> {
    let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
    on_big_stack(move || {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(dispatch(parse(&args), context(None)))
    })
}

#[test]
fn each_group_of_commands_is_routed_to_its_dispatcher() {
    // Members: the selection is read before anything is sent.
    assert_eq!(
        refusal(&[
            "extract-function",
            "src/a.rs",
            "1",
            "1",
            "--to",
            "12",
            "--name",
            "helper",
        ]),
        "--to takes LINE:COL"
    );
    // Query: a structural search over one directory of the checkout answers without a node.
    let here = std::env::current_dir()
        .expect("cwd")
        .join("src/commands/dispatch/exec");
    let here = here.to_string_lossy().into_owned();
    answered_locally(&[
        "structural-search",
        "$a.unwrap()",
        "--path",
        &here,
        "--json",
    ])
    .expect("the search is answered locally");
}
fn workspace_root() -> std::path::PathBuf {
    let cwd = std::env::current_dir().expect("cwd");
    find_workspace_root(&cwd).unwrap_or(cwd)
}

#[test]
fn grep_needs_a_pattern() {
    assert_eq!(
        refusal(&["grep", ""]),
        "code_grep requires a non-empty `pattern`"
    );
    // A blank path is no path: the empty pattern is what stops the search.
    assert_eq!(
        refusal(&["grep", "", "--path", "   "]),
        "code_grep requires a non-empty `pattern`"
    );
}

#[test]
fn grep_trims_its_path_and_prefers_the_flag_to_the_positional() {
    let missing = |dir: &str| {
        format!(
            "search path does not exist: {}",
            workspace_root().join(dir).display()
        )
    };
    let cases: [(&[&str], &str); 3] = [
        (&["grep", "needle", " no_such_dir_one "], "no_such_dir_one"),
        (
            &["grep", "needle", "--path", "\tno_such_dir_two\n"],
            "no_such_dir_two",
        ),
        (
            &[
                "grep",
                "needle",
                "no_such_dir_three",
                "--path",
                "no_such_dir_four",
            ],
            "no_such_dir_four",
        ),
    ];
    for (argv, dir) in cases {
        let message = refusal(argv);
        assert!(message.starts_with(&missing(dir)), "{argv:?}: {message}");
    }
}
