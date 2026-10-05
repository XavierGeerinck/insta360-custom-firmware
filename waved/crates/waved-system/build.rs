//! Sets WAVED_BUILD_ID to the short git commit, or to "" when git is missing
//! (for example inside the cross build container).

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
    if let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        // Commits, checkouts and resets all append to logs/HEAD.
        println!("cargo:rerun-if-changed={dir}/HEAD");
        println!("cargo:rerun-if-changed={dir}/logs/HEAD");
    }
    let id = git(&["rev-parse", "--short=7", "HEAD"]).unwrap_or_default();
    println!("cargo:rustc-env=WAVED_BUILD_ID={id}");
}
