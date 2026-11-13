//! The monarch's movement: walking, galloping, double-tap, island edges.

use std::collections::BTreeMap;

use super::island;
use crate::scenario::{no_metrics, Area, Class, Scenario};
use crate::script::Script;
use crate::sim::Sim;
use crate::worlds;

/// The speed setting: the monarch's velocity at the last checkpoint
/// before letting go (the scripts hold the direction for 4 s, which is
/// long past accelerating).
fn speed_at(s: &Sim, tick: u32) -> f64 {
    s.checkpoints
        .iter()
        .find(|c| c.tick == tick)
        .map_or(f64::NAN, |c| c.monarch_velocity.abs() as f64)
}

/// Effective speed: distance covered between two checkpoints, per second.
/// The original moves slower than its speed setting (ground friction acts
/// on every physics step), so this is what an original-game trace measures.
fn travelled(s: &Sim, a: u32, b: u32) -> f64 {
    let x = |t: u32| {
        s.checkpoints
            .iter()
            .find(|c| c.tick == t)
            .map(|c| c.monarch_x as f64)
    };
    match (x(a), x(b)) {
        (Some(xa), Some(xb)) => (xb - xa).abs() / ((b - a) as f64 * game::state::TICK as f64),
        _ => f64::NAN,
    }
}

/// A peasant with its AI off (`orig::drive_peasant`) holding a `Mover`
/// speed for 9 s (540 ticks = 450 physics steps), measured from where it
/// spawned: the port's `Mover` reaches the setting on the first tick.
/// The original's side is `orig_citizen_walk` / `orig_citizen_run`.
/// (A macro: scenario fields are plain `fn`s, which can't capture.)
macro_rules! citizen {
    ($name:literal, $metric:literal, $speed:literal) => {
        Scenario {
            name: $name,
            area: Area::Movement,
            class: Class::Internal,
            summary: concat!(
                "A citizen with its AI off holds Mover speed ",
                stringify!($speed),
                ": effective speed after friction and drag"
            ),
            seed: 1,
            setup: |seed| {
                let mut s = island(seed, 0);
                crate::orig::drive_peasant(&mut s.game, $speed);
                s
            },
            script: || Script::new().idle(9.0),
            checkpoint_every: 60,
            check: |s, c| {
                c.that(driven(s).is_some(), || "the driven peasant is gone".into());
            },
            metrics: |s| {
                let x = driven(s).map_or(f64::NAN, |u| u.x as f64);
                let start = s.game.campfire as f64 + 1.0;
                BTreeMap::from([($metric.into(), (x - start).abs() / 9.0)])
            },
            known_issue: None,
        }
    };
}

fn driven(s: &Sim) -> Option<&game::units::Unit> {
    s.game
        .population
        .units
        .iter()
        .find(|u| u.stunned > crate::orig::DRIVEN_STUN * 0.5)
}

pub fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "monarch_walk",
            area: Area::Movement,
            class: Class::Parity(&["horse-regular-walk-speed"]),
            summary: "Holding right walks at the steed's walk speed, then brakes to a stop",
            seed: 1,
            setup: |seed| island(seed, 0),
            script: || Script::new().walk(1, 4.0).idle(3.0),
            checkpoint_every: 60,
            check: |s, c| {
                c.that(s.game.monarch.x > 5.0, || {
                    format!("only got to x {}", s.game.monarch.x)
                });
                c.near("stopped", s.game.monarch.velocity, 0.0, 1e-3);
            },
            metrics: |s| {
                BTreeMap::from([
                    ("walk_speed_setting".into(), speed_at(s, 240)),
                    ("walk_speed".into(), travelled(s, 60, 240)),
                ])
            },
            known_issue: None,
        },
        Scenario {
            name: "monarch_gallop",
            area: Area::Movement,
            class: Class::Parity(&["horse-regular-gallop-speed"]),
            summary: "Holding sprint gallops at the steed's gallop speed",
            seed: 1,
            setup: |seed| island(seed, 0),
            script: || Script::new().gallop(1, 4.0).idle(3.0),
            checkpoint_every: 60,
            check: |s, c| {
                c.near("stopped", s.game.monarch.velocity, 0.0, 1e-3);
            },
            metrics: |s| {
                BTreeMap::from([
                    ("gallop_speed_setting".into(), speed_at(s, 240)),
                    ("gallop_speed".into(), travelled(s, 60, 240)),
                ])
            },
            known_issue: None,
        },
        citizen!("citizen_walk", "citizen_walk_speed", 0.6),
        citizen!("citizen_run", "citizen_run_speed", 1.4),
        Scenario {
            name: "double_tap_gallop",
            area: Area::Movement,
            class: Class::Internal,
            summary: "Two quick presses of a direction start a gallop; two slow ones don't",
            seed: 1,
            setup: |seed| island(seed, 0),
            script: || {
                Script::new()
                    // Quick: press, release, press within 0.6 s, hold.
                    .walk(1, 0.1)
                    .idle(0.1)
                    .walk(1, 3.0)
                    .idle(3.0)
                    // Slow: 1 s between presses.
                    .walk(-1, 0.1)
                    .idle(1.0)
                    .walk(-1, 3.0)
                    .idle(3.0)
            },
            checkpoint_every: 30,
            check: |s, c| {
                let fast = s
                    .checkpoints
                    .iter()
                    .find(|c| c.tick == 180)
                    .map(|c| c.monarch_velocity);
                let slow = s
                    .checkpoints
                    .iter()
                    .find(|c| c.tick == 780)
                    .map(|c| c.monarch_velocity);
                match (fast, slow) {
                    (Some(f), Some(sl)) => c.that(f.abs() > sl.abs() + 0.5, || {
                        format!("double tap speed {f} isn't a gallop compared with {sl}")
                    }),
                    _ => c
                        .failures
                        .push("checkpoints at ticks 180/780 missing".into()),
                }
            },
            metrics: no_metrics,
            known_issue: None,
        },
        Scenario {
            name: "island_edge",
            area: Area::Movement,
            class: Class::Internal,
            summary: "Galloping into the left edge stops at the ground's end, never past it",
            seed: 1,
            setup: |seed| island(seed, 0),
            script: || Script::new().gallop(-1, 12.0).walk(-1, 12.0).idle(2.0),
            checkpoint_every: 60,
            check: |s, c| {
                let min = s
                    .checkpoints
                    .iter()
                    .map(|c| c.monarch_x)
                    .fold(f32::INFINITY, f32::min);
                c.that(min >= worlds::LEFT - 0.01, || {
                    format!("monarch reached x {min}, past the edge at {}", worlds::LEFT)
                });
                c.that(s.game.monarch.x < worlds::LEFT + 3.0, || {
                    format!("monarch at {} never reached the edge", s.game.monarch.x)
                });
            },
            metrics: no_metrics,
            known_issue: Some(
                "nothing stops the monarch at the island's ends: on Land One (seed 1) the \
                 ground spans x -162..146 but walking reaches -857 and +700",
            ),
        },
    ]
}
