//! Canonical, versioned snapshots of the simulation, and how two of them
//! are compared: discrete state exactly, numbers within a tolerance, the
//! full-state hash only when asked to be strict.

use std::collections::BTreeMap;

use game::coins::Motion;
use game::data::Role;
use serde::{Deserialize, Serialize};

use crate::sim::Sim;

/// Bumped whenever a field is added, removed or changes meaning.
pub const SNAPSHOT_VERSION: u32 = 1;

pub fn fnv64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: u32,
    pub tick: u32,

    // Discrete: compared exactly.
    pub clock_day: u32,
    pub day_shown: u32,
    pub daytime: bool,
    pub season: String,
    pub wallet: u32,
    /// Loose coins in the world (`sim::is_loose`).
    pub coins_live: u32,
    /// Coins hovering over or flying to a payable (`Moving` / `Floating`).
    pub coins_floating: u32,
    /// Coins held by citizens.
    pub coins_held: u32,
    /// Citizens by role: beggar, peasant, worker, archer, farmer, knight.
    pub roles: [u32; 6],
    pub objects: u32,
    pub under_construction: u32,
    pub has_crown: bool,
    pub game_over: bool,
    pub won: bool,
    /// Event counts by kind, up to this tick.
    pub events: BTreeMap<String, u32>,

    // Numeric: compared within tolerances.
    pub clock_time: f32,
    pub monarch_x: f32,
    pub monarch_velocity: f32,

    // Same binary, same platform only.
    pub state_hash: String,
    pub event_hash: String,
}

/// How far numbers may drift between two snapshots and still match.
#[derive(Clone, Copy, Debug)]
pub struct Tolerance {
    /// Hours.
    pub clock_time: f32,
    /// World units.
    pub position: f32,
    /// Units per second.
    pub velocity: f32,
}

impl Default for Tolerance {
    fn default() -> Self {
        Self {
            clock_time: 1e-3,
            position: 1e-3,
            velocity: 1e-3,
        }
    }
}

fn role_index(r: Role) -> usize {
    match r {
        Role::Beggar => 0,
        Role::Peasant => 1,
        Role::Worker => 2,
        Role::Archer => 3,
        Role::Farmer => 4,
        Role::Knight => 5,
    }
}

impl Snapshot {
    pub fn of(sim: &Sim) -> Self {
        let g = &sim.game;
        let mut roles = [0u32; 6];
        for u in &g.population.units {
            roles[role_index(u.role)] += 1;
        }
        Snapshot {
            version: SNAPSHOT_VERSION,
            tick: sim.tick,
            clock_day: g.clock.day,
            day_shown: g.days.day,
            daytime: g.days.is_daytime,
            season: format!("{:?}", g.season),
            wallet: g.wallet.coins(),
            coins_live: g
                .coins
                .list
                .iter()
                .filter(|c| crate::sim::is_loose(c))
                .count() as u32,
            coins_floating: g
                .coins
                .list
                .iter()
                .filter(|c| matches!(c.motion, Motion::Floating { .. } | Motion::Moving { .. }))
                .count() as u32,
            coins_held: g.population.units.iter().map(|u| u.coins).sum(),
            roles,
            objects: g.level.objects.len() as u32,
            under_construction: g
                .level
                .objects
                .iter()
                .filter(|o| game::build::under_construction(&sim.world, o))
                .count() as u32,
            has_crown: g.has_crown,
            game_over: g.game_over,
            won: g.won,
            events: sim.log.kinds.clone(),
            clock_time: g.clock.time,
            monarch_x: g.monarch.x,
            monarch_velocity: g.monarch.velocity,
            state_hash: format!("{:016x}", sim.state_hash()),
            event_hash: format!("{:016x}", sim.log.hash()),
        }
    }

    /// Differences that matter: everything except the hashes. Empty means
    /// the outcomes match.
    pub fn diff(&self, other: &Snapshot, tol: Tolerance) -> Vec<String> {
        let mut out = Vec::new();
        if self.version != other.version {
            out.push(format!(
                "snapshot version {} vs {} (re-bless)",
                self.version, other.version
            ));
            return out;
        }
        macro_rules! exact {
            ($($f:ident),*) => {$(
                if self.$f != other.$f {
                    out.push(format!("{}: {:?} -> {:?}", stringify!($f), self.$f, other.$f));
                }
            )*};
        }
        exact!(
            tick,
            clock_day,
            day_shown,
            daytime,
            season,
            wallet,
            coins_live,
            coins_floating,
            coins_held,
            roles,
            objects,
            under_construction,
            has_crown,
            game_over,
            won
        );
        let keys: std::collections::BTreeSet<&String> =
            self.events.keys().chain(other.events.keys()).collect();
        for k in keys {
            let (a, b) = (
                self.events.get(k).copied().unwrap_or(0),
                other.events.get(k).copied().unwrap_or(0),
            );
            if a != b {
                out.push(format!("events[{k}]: {a} -> {b}"));
            }
        }
        let mut near = |name: &str, a: f32, b: f32, t: f32| {
            if (a - b).abs() > t || a.is_finite() != b.is_finite() {
                out.push(format!("{name}: {a} -> {b} (tolerance {t})"));
            }
        };
        near(
            "clock_time",
            self.clock_time,
            other.clock_time,
            tol.clock_time,
        );
        near("monarch_x", self.monarch_x, other.monarch_x, tol.position);
        near(
            "monarch_velocity",
            self.monarch_velocity,
            other.monarch_velocity,
            tol.velocity,
        );
        out
    }

    pub fn same_trajectory(&self, other: &Snapshot) -> bool {
        self.state_hash == other.state_hash && self.event_hash == other.event_hash
    }
}
