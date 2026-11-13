//! Measuring the original Kingdom: New Lands (PC) with the KingdomTrace
//! harness (`tools/orig-harness/`), and comparing it with the port.
//!
//! The harness runs inside the user's own copy of the game. It fixes the
//! frame rate at 60 (one frame = one port tick), seeds Unity's RNG, feeds
//! the same input scripts the port uses, and writes a trace: header lines,
//! sampled state and hooked events. Nothing from the game enters the
//! repository: traces stay in `gamefiles/harness/traces/`, and only the
//! numbers measured from them go into reference fixtures.
//!
//! Each [`OrigScenario`] computes its metrics from a [`Trace`]. The port
//! produces the same kind of trace ([`port_trace`]) from the same script on
//! the user's converted island, so one metrics function serves both.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use game::data::WorldData;
use game::state::{GameState, TICK};

use crate::reference::{Confidence, Evidence, EvidenceKind, Expected, Original, Reference, Setup};
use crate::script::Script;
use crate::sim::Sim;

/// The probe list shared by every config (original member names only).
pub const PROBES: &str = include_str!("../../../tools/orig-harness/probes.cfg");

/// A parsed trace, from the original or the port.
#[derive(Clone, Debug, Default)]
pub struct Trace {
    /// `H key value` lines (repeated keys keep every value).
    pub header: Vec<(String, String)>,
    /// `(frame, key -> value)`, frame 0 being the start of the run.
    pub samples: Vec<(u32, BTreeMap<String, String>)>,
    /// `(frame, event)`.
    pub events: Vec<(u32, String)>,
    /// The run reached its last frame (not aborted).
    pub complete: bool,
}

impl Trace {
    pub fn parse(text: &str) -> Result<Trace, String> {
        let mut t = Trace::default();
        for (n, line) in text.lines().enumerate() {
            let mut it = line.splitn(2, ' ');
            let (tag, rest) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
            let bad = || format!("trace line {}: {line:?}", n + 1);
            match tag {
                "H" => {
                    let (k, v) = rest.split_once(' ').unwrap_or((rest, ""));
                    t.header.push((k.to_string(), v.to_string()));
                }
                "S" => {
                    let mut parts = rest.split(' ');
                    let frame = parts.next().and_then(|f| f.parse().ok()).ok_or_else(bad)?;
                    let kv = parts
                        .filter_map(|p| p.split_once('='))
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect();
                    t.samples.push((frame, kv));
                }
                "E" => {
                    let mut parts = rest.split(' ');
                    let frame = parts.next().and_then(|f| f.parse().ok()).ok_or_else(bad)?;
                    let name = parts.next().ok_or_else(bad)?.to_string();
                    t.events.push((frame, name));
                }
                "END" => t.complete = true,
                "ABORT" | "" => {}
                _ => return Err(bad()),
            }
        }
        Ok(t)
    }

    pub fn header(&self, key: &str) -> Option<&str> {
        self.header
            .iter()
            .find(|h| h.0 == key)
            .map(|h| h.1.as_str())
    }

    /// `(frame, value)` for a numeric (or 0/1) sample key.
    pub fn series(&self, key: &str) -> Vec<(u32, f64)> {
        self.samples
            .iter()
            .filter_map(|(f, kv)| Some((*f, kv.get(key)?.parse().ok()?)))
            .collect()
    }

    pub fn at(&self, key: &str, frame: u32) -> Option<f64> {
        self.series(key)
            .into_iter()
            .find(|s| s.0 == frame)
            .map(|s| s.1)
    }

    pub fn last(&self, key: &str) -> Option<f64> {
        self.series(key).last().map(|s| s.1)
    }

    pub fn count(&self, event: &str) -> usize {
        self.events.iter().filter(|e| e.1 == event).count()
    }

    /// `(hours since day 0 00:00, ambient luma)` from the `day`, `time` and
    /// `ambient` samples.
    pub fn light_series(&self) -> Vec<(f64, f64)> {
        self.samples
            .iter()
            .filter_map(|(_, kv)| {
                let day: f64 = kv.get("day")?.parse().ok()?;
                let time: f64 = kv.get("time")?.parse().ok()?;
                let c = crate::tod::parse_color(kv.get("ambient")?)?;
                Some((day * 24.0 + time, crate::tod::luma(c)))
            })
            .collect()
    }

    /// `(hours since day 0 00:00, is daytime)` where daytime flips.
    pub fn daytime_edges(&self) -> Vec<(f64, bool)> {
        let at: Vec<(f64, bool)> = self
            .samples
            .iter()
            .filter_map(|(_, kv)| {
                let day: f64 = kv.get("day")?.parse().ok()?;
                let time: f64 = kv.get("time")?.parse().ok()?;
                Some((day * 24.0 + time, kv.get("daytime")? == "1"))
            })
            .collect();
        at.windows(2)
            .filter(|w| w[0].1 != w[1].1)
            .map(|w| (w[1].0, w[1].1))
            .collect()
    }

    /// Frames where a 0/1 key changes, with the new value.
    pub fn edges(&self, key: &str) -> Vec<(u32, bool)> {
        let s = self.series(key);
        s.windows(2)
            .filter(|w| (w[0].1 != 0.0) != (w[1].1 != 0.0))
            .map(|w| (w[1].0, w[1].1 != 0.0))
            .collect()
    }

    /// Problems that make the trace unusable as evidence.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.complete {
            out.push("the run didn't reach its last frame".into());
        }
        if self.samples.is_empty() {
            out.push("no samples".into());
        }
        let skipped = self.count("Harness.InputSkipped");
        if skipped > 0 {
            out.push(format!("{skipped} frames where the game didn't take input"));
        }
        for (k, v) in &self.header {
            if v.ends_with("MISSING") || k == "setup_failed" {
                out.push(format!("{k} {v}"));
            }
        }
        out
    }
}

/// Seconds from frames.
fn s(frames: u32) -> f64 {
    frames as f64 * TICK as f64
}

/// What one measurement on the original is.
pub struct OrigScenario {
    pub name: &'static str,
    pub summary: &'static str,
    /// Frames to run after the game starts playing.
    pub frames: u32,
    pub sample_every: u32,
    /// Original state to set at frame 0: (member path, value).
    pub setup: &'static [(&'static str, &'static str)],
    /// The port's equivalent of `setup`, for [`port_trace`].
    pub port_setup: fn(&mut GameState),
    pub script: fn() -> Script,
    pub metrics: fn(&Trace) -> BTreeMap<String, f64>,
    /// The fixture written from the measurements.
    pub fixture_id: &'static str,
    pub title: &'static str,
    /// (metric, unit, smallest tolerance: the measurement's resolution).
    pub resolution: &'static [(&'static str, &'static str, f64)],
    /// Port scenarios that report the same metrics.
    pub port_scenarios: &'static [&'static str],
}

/// Effective speed over frames `[a, b]`: displacement over the longest
/// stretch outside forests (which slow the steed by
/// `forestSpeedMultiplier`), in whole 6-frame blocks. The original steps
/// physics at 50 Hz under 60 frames a second, so the body moves on 5 frames
/// of every 6; 6 frames are exactly 5 steps. Islands differ between the two
/// games, so a forest can sit in one's path and not the other's. NaN with
/// under 1 s of open ground.
fn speed(t: &Trace, a: u32, b: u32) -> f64 {
    let open = |kv: &BTreeMap<String, String>| kv.get("ground").is_none_or(|g| g != "Forest");
    let at: BTreeMap<u32, (f64, bool)> = t
        .samples
        .iter()
        .filter(|(f, _)| (a..=b).contains(f))
        .filter_map(|(f, kv)| Some((*f, (kv.get("x")?.parse().ok()?, open(kv)))))
        .collect();
    // Longest run of consecutive open frames.
    let (mut best, mut start) = ((0u32, 0u32), None);
    for f in a..=b + 1 {
        match (at.get(&f).is_some_and(|p| p.1), start) {
            (true, None) => start = Some(f),
            (false, Some(s0)) => {
                if f - s0 > best.1 - best.0 {
                    best = (s0, f);
                }
                start = None;
            }
            _ => {}
        }
    }
    let frames = (best.1 - best.0).saturating_sub(1) / 6 * 6;
    if frames < 60 {
        return f64::NAN;
    }
    let (x0, x1) = (at[&best.0].0, at[&(best.0 + frames)].0);
    (x1 - x0).abs() / s(frames)
}

/// Effective speed of the driven citizen (`@drive`) over frames `[a, b]`
/// (`b - a` a multiple of 6: whole physics steps). NaN unless `Mover` held
/// `setting` the whole time. Citizens aren't slowed by forests.
fn driven_speed(t: &Trace, a: u32, b: u32, setting: f64) -> f64 {
    let held = t
        .samples
        .iter()
        .filter(|(f, _)| (a..=b).contains(f))
        .all(|(_, kv)| {
            kv.get("cset")
                .and_then(|v| v.parse::<f64>().ok())
                .is_some_and(|v| (v - setting).abs() < 1e-4)
        });
    match (held, t.at("cx", a), t.at("cx", b)) {
        (true, Some(x0), Some(x1)) => (x1 - x0).abs() / s(b - a),
        _ => f64::NAN,
    }
}

/// The port's `@drive`: a peasant one unit right of the campfire whose AI
/// is off (stunned for good: only its `Mover` runs), holding `speed`
/// rightwards. [`port_sample`] finds it by [`DRIVEN_STUN`].
pub fn drive_peasant(g: &mut GameState, speed: f32) {
    at_campfire(g);
    let id = g
        .population
        .spawn(game::data::Role::Peasant, g.campfire + 1.0, &mut g.rng);
    if let Some(u) = g.population.units.iter_mut().find(|u| u.id == id) {
        u.stunned = DRIVEN_STUN;
        u.mover.set_speed(speed, 1);
    }
}

/// Stun time that marks the driven citizen in the port.
pub const DRIVEN_STUN: f32 = 1e9;

fn at_campfire(g: &mut GameState) {
    g.monarch.x = g.campfire;
    g.monarch.velocity = 0.0;
}

pub fn scenarios() -> Vec<OrigScenario> {
    vec![
        OrigScenario {
            name: "orig_walk",
            summary: "Walk right for 10 s from the campfire on Land One: steady walking speed outside forests",
            frames: 13 * 60,
            sample_every: 1,
            // Both games start the monarch somewhere else (the original after
            // its intro, the port at the intro's start): use the campfire.
            setup: &[("@player_x", "Managers.kingdom.campfirePosition")],
            port_setup: at_campfire,
            script: || Script::new().idle(1.0).walk(1, 10.0).idle(2.0),
            // After 1 s to get up to speed, until letting go.
            metrics: |t| BTreeMap::from([("walk_speed".into(), speed(t, 120, 660))]),
            fixture_id: "trace-walk-speed",
            title: "Walking speed on the default steed, measured in the original",
            resolution: &[("walk_speed", "units/s", 0.02)],
            port_scenarios: &["monarch_walk"],
        },
        OrigScenario {
            name: "orig_gallop",
            summary: "Gallop (sprint held) right for 10 s from the campfire on Land One: steady gallop speed outside forests",
            frames: 13 * 60,
            sample_every: 1,
            // Both games start the monarch somewhere else (the original after
            // its intro, the port at the intro's start): use the campfire.
            setup: &[("@player_x", "Managers.kingdom.campfirePosition")],
            port_setup: at_campfire,
            script: || Script::new().idle(1.0).gallop(1, 10.0).idle(2.0),
            metrics: |t| BTreeMap::from([("gallop_speed".into(), speed(t, 120, 660))]),
            fixture_id: "trace-gallop-speed",
            title: "Gallop speed on the default steed, measured in the original",
            resolution: &[("gallop_speed", "units/s", 0.03)],
            port_scenarios: &["monarch_gallop"],
        },
        OrigScenario {
            name: "orig_citizen_walk",
            summary: "A peasant spawned by the campfire with its AI off, Mover held at its walk speed (0.6) rightwards: effective citizen walking speed",
            frames: 12 * 60,
            sample_every: 1,
            setup: &[
                ("@player_x", "Managers.kingdom.campfirePosition"),
                ("@drive", "Peasant 0.6 1"),
            ],
            port_setup: |g| drive_peasant(g, 0.6),
            script: || Script::new().idle(12.0),
            // After 2 s to land and settle, 9 s (540 frames = 450 steps).
            metrics: |t| BTreeMap::from([("citizen_walk_speed".into(), driven_speed(t, 120, 660, 0.6))]),
            fixture_id: "trace-citizen-walk-speed",
            title: "Citizen (peasant) walking speed, measured in the original",
            resolution: &[("citizen_walk_speed", "units/s", 0.01)],
            port_scenarios: &["citizen_walk"],
        },
        OrigScenario {
            name: "orig_citizen_run",
            summary: "A peasant spawned by the campfire with its AI off, Mover held at its run speed (1.4) rightwards: effective citizen running speed",
            frames: 12 * 60,
            sample_every: 1,
            setup: &[
                ("@player_x", "Managers.kingdom.campfirePosition"),
                ("@drive", "Peasant 1.4 1"),
            ],
            port_setup: |g| drive_peasant(g, 1.4),
            script: || Script::new().idle(12.0),
            metrics: |t| BTreeMap::from([("citizen_run_speed".into(), driven_speed(t, 120, 660, 1.4))]),
            fixture_id: "trace-citizen-run-speed",
            title: "Citizen (peasant) running speed, measured in the original",
            resolution: &[("citizen_run_speed", "units/s", 0.01)],
            port_scenarios: &["citizen_run"],
        },
        OrigScenario {
            name: "orig_day_cycle",
            summary: "Three days and nights from the start of Land One, greed spawns off: dawn and dusk timing",
            frames: 3 * 240 * 60 + 600,
            sample_every: 1,
            // The original's own debug switch: no greed, so an idle monarch
            // survives the nights. Clock timing doesn't depend on greed.
            setup: &[("Portal.DebugDisableSpawns", "1")],
            port_setup: |g| g.god = true,
            script: || Script::new().idle(3.0 * 240.0 + 10.0),
            metrics: |t| {
                let e = t.edges("daytime");
                let at = |i: usize| e.get(i).map_or(f64::NAN, |x| s(x.0));
                let start_hour = t.at("time", 0).unwrap_or(f64::NAN);
                BTreeMap::from([
                    ("start_hour".into(), start_hour),
                    ("first_dawn_s".into(), at(0)),
                    ("daytime_s".into(), at(1) - at(0)),
                    ("night_s".into(), at(2) - at(1)),
                    ("day_length_s".into(), at(2) - at(0)),
                    ("day_length_3_avg_s".into(), (at(6) - at(0)) / 3.0),
                ])
            },
            fixture_id: "trace-day-night-cycle",
            title: "Dawn and dusk timing over three days, measured in the original",
            resolution: &[
                ("start_hour", "h", 0.01),
                ("first_dawn_s", "s", 0.05),
                ("daytime_s", "s", 0.05),
                ("night_s", "s", 0.05),
                ("day_length_s", "s", 0.05),
                ("day_length_3_avg_s", "s", 0.05),
            ],
            port_scenarios: &["day_night_cycle"],
        },
        OrigScenario {
            name: "orig_time_of_day",
            summary: "Three days and nights at the campfire on Land One, greed spawns off: when each lighting phase starts, and the lit day and night",
            frames: 3 * 240 * 60 + 1800,
            sample_every: 6,
            setup: &[
                ("@player_x", "Managers.kingdom.campfirePosition"),
                ("Portal.DebugDisableSpawns", "1"),
            ],
            port_setup: |g| {
                at_campfire(g);
                g.god = true;
            },
            script: || Script::new().idle(3.0 * 240.0 + 30.0),
            metrics: |t| {
                let mut m = crate::tod::light_metrics(&t.light_series());
                let (day, night) = crate::tod::daytime_lengths(&t.daytime_edges());
                m.insert("daytime_s".into(), day);
                m.insert("night_s".into(), night);
                m
            },
            fixture_id: "trace-time-of-day-lighting",
            title: "When each lighting phase starts, measured in the original",
            // Samples every 6 frames: 0.01 h.
            resolution: &[
                ("dawn_light_start_h", "h", 0.02),
                ("day_light_start_h", "h", 0.02),
                ("evening_light_start_h", "h", 0.02),
                ("night_light_start_h", "h", 0.02),
                ("light_cycle_s", "s", 0.25),
                ("unscheduled_light_changes", "count", 0.0),
                ("daytime_s", "s", 0.25),
                ("night_s", "s", 0.25),
            ],
            port_scenarios: &["time_of_day_lighting"],
        },
        OrigScenario {
            name: "orig_purse_overflow",
            summary: "Purse set to 90 coins at the campfire: how many spill and where it settles",
            frames: 40 * 60,
            sample_every: 6,
            setup: &[
                ("@player_x", "Managers.kingdom.campfirePosition"),
                ("Managers.kingdom.player._wallet.coins", "90"),
            ],
            port_setup: |g| {
                at_campfire(g);
                g.wallet.set(90);
            },
            script: || Script::new().idle(40.0),
            metrics: |t| {
                let last = t
                    .events
                    .iter()
                    .filter(|e| e.1 == "Bag.Spilled")
                    .map(|e| e.0)
                    .max()
                    .unwrap_or(0);
                BTreeMap::from([
                    ("purse_after_overflow".into(), t.last("wallet").unwrap_or(f64::NAN)),
                    ("overflow_spills".into(), t.count("Bag.Spilled") as f64),
                    ("last_spill_s".into(), s(last)),
                ])
            },
            fixture_id: "trace-purse-overflow",
            title: "Purse overflow from 90 coins, measured in the original",
            resolution: &[
                ("purse_after_overflow", "coins", 1.0),
                ("overflow_spills", "coins", 1.0),
                ("last_spill_s", "s", 0.5),
            ],
            // Not the synthetic `purse_overflow`: its purse isn't the real
            // one's size. Compare on the real island (`regress orig check`).
            port_scenarios: &[],
        },
    ]
}

pub fn find(name: &str) -> Option<OrigScenario> {
    scenarios().into_iter().find(|s| s.name == name)
}

/// The harness config for one run of `sc`.
pub fn config(sc: &OrigScenario, seed: u32, out: &Path) -> Result<String, String> {
    config_with_shots(sc, seed, out, &[])
}

/// [`config`] plus screenshots at clock times: `(day, hour, png path)`.
pub fn config_with_shots(
    sc: &OrigScenario,
    seed: u32,
    out: &Path,
    shots: &[(u32, f32, PathBuf)],
) -> Result<String, String> {
    let mut c = String::new();
    let _ = writeln!(
        c,
        "# Written by `regress orig export`. Harness: tools/orig-harness/"
    );
    let _ = writeln!(c, "out = {}", out.display());
    let _ = writeln!(c, "scenario = {}", sc.name);
    let _ = writeln!(c, "seed = {seed}");
    let _ = writeln!(c, "frames = {}", sc.frames);
    let _ = writeln!(c, "sample_every = {}", sc.sample_every);
    let _ = writeln!(c, "start_when = Managers.game.state == Playing");
    c.push('\n');
    c.push_str(PROBES);
    let _ = writeln!(c, "\n[setup]");
    for (k, v) in sc.setup {
        let _ = writeln!(c, "{k} = {v}");
    }
    if !shots.is_empty() {
        let _ = writeln!(c, "\n[shots]\n# day hour path");
        for (day, hour, path) in shots {
            let _ = writeln!(c, "{day} {hour} {}", path.display());
        }
    }
    let _ = writeln!(c, "\n[script]\n# ticks dir sprint pay");
    for seg in &(sc.script)().segments {
        if seg.tap {
            return Err(format!(
                "{}: the original has no bag tap (a 3DS touch-screen addition)",
                sc.name
            ));
        }
        let _ = writeln!(
            c,
            "{} {} {} {}",
            seg.ticks, seg.dir, seg.sprint as u8, seg.pay as u8
        );
    }
    Ok(c)
}

/// The presets on the blender's stack as the harness writes the original's
/// (`@blender_stack`): `name:weight|...`, bottom first, spaces as `_`.
pub fn port_light_stack(g: &GameState, world: &WorldData) -> String {
    g.blender
        .presets()
        .map(|(p, w)| {
            let name = world
                .lights
                .get(p as usize)
                .map_or("?", |l| l.name.as_str());
            format!(
                "{}:{}",
                name.replace(' ', "_"),
                (w * 1000.0).round() / 1000.0
            )
        })
        .collect::<Vec<_>>()
        .join("|")
}

/// The port's side of each probe key.
fn port_sample(g: &GameState, world: &WorldData) -> BTreeMap<String, String> {
    let driven = g
        .population
        .units
        .iter()
        .find(|u| u.stunned > DRIVEN_STUN * 0.5);
    let or_null = |v: Option<f32>| v.map_or("null".into(), |v| v.to_string());
    let color = |i: usize| crate::tod::format_color(g.blender.state.colors[i]);
    BTreeMap::from([
        ("cx".into(), or_null(driven.map(|u| u.x))),
        ("cset".into(), or_null(driven.map(|u| u.mover.move_speed()))),
        ("time".into(), g.clock.time.to_string()),
        ("day".into(), g.clock.day.to_string()),
        ("daytime".into(), (g.days.is_daytime as u8).to_string()),
        ("shown_day".into(), g.days.day.to_string()),
        ("x".into(), g.monarch.x.to_string()),
        // As `GameState::ground` decides it (the original's `_groundType`).
        (
            "ground".into(),
            if game::build::in_forest(&g.forests, g.monarch.x + 0.5 * g.monarch.facing as f32) {
                "Forest"
            } else {
                "Normal"
            }
            .into(),
        ),
        ("wallet".into(), g.wallet.coins().to_string()),
        ("ambient".into(), color(game::lighting::color::AMBIENT)),
        ("sky_top".into(), color(game::lighting::color::SKY_TOP)),
        (
            "sky_bottom".into(),
            color(game::lighting::color::SKY_BOTTOM),
        ),
        (
            "stars".into(),
            g.blender.state.floats[game::lighting::float::STARS].to_string(),
        ),
        ("lights".into(), port_light_stack(g, world)),
        (
            "state".into(),
            if g.game_over { "Over" } else { "Playing" }.into(),
        ),
    ])
}

/// Runs `sc` on the port with the user's converted island and records the
/// same kind of trace the harness writes.
pub fn port_trace(sc: &OrigScenario, world: &WorldData, seed: u32) -> Trace {
    let mut g = GameState::new(world, 0, seed);
    (sc.port_setup)(&mut g);
    let mut sim = Sim::new(world.clone(), g);
    let mut t = Trace {
        header: vec![("harness".into(), "port".into())],
        ..Default::default()
    };
    t.samples.push((0, port_sample(&sim.game, &sim.world)));
    let mut seen = 0;
    for raw in (sc.script)()
        .inputs()
        .chain(std::iter::repeat(Default::default()))
        .take(sc.frames as usize)
    {
        sim.frame(raw, 1);
        // Events of interest, under the same names the hooks use.
        let total = sim.log.lines.len();
        for line in &sim.log.lines[seen..total] {
            let mut p = line.split(' ');
            let (Some(f), Some(kind), Some(detail)) = (p.next(), p.next(), p.next()) else {
                continue;
            };
            let name = match kind {
                "Coin.PickedUp" if detail.contains("Monarch") => "Coin.PickedUp",
                "Bag.Spilled" | "Pay.Complete" => kind,
                "Pay.ReleaseCoins" => "Pay.Cancel",
                _ => continue,
            };
            if let Ok(f) = f.parse() {
                t.events.push((f, name.to_string()));
            }
        }
        seen = total;
        if sim.tick.is_multiple_of(sc.sample_every) || sim.tick == sc.frames {
            t.samples
                .push((sim.tick, port_sample(&sim.game, &sim.world)));
        }
    }
    t.complete = true;
    t
}

/// Mean, min and max of each metric over several runs.
pub fn aggregate(runs: &[BTreeMap<String, f64>]) -> BTreeMap<String, (f64, f64, f64)> {
    let mut out: BTreeMap<String, (f64, f64, f64)> = BTreeMap::new();
    for k in runs
        .iter()
        .flat_map(|r| r.keys())
        .collect::<std::collections::BTreeSet<_>>()
    {
        let v: Vec<f64> = runs
            .iter()
            .filter_map(|r| r.get(k).copied())
            .filter(|v| v.is_finite())
            .collect();
        if v.is_empty() {
            continue;
        }
        let mean = v.iter().sum::<f64>() / v.len() as f64;
        let min = v.iter().copied().fold(f64::INFINITY, f64::min);
        let max = v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        out.insert(k.clone(), (mean, min, max));
    }
    out
}

/// A reference fixture from harness traces of the original.
pub fn fixture(sc: &OrigScenario, traces: &[(PathBuf, Trace)]) -> Result<Reference, String> {
    if traces.is_empty() {
        return Err("no traces".into());
    }
    for (p, t) in traces {
        if t.header("harness").is_none_or(|h| h == "port") {
            return Err(format!("{}: not a trace of the original", p.display()));
        }
        let problems = t.problems();
        if !problems.is_empty() {
            return Err(format!("{}: {}", p.display(), problems.join("; ")));
        }
    }
    let first = &traces[0].1;
    let version = first.header("game_version").unwrap_or("?");
    let unity = first.header("unity_version").unwrap_or("?");
    let platform = first.header("platform").unwrap_or("?");
    if traces
        .iter()
        .any(|(_, t)| t.header("game_version") != Some(version))
    {
        return Err("traces come from different game versions".into());
    }
    let runs: Vec<BTreeMap<String, f64>> = traces.iter().map(|(_, t)| (sc.metrics)(t)).collect();
    let agg = aggregate(&runs);
    let mut expected = Vec::new();
    for (metric, unit, resolution) in sc.resolution {
        let Some(&(mean, min, max)) = agg.get(*metric) else {
            return Err(format!("{metric} couldn't be measured from the traces"));
        };
        // The spread across runs, never finer than what a trace resolves.
        let tol = ((max - min) / 2.0).max(*resolution);
        expected.push(Expected {
            metric: metric.to_string(),
            value: (mean * 1e4).round() / 1e4,
            tolerance: (tol * 1e4).round() / 1e4,
            unit: unit.to_string(),
        });
    }
    let seeds: Vec<&str> = traces
        .iter()
        .filter_map(|(_, t)| t.header("seed"))
        .collect();
    let mut initial = BTreeMap::new();
    initial.insert(
        "island".into(),
        toml::Value::String("Land One, fresh start".into()),
    );
    initial.insert("seeds".into(), toml::Value::String(seeds.join(",")));
    for (k, v) in sc.setup {
        initial.insert(k.to_string(), toml::Value::String(v.to_string()));
    }
    let n = traces.len() as u32;
    Ok(Reference {
        id: sc.fixture_id.into(),
        title: sc.title.into(),
        original: Original {
            game_version: format!("Kingdom: New Lands {version} (Unity {unity})"),
            platform: platform.into(),
        },
        setup: Setup {
            description: format!(
                "{}. Intro skipped as the skip button does; Time.captureFramerate = 60; \
                 UnityEngine.Random seeded per run.",
                sc.summary
            ),
            initial,
        },
        actions: (sc.script)()
            .segments
            .iter()
            .map(|g| {
                format!(
                    "{} frames: dir {}{}{}",
                    g.ticks,
                    g.dir,
                    if g.sprint { ", sprint" } else { "" },
                    if g.pay { ", pay" } else { "" }
                )
            })
            .collect(),
        observations: Vec::new(),
        expected,
        evidence: Evidence {
            kind: if n >= 2 {
                EvidenceKind::RepeatedMeasurement
            } else {
                EvidenceKind::Trace
            },
            source: format!(
                "KingdomTrace harness (tools/orig-harness) scenario {}, {n} run(s); traces kept \
                 locally in gamefiles/harness/traces/{}/",
                sc.name, sc.name
            ),
            confidence: Confidence::Medium,
            independently_verified: false,
            verification: String::new(),
            samples: n,
            recorded_by: String::new(),
            date: first
                .header("started_utc")
                .map(|d| d.chars().take(10).collect())
                .unwrap_or_default(),
        },
        scenarios: sc.port_scenarios.iter().map(|s| s.to_string()).collect(),
    })
}

pub fn fixture_toml(r: &Reference) -> String {
    let mut s = String::from(
        "# Measured from the original game with the KingdomTrace harness\n\
         # (`regress orig`). Numbers only; the traces stay local.\n",
    );
    s.push_str(&toml::to_string(r).expect("fixture serializes"));
    s
}

/// The repository root (for the harness scripts and gamefiles/).
pub fn repo_root() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    root.canonicalize().unwrap_or(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRACE: &str = "H harness KingdomTrace 1.0.0\nH game_version 1.2.8\nH seed 1\n\
        S 0 x=0 daytime=0 wallet=5\nE 3 Bag.Spilled 12\nS 1 x=0.5 daytime=1 wallet=4\nEND 1\n";

    #[test]
    fn parses_traces() {
        let t = Trace::parse(TRACE).unwrap();
        assert!(t.complete);
        assert_eq!(t.header("game_version"), Some("1.2.8"));
        assert_eq!(t.series("x"), vec![(0, 0.0), (1, 0.5)]);
        assert_eq!(t.edges("daytime"), vec![(1, true)]);
        assert_eq!(t.count("Bag.Spilled"), 1);
        assert!(t.problems().is_empty());
    }

    #[test]
    fn configs_carry_script_setup_and_probes() {
        for sc in scenarios() {
            let c = config(&sc, 3, Path::new("/tmp/t.txt")).unwrap();
            assert!(c.contains("seed = 3") && c.contains("[sample]") && c.contains("[script]"));
            let ticks: u32 = c
                .split("[script]")
                .nth(1)
                .unwrap()
                .lines()
                .filter_map(|l| l.split(' ').next()?.parse::<u32>().ok())
                .sum();
            assert_eq!(ticks, (sc.script)().len_ticks(), "{}", sc.name);
        }
    }

    #[test]
    fn fixtures_need_original_traces() {
        let sc = find("orig_walk").unwrap();
        let mut t = Trace::parse(TRACE).unwrap();
        assert!(
            fixture(&sc, &[("a".into(), t.clone())]).is_err(),
            "walk can't be measured from 2 frames"
        );
        t.header[0].1 = "port".into();
        assert!(fixture(&sc, &[("a".into(), t)]).is_err());
    }
}
