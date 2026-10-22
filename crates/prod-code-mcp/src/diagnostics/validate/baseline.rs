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

/// A separate proposal is compared with the current checkout. For an already-written
/// proposal, only the committed blob is evidence of what preceded the edit: both local
/// disk and the node's pre-flight sync already contain the proposal (#1191).
pub(super) fn text(root: &Path, file: &Path, proposed: Option<&str>) -> Option<String> {
    let abs = if file.is_absolute() {
        file.to_path_buf()
    } else {
        root.join(file)
    };
    let current = std::fs::read_to_string(&abs).ok()?;
    if proposed != Some(current.as_str()) {
        return Some(current);
    }
    let rel = abs.strip_prefix(root).ok()?.to_str()?;
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("show")
        .arg(format!("HEAD:./{rel}"))
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use prod_code_testkit::Workspace;

    #[test]
    fn validate_baseline_preserves_disk_for_a_separate_proposal() {
        let ws = Workspace::new(&[("a.rs", "committed")]);
        ws.write("a.rs", "before proposal");
        assert_eq!(
            text(&ws.root(), Path::new("a.rs"), Some("proposal")).as_deref(),
            Some("before proposal")
        );
        assert_eq!(
            text(&ws.root(), Path::new("a.rs"), None).as_deref(),
            Some("before proposal")
        );
    }

    #[test]
    fn validate_baseline_uses_head_even_when_proposal_is_staged() {
        let ws = Workspace::new(&[("nested/a.rs", "committed")]);
        ws.write("nested/a.rs", "proposal");
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(ws.root())
                .args(["add", "."])
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            text(&ws.root(), &ws.path("nested/a.rs"), Some("proposal")).as_deref(),
            Some("committed")
        );
        assert_eq!(
            text(&ws.path("nested"), Path::new("a.rs"), Some("proposal")).as_deref(),
            Some("committed")
        );
    }

    #[test]
    fn validate_baseline_never_uses_an_untracked_proposal_as_old_text() {
        let ws = Workspace::empty();
        ws.write("a.rs", "proposal");
        assert!(text(&ws.root(), Path::new("a.rs"), Some("proposal")).is_none());
    }
}
