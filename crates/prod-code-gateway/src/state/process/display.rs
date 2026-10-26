/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! How a running command reads in status answers and cluster gossip. Those go to every peer
//! and status caller, so a value that looks like a credential is replaced, and the line is
//! bounded; the command itself runs with its real arguments (#1484).

/// The longest display line; the rest is cut at a character boundary.
const MAX_DISPLAY: usize = 240;
const REDACTED: &str = "<redacted>";

/// A name that announces a secret value: `--token`, `API_KEY=`, `X-Auth-Header`.
fn names_a_secret(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "token",
        "secret",
        "password",
        "passwd",
        "auth",
        "apikey",
        "api_key",
        "api-key",
        "credential",
        "cookie",
        "private_key",
        "private-key",
    ]
    .iter()
    .any(|word| name.contains(word))
}

/// `scheme://user:pass@host/…` keeps its scheme and host; the userinfo goes.
fn without_userinfo(arg: &str) -> Option<String> {
    let (scheme, rest) = arg.split_once("://")?;
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let (userinfo, host) = rest[..authority_end].rsplit_once('@')?;
    (!userinfo.is_empty())
        .then(|| format!("{scheme}://{REDACTED}@{host}{}", &rest[authority_end..]))
}

fn display_arg(arg: &str, after_secret_flag: bool) -> String {
    if after_secret_flag {
        return REDACTED.to_string();
    }
    let lower = arg.to_ascii_lowercase();
    if lower.starts_with("bearer ") || lower.starts_with("basic ") {
        return REDACTED.to_string();
    }
    // `KEY=value`, `--token=value`, `Authorization: value`.
    for separator in ['=', ':'] {
        if let Some((name, _)) = arg.split_once(separator)
            && !name.contains("//")
            && (names_a_secret(name) || name.eq_ignore_ascii_case("authorization"))
        {
            return format!("{name}{separator}{REDACTED}");
        }
    }
    without_userinfo(arg).unwrap_or_else(|| arg.to_string())
}

/// The command as status and gossip show it: credential-shaped values replaced, at most
/// [`MAX_DISPLAY`] bytes.
pub(crate) fn display_command(command: &[String]) -> String {
    let mut shown = Vec::with_capacity(command.len());
    let mut secret_next = false;
    for arg in command {
        shown.push(display_arg(arg, secret_next));
        // `--token value`, `-H 'Authorization: …'` is handled by the value itself.
        secret_next = arg.starts_with('-') && !arg.contains('=') && names_a_secret(arg);
    }
    let mut line = shown.join(" ");
    if line.len() > MAX_DISPLAY {
        let mut cut = MAX_DISPLAY;
        while !line.is_char_boundary(cut) {
            cut -= 1;
        }
        line.truncate(cut);
        line.push('…');
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(args: &[&str]) -> String {
        display_command(&args.iter().map(|a| a.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn ordinary_commands_read_as_they_are() {
        assert_eq!(
            shown(&["cargo", "test", "-p", "prod-code-mcp", "--lib", "patch::"]),
            "cargo test -p prod-code-mcp --lib patch::"
        );
        assert_eq!(
            shown(&["git", "clone", "https://example.invalid/r.git"]),
            "git clone https://example.invalid/r.git"
        );
    }

    #[test]
    fn credential_shaped_values_are_replaced() {
        // Sentinels only; nothing here is a real credential.
        assert_eq!(
            shown(&[
                "tool",
                "--token",
                "SENTINEL-1",
                "--api-key=SENTINEL-2",
                "next"
            ]),
            "tool --token <redacted> --api-key=<redacted> next"
        );
        assert_eq!(
            shown(&[
                "env",
                "AWS_SECRET_ACCESS_KEY=SENTINEL-3",
                "PATH=/bin",
                "run"
            ]),
            "env AWS_SECRET_ACCESS_KEY=<redacted> PATH=/bin run"
        );
        assert_eq!(
            shown(&[
                "curl",
                "-H",
                "Authorization: Bearer SENTINEL-4",
                "https://u:SENTINEL-5@example.invalid/x"
            ]),
            "curl -H Authorization:<redacted> https://<redacted>@example.invalid/x"
        );
        assert_eq!(shown(&["tool", "Bearer SENTINEL-6"]), "tool <redacted>");
        let line = shown(&["tool", "--password", "SENTINEL-7"]);
        assert!(!line.contains("SENTINEL"), "{line}");
    }

    /// What status answers and gossip carry comes from this table, so it is checked there.
    #[test]
    fn the_running_command_table_never_holds_the_secret() {
        let command: Vec<String> = ["tool", "--token", "SENTINEL-8"]
            .iter()
            .map(|a| a.to_string())
            .collect();
        let entry = super::super::RunningEntry::start(std::path::Path::new("/w/ws"), &command);
        let listed = super::super::running_commands();
        drop(entry);
        let ours = listed
            .iter()
            .find(|c| c.command.starts_with("tool --token"))
            .expect("the command is listed");
        assert_eq!(ours.command, "tool --token <redacted>");
        assert!(listed.iter().all(|c| !c.command.contains("SENTINEL-8")));
    }

    #[test]
    fn a_long_command_is_bounded_on_a_character_boundary() {
        let long = "é".repeat(400);
        let line = shown(&["echo", &long]);
        assert!(line.len() <= MAX_DISPLAY + '…'.len_utf8(), "{}", line.len());
        assert!(line.ends_with('…'));
    }
}
