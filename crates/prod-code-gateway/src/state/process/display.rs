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
        "pwd",
        "signature",
    ]
    .iter()
    .any(|word| name.contains(word))
}

/// `scheme://user:pass@host/…` keeps its scheme and host; the userinfo goes.
fn without_userinfo(arg: &str) -> Option<String> {
    let (scheme, rest) = arg.split_once("://")?;
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (userinfo, host) = rest[..authority_end].rsplit_once('@')?;
    (!userinfo.is_empty())
        .then(|| format!("{scheme}://{REDACTED}@{host}{}", &rest[authority_end..]))
}

/// The query of a URL keeps its keys; a value whose key names a secret goes:
/// `…/db?password=x&mode=ro` reads `…/db?password=<redacted>&mode=ro` (#1535).
fn without_query_secrets(arg: &str) -> Option<String> {
    let after_scheme = arg.find("://")? + 3;
    let query_start = after_scheme + arg[after_scheme..].find('?')? + 1;
    let query_end = arg[query_start..]
        .find('#')
        .map_or(arg.len(), |i| query_start + i);
    let mut changed = false;
    let pairs: Vec<String> = arg[query_start..query_end]
        .split('&')
        .map(|pair| match pair.split_once('=') {
            // A URL consumer decodes `p%61ssword` to `password`, so the check reads it decoded.
            Some((key, _)) if names_a_secret(&percent_decoded(key)) => {
                changed = true;
                format!("{key}={REDACTED}")
            }
            _ => pair.to_string(),
        })
        .collect();
    changed.then(|| {
        format!(
            "{}{}{}",
            &arg[..query_start],
            pairs.join("&"),
            &arg[query_end..]
        )
    })
}

/// A query key as a URL consumer reads it: `%XX` escapes decoded and `+` as a space. An
/// invalid escape is kept as it is (#1539).
fn percent_decoded(key: &str) -> String {
    let bytes = key.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                (Some(high), Some(low)) => {
                    out.push((high * 16 + low) as u8);
                    i += 3;
                    continue;
                }
                _ => out.push(b'%'),
            },
            b'+' => out.push(b' '),
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Both URL redactions, userinfo and query, on one argument.
fn without_url_secrets(arg: &str) -> Option<String> {
    let userinfo = without_userinfo(arg);
    let base = userinfo.as_deref().unwrap_or(arg);
    without_query_secrets(base).or(userinfo)
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
    without_url_secrets(arg).unwrap_or_else(|| arg.to_string())
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

    /// A credential in a URL query reads as redacted; the rest of the URL stays (#1535).
    #[test]
    fn secret_query_values_in_urls_are_replaced() {
        assert_eq!(
            shown(&[
                "psql",
                "postgresql://example.invalid/db?password=SYNTHETIC-CANARY"
            ]),
            "psql postgresql://example.invalid/db?password=<redacted>"
        );
        assert_eq!(
            shown(&[
                "curl",
                "https://u:SENTINEL-9@example.invalid/x?mode=ro&access_token=SENTINEL-10&X-Amz-Signature=SENTINEL-11#top"
            ]),
            "curl https://<redacted>@example.invalid/x?mode=ro&access_token=<redacted>&X-Amz-Signature=<redacted>#top"
        );
        assert_eq!(
            shown(&[
                "env",
                "DATABASE_URL=mysql://example.invalid/db?pwd=SENTINEL-12"
            ]),
            "env DATABASE_URL=mysql://example.invalid/db?pwd=<redacted>"
        );
        // Percent-encoded key names decode to a secret name (#1539).
        assert_eq!(
            shown(&[
                "tool",
                "postgresql://example.invalid/db?p%61ssword=SYNTHETIC-CANARY"
            ]),
            "tool postgresql://example.invalid/db?p%61ssword=<redacted>"
        );
        assert_eq!(
            shown(&[
                "curl",
                "https://example.invalid/x?%74oken=SYNTHETIC-CANARY&q=%zz"
            ]),
            "curl https://example.invalid/x?%74oken=<redacted>&q=%zz"
        );
        assert_eq!(percent_decoded("p%61ss+word%"), "pass word%");
        // A query with nothing secret is left as it is.
        assert_eq!(
            shown(&["curl", "https://example.invalid/search?q=rust&page=2"]),
            "curl https://example.invalid/search?q=rust&page=2"
        );
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
