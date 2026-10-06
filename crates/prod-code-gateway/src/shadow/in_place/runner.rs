/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_protocol::{FileDelta, ShadowHypothesisResult};
use std::path::{Path, PathBuf};

use super::super::process::run_child;
use super::super::staging::{check_files, failed};
use super::super::types::Job;
use super::lock::WorkspaceTurn;

/// Runs one hypothesis in place: waits until no other in-place hypothesis of any request uses
/// the workspace, writes its files into the workspace copy, runs the command, then puts back
/// every path that is still what the hypothesis left there. A path someone else changed
/// meanwhile is left alone, and the result's `error` says what could not be restored.
pub async fn run_in_place(
    job: Job,
    mut cancel: tokio::sync::watch::Receiver<bool>,
) -> ShadowHypothesisResult {
    let Some(_turn) = WorkspaceTurn::wait(&job.workspace, &mut cancel).await else {
        return failed(&job.name, "cancelled: the client left".to_string());
    };
    let paths = match check_files(&job.workspace, &job.files) {
        Ok(paths) => paths,
        Err(why) => return failed(&job.name, format!("invalid hypothesis: {why}")),
    };
    let mut applied = InPlace {
        workspace: job.workspace.clone(),
        touched: Vec::new(),
        created: Vec::new(),
    };
    for (rel, file) in paths.iter().zip(&job.files) {
        if let Err(e) = applied.apply(rel, file) {
            let path = job.workspace.join(rel);
            let error = format!("cannot write {}: {e}", path.display());
            return failed(&job.name, with_restore_problems(error, applied.restore()));
        }
    }
    let mut cmd = tokio::process::Command::new("sh");
    cmd.args(["-c", "exec \"$@\" 2>&1", "sh"])
        .args(&job.argv)
        .envs(job.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .current_dir(job.workspace.join(&job.subdir));
    let mut result = run_child(cmd, &job, cancel).await;
    let problems = applied.restore();
    if !problems.is_empty() {
        let error = result.error.take().unwrap_or_default();
        result.error = Some(with_restore_problems(error, problems));
    }
    result
}

/// `error` followed by what could not be restored, if anything.
pub(crate) fn with_restore_problems(error: String, problems: Vec<String>) -> String {
    if problems.is_empty() {
        return error;
    }
    let note = format!(
        "the workspace could not be restored: {}",
        problems.join("; ")
    );
    if error.is_empty() {
        note
    } else {
        format!("{error}; {note}")
    }
}

/// What was at a path before the in-place mode replaced or deleted it.
enum Original {
    Missing,
    File {
        bytes: Vec<u8>,
        permissions: std::fs::Permissions,
        modified: Option<std::time::SystemTime>,
    },
    Link(PathBuf),
}

/// What the hypothesis left at a touched path.
enum Left {
    /// Nothing: the path was deleted, or its new file was never created.
    Nothing,
    /// A file this run created but did not finish. Its content proves nothing, so the open
    /// handle identifies it, and keeps its inode number from being reused meanwhile.
    Partial(std::fs::File),
    /// The proposed content, complete.
    File(Vec<u8>),
}

/// A path the in-place mode touched, and what the hypothesis left there.
struct Touched {
    path: PathBuf,
    original: Original,
    left: Left,
}

/// The in-place changes of one hypothesis. An existing file or symlink is removed and a new
/// file created in its place, so nothing is written through a symlink or into an inode that a
/// hard link outside the workspace shares; restoring does the same in reverse.
struct InPlace {
    workspace: PathBuf,
    touched: Vec<Touched>,
    /// Directories created for proposed files, removed again on restore.
    created: Vec<PathBuf>,
}

impl InPlace {
    /// Applies one file whose path `check_files` accepted.
    fn apply(&mut self, rel: &Path, file: &FileDelta) -> std::io::Result<()> {
        let path = self.workspace.join(rel);
        if file.content.is_some() {
            let mut dir = self.workspace.clone();
            for component in rel.parent().into_iter().flat_map(Path::components) {
                dir.push(component);
                match std::fs::create_dir(&dir) {
                    Ok(()) => self.created.push(dir.clone()),
                    // A real directory: `check_files` refused anything else.
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(e),
                }
            }
        }
        let original = match std::fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Original::Missing,
            Err(e) => return Err(e),
            Ok(meta) if meta.file_type().is_symlink() => Original::Link(std::fs::read_link(&path)?),
            Ok(meta) => Original::File {
                bytes: std::fs::read(&path)?,
                permissions: meta.permissions(),
                modified: meta.modified().ok(),
            },
        };
        if !matches!(original, Original::Missing) {
            std::fs::remove_file(&path)?;
        }
        // Recorded before the new file exists, and updated at each step, so a failure at any
        // point leaves exactly what this stage owns for `restore`.
        self.touched.push(Touched {
            path: path.clone(),
            original,
            left: Left::Nothing,
        });
        if let Some(bytes) = &file.content {
            // The same modes the overlay stages: 0755 when executable, else the umask default.
            #[cfg(unix)]
            let permissions = file.is_executable.then(|| {
                use std::os::unix::fs::PermissionsExt;
                std::fs::Permissions::from_mode(0o755)
            });
            #[cfg(not(unix))]
            let permissions = None;
            let mut new = create_new(&path)?;
            let filled = fill(&mut new, bytes, permissions, None);
            self.touched.last_mut().expect("pushed above").left = match filled {
                Ok(()) => Left::File(bytes.clone()),
                Err(_) => Left::Partial(new),
            };
            filled?;
        }
        Ok(())
    }

    /// Puts back every touched path, and returns what could not be put back.
    fn restore(self) -> Vec<String> {
        let mut problems = Vec::new();
        for touched in self.touched.iter().rev() {
            if let Err(why) = self.restore_one(touched) {
                tracing::warn!(path = %touched.path.display(), "shadow run: {why}");
                problems.push(format!("{}: {why}", touched.path.display()));
            }
        }
        // A directory the command filled is left like the rest of its output.
        for dir in self.created.iter().rev() {
            if !self.real_parents(dir) {
                problems.push(format!(
                    "{}: a parent is no longer a real directory; not removed",
                    dir.display()
                ));
                continue;
            }
            if let Err(e) = std::fs::remove_dir(dir) {
                tracing::warn!(
                    path = %dir.display(),
                    error = %e,
                    "shadow run: could not remove a directory the hypothesis created"
                );
            }
        }
        problems
    }

    fn restore_one(&self, touched: &Touched) -> std::result::Result<(), String> {
        // The command may have replaced a parent with a symlink; never restore through one.
        if !self.real_parents(&touched.path) {
            return Err("a parent is no longer a real directory; not restored".to_string());
        }
        let current = std::fs::symlink_metadata(&touched.path);
        let unchanged = match (&touched.left, &current) {
            (Left::Nothing, Err(e)) => e.kind() == std::io::ErrorKind::NotFound,
            (Left::Partial(file), Ok(meta)) => {
                meta.is_file() && file.metadata().is_ok_and(|own| same_file(&own, meta))
            }
            (Left::File(bytes), Ok(meta)) => {
                meta.is_file() && std::fs::read(&touched.path).is_ok_and(|b| &b == bytes)
            }
            _ => false,
        };
        if !unchanged {
            return Err("file changed while the hypothesis ran; not restored".to_string());
        }
        let restored = (|| -> std::io::Result<()> {
            if !matches!(touched.left, Left::Nothing) {
                std::fs::remove_file(&touched.path)?;
            }
            match &touched.original {
                Original::Missing => Ok(()),
                Original::File {
                    bytes,
                    permissions,
                    modified,
                } => create_file(&touched.path, bytes, Some(permissions.clone()), *modified),
                #[cfg(unix)]
                Original::Link(target) => std::os::unix::fs::symlink(target, &touched.path),
                #[cfg(not(unix))]
                Original::Link(_) => Err(std::io::Error::other("symlinks need unix")),
            }
        })();
        restored.map_err(|e| format!("could not restore file: {e}"))
    }

    /// Whether every directory between the workspace and `path` is a directory, not a symlink.
    fn real_parents(&self, path: &Path) -> bool {
        let Some(rel) = path
            .strip_prefix(&self.workspace)
            .ok()
            .and_then(Path::parent)
        else {
            return false;
        };
        let mut dir = self.workspace.clone();
        rel.components().all(|component| {
            dir.push(component);
            std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir())
        })
    }
}

/// Whether two metadata describe the same inode.
#[cfg(unix)]
fn same_file(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}

/// Without inode numbers a partly written file cannot be told apart; it is left alone.
#[cfg(not(unix))]
fn same_file(_: &std::fs::Metadata, _: &std::fs::Metadata) -> bool {
    false
}

/// Creates a new file (failing if anything, a symlink included, is at `path`) with `bytes`,
/// and optionally the given permissions and modification time.
fn create_file(
    path: &Path,
    bytes: &[u8],
    permissions: Option<std::fs::Permissions>,
    modified: Option<std::time::SystemTime>,
) -> std::io::Result<()> {
    fill(&mut create_new(path)?, bytes, permissions, modified)
}

/// Opens a new, empty file, failing if anything, a symlink included, is at `path`.
fn create_new(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

/// A file operation failure a test injects: `fill` calls it in place of its own work, once.
#[cfg(test)]
pub(crate) type FillFault = Box<dyn FnOnce(&mut std::fs::File, &[u8]) -> std::io::Result<()>>;

#[cfg(test)]
thread_local! {
    static FILL_FAULT: std::cell::RefCell<Option<FillFault>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn inject_fill_fault(fault: FillFault) {
    FILL_FAULT.with(|f| *f.borrow_mut() = Some(fault));
}

/// Writes `bytes` into a file `create_new` opened, then optionally sets its permissions and
/// modification time.
fn fill(
    file: &mut std::fs::File,
    bytes: &[u8],
    permissions: Option<std::fs::Permissions>,
    modified: Option<std::time::SystemTime>,
) -> std::io::Result<()> {
    use std::io::Write;
    #[cfg(test)]
    if let Some(fault) = FILL_FAULT.with(|f| f.borrow_mut().take()) {
        return fault(file, bytes);
    }
    file.write_all(bytes)?;
    if let Some(permissions) = permissions {
        file.set_permissions(permissions)?;
    }
    if let Some(modified) = modified {
        file.set_modified(modified)?;
    }
    Ok(())
}
