//! Applying gateway refactorings (LSP `WorkspaceEdit`) to the local checkout. Rewritten files
//! are dropped from the sync watermark so the next pre-flight uploads them.

use anyhow::{Context, Result, anyhow, bail};
use std::path::{Path, PathBuf};
use url::Url;

fn uri_to_relative(root: &Path, uri: &str) -> Result<String> {
    let path = Url::parse(uri)
        .ok()
        .and_then(|u| u.to_file_path().ok())
        .ok_or_else(|| anyhow!("not a file URI: {uri}"))?;
    let path = resolve(&path)?;
    let rel = path.strip_prefix(root).map_err(|_| {
        anyhow!(
            "{} is outside the checkout {}",
            path.display(),
            root.display()
        )
    })?;
    anyhow::ensure!(
        !rel.as_os_str().is_empty(),
        "{uri} names the checkout itself, not a file in it"
    );
    Ok(rel.to_string_lossy().replace('\\', "/"))
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
/// a `WorkspaceEdit` applied in memory to the files as they are. File renames, creations and
/// deletions are not modelled; the second value says whether the edit had any, so a caller can
/// say that part was not checked.
pub(crate) fn planned_texts(
    root: &Path,
    edit: &serde_json::Value,
) -> Result<(Vec<(std::path::PathBuf, String)>, bool)> {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut out = Vec::new();
    let mut moves_files = false;
    for op in operations(&root, edit)? {
        match op {
            Op::Text { rel, edits } => {
                let abs = root.join(&rel);
                let (_, current) = text_for_edit(&abs)?;
                out.push((abs, apply_text_edits(&current, &edits)?));
            }
            _ => moves_files = true,
        }
    }
    Ok((out, moves_files))
}

/// One change of a `WorkspaceEdit`, its paths resolved inside the checkout.
enum Op {
    Text {
        rel: String,
        edits: Vec<serde_json::Value>,
    },
    Create {
        rel: String,
        overwrite: bool,
        ignore_if_exists: bool,
    },
    Rename {
        from: String,
        to: String,
        overwrite: bool,
        ignore_if_exists: bool,
    },
    Delete {
        rel: String,
        recursive: bool,
    },
}

/// Every change of `edit`, in the order it names them (`documentChanges` wins over `changes`, as
/// LSP says). Refuses a path outside the checkout and a resource operation it does not know,
/// before anything is written.
fn operations(root: &Path, edit: &serde_json::Value) -> Result<Vec<Op>> {
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
                Some("rename") => Op::Rename {
                    from: uri_to_relative(root, uri_at("oldUri"))?,
                    to: uri_to_relative(root, uri_at("newUri"))?,
                    overwrite: option("overwrite"),
                    ignore_if_exists: option("ignoreIfExists"),
                },
                Some("create") => Op::Create {
                    rel: uri_to_relative(root, uri_at("uri"))?,
                    overwrite: option("overwrite"),
                    ignore_if_exists: option("ignoreIfExists"),
                },
                Some("delete") => Op::Delete {
                    rel: uri_to_relative(root, uri_at("uri"))?,
                    recursive: option("recursive"),
                },
                Some(other) => bail!(
                    "unsupported resource operation `{other}` in the workspace edit; only \
                     create, rename, delete and text edits can be applied, so nothing was written"
                ),
                None => Op::Text {
                    rel: uri_to_relative(
                        root,
                        change
                            .pointer("/textDocument/uri")
                            .and_then(|u| u.as_str())
                            .unwrap_or(""),
                    )?,
                    edits: change
                        .get("edits")
                        .and_then(|e| e.as_array())
                        .cloned()
                        .unwrap_or_default(),
                },
            });
        }
    } else if let Some(changes) = edit.get("changes").and_then(|c| c.as_object()) {
        for (uri, edits) in changes {
            ops.push(Op::Text {
                rel: uri_to_relative(root, uri)?,
                edits: edits.as_array().cloned().unwrap_or_default(),
            });
        }
    }
    Ok(ops)
}

/// What can be told wrong about an edit from the checkout as it is: a file to edit that is not
/// readable text, a directory to delete that is not empty without `recursive`. Found here,
/// nothing has been written yet.
fn check(root: &Path, ops: &[Op]) -> Result<()> {
    for op in ops {
        match op {
            Op::Text { rel, .. } => {
                text_for_edit(&root.join(rel))?;
            }
            Op::Delete {
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
                    bail!(
                        "deleting the directory {rel} needs `recursive: true`, it is not empty"
                    );
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Applies a `WorkspaceEdit` (`documentChanges` or `changes`) to the checkout at `root`, its
/// changes in order. Returns the relative paths written, moved or deleted, in application order.
///
/// A multi-file refactor that stops halfway is worse than one that never started: the checkout
/// is inconsistent and nothing says which half landed. Everything is checked before the first
/// byte moves, and each step records how to take it back, renames of whole directories
/// included, so a failure puts every path and byte back as it was.
pub fn apply_workspace_edit(root: &Path, edit: &serde_json::Value) -> Result<Vec<String>> {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let ops = operations(&root, edit)?;
    check(&root, &ops).context("nothing was written")?;
    let mut run = Run::default();
    match run.apply(&root, &ops) {
        Ok(()) => {
            run.journal.commit();
            remember_applied(&root, &run.originals);
            let mut forget = run.touched.clone();
            forget.extend(run.also_forget);
            crate::sync::forget_synced_files(&root, &forget);
            Ok(run.touched)
        }
        Err(err) => {
            let (restored, failed) = run.journal.roll_back();
            let mut forget = run.touched;
            forget.extend(run.also_forget);
            crate::sync::forget_synced_files(&root, &forget);
            if failed.is_empty() {
                Err(err.context(format!(
                    "the edit failed partway and was undone: {restored} change(s) put back as \
                     they were"
                )))
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
            let candidate =
                abs.with_file_name(format!(".{name}.prod-code-undo-{}-{seq}", std::process::id()));
            if std::fs::symlink_metadata(&candidate).is_err() {
                break candidate;
            }
        };
        std::fs::rename(abs, &aside)
            .with_context(|| format!("cannot move {} out of the way", abs.display()))?;
        self.undo.push(Undo::Move(aside.clone(), abs.to_path_buf()));
        self.set_aside.push(aside);
        Ok(())
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
                Undo::RemoveFile(abs) => (abs.clone(), std::fs::remove_file(abs)),
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

/// The state of one edit being applied.
#[derive(Default)]
struct Run {
    journal: Journal,
    /// Relative paths written, moved or deleted, in order.
    touched: Vec<String>,
    /// Further paths the sync watermark must drop: the files a directory move carried along and
    /// the files an edit created.
    also_forget: Vec<String>,
    /// What each file held before the edit first touched it, for [`remember_applied`].
    originals: Vec<(String, Option<Vec<u8>>)>,
    /// The renames done so far, `(from, to)`.
    moved: Vec<(String, String)>,
}

impl Run {
    fn original(&mut self, rel: &str, bytes: Option<Vec<u8>>) {
        if !self.originals.iter().any(|(r, _)| r == rel) {
            self.originals.push((rel.to_string(), bytes));
        }
    }

    fn apply(&mut self, root: &Path, ops: &[Op]) -> Result<()> {
        for op in ops {
            match op {
                Op::Text { rel, edits } => self.text(root, rel, edits)?,
                Op::Create {
                    rel,
                    overwrite,
                    ignore_if_exists,
                } => {
                    let abs = root.join(rel);
                    if std::fs::symlink_metadata(&abs).is_ok() {
                        if *overwrite {
                            self.original(rel, read_existing(&abs).ok().flatten());
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
                        self.original(rel, None);
                    }
                    self.journal.create_parents(&abs)?;
                    std::fs::write(&abs, b"").with_context(|| format!("create {rel}"))?;
                    self.journal.undo.push(Undo::RemoveFile(abs));
                    self.also_forget.push(rel.clone());
                }
                Op::Rename {
                    from,
                    to,
                    overwrite,
                    ignore_if_exists,
                } => {
                    if from == to {
                        continue;
                    }
                    let (from_abs, to_abs) = (root.join(from), root.join(to));
                    anyhow::ensure!(
                        std::fs::symlink_metadata(&from_abs).is_ok(),
                        "rename {from} -> {to}: {from} does not exist"
                    );
                    if std::fs::symlink_metadata(&to_abs).is_ok() {
                        if *overwrite {
                            self.original(to, read_existing(&to_abs).ok().flatten());
                            self.journal.set_aside(&to_abs)?;
                        } else if *ignore_if_exists {
                            continue;
                        } else {
                            bail!(
                                "rename {from} -> {to}: {to} already exists, and the edit does \
                                 not overwrite it"
                            );
                        }
                    }
                    self.original(from, read_existing(&from_abs).ok().flatten());
                    self.original(to, None);
                    self.journal.create_parents(&to_abs)?;
                    std::fs::rename(&from_abs, &to_abs)
                        .with_context(|| format!("rename {from} -> {to}"))?;
                    self.journal.undo.push(Undo::Move(to_abs.clone(), from_abs));
                    if to_abs.is_dir() {
                        for inner in files_under(&to_abs) {
                            self.also_forget.push(format!("{from}/{inner}"));
                            self.also_forget.push(format!("{to}/{inner}"));
                        }
                    }
                    self.moved.push((from.clone(), to.clone()));
                    self.touched.push(from.clone());
                    self.touched.push(to.clone());
                }
                Op::Delete { rel, recursive } => {
                    let abs = root.join(rel);
                    if std::fs::symlink_metadata(&abs).is_ok() {
                        if abs.is_dir() && !abs.is_symlink() {
                            anyhow::ensure!(
                                *recursive || std::fs::read_dir(&abs)?.next().is_none(),
                                "deleting the directory {rel} needs `recursive: true`, it is \
                                 not empty"
                            );
                            for inner in files_under(&abs) {
                                self.also_forget.push(format!("{rel}/{inner}"));
                            }
                        } else {
                            self.original(rel, read_existing(&abs)?);
                        }
                        self.journal.set_aside(&abs)?;
                    }
                    self.touched.push(rel.clone());
                }
            }
        }
        Ok(())
    }

    fn text(&mut self, root: &Path, rel: &str, edits: &[serde_json::Value]) -> Result<()> {
        let mut rel = rel.to_string();
        // An edit computed against a file this edit has already moved names its old path; it
        // meant the file, which is at the new one now.
        if read_existing(&root.join(&rel))?.is_none() {
            let followed = follow_moves(&rel, &self.moved);
            if followed != rel && root.join(&followed).is_file() {
                rel = followed;
            }
        }
        let abs = root.join(&rel);
        let (bytes, current) = text_for_edit(&abs)?;
        let new_text = apply_text_edits(&current, edits).with_context(|| format!("edit {rel}"))?;
        self.original(&rel, bytes.clone());
        match bytes {
            Some(bytes) => self.journal.undo.push(Undo::Write(abs.clone(), bytes)),
            None => {
                self.journal.create_parents(&abs)?;
                self.journal.undo.push(Undo::RemoveFile(abs.clone()));
            }
        }
        std::fs::write(&abs, new_text).with_context(|| format!("write {rel}"))?;
        self.touched.push(rel);
        Ok(())
    }
}

/// Where `rel` is after the renames in `moved`, applied in order; a rename of a directory
/// carries what is under it.
fn follow_moves(rel: &str, moved: &[(String, String)]) -> String {
    let mut rel = rel.to_string();
    for (from, to) in moved {
        if rel == *from {
            rel = to.clone();
        } else if let Some(rest) = rel.strip_prefix(&format!("{from}/")) {
            rel = format!("{to}/{rest}");
        }
    }
    rel
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

/// Records, for every file an edit rewrote, the text it had before and the bytes it has now, so a
/// report rendered after the write can still show what changed (#122).
fn remember_applied(root: &Path, before: &[(String, Option<Vec<u8>>)]) {
    let Ok(mut map) = applied().lock() else {
        return;
    };
    for (rel, old) in before {
        let abs = root.join(rel);
        let Ok(now) = std::fs::read(&abs) else {
            continue;
        };
        let old = old
            .as_deref()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default();
        map.insert(abs, (old, now));
    }
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
                let rel = path.strip_prefix(root).unwrap().to_string_lossy().into_owned();
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

    /// An analyzer that moves a file and rewrites it names the rewrite by the old path. The text
    /// goes to the file where it now is; the old path is not brought back.
    #[test]
    fn an_edit_naming_a_file_the_batch_moved_follows_it() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        crate::sync::clear_sync_cache(&root);
        std::fs::write(root.join("old.rs"), "use crate::old;\n").unwrap();
        let uri = |rel: &str| format!("file://{}/{}", root.display(), rel);
        let edit = serde_json::json!({ "documentChanges": [
            { "kind": "rename", "oldUri": uri("old.rs"), "newUri": uri("new.rs") },
            { "textDocument": { "uri": uri("old.rs"), "version": null }, "edits": whole("use crate::new;\n") }
        ]});
        let touched = apply_workspace_edit(&root, &edit).unwrap();
        assert_eq!(touched, vec!["old.rs", "new.rs", "new.rs"]);
        assert!(!root.join("old.rs").exists());
        assert_eq!(
            std::fs::read_to_string(root.join("new.rs")).unwrap(),
            "use crate::new;\n"
        );
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
        assert!(format!("{err:#}").contains("unsupported resource operation"), "{err:#}");
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
}
