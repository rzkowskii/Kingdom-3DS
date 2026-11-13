//! The scenario catalogue. Add new ones to [`all`].

mod construction;
mod economy;
mod movement;
mod time;

use crate::scenario::Scenario;
use crate::sim::Sim;
use crate::worlds;

pub fn all() -> Vec<Scenario> {
    let mut v = Vec::new();
    v.extend(economy::scenarios());
    v.extend(movement::scenarios());
    v.extend(construction::scenarios());
    v.extend(time::scenarios());
    v
}

pub fn find(name: &str) -> Option<Scenario> {
    all().into_iter().find(|s| s.name == name)
}

/// A synthetic island with `coins` in the purse, ready to run.
pub(crate) fn island(seed: u32, coins: u32) -> Sim {
    let world = worlds::world(0);
    let game = worlds::island(&world, seed, coins);
    Sim::new(world, game)
}
