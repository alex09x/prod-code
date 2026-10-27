/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Command-line spellings accepted before clap sees the arguments.

/// The navigation commands whose file is positional (`def FILE LINE COL`, `def --symbol NAME
/// [FILE]`). None of them has a `--path` option of its own.
const POSITIONAL_FILE_COMMANDS: &[&str] = &[
    "def",
    "hover",
    "refs",
    "callers",
    "callees",
    "impls",
    "supertypes",
];

/// `prod-code def --symbol NAME --path FILE` (or `--path=FILE`) as `prod-code def --symbol
/// NAME FILE`. `test`, `check` and the MCP tools spell the file `path`, and agents carried that
/// spelling over and got `unexpected argument '--path'` (#1159).
pub fn normalize_args(args: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut args: Vec<String> = args.into_iter().collect();
    let Some(command) = command_index(&args) else {
        return args;
    };
    if !POSITIONAL_FILE_COMMANDS.contains(&args[command].as_str()) {
        return args;
    }
    let mut i = command + 1;
    while i < args.len() {
        if args[i] == "--" {
            break;
        }
        if args[i] == "--path" && i + 1 < args.len() {
            args.remove(i);
        } else if let Some(file) = args[i].strip_prefix("--path=") {
            args[i] = file.to_string();
        }
        i += 1;
    }
    args
}

/// The parsed command line, and whether `-r`/`--remote` was given on it, before or after the
/// subcommand. Clap says where the value came from, so a `-r` inside a trailing command, as
/// in `prod-code exec -- grep -r x .`, does not pin the client to its seed (#1454).
pub fn parse(args: impl IntoIterator<Item = String>) -> (super::Cli, bool) {
    use clap::{CommandFactory, FromArgMatches};
    let mut matches = super::Cli::command().get_matches_from(normalize_args(args));
    let pinned = remote_given(&matches);
    let cli = super::Cli::from_arg_matches_mut(&mut matches).unwrap_or_else(|e| e.exit());
    (cli, pinned)
}

/// Whether this command line runs an editor's language server: `prod-code lsp`, or
/// `prod-code` with no subcommand, which is `lsp` too. Both learn why a server could not start
/// from the answer to `initialize` (#1519).
pub fn starts_an_editor_session(cli: &super::Cli) -> bool {
    matches!(cli.command, None | Some(super::Commands::Lsp { .. }))
}

/// Whether the global `remote` option was typed at this level or a subcommand below it.
fn remote_given(matches: &clap::ArgMatches) -> bool {
    matches.value_source("remote") == Some(clap::parser::ValueSource::CommandLine)
        || matches
            .subcommand()
            .is_some_and(|(_, sub)| remote_given(sub))
}

/// The index of the subcommand: the first argument after the program name that is neither an
/// option nor the value of the global `-r`/`--remote` option.
fn command_index(args: &[String]) -> Option<usize> {
    let mut i = 1;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "-r" || arg == "--remote" {
            i += 2;
        } else if arg.starts_with('-') {
            i += 1;
        } else {
            return Some(i);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::normalize_args;

    fn normalized(args: &[&str]) -> Vec<String> {
        normalize_args(args.iter().map(|a| a.to_string()))
    }

    #[test]
    fn path_option_becomes_the_positional_file_of_navigation_commands() {
        assert_eq!(
            normalized(&["prod-code", "def", "--symbol", "f", "--path", "src/lib.rs"]),
            ["prod-code", "def", "--symbol", "f", "src/lib.rs"]
        );
        assert_eq!(
            normalized(&[
                "prod-code",
                "-r",
                "h:1",
                "refs",
                "--path=src/a.rs",
                "--symbol",
                "f"
            ]),
            [
                "prod-code",
                "-r",
                "h:1",
                "refs",
                "src/a.rs",
                "--symbol",
                "f"
            ]
        );
    }

    #[test]
    fn a_normalized_def_parses_with_the_file_set() {
        use clap::Parser;
        let args = normalized(&["prod-code", "def", "--symbol", "f", "--path", "src/lib.rs"]);
        // Building the whole command tree needs more than a test thread's default stack in a
        // debug build; the binary parses on the main thread.
        std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(move || {
                let cli = crate::cli::Cli::try_parse_from(args)
                    .expect("`def --symbol NAME --path FILE` parses");
                match cli.command {
                    Some(crate::cli::Commands::Def { file, symbol, .. }) => {
                        assert_eq!(file.as_deref(), Some(std::path::Path::new("src/lib.rs")));
                        assert_eq!(symbol, ["f"]);
                    }
                    other => panic!("not a def: {other:?}"),
                }
            })
            .unwrap()
            .join()
            .unwrap();
    }

    /// Only a `-r`/`--remote` of prod-code's own pins the client; one inside the trailing
    /// command of `exec` does not (#1454).
    #[test]
    fn only_prod_codes_own_remote_option_pins_the_client() {
        let cases: Vec<(Vec<&str>, bool)> = vec![
            (
                vec!["prod-code", "exec", "--", "grep", "-r", "x", "."],
                false,
            ),
            (
                vec!["prod-code", "exec", "--", "tool", "--remote", "h:2"],
                false,
            ),
            (
                vec!["prod-code", "exec", "--", "tool", "--remote=h:2"],
                false,
            ),
            (vec!["prod-code", "-r", "h:1", "exec", "--", "ls"], true),
            (vec!["prod-code", "--remote=h:1", "exec", "--", "ls"], true),
            (
                vec!["prod-code", "exec", "-r", "h:1", "--", "grep", "-r", "x"],
                true,
            ),
            (vec!["prod-code", "status"], false),
        ];
        std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(move || {
                for (args, pinned) in cases {
                    let owned: Vec<String> = args.iter().map(|a| a.to_string()).collect();
                    let (cli, got) = super::parse(owned);
                    assert_eq!(got, pinned, "{args:?}");
                    if pinned {
                        assert_eq!(cli.remote, "h:1", "{args:?}");
                    }
                }
                // No subcommand is `lsp` too, so it gets the same refusal at `initialize` (#1519).
                for (args, editor) in [
                    (&["prod-code"][..], true),
                    (&["prod-code", "lsp"][..], true),
                    (&["prod-code", "-r", "h:1"][..], true),
                    (&["prod-code", "mcp"][..], false),
                    (&["prod-code", "status"][..], false),
                ] {
                    let (cli, _) = super::parse(args.iter().map(|a| a.to_string()));
                    assert_eq!(super::starts_an_editor_session(&cli), editor, "{args:?}");
                }
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn other_commands_and_trailing_arguments_keep_their_path() {
        for args in [
            &["prod-code", "test", "--path", "crates/a"][..],
            &["prod-code", "exec", "--", "tool", "--path", "x"][..],
            &["prod-code", "def", "--symbol", "f", "--path"][..],
        ] {
            assert_eq!(normalized(args), args);
        }
    }
}
