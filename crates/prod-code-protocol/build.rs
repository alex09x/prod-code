fn main() {
    println!("cargo:rerun-if-env-changed=PROD_CODE_GIT_COMMIT");
    if let Ok(output) = std::process::Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .output()
    {
        if output.status.success() {
            if let Ok(git_dir) = String::from_utf8(output.stdout) {
                let p = std::path::Path::new(git_dir.trim());
                println!("cargo:rerun-if-changed={}", p.join("HEAD").display());
            }
        }
    }
    let commit = std::env::var("PROD_CODE_GIT_COMMIT").ok().or_else(|| {
        let output = std::process::Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .output()
            .ok()?;
        if output.status.success() {
            let s = String::from_utf8(output.stdout).ok()?;
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
        None
    });
    if let Some(c) = commit {
        println!("cargo:rustc-env=PROD_CODE_GIT_COMMIT={c}");
    }
}
