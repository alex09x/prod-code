//! Reporting a bug in prod-code itself as a GitHub issue, from the agent that hit it (#290).
//!
//! Most of prod-code's bugs are found by an agent in the middle of other work: a wrong answer,
//! a hang, an error that says nothing. The report goes to the public repository through the
//! `gh` CLI of the machine the client runs on. Before anything leaves the machine, the text is
//! scrubbed of what identifies it (LAN addresses, the host name, home-directory paths), an
//! Environment section is added, and issues with a similar title are listed first, so that the
//! same problem is not filed twice. Details that cannot be public stay in the reporter's own
//! private records; the issue carries only the id of that record (`private_ref`).

use anyhow::{Context, Result};
use std::net::SocketAddr;
use std::path::Path;

/// Where prod-code's issues live.
pub const REPOSITORY: &str = "alex09x/prod-code";

/// Open issues listed as possible duplicates before a new one is filed.
const DUPLICATES_SHOWN: usize = 5;

/// What kind of issue it is: every issue carries one of these (#321).
pub const TYPE_LABELS: &[&str] = &["bug", "enhancement", "documentation", "perf"];

/// Which part of prod-code it is about: an issue carries those it touches.
pub const AREA_LABELS: &[&str] = &[
    "gateway", "client", "mcp", "cluster", "worktree", "infra", "test",
];

/// The labels an issue is filed with: those asked for, lower-cased and once each, and `bug`
/// first when none of them says what kind of issue it is. A label the repository does not
/// have is refused here, before anything is searched or sent.
pub fn issue_labels(asked: &[String]) -> Result<Vec<String>> {
    let mut labels: Vec<String> = Vec::new();
    for label in asked
        .iter()
        .map(|l| l.trim().to_ascii_lowercase())
        .filter(|l| !l.is_empty())
    {
        anyhow::ensure!(
            TYPE_LABELS.contains(&label.as_str()) || AREA_LABELS.contains(&label.as_str()),
            "{REPOSITORY} has no label `{label}`: give one type ({}) and the areas the issue \
             is about ({})",
            TYPE_LABELS.join(", "),
            AREA_LABELS.join(", ")
        );
        if !labels.contains(&label) {
            labels.push(label);
        }
    }
    if !labels.iter().any(|l| TYPE_LABELS.contains(&l.as_str())) {
        labels.insert(0, "bug".to_string());
    }
    Ok(labels)
}

/// `text` without the reporter's private details: IPv4 addresses become `<node>` (loopback,
/// the unspecified address and the documentation ranges stay), the home directory and any
/// `/Users/<name>` or `/home/<name>` become `~`, and `host` (with its first label alone) becomes
/// `<host>`.
pub fn scrub(text: &str, home: Option<&str>, host: Option<&str>) -> String {
    let mut out = text.to_string();
    if let Some(home) = home.filter(|h| h.len() > 1) {
        out = out.replace(home.trim_end_matches('/'), "~");
    }
    for prefix in ["/Users/", "/home/"] {
        out = replace_user_dirs(&out, prefix);
    }
    out = replace_ipv4(&out);
    if let Some(host) = host.filter(|h| !h.is_empty() && *h != "unknown") {
        let short = host.split('.').next().unwrap_or(host);
        out = replace_word(&out, host, "<host>");
        if short.len() >= 3 {
            out = replace_word(&out, short, "<host>");
        }
    }
    out
}

/// `word` replaced wherever it is not part of a longer name: a host called `dev` must not turn
/// `device` into `<host>ice`.
fn replace_word(text: &str, word: &str, with: &str) -> String {
    let is_name = |c: char| c.is_alphanumeric() || c == '-' || c == '_';
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(word) {
        let before = rest[..at].chars().next_back();
        let after = rest[at + word.len()..].chars().next();
        out.push_str(&rest[..at]);
        if before.is_some_and(is_name) || after.is_some_and(is_name) {
            out.push_str(word);
        } else {
            out.push_str(with);
        }
        rest = &rest[at + word.len()..];
    }
    out.push_str(rest);
    out
}

/// `/Users/<name>/rest` and `/Users/<name>` at the end of a word become `~/rest` and `~`.
fn replace_user_dirs(text: &str, prefix: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(prefix) {
        out.push_str(&rest[..at]);
        let after = &rest[at + prefix.len()..];
        let name_len = after
            .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-' || c == '.'))
            .unwrap_or(after.len());
        if name_len == 0 {
            out.push_str(prefix);
            rest = after;
            continue;
        }
        out.push('~');
        rest = &after[name_len..];
    }
    out.push_str(rest);
    out
}

/// Every dotted IPv4 address in `text` that is not loopback, unspecified or a documentation
/// address becomes `<node>`; a port after it stays.
fn replace_ipv4(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        let starts_word = i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'.');
        if starts_word
            && bytes[i].is_ascii_digit()
            && let Some(len) = ipv4_at(&text[i..])
        {
            let address = &text[i..i + len];
            if keeps(address) {
                out.push_str(address);
            } else {
                out.push_str("<node>");
            }
            i += len;
            continue;
        }
        let ch = text[i..].chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// The length of the IPv4 address `text` starts with, if it starts with one: four numbers of
/// 0-255 joined by dots, not followed by another digit, dot-digit or letter.
fn ipv4_at(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut at = 0;
    for part in 0..4 {
        let start = at;
        while at < bytes.len() && bytes[at].is_ascii_digit() && at - start < 3 {
            at += 1;
        }
        if at == start || text[start..at].parse::<u16>().ok()? > 255 {
            return None;
        }
        if part < 3 {
            if bytes.get(at) != Some(&b'.') {
                return None;
            }
            at += 1;
        }
    }
    match bytes.get(at) {
        Some(b) if b.is_ascii_alphanumeric() => None,
        Some(b'.') if bytes.get(at + 1).is_some_and(|b| b.is_ascii_digit()) => None,
        _ => Some(at),
    }
}

/// Addresses that identify nothing: loopback, unspecified, and the documentation ranges.
fn keeps(address: &str) -> bool {
    address.starts_with("127.")
        || address == "0.0.0.0"
        || address.starts_with("192.0.2.")
        || address.starts_with("198.51.100.")
        || address.starts_with("203.0.113.")
}

/// The Environment section: the client's version and platform, and the platform and engines
/// of the node the checkout is placed on, when it answered.
pub fn environment(node: Option<&prod_code_protocol::StatusResponse>) -> String {
    let mut out = format!(
        "## Environment\n\n- client: prod-code {} on {}\n",
        env!("CARGO_PKG_VERSION"),
        prod_code_protocol::platform()
    );
    if let Some(status) = node {
        out.push_str(&format!(
            "- node: {}, engines: {}\n",
            status.platform.as_deref().unwrap_or("unknown platform"),
            status.detected_engines.join(", ")
        ));
    }
    out
}

/// An issue as it will be filed.
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub title: String,
    pub body: String,
    /// See [`issue_labels`].
    pub labels: Vec<String>,
}

/// The scrubbed title and the scrubbed body with the Environment section, or an error when the
/// report is too thin to act on.
pub fn draft(
    title: &str,
    body: &str,
    node: Option<&prod_code_protocol::StatusResponse>,
) -> Result<Draft> {
    let title = title.trim();
    let body = body.trim();
    anyhow::ensure!(
        title.chars().count() >= 10,
        "the title is too short to search for: say what went wrong and where, e.g. \
         `code_references returns nothing for a field in a Go struct`"
    );
    anyhow::ensure!(
        body.chars().count() >= 40,
        "the body must say what was run, what came back and what was expected"
    );
    let home = std::env::var("HOME").ok();
    let host = prod_code_protocol::client_host();
    let clean = |text: &str| scrub(text, home.as_deref(), Some(&host));
    Ok(Draft {
        title: clean(title),
        body: format!(
            "{}\n\n{}\n_Filed with `prod-code report-issue`._\n",
            clean(body),
            clean(&environment(node))
        ),
        labels: Vec::new(),
    })
}

/// An issue, open or closed, whose title matched. A closed one may be a bug that is already
/// fixed in a newer release, which is worth knowing before reporting it again.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct Similar {
    pub number: u64,
    pub title: String,
    pub url: String,
    #[serde(default)]
    pub state: String,
}

/// What reporting did.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Nothing was sent: the draft as it would be filed.
    DryRun(Draft),
    /// Nothing was filed: issues that may be the same problem.
    Similar(Draft, Vec<Similar>),
    /// The new issue's URL.
    Filed(String),
}

impl Outcome {
    pub fn render(&self) -> String {
        match self {
            Outcome::DryRun(draft) => format!(
                "Dry run, nothing was filed. The issue would be:\n\n# {}\nLabels: {}\n\n{}",
                draft.title,
                draft.labels.join(", "),
                draft.body
            ),
            Outcome::Similar(draft, similar) => {
                let mut out = format!(
                    "Nothing was filed: {} issue(s) in {REPOSITORY} have a similar title.\n",
                    similar.len()
                );
                for issue in similar {
                    out.push_str(&format!(
                        "  #{} [{}] {} — {}\n",
                        issue.number,
                        issue.state.to_lowercase(),
                        issue.title,
                        issue.url
                    ));
                }
                out.push_str(&format!(
                    "\nAn open one that is the same problem: add what you saw with `gh issue \
                     comment <number> --repo {REPOSITORY} --body …`. A closed one may be fixed \
                     in a newer release than this client ({}): check its closing change and \
                     `prod-code --version`. A different problem: report again with `force` to \
                     file \"{}\".",
                    env!("CARGO_PKG_VERSION"),
                    draft.title
                ));
                out
            }
            Outcome::Filed(url) => format!("Filed: {url}"),
        }
    }
}

/// A report as the reporter gives it.
pub struct ReportRequest<'a> {
    pub title: &'a str,
    pub body: &'a str,
    /// File it even when issues with a similar title exist.
    pub force: bool,
    /// Draft it and send nothing.
    pub dry_run: bool,
    /// The id of a private record of the details the issue cannot carry, kept by the reporter
    /// elsewhere; the issue names it so that the details can be looked up.
    pub private_ref: Option<&'a str>,
    /// The labels asked for; see [`issue_labels`].
    pub labels: &'a [String],
}

/// Files the issue with the `gh` program `gh`, unless it is a dry run or issues look the same
/// and `force` is not set. `remote` is the node asked for the Environment section.
pub async fn report(
    remote: Option<SocketAddr>,
    request: ReportRequest<'_>,
    gh: &Path,
) -> Result<Outcome> {
    let labels = issue_labels(request.labels)?;
    let node = match remote {
        Some(addr) => crate::cluster::node_status(addr).await.ok(),
        None => None,
    };
    let mut draft = draft(request.title, request.body, node.as_ref())?;
    draft.labels = labels;
    if let Some(reference) = request.private_ref.map(str::trim).filter(|r| !r.is_empty()) {
        draft.body = draft.body.replace(
            "_Filed with `prod-code report-issue`._",
            &format!(
                "Private details: report `{}`.\n_Filed with `prod-code report-issue`._",
                scrub(reference, None, None)
            ),
        );
    }
    if request.dry_run {
        return Ok(Outcome::DryRun(draft));
    }
    if !request.force {
        let similar = search(gh, &draft.title)?;
        if !similar.is_empty() {
            return Ok(Outcome::Similar(draft, similar));
        }
    }
    // The body goes through stdin, so that it is never written to a file on this machine.
    let mut child = std::process::Command::new(gh)
        .args([
            "issue",
            "create",
            "--repo",
            REPOSITORY,
            "--title",
            &draft.title,
        ])
        .args(
            draft
                .labels
                .iter()
                .flat_map(|label| ["--label", label.as_str()]),
        )
        .args(["--body-file", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("running {}", gh.display()))?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin.write_all(draft.body.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    anyhow::ensure!(
        output.status.success(),
        "gh issue create failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let url = String::from_utf8_lossy(&output.stdout)
        .lines()
        .rev()
        .find(|line| line.starts_with("https://"))
        .unwrap_or("")
        .to_string();
    anyhow::ensure!(!url.is_empty(), "gh issue create printed no issue URL");
    Ok(Outcome::Filed(url))
}

/// Issues, open or closed, whose titles match the words of `title`.
fn search(gh: &Path, title: &str) -> Result<Vec<Similar>> {
    let query = format!("{title} in:title");
    let output = std::process::Command::new(gh)
        .args([
            "issue", "list", "--repo", REPOSITORY, "--state", "all", "--search",
        ])
        .arg(&query)
        .args(["--json", "number,title,url,state", "--limit"])
        .arg(DUPLICATES_SHOWN.to_string())
        .output()
        .with_context(|| format!("running {}", gh.display()))?;
    anyhow::ensure!(
        output.status.success(),
        "gh issue list failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(serde_json::from_slice(&output.stdout).unwrap_or_default())
}

/// The `gh` program: `PROD_CODE_GH` when set, `gh` from the PATH otherwise.
pub fn gh_program() -> std::path::PathBuf {
    std::env::var_os("PROD_CODE_GH")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("gh"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_details_are_scrubbed_and_documentation_addresses_stay() {
        let text = "gateway 192.168.2.190:9400 and 10.0.0.7 failed; loopback 127.0.0.1:9400, \
                    docs 192.0.2.20:9400; version 1.2.3.4.5 and 300.1.1.1; \
                    /Users/alice/work/repo/src/a.rs and /home/bob/.cargo/registry; \
                    /Users/carol:end; host studio-7.local answered, studio-7 too";
        let clean = scrub(text, Some("/Users/alice"), Some("studio-7.local"));
        assert_eq!(
            clean,
            "gateway <node>:9400 and <node> failed; loopback 127.0.0.1:9400, \
             docs 192.0.2.20:9400; version 1.2.3.4.5 and 300.1.1.1; \
             ~/work/repo/src/a.rs and ~/.cargo/registry; \
             ~:end; host <host> answered, <host> too"
        );
        assert_eq!(
            scrub("/Users/ and /home/", None, None),
            "/Users/ and /home/"
        );
        assert_eq!(
            scrub("a1.2.3.4 1.2.3.4b", None, Some("unknown")),
            "a1.2.3.4 1.2.3.4b"
        );
        assert_eq!(
            scrub("dev device dev-box dev.", None, Some("dev")),
            "<host> device dev-box <host>."
        );
    }

    #[test]
    fn a_draft_needs_a_searchable_title_and_a_body_and_gets_an_environment() {
        assert!(
            draft(
                "short",
                "a body that is long enough to act on, really",
                None
            )
            .is_err()
        );
        assert!(draft("code_references misses a field", "too thin", None).is_err());
        let node = prod_code_protocol::StatusResponse {
            server_pid: 1,
            uptime_seconds: 1,
            active_sessions: 0,
            loaded_workspaces: 0,
            detected_engines: vec!["rust".into(), "go".into()],
            memory_rss_bytes: None,
            total_queries: 0,
            active_queries: 0,
            load_average_millis: None,
            cpu_count: None,
            platform: Some("linux x86_64".into()),
            running_commands: Vec::new(),
            host: Default::default(),
        };
        let draft = draft(
            "  code_references misses a field  ",
            "Ran `prod-code refs --symbol Store::limit` against 10.1.2.3 and got nothing back.",
            Some(&node),
        )
        .unwrap();
        assert_eq!(draft.title, "code_references misses a field");
        assert!(
            draft.body.contains("against <node> and got"),
            "{}",
            draft.body
        );
        assert!(
            draft
                .body
                .contains("- node: linux x86_64, engines: rust, go"),
            "{}",
            draft.body
        );
        assert!(
            draft
                .body
                .contains(&format!("prod-code {}", env!("CARGO_PKG_VERSION")))
        );
        assert!(
            draft
                .body
                .ends_with("_Filed with `prod-code report-issue`._\n")
        );
    }

    /// A stand-in `gh` that records its arguments and answers like the real one: `list` prints
    /// the JSON given in `similar`, `create` prints an issue URL.
    fn fake_gh(dir: &Path, similar: &str) -> std::path::PathBuf {
        let script = dir.join("gh");
        let log = dir.join("calls.log");
        let text = format!(
            "#!/bin/sh\necho \"$@\" >> '{}'\ncase \"$2\" in\n  list) echo '{similar}' ;;\n  \
             create) cat > '{}' ; echo 'https://github.com/{REPOSITORY}/issues/999' ;;\nesac\n",
            log.display(),
            dir.join("body.md").display()
        );
        // Written by a child process: a descriptor for writing to the script held by this
        // process would be inherited by a process another test forks meanwhile, until it execs,
        // and Linux refuses to run a file open for writing anywhere (ETXTBSY) (#382).
        use std::io::Write;
        let mut writer = std::process::Command::new("sh")
            .arg("-c")
            .arg("cat > \"$1\" && chmod 755 \"$1\"")
            .arg("sh")
            .arg(&script)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        writer
            .stdin
            .take()
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
        assert!(writer.wait().unwrap().success(), "{}", script.display());
        script
    }

    #[test]
    fn labels_default_to_a_bug_and_a_label_the_repository_lacks_is_refused() {
        let given = |labels: &[&str]| labels.iter().map(|l| l.to_string()).collect::<Vec<_>>();
        assert_eq!(issue_labels(&[]).unwrap(), ["bug"]);
        assert_eq!(issue_labels(&given(&["mcp"])).unwrap(), ["bug", "mcp"]);
        assert_eq!(
            issue_labels(&given(&[" Perf ", "gateway", "gateway", ""])).unwrap(),
            ["perf", "gateway"]
        );
        let err = issue_labels(&given(&["mcp", "urgent"]))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("`urgent`") && err.contains("enhancement") && err.contains("worktree"),
            "the refusal names the label and the ones there are: {err}"
        );
    }

    const BODY: &str = "Ran `prod-code outline README.md` on /Users/alice/repo and it printed an \
                        empty outline instead of an error.";

    #[tokio::test]
    async fn a_report_is_filed_when_nothing_similar_is_open() {
        let dir = tempfile::tempdir().unwrap();
        let gh = fake_gh(dir.path(), "[]");
        let labels = ["mcp".to_string()];
        let outcome = report(
            None,
            ReportRequest {
                title: "outline prints nothing for Markdown",
                body: BODY,
                force: false,
                dry_run: false,
                private_ref: None,
                labels: &labels,
            },
            &gh,
        )
        .await
        .unwrap();
        assert_eq!(
            outcome,
            Outcome::Filed(format!("https://github.com/{REPOSITORY}/issues/999"))
        );
        let calls = std::fs::read_to_string(dir.path().join("calls.log")).unwrap();
        assert!(calls.contains("issue list --repo alex09x/prod-code --state all --search outline prints nothing for Markdown in:title --json number,title,url,state"), "{calls}");
        // The type defaults to a bug, and the area asked for goes with it.
        assert!(calls.contains("issue create --repo alex09x/prod-code --title outline prints nothing for Markdown --label bug --label mcp --body-file -"), "{calls}");
        let sent = std::fs::read_to_string(dir.path().join("body.md")).unwrap();
        assert!(sent.contains("on ~/repo and it printed"), "{sent}");
        assert!(!sent.contains("alice"), "{sent}");
        assert!(outcome.render().starts_with("Filed: https://"));
    }

    #[tokio::test]
    async fn a_similar_open_issue_stops_the_report_unless_forced() {
        let dir = tempfile::tempdir().unwrap();
        let gh = fake_gh(
            dir.path(),
            r#"[{"number":270,"title":"code_outline returns an empty outline","url":"https://github.com/alex09x/prod-code/issues/270","state":"CLOSED"}]"#,
        );
        let outcome = report(
            None,
            ReportRequest {
                title: "outline prints nothing for Markdown",
                body: BODY,
                force: false,
                dry_run: false,
                private_ref: None,
                labels: &[],
            },
            &gh,
        )
        .await
        .unwrap();
        let Outcome::Similar(_, similar) = &outcome else {
            panic!("{outcome:?}");
        };
        assert_eq!(similar[0].number, 270);
        let text = outcome.render();
        assert!(
            text.contains("#270 [closed] code_outline returns an empty outline"),
            "{text}"
        );
        assert!(text.contains("may be fixed in a newer release"), "{text}");
        assert!(text.contains("gh issue comment <number>"), "{text}");
        let calls = std::fs::read_to_string(dir.path().join("calls.log")).unwrap();
        assert!(!calls.contains("issue create"), "{calls}");

        let forced = report(
            None,
            ReportRequest {
                title: "outline prints nothing for Markdown",
                body: BODY,
                force: true,
                dry_run: false,
                private_ref: None,
                labels: &[],
            },
            &gh,
        )
        .await
        .unwrap();
        assert!(matches!(forced, Outcome::Filed(_)), "{forced:?}");
    }

    /// A private reference is named in the issue, and nothing else about it is sent (#290).
    #[tokio::test]
    async fn a_private_reference_is_named_in_the_issue() {
        let dir = tempfile::tempdir().unwrap();
        let gh = fake_gh(dir.path(), "[]");
        let outcome = report(
            None,
            ReportRequest {
                title: "outline prints nothing for Markdown",
                body: BODY,
                force: false,
                dry_run: false,
                private_ref: Some("  inc-2026-09-24-outline  "),
                labels: &[],
            },
            &gh,
        )
        .await
        .unwrap();
        assert!(matches!(outcome, Outcome::Filed(_)), "{outcome:?}");
        let public = std::fs::read_to_string(dir.path().join("body.md")).unwrap();
        assert!(
            public.contains("Private details: report `inc-2026-09-24-outline`.\n_Filed with"),
            "{public}"
        );
        let dry = report(
            None,
            ReportRequest {
                title: "outline prints nothing for Markdown",
                body: BODY,
                force: false,
                dry_run: true,
                private_ref: Some(""),
                labels: &[],
            },
            &gh,
        )
        .await
        .unwrap();
        assert!(
            !dry.render().contains("Private details"),
            "{}",
            dry.render()
        );
    }

    #[tokio::test]
    async fn a_dry_run_sends_nothing_and_a_failing_gh_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let gh = fake_gh(dir.path(), "[]");
        let outcome = report(
            None,
            ReportRequest {
                title: "outline prints nothing for Markdown",
                body: BODY,
                force: false,
                dry_run: true,
                private_ref: None,
                labels: &[],
            },
            &gh,
        )
        .await
        .unwrap();
        assert!(matches!(outcome, Outcome::DryRun(_)));
        assert!(
            outcome
                .render()
                .contains("# outline prints nothing for Markdown\nLabels: bug\n")
        );
        assert!(!dir.path().join("calls.log").exists());

        // A label the repository does not have is refused before anything is searched.
        let unknown = ["urgent".to_string()];
        let err = report(
            None,
            ReportRequest {
                title: "outline prints nothing for Markdown",
                body: BODY,
                force: false,
                dry_run: false,
                private_ref: None,
                labels: &unknown,
            },
            &gh,
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("`urgent`"), "{err:#}");
        assert!(!dir.path().join("calls.log").exists());

        let missing = dir.path().join("no-such-gh");
        let err = report(
            None,
            ReportRequest {
                title: "outline prints nothing for Markdown",
                body: BODY,
                force: false,
                dry_run: false,
                private_ref: None,
                labels: &[],
            },
            &missing,
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("no-such-gh"), "{err:#}");
        assert_eq!(gh_program().file_name().unwrap(), "gh");
    }
}
