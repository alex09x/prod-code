/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;

/// Where prod-code's issues live.
pub const REPOSITORY: &str = "alex09x/prod-code";

/// Open issues listed as possible duplicates before a new one is filed.
pub const DUPLICATES_SHOWN: usize = 5;

/// What kind of issue it is: every issue carries one of these (#321).
pub const TYPE_LABELS: &[&str] = &["bug", "enhancement", "documentation", "perf"];

/// Which part of prod-code it is about: an issue carries those it touches.
pub const AREA_LABELS: &[&str] = &[
    "gateway", "client", "mcp", "cluster", "worktree", "infra", "test",
];

/// The labels an issue is filed with: those asked for, lower-cased and once each, and `bug`
/// first when none of them says what kind of issue it is. The documented `roadmap` marker
/// is also supported. Other labels are refused by this local policy before any request.
pub fn issue_labels(asked: &[String]) -> Result<Vec<String>> {
    let mut labels: Vec<String> = Vec::new();
    for label in asked
        .iter()
        .map(|l| l.trim().to_ascii_lowercase())
        .filter(|l| !l.is_empty())
    {
        anyhow::ensure!(
            TYPE_LABELS.contains(&label.as_str())
                || AREA_LABELS.contains(&label.as_str())
                || label == "roadmap",
            "unsupported report label `{label}`: give one type ({}) and the areas the issue \
             is about ({}), optionally with `roadmap`",
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

/// An issue as it will be filed.
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub title: String,
    pub body: String,
    /// See [`issue_labels`].
    pub labels: Vec<String>,
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
