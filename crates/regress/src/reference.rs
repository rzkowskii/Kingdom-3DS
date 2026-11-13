//! Original-game reference fixtures: what Kingdom: New Lands (PC) is known
//! to do, where that knowledge came from, and how sure we are.
//!
//! The original Unity game exposes no deterministic state and can't be
//! driven programmatically, so a reference is evidence, not a replay. A
//! fixture never claims more than its evidence: `independently_verified`
//! stays false until someone has checked the running original.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceKind {
    /// Someone watched it happen in the original and wrote it down.
    ManualObservation,
    /// Timed from a gameplay recording (kept outside the repository).
    Recording,
    /// A structured trace (logs, memory reads) where legally obtainable.
    Trace,
    /// A value from local reference material, such as the scene data
    /// documented in `docs/systems.md`: how the original is configured,
    /// not an observation of it running.
    DocumentedMechanic,
    /// The same measurement repeated over several original-game runs.
    RepeatedMeasurement,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Confidence {
    Low,
    Medium,
    High,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Original {
    /// e.g. "Steam Linux Mono build, Unity 2017.1.2p1".
    pub game_version: String,
    pub platform: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Setup {
    pub description: String,
    /// Initial conditions, free-form (coins, island, hour, …).
    #[serde(default)]
    pub initial: BTreeMap<String, toml::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Observation {
    pub event: String,
    #[serde(default)]
    pub at_seconds: Option<f64>,
    #[serde(default)]
    pub tolerance_seconds: Option<f64>,
    #[serde(default)]
    pub note: String,
}

/// One machine-checkable expectation. A scenario reports `metric`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Expected {
    pub metric: String,
    pub value: f64,
    pub tolerance: f64,
    #[serde(default)]
    pub unit: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Evidence {
    pub kind: EvidenceKind,
    /// Where it came from: a document section, a recording's description,
    /// the measurement procedure. Never a copy of proprietary material.
    pub source: String,
    pub confidence: Confidence,
    pub independently_verified: bool,
    /// Who checked it against the running original, and how. Required
    /// when `independently_verified`.
    #[serde(default)]
    pub verification: String,
    /// Number of runs, for `repeated-measurement`.
    #[serde(default)]
    pub samples: u32,
    #[serde(default)]
    pub recorded_by: String,
    #[serde(default)]
    pub date: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reference {
    pub id: String,
    pub title: String,
    pub original: Original,
    pub setup: Setup,
    #[serde(default)]
    pub actions: Vec<String>,
    #[serde(default, rename = "observation")]
    pub observations: Vec<Observation>,
    #[serde(rename = "expected")]
    pub expected: Vec<Expected>,
    pub evidence: Evidence,
    /// Port scenarios that report these metrics and are compared against
    /// this fixture automatically, whatever their class.
    #[serde(default)]
    pub scenarios: Vec<String>,
}

const MEDIA: &[&str] = &[
    ".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".mp4", ".mkv", ".webm", ".mov", ".avi",
    ".ogg", ".wav", ".mp3",
];

impl Reference {
    /// Problems that make the fixture unusable or overclaiming.
    pub fn validate(&self, file_stem: Option<&str>) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(stem) = file_stem {
            if stem != self.id {
                out.push(format!("id {:?} doesn't match file name {stem:?}", self.id));
            }
        }
        for (name, v) in [
            ("title", &self.title),
            ("original.game_version", &self.original.game_version),
            ("original.platform", &self.original.platform),
            ("setup.description", &self.setup.description),
            ("evidence.source", &self.evidence.source),
        ] {
            if v.trim().is_empty() {
                out.push(format!("{name} is empty"));
            }
        }
        if self.expected.is_empty() {
            out.push("no [[expected]] entries: nothing to compare".into());
        }
        for e in &self.expected {
            if !e.value.is_finite() || !e.tolerance.is_finite() || e.tolerance < 0.0 {
                out.push(format!(
                    "{}: value/tolerance must be finite, tolerance >= 0",
                    e.metric
                ));
            }
        }
        let ev = &self.evidence;
        if ev.independently_verified && ev.verification.trim().is_empty() {
            out.push("independently_verified needs evidence.verification (who, how)".into());
        }
        if ev.confidence == Confidence::High && !ev.independently_verified {
            out.push("high confidence needs independent verification".into());
        }
        if ev.kind == EvidenceKind::RepeatedMeasurement && ev.samples < 2 {
            out.push("repeated-measurement needs evidence.samples >= 2".into());
        }
        let src = ev.source.to_ascii_lowercase();
        if MEDIA.iter().any(|m| src.contains(m)) && !src.contains("://") {
            out.push(
                "evidence.source points at a media file: keep recordings and screenshots \
                 out of the repository, describe them instead"
                    .into(),
            );
        }
        out
    }

    /// How a pass against this fixture is labelled in reports.
    pub fn label(&self) -> String {
        let kind = format!("{:?}", self.evidence.kind);
        let conf = format!("{:?}", self.evidence.confidence).to_lowercase();
        if self.evidence.independently_verified {
            format!("PARITY (verified {kind}, {conf})")
        } else {
            format!("PARITY (unverified {kind}, {conf})")
        }
    }

    /// Compares `metrics` against every expectation.
    pub fn compare(&self, metrics: &BTreeMap<String, f64>) -> Vec<String> {
        let mut out = Vec::new();
        for e in &self.expected {
            // Written so a NaN measurement fails.
            let within = |v: f64| (v - e.value).abs() <= e.tolerance;
            match metrics.get(&e.metric) {
                None => out.push(format!(
                    "{}: {} not measured by the scenario",
                    self.id, e.metric
                )),
                Some(&v) if !within(v) => out.push(format!(
                    "{}: {} = {v:.4} {}, original {} ± {}",
                    self.id, e.metric, e.unit, e.value, e.tolerance
                )),
                _ => {}
            }
        }
        out
    }
}

pub fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("reference")
}

/// Every `*.toml` fixture in `dir`, sorted by id, with load/validation errors.
pub fn load_all(dir: &Path) -> (Vec<Reference>, Vec<String>) {
    let mut refs = Vec::new();
    let mut errors = Vec::new();
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    paths.sort();
    for p in paths
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
    {
        let stem = p.file_stem().and_then(|s| s.to_str());
        match std::fs::read_to_string(p)
            .map_err(|e| e.to_string())
            .and_then(|s| toml::from_str::<Reference>(&s).map_err(|e| e.to_string()))
        {
            Ok(r) => {
                for problem in r.validate(stem) {
                    errors.push(format!("{}: {problem}", p.display()));
                }
                refs.push(r);
            }
            Err(e) => errors.push(format!("{}: {e}", p.display())),
        }
    }
    refs.sort_by(|a, b| a.id.cmp(&b.id));
    (refs, errors)
}
