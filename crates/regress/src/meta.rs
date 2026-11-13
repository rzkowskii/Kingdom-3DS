//! What a run was: enough to reproduce it and to know where it came from.

use serde::{Deserialize, Serialize};

/// Bumped when the metadata layout changes.
pub const META_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunMeta {
    pub schema: u32,
    /// `git rev-parse --short HEAD` at build time, `-dirty` if modified.
    pub commit: String,
    /// The crate version.
    pub build: String,
    /// `debug` or `release`.
    pub profile: String,
    /// The target triple the runner was built for.
    pub platform: String,
    /// Scenario name, or the replay file.
    pub fixture: String,
    /// `synthetic` (built in code) or the world.bin fingerprint.
    pub world: String,
    pub island: usize,
    pub seed: u32,
    pub ticks_per_frame: u32,
    pub ticks: u32,
    /// `state::TICK` in seconds.
    pub tick_seconds: f32,
}

impl RunMeta {
    pub fn new(fixture: &str, world: &str, island: usize, seed: u32) -> Self {
        Self {
            schema: META_VERSION,
            commit: env!("REGRESS_COMMIT").to_string(),
            build: env!("CARGO_PKG_VERSION").to_string(),
            profile: env!("REGRESS_PROFILE").to_string(),
            platform: env!("REGRESS_TARGET").to_string(),
            fixture: fixture.to_string(),
            world: world.to_string(),
            island,
            seed,
            ticks_per_frame: 1,
            ticks: 0,
            tick_seconds: game::state::TICK,
        }
    }
}
