//! Recorded runs on a real island: the desktop writes one with `--record`,
//! `regress replay` plays it back through the same `Session` frame loop and
//! checks it ends in the same state. A replay holds only inputs, a seed and
//! the world fingerprint, never game data; it needs the user's `world.bin`.

use game::controls::RawInput;
use game::data::WorldData;
use game::progress::Progress;
use scene::session::Session;
use serde::{Deserialize, Serialize};

use crate::meta::RunMeta;
use crate::sim::state_hash;

/// Bumped when the replay layout changes.
pub const REPLAY_VERSION: u32 = 1;

/// `repeat` frames in a row of `ticks` ticks each with the same buttons.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frames {
    pub repeat: u32,
    pub ticks: u32,
    #[serde(default)]
    pub dir: i8,
    #[serde(default)]
    pub sprint: bool,
    #[serde(default)]
    pub pay: bool,
    #[serde(default)]
    pub tap: bool,
}

impl Frames {
    fn raw(&self) -> RawInput {
        RawInput {
            direction: self.dir,
            sprint: self.sprint,
            pay: self.pay,
            bag_tap: self.tap,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Replay {
    pub version: u32,
    pub meta: RunMeta,
    /// `save::fingerprint` of the `world.bin` it was recorded on.
    pub world_fingerprint: u32,
    /// The unlocks the run started with (`save::save_progress`, hex).
    pub progress: String,
    #[serde(default)]
    pub start_hour: Option<f32>,
    /// Why this replay may not reproduce (dev cheats, menus that changed
    /// the state outside the simulation). Empty for a clean recording.
    #[serde(default)]
    pub tainted: Vec<String>,
    /// The state when recording stopped.
    pub final_tick: u32,
    pub final_state_hash: String,
    #[serde(rename = "frames")]
    pub frames: Vec<Frames>,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Records a desktop session frame by frame.
pub struct Recorder {
    pub replay: Replay,
    ticks: u32,
}

impl Recorder {
    pub fn new(
        fixture: &str,
        world_fingerprint: u32,
        island: usize,
        seed: u32,
        progress: &Progress,
        start_hour: Option<f32>,
    ) -> Self {
        Self {
            replay: Replay {
                version: REPLAY_VERSION,
                meta: RunMeta::new(fixture, &format!("{world_fingerprint:08x}"), island, seed),
                world_fingerprint,
                progress: hex(&game::save::save_progress(progress)),
                start_hour,
                tainted: Vec::new(),
                final_tick: 0,
                final_state_hash: String::new(),
                frames: Vec::new(),
            },
            ticks: 0,
        }
    }

    /// Call right before `Session::step_ticks(raw, ticks)` when it will
    /// advance the game (not paused).
    pub fn frame(&mut self, raw: RawInput, ticks: u32) {
        if ticks == 0 {
            return;
        }
        self.ticks += ticks;
        let f = Frames {
            repeat: 1,
            ticks,
            dir: raw.direction,
            sprint: raw.sprint,
            pay: raw.pay,
            tap: raw.bag_tap,
        };
        if let Some(last) = self.replay.frames.last_mut() {
            if (Frames { repeat: 1, ..*last }) == f {
                last.repeat += 1;
                return;
            }
        }
        self.replay.frames.push(f);
    }

    /// Notes something that changed the state outside the simulation.
    pub fn taint(&mut self, why: impl Into<String>) {
        let why = why.into();
        if !self.replay.tainted.contains(&why) {
            self.replay.tainted.push(why);
        }
    }

    pub fn finish(mut self, session: &Session) -> Replay {
        self.replay.final_tick = self.ticks;
        self.replay.meta.ticks = self.ticks;
        self.replay.final_state_hash = format!("{:016x}", state_hash(&session.game));
        self.replay
    }
}

impl Replay {
    pub fn to_toml(&self) -> String {
        let mut s = String::from(
            "# Kingdom 3DS input replay. Holds inputs only; it needs the same world.bin.\n",
        );
        s.push_str(&toml::to_string(self).expect("replay serializes"));
        s
    }

    pub fn from_toml(s: &str) -> Result<Self, String> {
        let r: Replay = toml::from_str(s).map_err(|e| e.to_string())?;
        if r.version != REPLAY_VERSION {
            return Err(format!(
                "replay version {} (this build reads {REPLAY_VERSION})",
                r.version
            ));
        }
        Ok(r)
    }

    /// Plays the replay through a fresh `Session`, the same frame loop the
    /// backends use. Returns the session at the end.
    pub fn play(&self, world: WorldData) -> Result<Session, String> {
        let progress = unhex(&self.progress)
            .and_then(|b| game::save::load_progress(&b).ok())
            .ok_or("the replay's progress doesn't decode")?;
        let mut s = Session::new(world, self.meta.island, self.meta.seed, progress);
        if let Some(h) = self.start_hour {
            s.game.clock.time = h;
        }
        for f in &self.frames {
            for _ in 0..f.repeat {
                s.step_ticks(f.raw(), f.ticks);
            }
        }
        Ok(s)
    }
}
