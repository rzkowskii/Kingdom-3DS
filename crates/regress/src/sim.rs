//! The headless runner: one island's [`GameState`] driven by an input
//! script exactly the way `scene::session::Session::step_ticks` drives it,
//! with an event log, invariant checks and checkpoints.
//!
//! Simulation time is `tick × TICK` and nothing here reads a clock while
//! simulating.

use std::collections::BTreeMap;

use game::coins::{Motion, MAX_COINS};
use game::controls::{ControlMapper, RawInput};
use game::data::WorldData;
use game::payment::PayEvent;
use game::state::{GameEvent, GameState, TICK};

use crate::script::Script;
use crate::snapshot::{fnv64, Snapshot};
use crate::tod::LightSample;

/// The event log: every [`GameEvent`] in emission order, hashed as it grows.
#[derive(Clone, Debug, Default)]
pub struct EventLog {
    /// `tick kind detail` lines. Kept up to `keep` lines; the hash and
    /// counts always cover everything.
    pub lines: Vec<String>,
    pub keep: usize,
    pub total: u64,
    hash: u64,
    /// How many of each kind (`Coin.PickedUp`, `Built`, …).
    pub kinds: BTreeMap<String, u32>,
    /// The tick each kind first happened on.
    pub first: BTreeMap<String, u32>,
}

/// `Day(Started { day: 1 })` → `Day.Started`; `Built(3)` → `Built`.
pub fn kind_of(debug: &str) -> String {
    let end = |s: &str| s.find(['(', ' ', '{', ')', ',']).unwrap_or(s.len());
    let outer = &debug[..end(debug)];
    match debug[outer.len()..].strip_prefix('(') {
        // `Upgraded(Upgraded { .. })` wraps a struct of the same name.
        Some(rest)
            if rest.starts_with(|c: char| c.is_ascii_uppercase())
                && &rest[..end(rest)] != outer =>
        {
            format!("{outer}.{}", &rest[..end(rest)])
        }
        _ => outer.to_string(),
    }
}

impl EventLog {
    pub fn new(keep: usize) -> Self {
        Self {
            keep,
            hash: 0xcbf2_9ce4_8422_2325,
            ..Default::default()
        }
    }

    pub fn push(&mut self, tick: u32, e: &GameEvent) {
        // Contact position affects audio playback, so keep the existing
        // simulation event identity and replay hash for coin impacts.
        let detail = match e {
            GameEvent::Coin(game::coins::CoinEvent::HitGroundAt { .. }) => "Coin(HitGround)".into(),
            _ => format!("{e:?}"),
        };
        let kind = kind_of(&detail);
        let line = format!("{tick} {kind} {detail}");
        for b in line.bytes().chain(std::iter::once(b'\n')) {
            self.hash ^= b as u64;
            self.hash = self.hash.wrapping_mul(0x0100_0000_01b3);
        }
        self.total += 1;
        *self.kinds.entry(kind.clone()).or_default() += 1;
        self.first.entry(kind).or_insert(tick);
        if self.lines.len() < self.keep {
            self.lines.push(line);
        }
    }

    pub fn hash(&self) -> u64 {
        self.hash
    }

    pub fn count(&self, kind: &str) -> u32 {
        self.kinds.get(kind).copied().unwrap_or(0)
    }
}

/// A coin in the world that belongs to nobody yet: not spent and fading,
/// and not being sucked into a purse (that coin was credited to its new
/// owner the moment the grab began).
pub fn is_loose(c: &game::coins::Coin) -> bool {
    !matches!(c.motion, Motion::Fading { .. } | Motion::Grabbed { .. })
}

/// Coins in play: the purse, loose coins in the world, and citizens' purses.
pub fn coins_in_play(g: &GameState) -> u64 {
    let live = g.coins.list.iter().filter(|c| is_loose(c)).count() as u64;
    let held: u64 = g.population.units.iter().map(|u| u.coins as u64).sum();
    g.wallet.coins() as u64 + live + held
}

/// Coin conservation for scenarios with no income: coins in play plus the
/// coins that left play stays at the starting total.
///
/// Coins leave play by fading out. Each fade is classified the tick it
/// starts: payment spends fade over 0.2 s (`coins::SPENT_FADE`) and are set
/// before the coins step, so they're first seen at `t == TICK`; coins that
/// fall from the purse into the water and loose coins expiring after 120 s
/// fade over 1 s and are first seen at `t == 0`. If that ordering ever
/// changes, conservation fails loudly rather than silently passing, and
/// `spent` is cross-checked against the `Pay.Complete` events.
#[derive(Clone, Debug, Default)]
pub struct Ledger {
    pub start: u64,
    /// Spent on payables (seen fading).
    pub spent: u64,
    /// Fell in the water or expired on the ground.
    pub faded: u64,
    /// Sum of `Pay.Complete { coins }`.
    pub paid: u64,
}

impl Ledger {
    fn observe(&mut self, g: &GameState) {
        for c in &g.coins.list {
            if let Motion::Fading { t, duration } = c.motion {
                if duration < 0.5 && t > 0.0 && t < TICK * 1.5 {
                    self.spent += 1;
                } else if duration >= 0.5 && t == 0.0 {
                    self.faded += 1;
                }
            }
        }
    }
}

pub struct Sim {
    pub world: WorldData,
    pub game: GameState,
    mapper: ControlMapper,
    /// Ticks simulated so far.
    pub tick: u32,
    pub log: EventLog,
    pub ledger: Option<Ledger>,
    /// Invariant failures, `(tick, what)`; at most [`Sim::MAX_VIOLATIONS`].
    pub violations: Vec<(u32, String)>,
    /// Snapshots taken every `checkpoint_every` ticks.
    pub checkpoints: Vec<Snapshot>,
    pub checkpoint_every: Option<u32>,
    /// What was fed in, tick by tick (the recorder).
    pub recorded: Script,
    /// Record the lighting into [`Sim::lights`] every this many ticks.
    pub light_every: Option<u32>,
    pub lights: Vec<LightSample>,
    events: Vec<GameEvent>,
}

impl Sim {
    pub const MAX_VIOLATIONS: usize = 20;

    pub fn new(world: WorldData, game: GameState) -> Self {
        Self {
            world,
            game,
            mapper: ControlMapper::default(),
            tick: 0,
            log: EventLog::new(200_000),
            ledger: None,
            violations: Vec::new(),
            checkpoints: Vec::new(),
            checkpoint_every: None,
            recorded: Script::new(),
            light_every: None,
            lights: Vec::new(),
            events: Vec::new(),
        }
    }

    /// Declares the economy closed: no coins may appear or vanish from here on.
    pub fn closed_economy(&mut self) {
        self.ledger = Some(Ledger {
            start: coins_in_play(&self.game),
            ..Default::default()
        });
    }

    pub fn seconds(&self) -> f32 {
        self.tick as f32 * TICK
    }

    fn violate(&mut self, what: String) {
        if self.violations.len() < Self::MAX_VIOLATIONS {
            self.violations.push((self.tick, what));
        }
    }

    /// One tick with already-mapped input.
    fn tick_once(&mut self, raw: RawInput) {
        let controls = self.mapper.map(TICK, raw);
        self.events.clear();
        self.game.tick(&self.world, &controls, &mut self.events);
        self.tick += 1;
        let events = std::mem::take(&mut self.events);
        for e in &events {
            self.log.push(self.tick, e);
            if let (Some(l), GameEvent::Pay(PayEvent::Complete { coins, .. })) =
                (&mut self.ledger, e)
            {
                l.paid += *coins as u64;
            }
        }
        self.events = events;
        if let Some(l) = &mut self.ledger {
            l.observe(&self.game);
        }
        self.check_invariants();
        if let Some(n) = self.light_every {
            if n > 0 && self.tick.is_multiple_of(n) {
                self.lights.push(LightSample::of(self.tick, &self.game));
            }
        }
        if let Some(n) = self.checkpoint_every {
            if n > 0 && self.tick.is_multiple_of(n) {
                self.checkpoints.push(Snapshot::of(self));
            }
        }
    }

    /// One frame: `ticks` ticks on one input sample, as `Session::step_ticks`
    /// does it (the bag tap only counts on the first).
    pub fn frame(&mut self, raw: RawInput, ticks: u32) {
        let mut raw = raw;
        for _ in 0..ticks {
            self.recorded.push(raw, 1);
            self.tick_once(raw);
            raw.bag_tap = false;
        }
    }

    /// Plays `script`, sampling input once per frame of `ticks_per_frame`.
    pub fn run(&mut self, script: &Script, ticks_per_frame: u32) {
        let inputs: Vec<RawInput> = script.inputs().collect();
        self.run_inputs(&inputs, ticks_per_frame);
    }

    /// Plays per-tick inputs, sampling once per frame of `ticks_per_frame`.
    pub fn run_inputs(&mut self, inputs: &[RawInput], ticks_per_frame: u32) {
        for chunk in inputs.chunks(ticks_per_frame.max(1) as usize) {
            self.frame(chunk[0], chunk.len() as u32);
        }
    }

    /// Runs with no input for `seconds`.
    pub fn idle(&mut self, seconds: f32) {
        self.run(&Script::new().idle(seconds), 1);
    }

    fn check_invariants(&mut self) {
        let g = &self.game;
        let mut bad = Vec::new();
        if !(g.monarch.x.is_finite() && g.monarch.velocity.is_finite()) {
            bad.push(format!(
                "monarch not finite: x {} v {}",
                g.monarch.x, g.monarch.velocity
            ));
        }
        if let Some(u) = g.population.units.iter().find(|u| !u.x.is_finite()) {
            bad.push(format!("unit {} ({:?}) x not finite", u.id, u.role));
        }
        if let Some(c) = g
            .coins
            .list
            .iter()
            .find(|c| !(c.x.is_finite() && c.y.is_finite()))
        {
            bad.push(format!("coin not finite at ({}, {})", c.x, c.y));
        }
        if g.wallet.coins() > MAX_COINS {
            bad.push(format!("wallet {} over {MAX_COINS}", g.wallet.coins()));
        }
        if !g.clock.time.is_finite() || !(0.0..24.0).contains(&g.clock.time) {
            bad.push(format!("clock time {} out of [0, 24)", g.clock.time));
        }
        if let Some(l) = &self.ledger {
            let now = coins_in_play(g);
            if now + l.spent + l.faded != l.start {
                bad.push(format!(
                    "coins not conserved: {now} in play + {} spent + {} faded != {} at start",
                    l.spent, l.faded, l.start
                ));
            }
        }
        for b in bad {
            self.violate(b);
        }
    }

    pub fn state_hash(&self) -> u64 {
        state_hash(&self.game)
    }

    /// End-of-run ledger checks (payment events lag the spend by a tick, so
    /// this one can't run every tick).
    pub fn finish(&mut self) {
        if let Some(l) = self.ledger.clone() {
            if l.spent != l.paid {
                self.violate(format!(
                    "{} coins seen spent but Pay.Complete paid {}",
                    l.spent, l.paid
                ));
            }
        }
    }

    /// Scaffolds not finished yet.
    pub fn final_under_construction(&self) -> usize {
        self.game
            .level
            .objects
            .iter()
            .filter(|o| game::build::under_construction(&self.world, o))
            .count()
    }
}

/// FNV-1a 64 of the save encoding of the whole state: equal hashes mean
/// equal states, down to the RNG.
pub fn state_hash(g: &GameState) -> u64 {
    fnv64(&game::save::save_game(g, 0))
}
