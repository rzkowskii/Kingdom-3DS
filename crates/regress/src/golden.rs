//! Golden snapshots: what each internal scenario produced when it was last
//! blessed. A mismatch means behavior changed, not that it's wrong; bless
//! again (`regress run --bless`) only once the change is intended.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::meta::RunMeta;
use crate::snapshot::{Snapshot, Tolerance};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Golden {
    pub meta: RunMeta,
    #[serde(rename = "checkpoint")]
    pub checkpoints: Vec<Snapshot>,
}

#[derive(Debug, Default)]
pub struct GoldenResult {
    pub missing: bool,
    /// Outcome differences: these fail.
    pub diffs: Vec<String>,
    /// The outcomes held but the exact trajectory (state or event hash)
    /// didn't: fails only when strict.
    pub trajectory_changed: bool,
}

pub fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("golden")
}

pub fn path(dir: &Path, scenario: &str) -> PathBuf {
    dir.join(format!("{scenario}.toml"))
}

pub fn load(dir: &Path, scenario: &str) -> Option<Result<Golden, String>> {
    let s = std::fs::read_to_string(path(dir, scenario)).ok()?;
    Some(toml::from_str(&s).map_err(|e| e.to_string()))
}

pub fn save(dir: &Path, scenario: &str, golden: &Golden) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut text = String::from(
        "# Blessed by `cargo run -p regress -- run --bless`. Internal regression\n\
         # baseline, not original-game evidence.\n",
    );
    text.push_str(&toml::to_string(golden).expect("golden serializes"));
    std::fs::write(path(dir, scenario), text)
}

pub fn compare(golden: &Golden, now: &[Snapshot], tol: Tolerance) -> GoldenResult {
    let mut r = GoldenResult::default();
    if golden.checkpoints.len() != now.len() {
        r.diffs.push(format!(
            "{} checkpoints blessed, {} now",
            golden.checkpoints.len(),
            now.len()
        ));
    }
    for (a, b) in golden.checkpoints.iter().zip(now) {
        for d in a.diff(b, tol) {
            r.diffs.push(format!("tick {}: {d}", a.tick));
        }
        if !a.same_trajectory(b) {
            r.trajectory_changed = true;
        }
    }
    r
}
