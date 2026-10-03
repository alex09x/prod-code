//! Applying gateway refactorings (LSP `WorkspaceEdit`) to the local checkout. Rewritten files
//! are dropped from the sync watermark so the next pre-flight uploads them.

use anyhow::{Context, Result, anyhow, bail};
use std::path::{Path, PathBuf};
use url::Url;

fn uri_to_root_and_relative<'a>(
    canonical_roots: &'a [PathBuf],
    uri: &str,
) -> Result<(&'a Path, String)> {
    let path = Url::parse(uri)
        .ok()
        .and_then(|u| u.to_file_path().ok())
        .ok_or_else(|| anyhow!("not a file URI: {uri}"))?;
    let path = resolve(&path)?;
    let mut best_match: Option<(&Path, &Path)> = None;
    for root in canonical_roots {
        if let Ok(rel) = path.strip_prefix(root) {
            if rel.as_os_str().is_empty() {
                bail!("{uri} names the checkout itself, not a file in it");
            }
            match &best_match {
                Some((best_root, _)) if root.as_os_str().len() <= best_root.as_os_str().len() => {}
                _ => best_match = Some((root.as_path(), rel)),
            }
        }
    }
    let (matched_root, rel) = match best_match {
        Some((r, rel)) => (r, rel),
        None => {
            if canonical_roots.len() == 1 {
                bail!(
                    "{} is outside the checkout {}",
                    path.display(),
                    canonical_roots[0].display()
                );
            } else {
                bail!(
                    "{} is outside any of the specified repository roots ({})",
                    path.display(),
                    canonical_roots
                        .iter()
                        .map(|r| r.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
    };
    Ok((matched_root, rel.to_string_lossy().replace('\\', "/")))
}

#[allow(dead_code)]
fn uri_to_relative(root: &Path, uri: &str) -> Result<String> {
    let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let (_, rel) = uri_to_root_and_relative(&[canonical], uri)?;
    Ok(rel)
}


/// `root/rel`, refused unless it still resolves inside the checkout. Every path was resolved
/// before the first step, against the checkout as it was; an earlier step can move a directory
/// that holds a symlink so that a later path runs through it, so each step checks its paths
/// again right before it acts on them.
fn contained(root: &Path, rel: &str) -> Result<PathBuf> {
    let abs = root.join(rel);
    let real = resolve(&abs)?;
    anyhow::ensure!(
        real.starts_with(root),
        "{rel} leads outside the checkout, to {}, after the earlier steps of the edit",
        real.display()
    );
    Ok(abs)
}


/// `path` with every symlink on it resolved, including those above a part that does not exist
/// yet: a new file under a symlinked directory lands where the symlink points, and that is what
/// has to be inside the checkout. A symlink that cannot be resolved is refused, since writing
/// through it could land anywhere.
fn resolve(path: &Path) -> Result<PathBuf> {
    let mut missing = Vec::new();
    let mut at = path.to_path_buf();
    loop {
        match std::fs::canonicalize(&at) {
            Ok(mut real) => {
                real.extend(missing.iter().rev());
                return Ok(real);
            }
            Err(err) => {
                if std::fs::symlink_metadata(&at).is_ok() {
                    bail!("{} cannot be resolved: {err}", at.display());
                }
                let (Some(parent), Some(name)) = (at.parent(), at.file_name()) else {
                    bail!("{} has no existing directory above it", path.display());
                };
                missing.push(name.to_os_string());
                at = parent.to_path_buf();
            }
        }
    }
}

/// Applies LSP text edits (0-based line/character, character counted in UTF-16 code units, the
/// LSP default; prod-code negotiates no other position encoding) to `text`. A single edit
/// starting at 0:0 and ending at or past the last line replaces the whole file.
pub(crate) fn apply_text_edits(text: &str, edits: &[serde_json::Value]) -> Result<String> {
    apply_edits_counting(text, edits, char::len_utf16)
}

/// [`apply_text_edits`] for edits whose character counts Unicode scalar values, the columns the
/// crate's own text scanners produce rather than an analyzer's.
pub(crate) fn apply_scalar_text_edits(text: &str, edits: &[serde_json::Value]) -> Result<String> {
    apply_edits_counting(text, edits, |_| 1)
}

fn apply_edits_counting(
    text: &str,
    edits: &[serde_json::Value],
    width: fn(char) -> usize,
) -> Result<String> {
    let line_count = text.lines().count() as u64;
    if let [edit] = edits
        && edit.pointer("/range/start/line").and_then(|v| v.as_u64()) == Some(0)
        && edit
            .pointer("/range/start/character")
            .and_then(|v| v.as_u64())
            == Some(0)
        && edit
            .pointer("/range/end/line")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            >= line_count
    {
        return Ok(edit
            .get("newText")
            .and_then(|t| t.as_str())
            .unwrap_or_default()
            .to_string());
    }
    // General case: convert positions to byte offsets and apply from the end backwards.
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(
            text.char_indices()
                .filter(|(_, c)| *c == '\n')
                .map(|(i, _)| i + 1),
        )
        .collect();
    // A column past the end of its line is the line's end; one inside a character (half a
    // surrogate pair) is the character's end.
    let offset = |line: u64, character: u64| -> usize {
        let start = line_starts
            .get(line as usize)
            .copied()
            .unwrap_or(text.len());
        let rest = &text[start..];
        let end_of_line = rest.find('\n').unwrap_or(rest.len());
        let mut units = 0u64;
        for (i, c) in rest[..end_of_line].char_indices() {
            if units >= character {
                return start + i;
            }
            units += width(c) as u64;
        }
        start + end_of_line
    };
    let mut spans: Vec<(usize, usize, String)> = edits
        .iter()
        .map(|e| {
            let sl = e
                .pointer("/range/start/line")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let sc = e
                .pointer("/range/start/character")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let el = e
                .pointer("/range/end/line")
                .and_then(|v| v.as_u64())
                .unwrap_or(sl);
            let ec = e
                .pointer("/range/end/character")
                .and_then(|v| v.as_u64())
                .unwrap_or(sc);
            let new_text = e
                .get("newText")
                .and_then(|t| t.as_str())
                .unwrap_or_default()
                .to_string();
            (offset(sl, sc), offset(el, ec), new_text)
        })
        .collect();
    spans.sort_by_key(|span| std::cmp::Reverse(span.0));
    let mut out = text.to_string();
    for (start, end, new_text) in spans {
        if start > end || end > out.len() {
            return Err(anyhow!("text edit range {start}..{end} out of bounds"));
        }
        out.replace_range(start..end, &new_text);
    }
    Ok(out)
}

/// The bytes of the file at `abs`, or `None` where there is no file and one could be created
/// (nothing there, or a file where a directory above it would be). Any other failure is an
/// error: a file that exists but cannot be read is not an empty one.
fn read_existing(abs: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(abs) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err)
            if matches!(
                err.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(None)
        }
        Err(err) => Err(anyhow!(err).context(format!("cannot read {}", abs.display()))),
    }
}

/// The text of the file at `abs` for editing: empty where there is no file yet.
fn text_for_edit(abs: &Path) -> Result<(Option<Vec<u8>>, String)> {
    let bytes = read_existing(abs)?;
    let text = match &bytes {
        Some(b) => String::from_utf8(b.clone())
            .map_err(|_| anyhow!("{} is not UTF-8 text; it cannot be edited", abs.display()))?,
        None => String::new(),
    };
    Ok((bytes, text))
}

/// What every file an edit rewrites would contain, without writing anything: the text edits of
/// a `WorkspaceEdit` applied in memory to the files as they are, each at the path it names. File
/// renames, creations and deletions are not modelled, so an edit that follows one in the same
/// batch is read from whatever that path holds now; this is a preview, not the ordered,
/// transactional check [`apply_workspace_edit`] makes. The second value says whether the edit
/// had any resource operation, so a caller can say that part was not checked.
pub(crate) fn planned_texts(
    root: &Path,
    edit: &serde_json::Value,
) -> Result<(Vec<(std::path::PathBuf, String)>, bool)> {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    planned_multi_texts(&[&root], edit)
}

/// [`planned_texts`] across multiple repository roots.
pub(crate) fn planned_multi_texts(
    roots: &[&Path],
    edit: &serde_json::Value,
) -> Result<(Vec<(std::path::PathBuf, String)>, bool)> {
    let canonical_roots: Vec<PathBuf> = roots
        .iter()
        .map(|r| std::fs::canonicalize(r).unwrap_or_else(|_| r.to_path_buf()))
        .collect();
    let mut out = Vec::new();
    let mut moves_files = false;
    for op in multi_operations(&canonical_roots, edit)? {
        match op {
            MultiOp::Text { root, rel, edits } => {
                let abs = root.join(&rel);
                let (_, current) = text_for_edit(&abs)?;
                out.push((abs, apply_text_edits(&current, &edits)?));
            }
            _ => moves_files = true,
        }
    }
    Ok((out, moves_files))
}

/// One change of a `WorkspaceEdit`, its paths resolved inside the designated repository root.
enum MultiOp {
    Text {
        root: PathBuf,
        rel: String,
        edits: Vec<serde_json::Value>,
    },
    Create {
        root: PathBuf,
        rel: String,
        overwrite: bool,
        ignore_if_exists: bool,
    },
    Rename {
        from_root: PathBuf,
        from_rel: String,
        to_root: PathBuf,
        to_rel: String,
        overwrite: bool,
        ignore_if_exists: bool,
    },
    Delete {
        root: PathBuf,
        rel: String,
        recursive: bool,
    },
}

/// Every change of `edit`, in the order it names them (`documentChanges` wins over `changes`, as
/// LSP says). Refuses a path outside all designated repository checkouts and a resource operation
/// it does not know, before anything is written.
fn multi_operations(canonical_roots: &[PathBuf], edit: &serde_json::Value) -> Result<Vec<MultiOp>> {
    let mut ops = Vec::new();
    if let Some(changes) = edit.get("documentChanges").and_then(|c| c.as_array()) {
        for change in changes {
            let uri_at = |key: &str| change.get(key).and_then(|u| u.as_str()).unwrap_or("");
            let option = |key: &str| {
                change
                    .pointer(&format!("/options/{key}"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            };
            ops.push(match change.get("kind").and_then(|k| k.as_str()) {
                Some("rename") => {
                    let (from_root, from_rel) =
                        uri_to_root_and_relative(canonical_roots, uri_at("oldUri"))?;
                    let (to_root, to_rel) =
                        uri_to_root_and_relative(canonical_roots, uri_at("newUri"))?;
                    MultiOp::Rename {
                        from_root: from_root.to_path_buf(),
                        from_rel,
                        to_root: to_root.to_path_buf(),
                        to_rel,
                        overwrite: option("overwrite"),
                        ignore_if_exists: option("ignoreIfExists"),
                    }
                }
                Some("create") => {
                    let (root, rel) = uri_to_root_and_relative(canonical_roots, uri_at("uri"))?;
                    MultiOp::Create {
                        root: root.to_path_buf(),
                        rel,
                        overwrite: option("overwrite"),
                        ignore_if_exists: option("ignoreIfExists"),
                    }
                }
                Some("delete") => {
                    let (root, rel) = uri_to_root_and_relative(canonical_roots, uri_at("uri"))?;
                    MultiOp::Delete {
                        root: root.to_path_buf(),
                        rel,
                        recursive: option("recursive"),
                    }
                }
                Some(other) => bail!(
                    "unsupported resource operation `{other}` in the workspace edit; only \
                     create, rename, delete and text edits can be applied, so nothing was written"
                ),
                None => {
                    let (root, rel) = uri_to_root_and_relative(
                        canonical_roots,
                        change
                            .pointer("/textDocument/uri")
                            .and_then(|u| u.as_str())
                            .unwrap_or(""),
                    )?;
                    MultiOp::Text {
                        root: root.to_path_buf(),
                        rel,
                        edits: change
                            .get("edits")
                            .and_then(|e| e.as_array())
                            .cloned()
                            .unwrap_or_default(),
                    }
                }
            });
        }
    } else if let Some(changes) = edit.get("changes").and_then(|c| c.as_object()) {
        for (uri, edits) in changes {
            let (root, rel) = uri_to_root_and_relative(canonical_roots, uri)?;
            ops.push(MultiOp::Text {
                root: root.to_path_buf(),
                rel,
                edits: edits.as_array().cloned().unwrap_or_default(),
            });
        }
    }
    Ok(ops)
}

/// What can be told wrong about an edit before any mutation occurs: a file to edit that is not
/// readable text, or a directory to delete that is not empty without `recursive`. Found here,
/// nothing has been written yet.
fn check_multi_ops(ops: &[MultiOp]) -> Result<()> {
    for op in ops {
        match op {
            MultiOp::Text { root, rel, .. } => {
                text_for_edit(&root.join(rel))?;
            }
            MultiOp::Delete {
                root,
                rel,
                recursive: false,
            } => {
                let abs = root.join(rel);
                if abs.is_dir()
                    && std::fs::read_dir(&abs)
                        .with_context(|| format!("cannot read {rel}"))?
                        .next()
                        .is_some()
                {
                    bail!("deleting the directory {rel} needs `recursive: true`, it is not empty");
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Applies a `WorkspaceEdit` (`documentChanges` or `changes`) to the checkout at `root`, its
/// changes in order, as LSP says: each change names its paths as they are after the changes
/// before it. Returns the relative paths written, moved or deleted, in application order.
///
/// A multi-file refactor that stops halfway is worse than one that never started: the checkout
/// is inconsistent and nothing says which half landed. Everything is checked before the first
/// byte moves, and each step records how to take it back, renames of whole directories
/// included, so a failure puts every path and byte back as it was.
pub fn apply_workspace_edit(root: &Path, edit: &serde_json::Value) -> Result<Vec<String>> {
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let touched = apply_multi_repository_workspace_edit(&[&canonical_root], edit)?;
    let mut out = Vec::new();
    for p in touched {
        let rel = p
            .strip_prefix(&canonical_root)
            .map(|r| r.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| p.to_string_lossy().replace('\\', "/"));
        out.push(rel);
    }
    Ok(out)
}

/// Applies an LSP `WorkspaceEdit` atomically across multiple repository checkouts (`roots`).
///
/// Every change in `documentChanges` or `changes` is matched to its corresponding repository root.
/// Pre-flight validation confirms file readability and deletion safety across all repositories
/// before a single byte moves. A unified transactional journal tracks all mutations across all
/// checkouts; if any change fails in any repository, every modified file, created file or directory,
/// and moved path across all repositories is rolled back to its exact original state.
///
/// On success, sync watermarks across all involved repository checkouts are invalidated so remote
/// gateways upload the updated files. Returns the absolute paths of all touched files/paths in
/// order of application.
pub fn apply_multi_repository_workspace_edit(
    roots: &[&Path],
    edit: &serde_json::Value,
) -> Result<Vec<PathBuf>> {
    anyhow::ensure!(
        !roots.is_empty(),
        "no repository roots provided for multi-repository workspace edit"
    );
    let canonical_roots: Vec<PathBuf> = roots
        .iter()
        .map(|r| std::fs::canonicalize(r).unwrap_or_else(|_| r.to_path_buf()))
        .collect();
    for r in &canonical_roots {
        anyhow::ensure!(
            r.is_dir(),
            "repository root {} does not exist or is not a directory",
            r.display()
        );
    }
    let ops = multi_operations(&canonical_roots, edit)?;
    check_multi_ops(&ops).context("nothing was written")?;
    let mut run = MultiRun::default();
    match run.apply(&ops) {
        Ok(()) => {
            run.journal.commit();
            remember_applied_multi(&run.originals);
            forget_synced_across_roots(&canonical_roots, &run.touched, &run.also_forget);
            crate::call_tree::clear_call_hierarchy_cache();
            Ok(run.touched)
        }
        Err(err) => {
            let (restored, failed) = run.journal.roll_back();
            forget_synced_across_roots(&canonical_roots, &run.touched, &run.also_forget);
            crate::call_tree::clear_call_hierarchy_cache();
            if failed.is_empty() {
                if canonical_roots.len() == 1 {
                    Err(err.context(format!(
                        "the edit failed partway and was undone: {restored} change(s) put back as \
                         they were"
                    )))
                } else {
                    Err(err.context(format!(
                        "the edit failed partway and was undone: {restored} change(s) put back as \
                         they were across repository roots"
                    )))
                }
            } else {
                Err(err.context(format!(
                    "the edit failed partway; {restored} change(s) were put back, but these \
                     could not be:\n  {}",
                    failed.join("\n  ")
                )))
            }
        }
    }
}

/// One step of an edit taken back.
enum Undo {
    /// The file had these bytes.
    Write(PathBuf, Vec<u8>),
    /// The edit created this file.
    RemoveFile(PathBuf),
    /// The edit created this directory.
    RemoveDir(PathBuf),
    /// The edit moved the first path from the second.
    Move(PathBuf, PathBuf),
}

/// How to take back what an edit did so far, and what it set aside to remove once it lands.
#[derive(Default)]
struct Journal {
    undo: Vec<Undo>,
    set_aside: Vec<PathBuf>,
}

impl Journal {
    /// Creates the missing directories above `abs`, remembering each to remove again.
    fn create_parents(&mut self, abs: &Path) -> Result<()> {
        let Some(parent) = abs.parent() else {
            return Ok(());
        };
        let mut missing = Vec::new();
        let mut at = parent;
        while std::fs::symlink_metadata(at).is_err() {
            missing.push(at.to_path_buf());
            match at.parent() {
                Some(up) => at = up,
                None => break,
            }
        }
        for dir in missing.into_iter().rev() {
            std::fs::create_dir(&dir)
                .with_context(|| format!("cannot create the directory {}", dir.display()))?;
            self.undo.push(Undo::RemoveDir(dir));
        }
        Ok(())
    }

    /// Moves what is at `abs` out of the way, next to it, until the edit lands or is undone.
    fn set_aside(&mut self, abs: &Path) -> Result<()> {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let name = abs.file_name().unwrap_or_default().to_string_lossy();
        let aside = loop {
            let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let candidate = abs.with_file_name(format!(
                ".{name}.prod-code-undo-{}-{seq}",
                std::process::id()
            ));
            if std::fs::symlink_metadata(&candidate).is_err() {
                break candidate;
            }
        };
        std::fs::rename(abs, &aside)
            .with_context(|| format!("cannot move {} out of the way", abs.display()))?;
        self.undo.push(Undo::Move(aside.clone(), abs.to_path_buf()));
        self.moved(abs, &aside);
        self.set_aside.push(aside);
        Ok(())
    }

    /// `from` was moved to `to`: what was set aside inside it is inside `to` now, and that is
    /// where it is removed from once the edit lands. The undo steps keep the old paths, which
    /// are right again by the time they run, the move having been taken back first.
    fn moved(&mut self, from: &Path, to: &Path) {
        for aside in &mut self.set_aside {
            if let Ok(rest) = aside.strip_prefix(from) {
                *aside = to.join(rest);
            }
        }
    }

    /// The edit landed: what it set aside goes.
    fn commit(&mut self) {
        for aside in self.set_aside.drain(..) {
            let removed = if aside.is_dir() && !aside.is_symlink() {
                std::fs::remove_dir_all(&aside)
            } else {
                std::fs::remove_file(&aside)
            };
            if let Err(error) = removed {
                tracing::warn!(path = %aside.display(), %error, "could not remove what an edit replaced");
            }
        }
        self.undo.clear();
    }

    /// Takes every step back, last first. Returns how many were taken back and the ones that
    /// could not be.
    fn roll_back(&mut self) -> (usize, Vec<String>) {
        let mut restored = 0;
        let mut failed = Vec::new();
        for step in self.undo.drain(..).rev() {
            let (what, result) = match &step {
                Undo::Write(abs, bytes) => {
                    if std::fs::read(abs).ok().as_deref() == Some(bytes.as_slice()) {
                        continue;
                    }
                    (abs.clone(), std::fs::write(abs, bytes))
                }
                Undo::RemoveFile(abs) => {
                    if !abs.exists() && std::fs::symlink_metadata(abs).is_err() {
                        continue;
                    }
                    (abs.clone(), std::fs::remove_file(abs))
                }
                Undo::RemoveDir(abs) => (abs.clone(), std::fs::remove_dir(abs)),
                Undo::Move(from, to) => (to.clone(), std::fs::rename(from, to)),
            };
            match result {
                Ok(()) => restored += 1,
                Err(err) => failed.push(format!("{}: {err}", what.display())),
            }
        }
        self.set_aside.clear();
        (restored, failed)
    }
}

/// The state of an edit being applied across one or more repository checkouts.
#[derive(Default)]
struct MultiRun {
    journal: Journal,
    /// Absolute paths written, moved or deleted, in order.
    touched: Vec<PathBuf>,
    /// Further (root, rel) paths the sync watermark must drop: the files a directory move carried
    /// along and the files an edit created.
    also_forget: Vec<(PathBuf, String)>,
    /// What each file held before the edit first touched it, for [`remember_applied_multi`].
    originals: Vec<(PathBuf, Option<Vec<u8>>)>,
    /// The paths moved away or deleted so far.
    vacated: Vec<PathBuf>,
}

impl MultiRun {
    fn original(&mut self, abs: &Path, bytes: Option<Vec<u8>>) {
        if !self.originals.iter().any(|(p, _)| p == abs) {
            self.originals.push((abs.to_path_buf(), bytes));
        }
    }

    fn apply(&mut self, ops: &[MultiOp]) -> Result<()> {
        for op in ops {
            match op {
                MultiOp::Text { root, rel, edits } => self.text(root, rel, edits)?,
                MultiOp::Create {
                    root,
                    rel,
                    overwrite,
                    ignore_if_exists,
                } => {
                    let abs = contained(root, rel)?;
                    if std::fs::symlink_metadata(&abs).is_ok() {
                        if *overwrite {
                            self.original(&abs, read_existing(&abs).ok().flatten());
                            self.journal.set_aside(&abs)?;
                        } else if *ignore_if_exists {
                            continue;
                        } else {
                            bail!(
                                "cannot create {rel}: it already exists, and the edit neither \
                                 overwrites nor ignores it"
                            );
                        }
                    } else {
                        self.original(&abs, None);
                    }
                    self.journal.create_parents(&abs)?;
                    std::fs::write(&abs, b"").with_context(|| format!("create {rel}"))?;
                    self.journal.undo.push(Undo::RemoveFile(abs));
                    self.also_forget.push((root.clone(), rel.clone()));
                }
                MultiOp::Rename {
                    from_root,
                    from_rel,
                    to_root,
                    to_rel,
                    overwrite,
                    ignore_if_exists,
                } => {
                    let from_abs = contained(from_root, from_rel)?;
                    let to_abs = contained(to_root, to_rel)?;
                    if from_abs == to_abs {
                        continue;
                    }
                    anyhow::ensure!(
                        std::fs::symlink_metadata(&from_abs).is_ok(),
                        "rename {from_rel} -> {to_rel}: {from_rel} does not exist"
                    );
                    if std::fs::symlink_metadata(&to_abs).is_ok() {
                        if *overwrite {
                            self.original(&to_abs, read_existing(&to_abs).ok().flatten());
                            self.journal.set_aside(&to_abs)?;
                        } else if *ignore_if_exists {
                            continue;
                        } else {
                            bail!(
                                "rename {from_rel} -> {to_rel}: {to_rel} already exists, and the edit does \
                                 not overwrite it"
                            );
                        }
                    }
                    self.original(&from_abs, read_existing(&from_abs).ok().flatten());
                    self.original(&to_abs, None);
                    self.journal.create_parents(&to_abs)?;
                    std::fs::rename(&from_abs, &to_abs)
                        .with_context(|| format!("rename {from_rel} -> {to_rel}"))?;
                    self.journal
                        .undo
                        .push(Undo::Move(to_abs.clone(), from_abs.clone()));
                    self.journal.moved(&from_abs, &to_abs);
                    if to_abs.is_dir() {
                        for inner in files_under(&to_abs) {
                            self.also_forget
                                .push((from_root.clone(), format!("{from_rel}/{inner}")));
                            self.also_forget
                                .push((to_root.clone(), format!("{to_rel}/{inner}")));
                        }
                        // A file an earlier step rewrote travels with its directory, and so
                        // does what it held before, for the report.
                        for (path, _) in &mut self.originals {
                            if let Ok(rest) = path.strip_prefix(&from_abs) {
                                *path = to_abs.join(rest);
                            }
                        }
                    }
                    self.vacated.push(from_abs.clone());
                    self.touched.push(from_abs);
                    self.touched.push(to_abs);
                }
                MultiOp::Delete {
                    root,
                    rel,
                    recursive,
                } => {
                    let abs = contained(root, rel)?;
                    if std::fs::symlink_metadata(&abs).is_ok() {
                        if abs.is_dir() && !abs.is_symlink() {
                            anyhow::ensure!(
                                *recursive || std::fs::read_dir(&abs)?.next().is_none(),
                                "deleting the directory {rel} needs `recursive: true`, it is \
                                 not empty"
                            );
                            for inner in files_under(&abs) {
                                self.also_forget.push((root.clone(), format!("{rel}/{inner}")));
                            }
                        } else {
                            self.original(&abs, read_existing(&abs)?);
                        }
                        self.journal.set_aside(&abs)?;
                    }
                    self.vacated.push(abs.clone());
                    self.touched.push(abs);
                }
            }
        }
        Ok(())
    }

    /// A text edit of `rel` as it is now, after the steps before it.
    fn text(&mut self, root: &Path, rel: &str, edits: &[serde_json::Value]) -> Result<()> {
        let abs = contained(root, rel)?;
        let (bytes, current) = text_for_edit(&abs)?;
        if bytes.is_none()
            && let Some(gone) = self.vacated.iter().find(|v| is_abs_at_or_under(&abs, v))
        {
            let gone_display = gone
                .strip_prefix(root)
                .map(|r| r.to_string_lossy().into_owned())
                .unwrap_or_else(|_| gone.display().to_string());
            bail!(
                "edit {rel}: an earlier step of the edit moved or deleted {gone_display}; the changes of a \
                 workspace edit apply in order, so an edit after a move names the new path"
            );
        }
        let new_text = apply_text_edits(&current, edits).with_context(|| format!("edit {rel}"))?;
        match bytes {
            Some(bytes) => {
                std::fs::write(&abs, new_text).with_context(|| format!("write {rel}"))?;
                self.journal.undo.push(Undo::Write(abs.clone(), bytes.clone()));
                self.original(&abs, Some(bytes));
            }
            None => {
                self.journal.create_parents(&abs)?;
                std::fs::write(&abs, new_text).with_context(|| format!("write {rel}"))?;
                self.journal.undo.push(Undo::RemoveFile(abs.clone()));
                self.original(&abs, None);
            }
        }
        self.touched.push(abs);
        Ok(())
    }
}

fn is_abs_at_or_under(path: &Path, under: &Path) -> bool {
    path == under || path.starts_with(under)
}

fn forget_synced_across_roots(
    roots: &[PathBuf],
    touched: &[PathBuf],
    also_forget: &[(PathBuf, String)],
) {
    let mut forget_by_root: std::collections::HashMap<PathBuf, Vec<String>> =
        std::collections::HashMap::new();
    for p in touched {
        let mut best: Option<(&PathBuf, &Path)> = None;
        for r in roots {
            if let Ok(rel) = p.strip_prefix(r) {
                match &best {
                    Some((best_r, _)) if r.as_os_str().len() <= best_r.as_os_str().len() => {}
                    _ => best = Some((r, rel)),
                }
            }
        }
        if let Some((r, rel)) = best {
            forget_by_root
                .entry(r.clone())
                .or_default()
                .push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    for (r, rel) in also_forget {
        forget_by_root
            .entry(r.clone())
            .or_default()
            .push(rel.clone());
    }
    for (r, paths) in &forget_by_root {
        crate::sync::forget_synced_files(r, paths);
    }
}

/// The files under the directory `dir`, relative to it, symlinks not followed.
fn files_under(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(at) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&at) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(t) if t.is_dir() => stack.push(path),
                Ok(_) => {
                    if let Ok(rel) = path.strip_prefix(dir) {
                        out.push(rel.to_string_lossy().replace('\\', "/"));
                    }
                }
                Err(_) => {}
            }
        }
    }
    out
}

/// What each file held before the last edit applied to it, and what the edit left there.
type Applied = std::collections::HashMap<std::path::PathBuf, (String, Vec<u8>)>;

fn applied() -> &'static std::sync::Mutex<Applied> {
    static APPLIED: std::sync::OnceLock<std::sync::Mutex<Applied>> = std::sync::OnceLock::new();
    APPLIED.get_or_init(Default::default)
}

/// Records, for every file an edit rewrote across repository checkouts, the text it had before
/// and the bytes it has now, so a report rendered after the write can still show what changed (#122).
fn remember_applied_multi(before: &[(PathBuf, Option<Vec<u8>>)]) {
    let Ok(mut map) = applied().lock() else {
        return;
    };
    for (abs, old) in before {
        let Ok(now) = std::fs::read(abs) else {
            continue;
        };
        let old = old
            .as_deref()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default();
        map.insert(abs.clone(), (old, now));
    }
}

#[allow(dead_code)]
pub(crate) fn remember_applied(root: &Path, before: &[(String, Option<Vec<u8>>)]) {
    let converted: Vec<(PathBuf, Option<Vec<u8>>)> = before
        .iter()
        .map(|(rel, old)| (root.join(rel), old.clone()))
        .collect();
    remember_applied_multi(&converted);
}

/// The text a file had before the edit that produced what is on disk now: what a report shows as
/// the old side of its diff. When no edit wrote the file, or the file has changed since, that is
/// simply what is on disk.
pub fn text_before_apply(path: &Path) -> String {
    let current = std::fs::read(path).unwrap_or_default();
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if let Ok(map) = applied().lock()
        && let Some((old, written)) = map.get(&canonical).or_else(|| map.get(path))
        && *written == current
    {
        return old.clone();
    }
    String::from_utf8_lossy(&current).into_owned()
}

/// The text of a file the analyzer reports a reference in, read once into `texts`. A file that
/// cannot be read is an error: taken as empty, its references would not be in it, and the plan
/// would go on as if they did not exist (#446).
pub(crate) fn referenced_text<'a>(
    texts: &'a mut std::collections::BTreeMap<std::path::PathBuf, String>,
    path: &Path,
) -> Result<&'a mut String> {
    use std::collections::btree_map::Entry;
    match texts.entry(path.to_path_buf()) {
        Entry::Occupied(known) => Ok(known.into_mut()),
        Entry::Vacant(slot) => {
            let text = std::fs::read_to_string(path).with_context(|| {
                format!(
                    "cannot read {}, where the analyzer reports a reference; nothing was planned",
                    path.display()
                )
            })?;
            Ok(slot.insert(text))
        }
    }
}

/// The locations of a `definition`, `declaration` or `implementation` answer, as (file, 1-based
/// line, 1-based column): `null` is none, and a `Location`, a `LocationLink` or a list of either
/// is read whole. Any other answer, and an entry without a file or a start, is an error naming
/// `what` was asked: a planner that dropped it would leave that declaration as it was (#446).
pub(crate) fn lsp_locations(
    answer: &serde_json::Value,
    what: &str,
) -> Result<Vec<(std::path::PathBuf, u32, u32)>> {
    let entries = match answer {
        serde_json::Value::Null => return Ok(Vec::new()),
        serde_json::Value::Array(all) => all.iter().collect(),
        one @ serde_json::Value::Object(_) => vec![one],
        other => anyhow::bail!("the analyzer's {what} is not a location or a list: {other}"),
    };
    let mut out = Vec::with_capacity(entries.len());
    for (n, loc) in entries.iter().enumerate() {
        let uri = loc.get("uri").or_else(|| loc.get("targetUri"));
        let start = loc
            .pointer("/range/start")
            .or_else(|| loc.pointer("/targetSelectionRange/start"));
        let at = |key: &str| {
            start
                .and_then(|s| s.get(key))
                .and_then(|v| v.as_u64())
                .and_then(|v| u32::try_from(v).ok())
                .and_then(|v| v.checked_add(1))
        };
        let (Some(uri), Some(line), Some(col)) =
            (uri.and_then(|u| u.as_str()), at("line"), at("character"))
        else {
            anyhow::bail!(
                "entry {} of {} in the analyzer's {what} has no file or start position: {loc}",
                n + 1,
                entries.len()
            );
        };
        let parsed = url::Url::parse(uri)
            .with_context(|| format!("invalid URI in the analyzer's {what}: {uri}"))?;
        anyhow::ensure!(
            parsed.scheme() == "file" && parsed.query().is_none() && parsed.fragment().is_none(),
            "the analyzer's {what} does not name a plain local file: {uri}"
        );
        let path = parsed.to_file_path().map_err(|_| {
            anyhow::anyhow!("the analyzer's {what} does not name a local file: {uri}")
        })?;
        out.push((path, line, col));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_file_and_ranged_edits() {
        let text = "fn a() {}\nfn b() {}\n";
        let whole = serde_json::json!([{ "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 2, "character": 0 } }, "newText": "fn z() {}\n" }]);
        assert_eq!(
            apply_text_edits(text, whole.as_array().unwrap()).unwrap(),
            "fn z() {}\n"
        );
        let ranged = serde_json::json!([
            { "range": { "start": { "line": 0, "character": 3 }, "end": { "line": 0, "character": 4 } }, "newText": "alpha" },
            { "range": { "start": { "line": 1, "character": 3 }, "end": { "line": 1, "character": 4 } }, "newText": "beta" }
        ]);
        assert_eq!(
            apply_text_edits(text, ranged.as_array().unwrap()).unwrap(),
            "fn alpha() {}\nfn beta() {}\n"
        );
    }

    #[test]
    fn apply_workspace_edit_writes_moves_and_records() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        crate::sync::clear_sync_cache(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "mod old_name;\n").unwrap();
        std::fs::write(root.join("src/old_name.rs"), "pub fn f() {}\n").unwrap();
        let uri = |rel: &str| format!("file://{}/{}", root.display(), rel);
        let edit = serde_json::json!({ "documentChanges": [
            { "kind": "rename", "oldUri": uri("src/old_name.rs"), "newUri": uri("src/new_name.rs") },
            { "textDocument": { "uri": uri("src/lib.rs"), "version": null },
              "edits": [ { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 1, "character": 0 } }, "newText": "mod new_name;\n" } ] }
        ]});
        let touched = apply_workspace_edit(&root, &edit).unwrap();
        assert_eq!(
            touched,
            vec!["src/old_name.rs", "src/new_name.rs", "src/lib.rs"]
        );
        assert!(!root.join("src/old_name.rs").exists());
        assert_eq!(
            std::fs::read_to_string(root.join("src/new_name.rs")).unwrap(),
            "pub fn f() {}\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("src/lib.rs")).unwrap(),
            "mod new_name;\n"
        );
        // Nothing the client rewrote counts as synced: every gateway still has the old text.
        let state = crate::sync::load_sync_cache(&root);
        assert!(!state.files.contains_key("src/lib.rs"));
        assert!(!state.files.contains_key("src/new_name.rs"));
        assert!(!state.files.contains_key("src/old_name.rs"));
        // Anything outside the checkout is refused.
        let outside = serde_json::json!({ "changes": { "file:///etc/hosts": [] } });
        assert!(apply_workspace_edit(&root, &outside).is_err());
        crate::sync::clear_sync_cache(&root);
    }

    #[test]
    fn apply_workspace_edit_invalidates_call_hierarchy_cache() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        let lib = root.join("src/lib.rs");
        std::fs::write(&lib, "pub fn old() {}\n").unwrap();
        // Seed call hierarchy cache
        {
            let mut lock = crate::call_tree::CALL_CACHE.lock().unwrap();
            lock.insert(
                (
                    "127.0.0.1:9000".parse().unwrap(),
                    root.to_string_lossy().into_owned(),
                    format!("file://{}/src/lib.rs", root.display()),
                    1,
                    1,
                    true,
                    1,
                ),
                crate::call_tree::CallCacheEntry {
                    edges: serde_json::json!([]),
                    timestamp: std::time::Instant::now(),
                },
            );
            assert_eq!(lock.len(), 1);
        }
        let edit = serde_json::json!({ "changes": { format!("file://{}", lib.display()): [
            { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 1, "character": 0 } },
              "newText": "pub fn new() {}\n" }
        ] } });
        let touched = apply_workspace_edit(&root, &edit).unwrap();
        assert_eq!(touched, vec!["src/lib.rs"]);
        // Call hierarchy cache must be cleared after edit is applied
        {
            let lock = crate::call_tree::CALL_CACHE.lock().unwrap();
            assert!(lock.is_empty(), "cache must be cleared on workspace edits");
        }
    }

    /// A report rendered after an edit was written still has the old text to diff against (#122),
    /// and a file changed again since, by anything, is read as it is.
    #[test]
    fn the_text_before_an_applied_edit_is_kept_until_the_file_changes_again() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        crate::sync::clear_sync_cache(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        let lib = root.join("src/lib.rs");
        std::fs::write(&lib, "pub fn old() {}\n").unwrap();
        assert_eq!(text_before_apply(&lib), "pub fn old() {}\n");

        let edit = serde_json::json!({ "changes": { format!("file://{}", lib.display()): [
            { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 1, "character": 0 } },
              "newText": "pub fn new() {}\n" }
        ] } });
        apply_workspace_edit(&root, &edit).unwrap();
        assert_eq!(std::fs::read_to_string(&lib).unwrap(), "pub fn new() {}\n");
        assert_eq!(text_before_apply(&lib), "pub fn old() {}\n");

        std::fs::write(&lib, "pub fn later() {}\n").unwrap();
        assert_eq!(text_before_apply(&lib), "pub fn later() {}\n");
    }

    /// A multi-file edit either lands whole or not at all. The second write here cannot happen —
    /// its parent directory is a regular file — and the first one, which already succeeded, has
    /// to be put back, or the checkout is left half-refactored with nothing to say so.
    #[test]
    fn a_write_that_fails_halfway_leaves_every_file_as_it_was() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        crate::sync::clear_sync_cache(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn old() {}\n").unwrap();
        std::fs::write(root.join("src/other.rs"), "pub fn keep() {}\n").unwrap();
        // A regular file where a directory would have to be.
        std::fs::write(root.join("blocker"), "not a directory\n").unwrap();

        let uri = |rel: &str| format!("file://{}/{}", root.display(), rel);
        let whole = |text: &str| {
            serde_json::json!([{ "range": { "start": { "line": 0, "character": 0 },
                                             "end": { "line": 1, "character": 0 } },
                                  "newText": text }])
        };
        let edit = serde_json::json!({ "documentChanges": [
            { "textDocument": { "uri": uri("src/lib.rs"), "version": null }, "edits": whole("pub fn new() {}\n") },
            { "kind": "rename", "oldUri": uri("src/other.rs"), "newUri": uri("src/moved.rs") },
            { "textDocument": { "uri": uri("blocker/inner.rs"), "version": null }, "edits": whole("x\n") }
        ]});

        let err = apply_workspace_edit(&root, &edit).expect_err("the third write cannot happen");
        assert!(
            format!("{err:#}").contains("put back"),
            "the error says the checkout was restored: {err:#}"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("src/lib.rs")).unwrap(),
            "pub fn old() {}\n",
            "the edit that succeeded before the failure is undone"
        );
        assert!(
            root.join("src/other.rs").is_file() && !root.join("src/moved.rs").exists(),
            "the rename is undone too"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("blocker")).unwrap(),
            "not a directory\n",
            "and nothing that was in the way is disturbed"
        );
        crate::sync::clear_sync_cache(&root);
    }

    /// Every path under `root` with its bytes (none for a directory) and permission bits: what a
    /// failed edit has to leave exactly as it found it.
    fn tree(root: &Path) -> std::collections::BTreeMap<String, (Option<Vec<u8>>, u32)> {
        let mut out = std::collections::BTreeMap::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                let meta = std::fs::symlink_metadata(&path).unwrap();
                #[cfg(unix)]
                let mode = std::os::unix::fs::PermissionsExt::mode(&meta.permissions());
                #[cfg(not(unix))]
                let mode = 0;
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                if meta.is_dir() {
                    stack.push(path);
                    out.insert(rel, (None, mode));
                } else {
                    out.insert(rel, (Some(std::fs::read(&path).unwrap_or_default()), mode));
                }
            }
        }
        out
    }

    fn whole(text: &str) -> serde_json::Value {
        serde_json::json!([{ "range": { "start": { "line": 0, "character": 0 },
                                         "end": { "line": 1, "character": 0 } },
                              "newText": text }])
    }

    fn at(line: u64, start: u64, end: u64, text: &str) -> serde_json::Value {
        serde_json::json!([{ "range": { "start": { "line": line, "character": start },
                                         "end": { "line": line, "character": end } },
                              "newText": text }])
    }

    /// LSP columns count UTF-16 code units: past an emoji, which is two of them, an analyzer's
    /// column is one more than the character count. The crate's own scanners count characters
    /// and keep their own entry point.
    #[test]
    fn lsp_columns_count_utf16_units_and_the_scanners_count_characters() {
        let text = "let s = \"\u{1F600}\"; let x = 1;\n";
        // `x` is character 17 and UTF-16 unit 18.
        let lsp = at(0, 18, 19, "y");
        assert_eq!(
            apply_text_edits(text, lsp.as_array().unwrap()).unwrap(),
            "let s = \"\u{1F600}\"; let y = 1;\n"
        );
        let scalar = at(0, 17, 18, "y");
        assert_eq!(
            apply_scalar_text_edits(text, scalar.as_array().unwrap()).unwrap(),
            "let s = \"\u{1F600}\"; let y = 1;\n"
        );
        // A column past the end of its line stops at the line's end.
        let past = at(0, 99, 99, " // end");
        assert_eq!(
            apply_text_edits(text, past.as_array().unwrap()).unwrap(),
            "let s = \"\u{1F600}\"; let x = 1; // end\n"
        );
    }

    /// The same through the whole applicator: every byte of the file is what the analyzer meant.
    #[test]
    fn a_workspace_edit_after_non_bmp_text_lands_on_its_utf16_column() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        crate::sync::clear_sync_cache(&root);
        std::fs::write(
            root.join("lib.rs"),
            "// \u{1D11E} clef\nfn f(\u{1F600}: u8, old: u8) {}\n",
        )
        .unwrap();
        let uri = format!("file://{}/lib.rs", root.display());
        // `old` starts at character 12 and UTF-16 unit 13 of the second line, `clef` at
        // character 5 and unit 6 of the first.
        let edit = serde_json::json!({ "changes": { uri: [
            { "range": { "start": { "line": 1, "character": 13 }, "end": { "line": 1, "character": 16 } },
              "newText": "new" },
            { "range": { "start": { "line": 0, "character": 6 }, "end": { "line": 0, "character": 10 } },
              "newText": "G clef" }
        ] } });
        apply_workspace_edit(&root, &edit).unwrap();
        assert_eq!(
            std::fs::read(root.join("lib.rs")).unwrap(),
            "// \u{1D11E} G clef\nfn f(\u{1F600}: u8, new: u8) {}\n".as_bytes()
        );
        crate::sync::clear_sync_cache(&root);
    }

    /// A batch that moves a whole directory, edits inside it, creates a file in a new directory,
    /// deletes a file and a directory tree, and then fails: every path and byte is back, the
    /// moved directory at its old name and nothing the batch created left behind.
    #[test]
    fn a_failure_after_a_directory_move_puts_every_path_and_byte_back() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        crate::sync::clear_sync_cache(&root);
        std::fs::create_dir_all(root.join("src/foo")).unwrap();
        std::fs::create_dir_all(root.join("src/stale/deep")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "mod foo;\n").unwrap();
        std::fs::write(root.join("src/foo/mod.rs"), "pub mod a;\n").unwrap();
        std::fs::write(root.join("src/foo/a.rs"), "// \u{1F600}\npub fn a() {}\n").unwrap();
        std::fs::write(root.join("src/gone.rs"), "pub fn gone() {}\n").unwrap();
        std::fs::write(root.join("src/stale/deep/x.rs"), "x\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let gone = root.join("src/gone.rs");
            std::fs::set_permissions(&gone, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::write(root.join("blocker"), "not a directory\n").unwrap();
        let before = tree(&root);

        let uri = |rel: &str| format!("file://{}/{}", root.display(), rel);
        let edit = serde_json::json!({ "documentChanges": [
            { "textDocument": { "uri": uri("src/lib.rs"), "version": null }, "edits": whole("mod bar;\n") },
            { "kind": "rename", "oldUri": uri("src/foo"), "newUri": uri("src/bar"), "options": { "overwrite": false } },
            { "textDocument": { "uri": uri("src/bar/a.rs"), "version": null }, "edits": at(1, 7, 8, "b") },
            { "kind": "create", "uri": uri("src/new/fresh.rs") },
            { "textDocument": { "uri": uri("src/new/fresh.rs"), "version": null }, "edits": at(0, 0, 0, "fresh\n") },
            { "kind": "delete", "uri": uri("src/gone.rs") },
            { "kind": "delete", "uri": uri("src/stale"), "options": { "recursive": true } },
            { "textDocument": { "uri": uri("blocker/inner.rs"), "version": null }, "edits": whole("x\n") }
        ]});

        let err = apply_workspace_edit(&root, &edit).expect_err("the last write cannot happen");
        assert!(format!("{err:#}").contains("put back"), "{err:#}");
        assert_eq!(tree(&root), before, "every path and byte is as it was");
        crate::sync::clear_sync_cache(&root);
    }

    /// The same batch without the failing step lands whole, the moved directory with its edit.
    #[test]
    fn a_directory_move_with_an_edit_inside_it_lands_whole() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        crate::sync::clear_sync_cache(&root);
        std::fs::create_dir_all(root.join("src/foo")).unwrap();
        std::fs::create_dir_all(root.join("src/stale/deep")).unwrap();
        std::fs::write(root.join("src/foo/a.rs"), "// \u{1F600}\npub fn a() {}\n").unwrap();
        std::fs::write(root.join("src/stale/deep/x.rs"), "x\n").unwrap();
        let uri = |rel: &str| format!("file://{}/{}", root.display(), rel);
        let edit = serde_json::json!({ "documentChanges": [
            { "kind": "rename", "oldUri": uri("src/foo"), "newUri": uri("src/bar") },
            { "textDocument": { "uri": uri("src/bar/a.rs"), "version": null }, "edits": at(1, 7, 8, "b") },
            { "kind": "delete", "uri": uri("src/stale"), "options": { "recursive": true } }
        ]});
        apply_workspace_edit(&root, &edit).unwrap();
        assert!(!root.join("src/foo").exists() && !root.join("src/stale").exists());
        assert_eq!(
            std::fs::read_to_string(root.join("src/bar/a.rs")).unwrap(),
            "// \u{1F600}\npub fn b() {}\n"
        );
        let names: Vec<String> = std::fs::read_dir(root.join("src"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["bar"], "nothing set aside is left behind");
        crate::sync::clear_sync_cache(&root);
    }

    /// A rename onto a file that exists, without `overwrite`, would destroy that file: it is
    /// refused, and the edit before it is undone.
    #[test]
    fn a_rename_onto_an_existing_file_is_refused_and_undone() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        crate::sync::clear_sync_cache(&root);
        std::fs::write(root.join("lib.rs"), "pub fn lib() {}\n").unwrap();
        std::fs::write(root.join("other.rs"), "pub fn other() {}\n").unwrap();
        let before = tree(&root);
        let uri = |rel: &str| format!("file://{}/{}", root.display(), rel);
        let edit = serde_json::json!({ "documentChanges": [
            { "textDocument": { "uri": uri("lib.rs"), "version": null }, "edits": whole("changed\n") },
            { "kind": "rename", "oldUri": uri("other.rs"), "newUri": uri("lib.rs"), "options": { "overwrite": false } }
        ]});
        let err = apply_workspace_edit(&root, &edit).expect_err("lib.rs exists");
        assert!(format!("{err:#}").contains("already exists"), "{err:#}");
        assert_eq!(tree(&root), before);
        crate::sync::clear_sync_cache(&root);
    }

    /// LSP applies `documentChanges` in order: a text edit names the path as it is at that step.
    /// After `a -> b` and `c -> a`, an edit of `a` is an edit of the file that was `c`, and one
    /// of `b` is an edit of the file that was `a`.
    #[test]
    fn an_ordered_edit_names_each_path_as_it_is_at_that_step() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        crate::sync::clear_sync_cache(&root);
        std::fs::write(root.join("a.rs"), "was a\n").unwrap();
        std::fs::write(root.join("c.rs"), "was c\n").unwrap();
        let uri = |rel: &str| format!("file://{}/{}", root.display(), rel);
        let edit = serde_json::json!({ "documentChanges": [
            { "kind": "rename", "oldUri": uri("a.rs"), "newUri": uri("b.rs") },
            { "kind": "rename", "oldUri": uri("c.rs"), "newUri": uri("a.rs") },
            { "textDocument": { "uri": uri("a.rs"), "version": null }, "edits": at(0, 5, 5, ", edited") },
            { "textDocument": { "uri": uri("b.rs"), "version": null }, "edits": at(0, 5, 5, ", edited") }
        ]});
        apply_workspace_edit(&root, &edit).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("a.rs")).unwrap(),
            "was c, edited\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("b.rs")).unwrap(),
            "was a, edited\n"
        );
        assert!(!root.join("c.rs").exists());
        crate::sync::clear_sync_cache(&root);
    }

    /// An edit naming a path an earlier step moved away or deleted, and nothing recreated, was
    /// computed against the checkout before the edit. Taken in order it names no file; sent to
    /// wherever the file went, it could land on another one. It is refused and nothing stays.
    #[test]
    fn an_edit_naming_a_path_an_earlier_step_vacated_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        crate::sync::clear_sync_cache(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("old.rs"), "use crate::old;\n").unwrap();
        std::fs::write(root.join("src/a.rs"), "pub fn a() {}\n").unwrap();
        let before = tree(&root);
        let uri = |rel: &str| format!("file://{}/{}", root.display(), rel);
        let edits = [
            serde_json::json!({ "documentChanges": [
                { "kind": "rename", "oldUri": uri("old.rs"), "newUri": uri("new.rs") },
                { "textDocument": { "uri": uri("old.rs"), "version": null }, "edits": whole("use crate::new;\n") }
            ]}),
            serde_json::json!({ "documentChanges": [
                { "kind": "rename", "oldUri": uri("src"), "newUri": uri("dst") },
                { "textDocument": { "uri": uri("src/a.rs"), "version": null }, "edits": whole("pub fn b() {}\n") }
            ]}),
            serde_json::json!({ "documentChanges": [
                { "kind": "delete", "uri": uri("old.rs") },
                { "textDocument": { "uri": uri("old.rs"), "version": null }, "edits": whole("back\n") }
            ]}),
        ];
        for edit in &edits {
            let err = apply_workspace_edit(&root, edit).expect_err("the path was vacated");
            assert!(format!("{err:#}").contains("earlier step"), "{err:#}");
            assert_eq!(tree(&root), before, "{edit}");
        }
        crate::sync::clear_sync_cache(&root);
    }

    /// A path a move vacated and a later step created again is a new file: its edit lands
    /// there, and the moved file keeps what it had.
    #[test]
    fn an_old_path_created_again_after_a_move_is_a_new_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        crate::sync::clear_sync_cache(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("a.rs"), "old a\n").unwrap();
        std::fs::write(root.join("src/x.rs"), "old x\n").unwrap();
        let uri = |rel: &str| format!("file://{}/{}", root.display(), rel);
        let edit = serde_json::json!({ "documentChanges": [
            { "kind": "rename", "oldUri": uri("a.rs"), "newUri": uri("b.rs") },
            { "kind": "create", "uri": uri("a.rs") },
            { "textDocument": { "uri": uri("a.rs"), "version": null }, "edits": at(0, 0, 0, "new a\n") },
            { "kind": "rename", "oldUri": uri("src"), "newUri": uri("dst") },
            { "kind": "create", "uri": uri("src/x.rs") },
            { "textDocument": { "uri": uri("src/x.rs"), "version": null }, "edits": at(0, 0, 0, "new x\n") }
        ]});
        apply_workspace_edit(&root, &edit).unwrap();
        let read = |rel: &str| std::fs::read_to_string(root.join(rel)).unwrap();
        assert_eq!(read("a.rs"), "new a\n");
        assert_eq!(read("b.rs"), "old a\n");
        assert_eq!(read("src/x.rs"), "new x\n");
        assert_eq!(read("dst/x.rs"), "old x\n");
        crate::sync::clear_sync_cache(&root);
    }

    /// A symlink leading out of the checkout is harmless where it is, but a directory move can
    /// carry it under a path a later step writes to. The paths were resolved before the move;
    /// each step resolves its own again right before it acts, so nothing outside is touched and
    /// the move is undone.
    #[cfg(unix)]
    #[test]
    fn a_symlink_carried_in_by_a_directory_move_is_not_written_through() {
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let away = std::fs::canonicalize(outside.path()).unwrap();
        crate::sync::clear_sync_cache(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("a.rs"), "pub fn a() {}\n").unwrap();
        std::fs::write(root.join("src/lib.rs"), "mod a;\n").unwrap();
        std::os::unix::fs::symlink(&away, root.join("src/link")).unwrap();
        std::fs::write(away.join("keep.rs"), "outside\n").unwrap();
        let (before, away_before) = (tree(&root), tree(&away));
        let uri = |rel: &str| format!("file://{}/{}", root.display(), rel);
        let moved =
            serde_json::json!({ "kind": "rename", "oldUri": uri("src"), "newUri": uri("dst") });
        let edits = [
            serde_json::json!({ "documentChanges": [ moved,
                { "textDocument": { "uri": uri("dst/link/new.rs"), "version": null }, "edits": whole("escaped\n") } ] }),
            serde_json::json!({ "documentChanges": [ moved,
                { "textDocument": { "uri": uri("dst/link/keep.rs"), "version": null }, "edits": whole("escaped\n") } ] }),
            serde_json::json!({ "documentChanges": [ moved,
                { "kind": "create", "uri": uri("dst/link/new.rs") } ] }),
            serde_json::json!({ "documentChanges": [ moved,
                { "kind": "rename", "oldUri": uri("a.rs"), "newUri": uri("dst/link/a.rs") } ] }),
            serde_json::json!({ "documentChanges": [ moved,
                { "kind": "delete", "uri": uri("dst/link/keep.rs") } ] }),
        ];
        for edit in &edits {
            let err = apply_workspace_edit(&root, edit).expect_err("the path leads outside");
            assert!(
                format!("{err:#}").contains("outside the checkout"),
                "{err:#}"
            );
            assert_eq!(tree(&root), before, "{edit}");
            assert_eq!(tree(&away), away_before, "{edit}");
        }
        crate::sync::clear_sync_cache(&root);
    }

    /// A file deleted inside a directory that a later step moves is set aside in that directory
    /// and travels with it. Once the edit lands it is gone from the new place, and a failure
    /// puts it back at the old one.
    #[test]
    fn a_file_deleted_before_its_directory_moves_does_not_survive_the_move() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        crate::sync::clear_sync_cache(&root);
        let seed = || {
            std::fs::create_dir_all(root.join("src/deep")).unwrap();
            std::fs::write(root.join("src/a.rs"), "gone\n").unwrap();
            std::fs::write(root.join("src/deep/d.rs"), "gone too\n").unwrap();
            std::fs::write(root.join("src/b.rs"), "kept\n").unwrap();
        };
        seed();
        std::fs::write(root.join("blocker"), "not a directory\n").unwrap();
        let before = tree(&root);
        let uri = |rel: &str| format!("file://{}/{}", root.display(), rel);
        let steps = [
            serde_json::json!({ "kind": "delete", "uri": uri("src/a.rs") }),
            serde_json::json!({ "kind": "delete", "uri": uri("src/deep"), "options": { "recursive": true } }),
            serde_json::json!({ "kind": "rename", "oldUri": uri("src"), "newUri": uri("dst") }),
        ];
        let mut failing = steps.to_vec();
        failing.push(serde_json::json!(
            { "textDocument": { "uri": uri("blocker/inner.rs"), "version": null }, "edits": whole("x\n") }));
        let err = apply_workspace_edit(&root, &serde_json::json!({ "documentChanges": failing }))
            .expect_err("the last write cannot happen");
        assert!(format!("{err:#}").contains("put back"), "{err:#}");
        assert_eq!(tree(&root), before, "every path and byte is back");

        apply_workspace_edit(&root, &serde_json::json!({ "documentChanges": steps })).unwrap();
        let mut after: Vec<String> = tree(&root).into_keys().collect();
        after.sort();
        assert_eq!(
            after,
            vec!["blocker", "dst", "dst/b.rs"],
            "nothing set aside survives"
        );
        crate::sync::clear_sync_cache(&root);
    }

    /// An edit computed before a directory move rewrites the file at its old path, and the move
    /// carries it: a report rendered afterwards still has the text it had before (#122).
    #[test]
    fn the_text_before_an_edit_follows_its_file_through_a_later_directory_move() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        crate::sync::clear_sync_cache(&root);
        std::fs::create_dir_all(root.join("src/foo")).unwrap();
        std::fs::write(root.join("src/foo/a.rs"), "pub fn old() {}\n").unwrap();
        let uri = |rel: &str| format!("file://{}/{}", root.display(), rel);
        let edit = serde_json::json!({ "documentChanges": [
            { "textDocument": { "uri": uri("src/foo/a.rs"), "version": null }, "edits": whole("pub fn new() {}\n") },
            { "kind": "rename", "oldUri": uri("src/foo"), "newUri": uri("src/bar") }
        ]});
        apply_workspace_edit(&root, &edit).unwrap();
        let moved = root.join("src/bar/a.rs");
        assert_eq!(
            std::fs::read_to_string(&moved).unwrap(),
            "pub fn new() {}\n"
        );
        assert_eq!(text_before_apply(&moved), "pub fn old() {}\n");
        crate::sync::clear_sync_cache(&root);
    }

    /// A file that cannot be read as text is not an empty one: editing it is refused before
    /// anything is written, and its bytes stay.
    #[test]
    fn a_file_that_is_not_text_is_refused_not_overwritten() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        crate::sync::clear_sync_cache(&root);
        std::fs::write(root.join("lib.rs"), "pub fn lib() {}\n").unwrap();
        std::fs::write(root.join("blob.rs"), b"\xff\xfe binary\n").unwrap();
        let before = tree(&root);
        let uri = |rel: &str| format!("file://{}/{}", root.display(), rel);
        let edit = serde_json::json!({ "documentChanges": [
            { "textDocument": { "uri": uri("lib.rs"), "version": null }, "edits": whole("changed\n") },
            { "textDocument": { "uri": uri("blob.rs"), "version": null }, "edits": at(0, 0, 0, "x") }
        ]});
        let err = apply_workspace_edit(&root, &edit).expect_err("blob.rs is not UTF-8");
        assert!(format!("{err:#}").contains("not UTF-8"), "{err:#}");
        assert_eq!(tree(&root), before);
        let unknown = serde_json::json!({ "documentChanges": [
            { "textDocument": { "uri": uri("lib.rs"), "version": null }, "edits": whole("changed\n") },
            { "kind": "chmod", "uri": uri("lib.rs") }
        ]});
        let err = apply_workspace_edit(&root, &unknown).expect_err("chmod is not an LSP operation");
        assert!(
            format!("{err:#}").contains("unsupported resource operation"),
            "{err:#}"
        );
        assert_eq!(tree(&root), before);
        crate::sync::clear_sync_cache(&root);
    }

    /// A path that does not exist yet, under a directory symlinked out of the checkout, is
    /// outside it; so is a dangling symlink inside it. Nothing is written through either.
    #[cfg(unix)]
    #[test]
    fn nothing_is_written_through_a_symlink_that_leads_out_of_the_checkout() {
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let away = std::fs::canonicalize(outside.path()).unwrap();
        crate::sync::clear_sync_cache(&root);
        std::fs::write(root.join("a.rs"), "pub fn a() {}\n").unwrap();
        std::os::unix::fs::symlink(&away, root.join("link")).unwrap();
        std::os::unix::fs::symlink(away.join("missing.rs"), root.join("dangling")).unwrap();
        let before = tree(&root);
        let uri = |rel: &str| format!("file://{}/{}", root.display(), rel);
        let edits = [
            serde_json::json!({ "changes": { uri("link/new.rs"): whole("escaped\n") } }),
            serde_json::json!({ "documentChanges": [ { "kind": "create", "uri": uri("link/sub/new.rs") } ] }),
            serde_json::json!({ "documentChanges": [
                { "kind": "rename", "oldUri": uri("a.rs"), "newUri": uri("link/a.rs") } ] }),
            serde_json::json!({ "changes": { uri("dangling"): whole("escaped\n") } }),
        ];
        for edit in &edits {
            assert!(apply_workspace_edit(&root, edit).is_err(), "{edit}");
            assert!(planned_texts(&root, edit).is_err(), "{edit}");
            assert_eq!(tree(&root), before, "{edit}");
            assert_eq!(tree(&away), Default::default(), "{edit}");
        }
        crate::sync::clear_sync_cache(&root);
    }

    #[test]
    fn test_multi_repository_workspace_edit_atomic_commit_and_rollback() {
        let temp_a = tempfile::tempdir().unwrap();
        let root_a = std::fs::canonicalize(temp_a.path()).unwrap();
        let temp_b = tempfile::tempdir().unwrap();
        let root_b = std::fs::canonicalize(temp_b.path()).unwrap();

        crate::sync::clear_sync_cache(&root_a);
        crate::sync::clear_sync_cache(&root_b);

        std::fs::create_dir_all(root_a.join("src")).unwrap();
        std::fs::write(root_a.join("src/lib.rs"), "pub fn a() -> u32 { 1 }\n").unwrap();
        std::fs::write(root_a.join("src/helper.rs"), "pub fn h() {}\n").unwrap();

        std::fs::create_dir_all(root_b.join("src")).unwrap();
        std::fs::write(root_b.join("src/lib.rs"), "pub fn b() -> u32 { 2 }\n").unwrap();

        let uri_a = |rel: &str| format!("file://{}/{}", root_a.display(), rel);
        let uri_b = |rel: &str| format!("file://{}/{}", root_b.display(), rel);

        // 1. Successful atomic multi-repo edit
        let successful_edit = serde_json::json!({ "documentChanges": [
            { "kind": "rename", "oldUri": uri_a("src/helper.rs"), "newUri": uri_a("src/renamed_helper.rs") },
            { "textDocument": { "uri": uri_a("src/lib.rs"), "version": null },
              "edits": [ { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 1, "character": 0 } },
                           "newText": "pub fn a() -> u32 { 10 }\n" } ] },
            { "textDocument": { "uri": uri_b("src/lib.rs"), "version": null },
              "edits": [ { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 1, "character": 0 } },
                           "newText": "pub fn b() -> u32 { 20 }\n" } ] },
            { "kind": "create", "uri": uri_b("src/extra.rs") },
            { "textDocument": { "uri": uri_b("src/extra.rs"), "version": null },
              "edits": [ { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } },
                           "newText": "pub fn extra() {}\n" } ] },
        ]});

        let touched = apply_multi_repository_workspace_edit(&[&root_a, &root_b], &successful_edit).unwrap();
        assert_eq!(
            touched,
            vec![
                root_a.join("src/helper.rs"),
                root_a.join("src/renamed_helper.rs"),
                root_a.join("src/lib.rs"),
                root_b.join("src/lib.rs"),
                root_b.join("src/extra.rs"),
            ]
        );

        assert!(!root_a.join("src/helper.rs").exists());
        assert_eq!(
            std::fs::read_to_string(root_a.join("src/renamed_helper.rs")).unwrap(),
            "pub fn h() {}\n"
        );
        assert_eq!(
            std::fs::read_to_string(root_a.join("src/lib.rs")).unwrap(),
            "pub fn a() -> u32 { 10 }\n"
        );
        assert_eq!(
            std::fs::read_to_string(root_b.join("src/lib.rs")).unwrap(),
            "pub fn b() -> u32 { 20 }\n"
        );
        assert_eq!(
            std::fs::read_to_string(root_b.join("src/extra.rs")).unwrap(),
            "pub fn extra() {}\n"
        );

        // Verify text_before_apply across repos
        assert_eq!(
            text_before_apply(&root_a.join("src/lib.rs")),
            "pub fn a() -> u32 { 1 }\n"
        );
        assert_eq!(
            text_before_apply(&root_b.join("src/lib.rs")),
            "pub fn b() -> u32 { 2 }\n"
        );

        // 2. Rollback across all repos when a later step in repo_b fails
        let tree_a_before = tree(&root_a);

        // Put a blocker file in repo_b
        std::fs::write(root_b.join("src/blocker"), "not a directory\n").unwrap();
        let tree_b_before_with_blocker = tree(&root_b);

        let failing_edit = serde_json::json!({ "documentChanges": [
            { "textDocument": { "uri": uri_a("src/lib.rs"), "version": null },
              "edits": [ { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 1, "character": 0 } },
                           "newText": "pub fn a() -> u32 { 999 }\n" } ] },
            { "kind": "rename", "oldUri": uri_a("src/renamed_helper.rs"), "newUri": uri_a("src/moved_helper.rs") },
            { "textDocument": { "uri": uri_b("src/lib.rs"), "version": null },
              "edits": [ { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 1, "character": 0 } },
                           "newText": "pub fn b() -> u32 { 999 }\n" } ] },
            // Fails: blocker is a file, cannot create blocker/sub.rs
            { "textDocument": { "uri": uri_b("src/blocker/sub.rs"), "version": null },
              "edits": [ { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } },
                           "newText": "failed\n" } ] },
        ]});

        let err = apply_multi_repository_workspace_edit(&[&root_a, &root_b], &failing_edit)
            .expect_err("the fourth write must fail and trigger all-or-nothing rollback");
        assert!(format!("{err:#}").contains("put back"), "{err:#}");
        assert!(format!("{err:#}").contains("across repository roots"), "{err:#}");

        // Exact all-or-nothing restoration verification
        assert_eq!(tree(&root_a), tree_a_before, "repo_a was completely restored");
        assert_eq!(tree(&root_b), tree_b_before_with_blocker, "repo_b was completely restored");
        assert_eq!(
            std::fs::read_to_string(root_a.join("src/lib.rs")).unwrap(),
            "pub fn a() -> u32 { 10 }\n"
        );
        assert!(root_a.join("src/renamed_helper.rs").is_file());
        assert!(!root_a.join("src/moved_helper.rs").exists());

        // 3. Pre-flight rejection of paths outside all roots
        let outside_edit = serde_json::json!({ "changes": {
            "file:///tmp/unrelated_outside_repo/foo.rs": []
        }});
        let err_outside = apply_multi_repository_workspace_edit(&[&root_a, &root_b], &outside_edit)
            .expect_err("outside repo edit must fail");
        assert!(format!("{err_outside:#}").contains("outside any of the specified repository roots"), "{err_outside:#}");

        // 4. Empty roots rejection
        let err_empty = apply_multi_repository_workspace_edit(&[], &successful_edit)
            .expect_err("empty roots must be rejected");
        assert!(format!("{err_empty:#}").contains("no repository roots provided"), "{err_empty:#}");

        crate::sync::clear_sync_cache(&root_a);
        crate::sync::clear_sync_cache(&root_b);
    }
}

#[cfg(test)]
mod required_location_tests {
    use super::lsp_locations;
    use serde_json::json;

    #[test]
    fn malformed_required_locations_return_errors_without_panicking() {
        for n in [u32::MAX as u64, u32::MAX as u64 + 1, u64::MAX] {
            for key in ["line", "character"] {
                let mut location = json!({"uri": "file:///tmp/a.rs", "range": {"start": {"line": 0, "character": 0}}});
                location["range"]["start"][key] = json!(n);
                let result = std::panic::catch_unwind(|| {
                    lsp_locations(&json!([location]), "implementations")
                });
                assert!(result.is_ok(), "coordinate {key}={n} panicked");
                assert!(
                    result.unwrap().is_err(),
                    "coordinate {key}={n} was accepted"
                );
            }
        }
        for uri in [
            "https://example.invalid/a.rs",
            "file:///tmp/a.rs?version=2",
            "file:///tmp/a.rs#part",
            "file://remote.invalid/a.rs",
            "relative.rs",
        ] {
            let location = json!({"uri": uri, "range": {"start": {"line": 0, "character": 0}}});
            assert!(
                lsp_locations(&json!([location]), "definitions").is_err(),
                "accepted {uri}"
            );
        }
    }

    #[test]
    fn required_locations_accept_protocol_empty_answers_and_encoded_local_files() {
        assert!(
            lsp_locations(&serde_json::Value::Null, "declarations")
                .unwrap()
                .is_empty()
        );
        assert!(
            lsp_locations(&json!([]), "declarations")
                .unwrap()
                .is_empty()
        );
        let path = std::env::temp_dir().join("a # %41 ü.rs");
        let uri = url::Url::from_file_path(&path).unwrap().to_string();
        for answer in [
            json!({"uri": uri, "range": {"start": {"line": 2, "character": 3}}}),
            json!({"targetUri": uri, "targetSelectionRange": {"start": {"line": 2, "character": 3}}}),
        ] {
            assert_eq!(
                lsp_locations(&answer, "declarations").unwrap(),
                vec![(path.clone(), 3, 4)]
            );
        }
    }
}


