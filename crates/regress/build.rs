//! Bakes the commit into run metadata, so a report says what it was run on.
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn main() {
    let commit = git(&["rev-parse", "--short=12", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let dirty =
        git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
    let commit = if dirty {
        format!("{commit}-dirty")
    } else {
        commit
    };
    println!("cargo:rustc-env=REGRESS_COMMIT={commit}");
    println!(
        "cargo:rustc-env=REGRESS_TARGET={}",
        std::env::var("TARGET").unwrap_or_default()
    );
    println!(
        "cargo:rustc-env=REGRESS_PROFILE={}",
        std::env::var("PROFILE").unwrap_or_default()
    );
    for p in [
        "../../.git/HEAD",
        "../../.git/logs/HEAD",
        "../../.git/index",
    ] {
        println!("cargo:rerun-if-changed={p}");
    }
}
