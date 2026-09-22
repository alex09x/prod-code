//! Shadow runs from the client side (roadmap 7.4): send named hypotheses (complete proposed
//! file contents) to the gateway, which runs a command once per hypothesis in a private shadow
//! of the workspace; rank the outcomes and describe the winner as a unified diff against the
//! checkout. Shared by the MCP tool `code_shadow_run` and the CLI `prod-code shadow-run`.

use crate::sync::{WorkspaceIdentity, push_workspace_sync, workspace_identity};
use crate::verify;
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    FileDelta, ProdCodeCodec, ShadowHypothesis, ShadowRunRequest, WireMessage,
};
use std::net::SocketAddr;
use std::path::Path;
use tokio::net::TcpStream;
use tokio_util::codec::Framed;

/// One proposed file of a hypothesis; `text: None` deletes the file.
#[derive(Debug, Clone)]
pub struct HypothesisEdit {
    /// `/`-separated path relative to the workspace root.
    pub relative_path: String,
    pub text: Option<String>,
}

#[derive(Debug, Clone)]
pub struct HypothesisSpec {
    pub name: String,
    pub edits: Vec<HypothesisEdit>,
}

/// What happened to one hypothesis, with the diff it stands for.
#[derive(Debug, Clone)]
pub struct HypothesisOutcome {
    pub name: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub timed_out: bool,
    pub error: Option<String>,
    /// Tail of the combined output (lossy UTF-8).
    pub output: String,
    pub output_len: u64,
    /// (passed, failed) when the command's output could be parsed as a test run.
    pub tests: Option<(u64, u64)>,
    pub diff: String,
    pub changed_lines: usize,
}

impl HypothesisOutcome {
    pub fn passed(&self) -> bool {
        self.error.is_none() && !self.timed_out && self.exit_code == Some(0)
    }
}

#[derive(Debug, Clone)]
pub struct ShadowOutcome {
    pub mode: String,
    pub server_workspace_root: String,
    pub results: Vec<HypothesisOutcome>,
    /// Indices into `results`, best first.
    pub ranking: Vec<usize>,
    /// The best hypothesis when it passed.
    pub winner: Option<usize>,
}

/// The `/`-separated path of `file` inside `root`.
pub fn relative_edit_path(root: &Path, file: &Path) -> Result<String> {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let file = if file.is_absolute() {
        file.to_path_buf()
    } else {
        root.join(file)
    };
    // The file may not exist yet: canonicalize its parent when possible.
    let file = match (file.parent(), file.file_name()) {
        (Some(parent), Some(name)) if parent.exists() => std::fs::canonicalize(parent)
            .map(|p| p.join(name))
            .unwrap_or(file),
        _ => file,
    };
    let rel = file
        .strip_prefix(&root)
        .with_context(|| format!("{} is outside the workspace", file.display()))?;
    let rel = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    anyhow::ensure!(!rel.is_empty(), "an edit needs a file path");
    Ok(rel)
}

/// Parses the `hypotheses` array shared by the MCP tool and the CLI spec file: every entry has
/// a `name`, `edits` of `{path, new_text}` or `{path, file}` (the proposed content read from
/// that local file, relative to `file_base`) and optional `delete` paths. A hypothesis without
/// edits is the baseline.
pub fn parse_specs(
    root: &Path,
    json: &serde_json::Value,
    file_base: Option<&Path>,
) -> Result<Vec<HypothesisSpec>> {
    let list = json
        .get("hypotheses")
        .and_then(|v| v.as_array())
        .context("missing 'hypotheses' array")?;
    anyhow::ensure!(!list.is_empty(), "'hypotheses' is empty");
    let mut specs = Vec::with_capacity(list.len());
    for (i, hypothesis) in list.iter().enumerate() {
        let name = hypothesis
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("h{}", i + 1));
        let mut edits = Vec::new();
        for edit in hypothesis
            .get("edits")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
        {
            let path = edit
                .get("path")
                .and_then(|v| v.as_str())
                .with_context(|| format!("hypothesis {name}: edit without 'path'"))?;
            let text = if let Some(text) = edit.get("new_text").and_then(|v| v.as_str()) {
                text.to_string()
            } else if let Some(file) = edit.get("file").and_then(|v| v.as_str()) {
                let file = Path::new(file);
                let file = if file.is_absolute() {
                    file.to_path_buf()
                } else {
                    file_base.unwrap_or(root).join(file)
                };
                std::fs::read_to_string(&file)
                    .with_context(|| format!("hypothesis {name}: cannot read {}", file.display()))?
            } else {
                anyhow::bail!("hypothesis {name}: edit for {path} needs 'new_text' or 'file'");
            };
            edits.push(HypothesisEdit {
                relative_path: relative_edit_path(root, Path::new(path))?,
                text: Some(text),
            });
        }
        for path in hypothesis
            .get("delete")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
        {
            edits.push(HypothesisEdit {
                relative_path: relative_edit_path(root, Path::new(path))?,
                text: None,
            });
        }
        specs.push(HypothesisSpec { name, edits });
    }
    Ok(specs)
}

/// Sends the hypotheses, waits for the gateway to run them all, ranks the outcomes.
#[allow(clippy::too_many_arguments)]
pub async fn run_shadow(
    remote: SocketAddr,
    root: &Path,
    subdir: Option<&str>,
    specs: &[HypothesisSpec],
    command: Vec<String>,
    env: Vec<(String, String)>,
    timeout_secs: u64,
    parallel: usize,
    tail_bytes: usize,
) -> Result<ShadowOutcome> {
    anyhow::ensure!(!command.is_empty(), "empty command");
    anyhow::ensure!(!specs.is_empty(), "no hypotheses");
    let identity: WorkspaceIdentity = workspace_identity(root);
    let stream = TcpStream::connect(remote)
        .await
        .with_context(|| format!("failed to connect to remote gateway at {remote}"))?;
    let _ = stream.set_nodelay(true);
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    push_workspace_sync(&mut framed, root, &identity, None)
        .await
        .context("pre-flight workspace sync failed")?;
    let hypotheses = specs
        .iter()
        .map(|spec| ShadowHypothesis {
            name: spec.name.clone(),
            files: spec
                .edits
                .iter()
                .map(|edit| FileDelta {
                    relative_path: edit.relative_path.clone(),
                    content: edit.text.as_ref().map(|t| t.as_bytes().to_vec()),
                    is_executable: false,
                })
                .collect(),
        })
        .collect();
    framed
        .send(WireMessage::ShadowRunRequest(ShadowRunRequest {
            client_workspace_root: root.to_string_lossy().to_string(),
            base_workspace_name: Some(identity.name.clone()),
            hypotheses,
            command: command.clone(),
            env,
            timeout_secs,
            subdir: subdir.map(str::to_string),
            parallel,
            tail_bytes,
            client_agent: Some(prod_code_protocol::detect_client_agent()),
            client_host: Some(prod_code_protocol::client_host()),
        }))
        .await?;
    let response = loop {
        match framed.next().await {
            Some(Ok(WireMessage::ShadowRunResponse(response))) => {
                let _ = framed
                    .send(WireMessage::Disconnect {
                        reason: "shadow run finished".to_string(),
                    })
                    .await;
                break response;
            }
            Some(Ok(WireMessage::Pong)) | Some(Ok(WireMessage::LspPayload(_))) => {}
            Some(Ok(other)) => anyhow::bail!("unexpected message during shadow run: {other:?}"),
            Some(Err(e)) => anyhow::bail!("frame decode error during shadow run: {e}"),
            None => anyhow::bail!("gateway closed the connection during the shadow run"),
        }
    };
    if let Some(error) = response.error {
        anyhow::bail!("shadow run refused: {error}");
    }
    let results: Vec<HypothesisOutcome> = response
        .results
        .into_iter()
        .map(|r| {
            let output =
                String::from_utf8_lossy(r.output_tail.as_deref().unwrap_or_default()).into_owned();
            let (diff, changed_lines) = specs
                .iter()
                .find(|s| s.name == r.name)
                .map(|s| unified_diff(root, s))
                .unwrap_or_default();
            HypothesisOutcome {
                tests: test_counts(&command, &output),
                name: r.name,
                exit_code: r.exit_code,
                duration_ms: r.duration_ms,
                timed_out: r.timed_out,
                error: r.error,
                output,
                output_len: r.output_len,
                diff,
                changed_lines,
            }
        })
        .collect();
    let ranking = rank(&results);
    let winner = ranking.first().copied().filter(|&i| results[i].passed());
    Ok(ShadowOutcome {
        mode: response.mode,
        server_workspace_root: response.server_workspace_root,
        results,
        ranking,
        winner,
    })
}

/// Best first: passed before failed, then fewer failing tests, more passing tests, a smaller
/// diff, and finally the order the hypotheses were given in.
pub fn rank(results: &[HypothesisOutcome]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..results.len()).collect();
    order.sort_by_key(|&i| {
        let r = &results[i];
        let (passed, failed) = r.tests.unwrap_or((0, 0));
        (
            !r.passed(),
            failed,
            std::cmp::Reverse(passed),
            r.changed_lines,
            i,
        )
    });
    order
}

/// (passed, failed) parsed from the output of a known test runner, chosen by the command.
pub fn test_counts(command: &[String], output: &str) -> Option<(u64, u64)> {
    let program = command
        .first()
        .map(|p| p.rsplit('/').next().unwrap_or(p))
        .unwrap_or("");
    let has = |word: &str| command.iter().skip(1).any(|a| a == word);
    let joined = command.join(" ");
    let (passed, failed, _) = match program {
        "cargo" if has("test") || has("nextest") => verify::parse_cargo_test_text(output),
        "go" if has("test") && has("-json") => verify::parse_go_test_json(output),
        "pytest" | "py.test" => verify::parse_pytest_text(output),
        "python" | "python3" if joined.contains("pytest") => verify::parse_pytest_text(output),
        "python" | "python3" if joined.contains("unittest") => verify::parse_unittest_text(output),
        "vitest" => verify::parse_vitest_text(output),
        "jest" => verify::parse_jest_text(output),
        "bun" if has("test") => verify::parse_bun_test_text(output),
        "npx" | "npm" | "pnpm" | "yarn" if joined.contains("vitest") => {
            verify::parse_vitest_text(output)
        }
        "npx" | "npm" | "pnpm" | "yarn" if joined.contains("jest") => {
            verify::parse_jest_text(output)
        }
        "swift" if has("test") => verify::parse_xctest_text(output),
        "xcodebuild" => verify::parse_xctest_text(output),
        "ctest" => verify::parse_ctest_text(output),
        "meson" if has("test") => verify::parse_meson_test_text(output),
        _ => return None,
    };
    Some((passed, failed))
}

/// Unified diff of the hypothesis against the checkout and the number of changed lines.
pub fn unified_diff(root: &Path, spec: &HypothesisSpec) -> (String, usize) {
    let mut out = String::new();
    let mut changed = 0;
    for edit in &spec.edits {
        let path = root.join(&edit.relative_path);
        let exists = path.exists();
        let old = std::fs::read_to_string(&path).unwrap_or_default();
        let new = edit.text.clone().unwrap_or_default();
        if old == new && exists == edit.text.is_some() {
            continue;
        }
        let diff = similar::TextDiff::from_lines(&old, &new);
        changed += diff
            .iter_all_changes()
            .filter(|c| c.tag() != similar::ChangeTag::Equal)
            .count();
        let a = if exists {
            format!("a/{}", edit.relative_path)
        } else {
            "/dev/null".to_string()
        };
        let b = if edit.text.is_some() {
            format!("b/{}", edit.relative_path)
        } else {
            "/dev/null".to_string()
        };
        out.push_str(
            &diff
                .unified_diff()
                .context_radius(3)
                .header(&a, &b)
                .to_string(),
        );
    }
    (out, changed)
}

/// Writes a hypothesis into the checkout; returns the paths written or deleted.
pub fn apply_hypothesis(root: &Path, spec: &HypothesisSpec) -> Result<Vec<String>> {
    let mut touched = Vec::new();
    for edit in &spec.edits {
        let path = root.join(&edit.relative_path);
        match &edit.text {
            Some(text) => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, text)
                    .with_context(|| format!("cannot write {}", path.display()))?;
            }
            None => {
                if path.exists() {
                    std::fs::remove_file(&path)
                        .with_context(|| format!("cannot delete {}", path.display()))?;
                }
            }
        }
        touched.push(edit.relative_path.clone());
    }
    Ok(touched)
}

/// One text for agents and the CLI: every hypothesis on a line, the winner's diff, the tail
/// of every failing hypothesis's output.
pub fn render_report(
    outcome: &ShadowOutcome,
    command: &[String],
    applied: Option<&[String]>,
    failure_tail_chars: usize,
) -> String {
    let mut text = format!(
        "$ {}   ({} hypothesis(es), {} mode, on {})\n",
        command.join(" "),
        outcome.results.len(),
        outcome.mode,
        outcome.server_workspace_root
    );
    let width = outcome
        .results
        .iter()
        .map(|r| r.name.len())
        .max()
        .unwrap_or(4)
        .max(4);
    for &i in &outcome.ranking {
        let r = &outcome.results[i];
        let status = match (&r.error, r.timed_out, r.exit_code) {
            (Some(err), _, _) => format!("failed: {err}"),
            (None, true, _) => "timed out".to_string(),
            (None, false, Some(code)) => format!("exit {code}"),
            (None, false, None) => "killed".to_string(),
        };
        let tests = match r.tests {
            Some((p, f)) => format!("  {p} passed, {f} failed"),
            None => String::new(),
        };
        let mark = if outcome.winner == Some(i) {
            "  <- winner"
        } else {
            ""
        };
        text.push_str(&format!(
            "  {:width$}  {status:<10} in {:>6.1}s{tests}  ~{} changed line(s){mark}\n",
            r.name,
            r.duration_ms as f64 / 1000.0,
            r.changed_lines,
            width = width
        ));
    }
    match outcome.winner {
        Some(i) => {
            let r = &outcome.results[i];
            text.push_str(&format!("winner: {}\n", r.name));
            if r.diff.is_empty() {
                text.push_str("(no difference from the checkout)\n");
            } else {
                text.push_str(&r.diff);
                if !r.diff.ends_with('\n') {
                    text.push('\n');
                }
            }
            if let Some(files) = applied {
                text.push_str(&format!(
                    "[applied {} file(s) to the checkout: {}]\n",
                    files.len(),
                    files.join(", ")
                ));
            }
        }
        None => {
            if let Some(&best) = outcome.ranking.first() {
                text.push_str(&format!(
                    "no hypothesis passed; closest: {}\n",
                    outcome.results[best].name
                ));
            }
        }
    }
    for &i in &outcome.ranking {
        let r = &outcome.results[i];
        if r.passed() || r.output.is_empty() {
            continue;
        }
        // Parallel cargos wait on the shared package-cache lock; that noise is not a finding.
        let output: String = r
            .output
            .lines()
            .filter(|l| !l.trim_start().starts_with("Blocking waiting for file lock"))
            .collect::<Vec<_>>()
            .join("\n");
        let tail: String = {
            let chars: Vec<char> = output.chars().collect();
            let start = chars.len().saturating_sub(failure_tail_chars);
            chars[start..].iter().collect()
        };
        text.push_str(&format!(
            "--- {} output (last {} of {} bytes) ---\n{}\n",
            r.name,
            tail.len(),
            r.output_len,
            tail.trim_end()
        ));
    }
    text.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(
        name: &str,
        exit: Option<i32>,
        tests: Option<(u64, u64)>,
        lines: usize,
    ) -> HypothesisOutcome {
        HypothesisOutcome {
            name: name.to_string(),
            exit_code: exit,
            duration_ms: 1000,
            timed_out: false,
            error: None,
            output: String::new(),
            output_len: 0,
            tests,
            diff: String::new(),
            changed_lines: lines,
        }
    }

    #[test]
    fn ranking_prefers_passing_then_fewer_failures_then_smaller_diff() {
        let results = vec![
            outcome("big", Some(0), Some((10, 0)), 40),
            outcome("broken", Some(101), Some((9, 1)), 5),
            outcome("small", Some(0), Some((10, 0)), 8),
            outcome("worse", Some(101), Some((7, 3)), 2),
        ];
        assert_eq!(rank(&results), vec![2, 0, 1, 3]);
        let none_passed = vec![
            outcome("a", Some(1), Some((3, 2)), 1),
            outcome("b", Some(1), Some((4, 1)), 9),
        ];
        assert_eq!(rank(&none_passed), vec![1, 0]);
    }

    #[test]
    fn test_counts_follow_the_command() {
        let cargo = ["cargo", "test", "-p", "x"].map(String::from);
        assert_eq!(
            test_counts(&cargo, "test result: ok. 6 passed; 0 failed; 0 ignored\n"),
            Some((6, 0))
        );
        let pytest = ["pytest", "-q"].map(String::from);
        assert_eq!(
            test_counts(&pytest, "===== 2 failed, 5 passed in 0.10s =====\n"),
            Some((5, 2))
        );
        let build = ["cargo", "build"].map(String::from);
        assert_eq!(test_counts(&build, "Finished"), None);
    }

    #[test]
    fn unified_diff_marks_new_changed_and_deleted_files() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a.rs"), "fn a() {}\nfn b() {}\n").unwrap();
        std::fs::write(root.path().join("gone.rs"), "x\n").unwrap();
        let spec = HypothesisSpec {
            name: "h".to_string(),
            edits: vec![
                HypothesisEdit {
                    relative_path: "a.rs".to_string(),
                    text: Some("fn a() {}\nfn c() {}\n".to_string()),
                },
                HypothesisEdit {
                    relative_path: "new.rs".to_string(),
                    text: Some("fn n() {}\n".to_string()),
                },
                HypothesisEdit {
                    relative_path: "gone.rs".to_string(),
                    text: None,
                },
                HypothesisEdit {
                    relative_path: "same.rs".to_string(),
                    text: None,
                },
            ],
        };
        let (diff, changed) = unified_diff(root.path(), &spec);
        assert!(diff.contains("--- a/a.rs\n+++ b/a.rs\n"), "{diff}");
        assert!(diff.contains("-fn b() {}\n+fn c() {}\n"), "{diff}");
        assert!(diff.contains("--- /dev/null\n+++ b/new.rs\n"), "{diff}");
        assert!(diff.contains("--- a/gone.rs\n+++ /dev/null\n"), "{diff}");
        assert!(!diff.contains("same.rs"));
        assert_eq!(changed, 2 + 1 + 1);
        let touched = apply_hypothesis(root.path(), &spec).unwrap();
        assert_eq!(touched.len(), 4);
        assert!(root.path().join("new.rs").exists() && !root.path().join("gone.rs").exists());
    }

    #[test]
    fn relative_edit_path_resolves_and_rejects_paths_outside_the_workspace() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("src")).unwrap();

        assert_eq!(
            relative_edit_path(root.path(), Path::new("src/a.rs")).unwrap(),
            "src/a.rs"
        );
        let abs = root.path().join("src/b.rs");
        assert_eq!(relative_edit_path(root.path(), &abs).unwrap(), "src/b.rs");

        let err = relative_edit_path(root.path(), Path::new("/etc/passwd")).unwrap_err();
        assert!(format!("{err:#}").contains("outside the workspace"));

        let err = relative_edit_path(root.path(), root.path()).unwrap_err();
        assert!(format!("{err:#}").contains("needs a file path"));
    }

    #[test]
    fn parse_specs_builds_edits_from_inline_text_files_and_deletes_with_defaults() {
        let root = tempfile::tempdir().unwrap();
        let content_dir = tempfile::tempdir().unwrap();
        std::fs::write(content_dir.path().join("body.rs"), "fn a() {}\n").unwrap();

        let json = serde_json::json!({
            "hypotheses": [
                {
                    "edits": [
                        { "path": "src/a.rs", "new_text": "fn a() {}\n" },
                        { "path": "src/b.rs", "file": "body.rs" }
                    ]
                },
                {
                    "name": "  named  ",
                    "edits": [ { "path": "src/c.rs", "new_text": "x\n" } ],
                    "delete": [ "src/old.rs" ]
                }
            ]
        });
        let specs = parse_specs(root.path(), &json, Some(content_dir.path())).unwrap();
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].name, "h1", "an unnamed hypothesis gets a default name");
        assert_eq!(specs[0].edits[0].relative_path, "src/a.rs");
        assert_eq!(specs[0].edits[0].text.as_deref(), Some("fn a() {}\n"));
        assert_eq!(specs[0].edits[1].relative_path, "src/b.rs");
        assert_eq!(
            specs[0].edits[1].text.as_deref(),
            Some("fn a() {}\n"),
            "the file's content is read relative to file_base"
        );
        assert_eq!(specs[1].name, "named", "the given name is trimmed");
        assert_eq!(specs[1].edits.len(), 2);
        assert_eq!(specs[1].edits[1].relative_path, "src/old.rs");
        assert_eq!(specs[1].edits[1].text, None, "a delete has no text");

        assert!(parse_specs(root.path(), &serde_json::json!({}), None).is_err());
        assert!(
            parse_specs(root.path(), &serde_json::json!({"hypotheses": []}), None).is_err()
        );
        let no_path = serde_json::json!({"hypotheses":[{"edits":[{"new_text":"x"}]}]});
        assert!(parse_specs(root.path(), &no_path, None).is_err());
        let no_text = serde_json::json!({"hypotheses":[{"edits":[{"path":"a.rs"}]}]});
        let err = parse_specs(root.path(), &no_text, None).unwrap_err();
        assert!(format!("{err:#}").contains("needs 'new_text' or 'file'"));
    }

    #[test]
    fn render_report_shows_applied_files_and_the_closest_hypothesis_when_none_passed() {
        let mut results = vec![outcome("ok", Some(0), Some((1, 0)), 0)];
        results[0].diff = "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n".to_string();
        let ranking = rank(&results);
        let winner = Some(ranking[0]);
        let shadow = ShadowOutcome {
            mode: "in-place".to_string(),
            server_workspace_root: "/srv/ws".to_string(),
            results,
            ranking,
            winner,
        };
        let text = render_report(
            &shadow,
            &["cargo".to_string(), "test".to_string()],
            Some(&["src/a.rs".to_string()]),
            100,
        );
        assert!(
            text.contains("[applied 1 file(s) to the checkout: src/a.rs]"),
            "{text}"
        );

        let results2 = vec![outcome("a", Some(1), None, 3), outcome("b", Some(1), None, 1)];
        let ranking2 = rank(&results2);
        let shadow2 = ShadowOutcome {
            mode: "overlay".to_string(),
            server_workspace_root: "/srv".to_string(),
            results: results2,
            ranking: ranking2,
            winner: None,
        };
        let text2 = render_report(&shadow2, &["go".to_string(), "test".to_string()], None, 100);
        assert!(text2.contains("no hypothesis passed; closest:"), "{text2}");
    }

    #[test]
    fn report_names_the_winner_and_shows_failing_output() {
        let mut results = vec![
            outcome("ok", Some(0), Some((3, 0)), 2),
            outcome("bad", Some(101), Some((2, 1)), 2),
        ];
        results[0].diff = "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n".to_string();
        results[1].output = "thread 'main' panicked\n".to_string();
        results[1].output_len = 24;
        let ranking = rank(&results);
        let winner = Some(ranking[0]);
        let shadow = ShadowOutcome {
            mode: "overlay".to_string(),
            server_workspace_root: "/srv/ws".to_string(),
            results,
            ranking,
            winner,
        };
        let text = render_report(
            &shadow,
            &["cargo".to_string(), "test".to_string()],
            None,
            100,
        );
        assert!(text.contains("<- winner"), "{text}");
        assert!(text.contains("winner: ok\n--- a/x"), "{text}");
        assert!(text.contains("--- bad output"), "{text}");
        assert!(text.contains("panicked"), "{text}");
    }
}
