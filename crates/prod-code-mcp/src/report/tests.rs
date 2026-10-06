/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

use super::gh::{gh_program, report};
use super::scrub::{draft, scrub};
use super::types::{Outcome, REPOSITORY, ReportRequest, issue_labels};

#[test]
fn private_details_are_scrubbed_and_documentation_addresses_stay() {
    let text = "gateway 192.168.77.5:9400 and 10.0.0.7 failed; loopback 127.0.0.1:9400, \
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
        ..Default::default()
    };
    let d1 = draft(
        "  code_references misses a field  ",
        "Ran `prod-code refs --symbol Store::limit` against 10.1.2.3 and got nothing back.",
        Some(&node),
    )
    .unwrap();
    assert_eq!(d1.title, "code_references misses a field");
    assert!(d1.body.contains("against <node> and got"), "{}", d1.body);
    assert!(
        d1.body.contains("- node: linux x86_64, engines: rust, go"),
        "{}",
        d1.body
    );

    let node_with_meta = prod_code_protocol::StatusResponse {
        server_pid: 2,
        platform: Some("linux x86_64".into()),
        detected_engines: vec!["rust".into()],
        version: Some("0.3.23".into()),
        git_commit: Some("3b0ae2a".into()),
        ..Default::default()
    };
    let draft_with_meta = draft(
        "code_references misses another field",
        "Ran `prod-code refs --symbol Store::limit` against 10.1.2.3 and got nothing back.",
        Some(&node_with_meta),
    )
    .unwrap();
    assert!(
        draft_with_meta
            .body
            .contains("- node: linux x86_64 (prod-code 0.3.23, commit 3b0ae2a), engines: rust"),
        "{}",
        draft_with_meta.body
    );
    assert!(
        d1.body
            .contains(&format!("prod-code {}", env!("CARGO_PKG_VERSION")))
    );
    assert!(
        d1.body
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
fn labels_default_to_a_bug_allow_roadmap_and_refuse_unsupported_labels() {
    let given = |labels: &[&str]| labels.iter().map(|l| l.to_string()).collect::<Vec<_>>();
    assert_eq!(issue_labels(&[]).unwrap(), ["bug"]);
    assert_eq!(issue_labels(&given(&["mcp"])).unwrap(), ["bug", "mcp"]);
    assert_eq!(
        issue_labels(&given(&[" Perf ", "gateway", "gateway", ""])).unwrap(),
        ["perf", "gateway"]
    );
    assert_eq!(
        issue_labels(&given(&["enhancement", "mcp", " Roadmap ", "roadmap"])).unwrap(),
        ["enhancement", "mcp", "roadmap"]
    );
    assert_eq!(
        issue_labels(&given(&["roadmap"])).unwrap(),
        ["bug", "roadmap"]
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
