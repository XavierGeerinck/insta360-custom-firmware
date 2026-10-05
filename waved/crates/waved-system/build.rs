//! Sets WAVED_BUILD_ID: the `WAVED_BUILD_ID` environment variable when it is
//! set and not blank (the cross build container has no git, so the build passes
//! the id in), otherwise the short git commit, otherwise "".

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|text| !text.is_empty())
}

fn main() {
    println!("cargo:rerun-if-env-changed=WAVED_BUILD_ID");
    if let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        // Commits, checkouts and resets all append to logs/HEAD.
        println!("cargo:rerun-if-changed={dir}/HEAD");
        println!("cargo:rerun-if-changed={dir}/logs/HEAD");
    }
    let from_env = std::env::var("WAVED_BUILD_ID")
        .ok()
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty());
    let id = from_env
        .or_else(|| git(&["rev-parse", "--short=7", "HEAD"]))
        .unwrap_or_default();
    println!("cargo:rustc-env=WAVED_BUILD_ID={id}");
}
