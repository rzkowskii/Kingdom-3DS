//! A simple scripted player for screenshots, headless runs and soak tests
//! (not part of the original game).
//!
//! It hands coins to beggars near the kingdom, then pays for build sites
//! and shop items, preferring the names in `prefer` and sites close to the
//! campfire, and otherwise waits at the campfire to collect taxes.

use alloc::string::String;
use alloc::vec::Vec;

use crate::controls::RawInput;
use crate::data::{Role, WorldData};
use crate::payable::Payable;
use crate::state::GameState;

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default)]
pub struct Autopilot {
    /// Payable name prefixes, most wanted first; others come after.
    pub prefer: Vec<String>,
    /// From frame `.1` on, walk to x `.0` and stay there.
    pub goto: Option<(f32, u32)>,
}

impl Autopilot {
    pub fn input(&self, g: &GameState, world: &WorldData, frame: u32) -> RawInput {
        let x = g.monarch.x;
        let walk_to = |t: f32| RawInput {
            direction: if t > x { 1 } else { -1 },
            ..Default::default()
        };
        if let Some((gx, from)) = self.goto {
            if frame >= from {
                return if (gx - x).abs() > 0.3 {
                    walk_to(gx)
                } else {
                    RawInput::default()
                };
            }
        }
        let beggar = g
            .population
            .units
            .iter()
            .filter(|u| u.role == Role::Beggar && (u.x - g.campfire).abs() < 60.0)
            .map(|u| u.x)
            .min_by(|a, b| (a - x).abs().total_cmp(&(b - x).abs()));
        let name = |p: &Payable| {
            g.level
                .index_of(p.id.0)
                .map(|i| {
                    world.protos[g.level.objects[i].proto as usize]
                        .name
                        .as_str()
                })
                .unwrap_or("")
        };
        let rank = |p: &Payable| {
            self.prefer
                .iter()
                .position(|n| name(p).starts_with(n.as_str()))
                .unwrap_or(self.prefer.len())
        };
        let site = g
            .payables
            .iter()
            .filter(|p| p.can_pay && p.price as u32 <= g.wallet.coins())
            .filter(|p| {
                g.level.index_of(p.id.0).is_some_and(|i| {
                    world.protos[g.level.objects[i].proto as usize]
                        .tree_work
                        .is_none()
                })
            })
            // Ties: closest to the campfire, as a player builds outward.
            .min_by(|a, b| {
                rank(a).cmp(&rank(b)).then(
                    (a.x - g.campfire)
                        .abs()
                        .total_cmp(&(b.x - g.campfire).abs()),
                )
            });
        // A preferred site comes before recruiting, unless tools wait on racks
        // with nobody to take them.
        let waiting = g.population.tools.iter().filter(|t| t.on_rack()).count();
        let takers = g.population.count(Role::Peasant);
        let preferred = site.is_some_and(|p| rank(p) < self.prefer.len()) && waiting <= takers;
        let site = site.map(|p| p.x);
        let near_beggar = beggar.filter(|_| g.wallet.coins() > 2 && !preferred);
        let taxpayer = g
            .population
            .units
            .iter()
            .filter(|u| u.coins >= 3 && (u.x - g.campfire).abs() < 60.0)
            .max_by_key(|u| u.coins)
            .map(|u| u.x);
        match (near_beggar, site) {
            (Some(b), _) if (b - x).abs() > 1.0 => walk_to(b),
            (Some(_), _) => RawInput {
                bag_tap: frame.is_multiple_of(60),
                ..Default::default()
            },
            (None, Some(t)) if (t - x).abs() > 0.3 => walk_to(t),
            // Release A between purchases: each one needs a fresh press.
            (None, Some(_)) => RawInput {
                pay: frame % 150 < 130,
                ..Default::default()
            },
            // Collect taxes from whoever carries the most.
            _ if taxpayer.is_some() => {
                let t = taxpayer.unwrap_or(x);
                if (t - x).abs() > 0.3 {
                    walk_to(t)
                } else {
                    RawInput::default()
                }
            }
            _ if (g.campfire - x).abs() > 2.0 => walk_to(g.campfire),
            _ => RawInput::default(),
        }
    }
}
