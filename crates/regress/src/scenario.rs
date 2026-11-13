//! Scenario definitions and running them.

use std::collections::BTreeMap;
use std::time::Instant;

use crate::meta::RunMeta;
use crate::reference::Reference;
use crate::script::Script;
use crate::sim::Sim;
use crate::snapshot::Snapshot;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Area {
    Economy,
    Movement,
    Construction,
    Time,
}

/// What a passing scenario proves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// Pins the port's own behavior; no original-game measurement behind it.
    Internal,
    /// Its metrics are compared against these reference fixture ids.
    Parity(&'static [&'static str]),
}

pub struct Scenario {
    pub name: &'static str,
    pub area: Area,
    pub class: Class,
    pub summary: &'static str,
    /// The seed goldens are blessed with.
    pub seed: u32,
    /// Builds the island (world, state, citizens, ledger) for a seed.
    pub setup: fn(u32) -> Sim,
    /// The player's input. Keep segment lengths multiples of 6 ticks so the
    /// script plays the same at 1, 2 and 3 ticks per frame.
    pub script: fn() -> Script,
    /// Snapshot every this many ticks (and at the end).
    pub checkpoint_every: u32,
    /// Assertions on the finished run. Must hold for every seed.
    pub check: fn(&Sim, &mut Checks),
    /// Numbers compared against reference fixtures (parity scenarios).
    pub metrics: fn(&Sim) -> BTreeMap<String, f64>,
    /// A known bug this scenario exposes. While set, a failure is reported
    /// as XFAIL and doesn't fail the run; a pass (XPASS) does, so the
    /// marker gets removed once the bug is fixed.
    pub known_issue: Option<&'static str>,
}

pub fn no_metrics(_: &Sim) -> BTreeMap<String, f64> {
    BTreeMap::new()
}

/// Collects check failures instead of panicking, so a run reports them all.
#[derive(Debug, Default)]
pub struct Checks {
    pub failures: Vec<String>,
}

impl Checks {
    pub fn that(&mut self, ok: bool, what: impl FnOnce() -> String) {
        if !ok {
            self.failures.push(what());
        }
    }

    pub fn eq<T: PartialEq + std::fmt::Debug>(&mut self, what: &str, got: T, want: T) {
        if got != want {
            self.failures
                .push(format!("{what}: got {got:?}, want {want:?}"));
        }
    }

    pub fn near(&mut self, what: &str, got: f32, want: f32, tol: f32) {
        // Written so a NaN fails.
        let within = (got - want).abs() <= tol;
        if !within {
            self.failures
                .push(format!("{what}: got {got}, want {want} ± {tol}"));
        }
    }

    pub fn range<T: PartialOrd + std::fmt::Debug>(&mut self, what: &str, got: T, lo: T, hi: T) {
        if !(got >= lo && got <= hi) {
            self.failures
                .push(format!("{what}: got {got:?}, want {lo:?}..={hi:?}"));
        }
    }
}

pub struct Outcome {
    pub meta: RunMeta,
    pub checkpoints: Vec<Snapshot>,
    pub failures: Vec<String>,
    pub violations: Vec<(u32, String)>,
    pub metrics: BTreeMap<String, f64>,
    /// `(fixture id, label, failures)` per reference.
    pub parity: Vec<(String, String, Vec<String>)>,
    pub event_lines: Vec<String>,
    pub elapsed_s: f64,
}

impl Outcome {
    pub fn final_snapshot(&self) -> &Snapshot {
        self.checkpoints
            .last()
            .expect("a run always ends with a snapshot")
    }

    pub fn passed(&self) -> bool {
        self.failures.is_empty()
            && self.violations.is_empty()
            && self.parity.iter().all(|p| p.2.is_empty())
    }
}

/// Runs `sc` with `seed` at `ticks_per_frame`.
pub fn run(sc: &Scenario, seed: u32, ticks_per_frame: u32, refs: &[Reference]) -> Outcome {
    let started = Instant::now();
    let mut sim = (sc.setup)(seed);
    sim.checkpoint_every = Some(sc.checkpoint_every);
    let script = (sc.script)();
    sim.run(&script, ticks_per_frame);
    sim.finish();
    // The end, unless a checkpoint just landed on it.
    if sim.checkpoints.last().is_none_or(|c| c.tick != sim.tick) {
        sim.checkpoints.push(Snapshot::of(&sim));
    }
    let elapsed_s = started.elapsed().as_secs_f64();

    let mut checks = Checks::default();
    (sc.check)(&sim, &mut checks);
    let metrics = (sc.metrics)(&sim);
    let mut parity = Vec::new();
    if let Class::Parity(ids) = sc.class {
        for id in ids {
            match refs.iter().find(|r| r.id == *id) {
                Some(r) => parity.push((r.id.clone(), r.label(), r.compare(&metrics))),
                None => checks
                    .failures
                    .push(format!("reference fixture {id:?} not found")),
            }
        }
    }
    // Fixtures that name this scenario (e.g. measured from the original
    // with `regress orig`).
    for r in refs
        .iter()
        .filter(|r| r.scenarios.iter().any(|s| s == sc.name))
    {
        if !parity.iter().any(|p| p.0 == r.id) {
            parity.push((r.id.clone(), r.label(), r.compare(&metrics)));
        }
    }
    let mut meta = RunMeta::new(sc.name, "synthetic", 0, seed);
    meta.ticks_per_frame = ticks_per_frame;
    meta.ticks = sim.tick;
    Outcome {
        meta,
        checkpoints: std::mem::take(&mut sim.checkpoints),
        failures: checks.failures,
        violations: sim.violations.clone(),
        metrics,
        parity,
        event_lines: std::mem::take(&mut sim.log.lines),
        elapsed_s,
    }
}
