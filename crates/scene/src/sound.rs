//! What plays when: game events become one-shot cues as the game runs, and
//! each frame [`Player`] starts them, keeps the loops (ambience, fires, the
//! shore) and the soundtrack going, and places everything around the
//! listener (the camera). Backends only mix: see [`Mixer`].
//!
//! orig: `AudioEmitter` / `AudioPool` (pools, retrigger limits, random
//! clips), `AudioManager` (the soundtrack queue), `TimeBasedAudio` and
//! `AmbientManager` (the ambience beds), and the scripts that play each
//! sound.

use game::build::{pay_kind, PayKind};
use game::coins::{CoinEvent, Who};
use game::data::{AudioClipId, Role, SoundDef, SoundGroup, SoundId, WorldData};
use game::greed::{GreedEvent, Target};
use game::monarch::{Action, MonarchEvent};
use game::payment::PayEvent;
use game::state::{GameEvent, GameState};
use game::units::UnitEvent;

use crate::session::Session;
use crate::sheet::Sheet;

/// A playing voice, as the backend numbers it.
pub type Voice = u32;

/// A backend's mixer. Gains are linear; `pan` runs -1 (left) to 1 (right).
pub trait Mixer {
    fn play(&mut self, clip: AudioClipId, gain: f32, pan: f32, looping: bool) -> Option<Voice>;
    fn set(&mut self, voice: Voice, gain: f32, pan: f32);
    fn stop(&mut self, voice: Voice);
    fn playing(&self, voice: Voice) -> bool;
    /// orig: `AudioListener.pause` (the game's pause menu).
    fn pause(&mut self, paused: bool);
}

/// A one-shot sound the game asked for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cue {
    pub sound: SoundId,
    /// World x, or `None` for at the listener.
    pub x: Option<f32>,
    /// Which clip (else the sound's own pick). orig: `Play(position, clipIndex)`.
    pub clip: Option<usize>,
}

/// Soundtrack requests. orig: `AudioManager.QueueTrack` / `PlayTrack` / `FadeCurrentTrack`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MusicCue {
    Queue(SoundId),
    Play(SoundId),
    Fade,
}

/// Music volume. orig: `PrefsStore` `MusicVolume` default.
pub const MUSIC_VOLUME: f32 = 0.5;
/// Sound effect volume. orig: `SoundVolume` default.
pub const SOUND_VOLUME: f32 = 1.0;
/// orig: the `Main Mixer`'s Master group, +2.73 dB.
pub const MASTER: f32 = 1.37;

/// A sound by script field, for the prefab `owner` if it has its own.
fn on(w: &WorldData, owner: &str, field: &str) -> Option<SoundId> {
    w.audio.sound_on(owner, field)
}

fn unit_x(g: &GameState, id: u32) -> Option<f32> {
    g.population.units.iter().find(|u| u.id == id).map(|u| u.x)
}

fn troll_x(g: &GameState, id: u32) -> Option<f32> {
    g.greed.trolls.iter().find(|t| t.id == id).map(|t| t.x)
}

fn object(g: &GameState, id: u32) -> Option<&game::level::Object> {
    g.level.index_of(id).map(|i| &g.level.objects[i])
}

fn proto_name<'w>(w: &'w WorldData, g: &GameState, id: u32) -> &'w str {
    object(g, id).map_or("", |o| w.protos[o.proto as usize].name.as_str())
}

/// The cues one game event makes.
pub fn on_event(
    w: &WorldData,
    g: &GameState,
    e: &GameEvent,
    cues: &mut Vec<Cue>,
    music: &mut Vec<MusicCue>,
) {
    let monarch = Some(g.monarch.x);
    let mut cue = |sound: Option<SoundId>, x: Option<f32>| {
        if let Some(sound) = sound {
            cues.push(Cue {
                sound,
                x,
                clip: None,
            });
        }
    };
    let steed = &w.steeds[g.steed as usize].name;
    match *e {
        GameEvent::Music(t) => music.push(MusicCue::Queue(t)),
        GameEvent::Day(game::time::DayEvent::Started {
            show_counter: true, ..
        }) => cue(w.audio.sound("InterfaceOverlay.daysAppearSound"), None),
        GameEvent::Monarch(m) => match m {
            MonarchEvent::Reared => cue(on(w, steed, "Steed.rearSound"), monarch),
            MonarchEvent::BecameTired => cue(on(w, steed, "Steed.puffSound"), monarch),
            _ => {}
        },
        GameEvent::Coin(c) => match c {
            CoinEvent::HitGroundAt { x } => cue(w.audio.sound("Coin.hitGroundSound"), Some(x)),
            CoinEvent::HitGround => cue(w.audio.sound("Coin.hitGroundSound"), monarch),
            CoinEvent::PickedUp(Who::Unit(u)) => {
                cue(w.audio.sound("Wallet.pickupSound"), unit_x(g, u))
            }
            CoinEvent::PickedUp(_) => {}
        },
        GameEvent::Pay(p) => match p {
            PayEvent::DropCoin => cue(w.audio.sound("Player.coinDropSound"), None),
            PayEvent::CoinToPayable { slot, .. } => {
                // orig: Player plays the launch sound with the coin's number as clip.
                if let Some(sound) = w.audio.sound("Player.coinLaunchSound") {
                    cues.push(Cue {
                        sound,
                        x: None,
                        clip: Some(slot as usize),
                    });
                }
            }
            PayEvent::Complete { id, .. } => {
                let o = object(g, id.0);
                let x = o.map(|o| o.x).or(monarch);
                let name = proto_name(w, g, id.0);
                // The paid object may already be its next stage: an upgrade.
                let kind = o.and_then(|o| pay_kind(w, o)).unwrap_or(PayKind::Upgrade);
                let field = match kind {
                    PayKind::Upgrade => "PayableUpgrade.soundOnPay",
                    PayKind::Shop => "PayableShop.soundOnPay",
                    PayKind::MarkTree => "PayableTree.chopAcknowledgedSound",
                    PayKind::Shield => "PayableShield.soundOnPay",
                    PayKind::Border => "PayableBorder.soundOnPay",
                    PayKind::Statue => "Statue.activateSound",
                    PayKind::Steed => "Player.changeSteedSound",
                    PayKind::Cabin => "Cabin.soundOnPay",
                    PayKind::Hermit => "Player.pickupPassengerSound",
                    PayKind::Merchant => "Merchant.soundOnPay",
                    PayKind::Workshop => "PayableWorkshop.soundOnPay",
                    PayKind::Teleporter => "PayableTeleporter.soundOnPay",
                    PayKind::Signpost => "Signpost.soundOnPay",
                    PayKind::Wharf | PayKind::Beach => return,
                };
                cue(on(w, name, field), x);
            }
            _ => {}
        },
        GameEvent::Upgraded(u) => {
            let name = proto_name(w, g, u.new);
            let x = object(g, u.new).map(|o| o.x);
            if u.castle_level.is_some() {
                cue(on(w, name, "Castle.setupSound"), x);
            }
        }
        GameEvent::Built(id) => {
            let x = object(g, id).map(|o| o.x);
            cue(
                on(
                    w,
                    proto_name(w, g, id),
                    "WorkableBuilding.buildingCompleteSound",
                ),
                x,
            );
        }
        GameEvent::Unit(u) => match u {
            UnitEvent::Promoted { unit, role } => {
                let owner = match role {
                    Role::Peasant => "Peasant",
                    Role::Archer => "Archer",
                    Role::Worker => "Worker",
                    Role::Farmer => "Farmer",
                    Role::Knight => "Knight",
                    _ => return,
                };
                cue(on(w, owner, "Character.spawnSound"), unit_x(g, unit));
            }
            UnitEvent::WorkSound { x, tree } => cue(
                w.audio.sound(if tree {
                    "WorkableTree.chopSound"
                } else {
                    "Worker.buildingSound"
                }),
                Some(x),
            ),
            UnitEvent::PaidTaxes(unit) => {
                cue(w.audio.sound("Wallet.payTaxesSound"), unit_x(g, unit))
            }
            UnitEvent::Farmed(unit) => cue(w.audio.sound("Farmer.farmSound"), unit_x(g, unit)),
            UnitEvent::Strike { from_x, .. } => {
                cue(w.audio.sound("Knight.slashSound"), Some(from_x))
            }
            UnitEvent::Arrow { x, .. } => cue(w.audio.sound("Archer.shootSound"), Some(x)),
            UnitEvent::Wound(id) => {
                let name = proto_name(w, g, id);
                let field = if name.contains("Catapult") {
                    "Catapult.windingUpSound"
                } else {
                    "Ballista.windingUpSound"
                };
                cue(w.audio.sound(field), object(g, id).map(|o| o.x));
            }
            _ => {}
        },
        GameEvent::Siege(game::siege::CatapultEvent::Fired { x, .. }) => {
            cue(w.audio.sound("Catapult.fireSound"), Some(x));
        }
        GameEvent::Greed(e) => match e {
            GreedEvent::BoulderThrown { x } => cue(w.audio.sound("Boss.throwSound"), Some(x)),
            GreedEvent::Charge(t) => cue(w.audio.sound("Troll.chargeSound"), troll_x(g, t)),
            GreedEvent::BossWaveSpawned { .. } => {
                cue(w.audio.sound("EnemyManager.enemiesSpawnedSound"), None);
            }
            GreedEvent::Hit { by, target, .. } => match target {
                Target::Object(o) => cue(
                    w.audio.sound("Damageable.hitSound"),
                    object(g, o).map(|o| o.x),
                ),
                _ => cue(w.audio.sound("Damageable.hitSound"), troll_x(g, by)),
            },
            GreedEvent::TookLoot { by, .. } => {
                cue(w.audio.sound("Troll.coinPickupSound"), troll_x(g, by))
            }
            GreedEvent::PortalHit { object: o, .. } => cue(
                w.audio.sound("Portal.provokedSound"),
                object(g, o).map(|o| o.x),
            ),
            GreedEvent::PortalRebuilt(o) => cue(
                w.audio.sound("Portal.rebuildCompleteSound"),
                object(g, o).map(|o| o.x),
            ),
            _ => {}
        },
        GameEvent::BoulderBurst { x, .. } => cue(w.audio.sound("Boulder.explosionSound"), Some(x)),
        GameEvent::BoulderImpact {
            x,
            collision_sound: true,
            ..
        } => {
            cue(w.audio.sound("Boulder.collisionSound"), Some(x));
        }
        GameEvent::CrownLost => cue(w.audio.sound("Player.lostCrownSound"), None),
        GameEvent::CrownRecovered => cue(w.audio.sound("Player.pickupCrownSound"), None),
        GameEvent::GameOver => {
            music.push(MusicCue::Fade);
            cue(w.audio.sound("Game.gameOverSound"), None);
        }
        GameEvent::Won => {
            if let Some(t) = w.audio.sound("Game.victoryMusic") {
                music.push(MusicCue::Play(t));
            }
        }
        GameEvent::Harbor(h) => {
            use game::boat::HarborEvent;
            let wharf = g
                .harbor
                .wharf
                .as_ref()
                .and_then(|wh| object(g, wh.object))
                .map(|o| o.x);
            match h {
                HarborEvent::PartStacked | HarborEvent::PartPlaced => {
                    cue(w.audio.sound("Wharf.partDropSound"), wharf.or(monarch))
                }
                HarborEvent::Launched => cue(w.audio.sound("Wharf.boatLaunchSound"), wharf),
                HarborEvent::SetSail => cue(w.audio.sound("Boat.sailSound"), monarch),
                HarborEvent::Arrived => {}
            }
        }
        GameEvent::Wild(game::wildlife::WildEvent::Hearts(x)) => {
            cue(w.audio.sound("Deer.attractedToPlayerSound"), Some(x))
        }
        GameEvent::Bag(game::bag::BagEvent::Clink) => {
            cue(w.audio.sound("BagCoin.coinTouchSound"), None)
        }
        _ => {}
    }
}

/// Fields whose sound loops while its object is near. orig: each script's
/// looping emitter (fires, the shore, portals, rivers…).
const OBJECT_LOOPS: &[&str] = &[
    "Castle.fireBurningSound",
    "Castle.bustleSound",
    "Baker.ambientLoop",
    "Beach.shorelineSound",
    "Portal.loopSound",
    "River.sound",
    "PayableTeleporter.loopSound",
    "SteedSpawn.ambientLoop",
    "TeleporterRift.ambientLoop",
];

struct Loop {
    key: u64,
    sound: SoundId,
    voice: Voice,
    /// Current gain (beds fade in and out).
    gain: f32,
    seen: bool,
}

/// Plays a session's sound through a backend's mixer.
pub struct Player {
    rng: u32,
    t: f32,
    /// Per sound: voices started and when it last started.
    pools: Vec<(Vec<Voice>, f32)>,
    /// Per proto: the loops its objects carry.
    proto_loops: Vec<Vec<SoundId>>,
    loops: Vec<Loop>,
    music: Option<(Voice, SoundId)>,
    queued: Option<SoundId>,
    fading: Vec<(Voice, f32, f32)>,
    /// The monarch's footsteps: the animation state last frame and the voice.
    steps: (Action, f32, Option<Voice>),
    paused: bool,
    /// Extra gain on everything, for small speakers (the 3DS plays its
    /// limiter-driven clips at their stored level, and louder).
    pub boost: f32,
}

impl Player {
    pub fn new(w: &WorldData) -> Self {
        let proto_loops = w
            .protos
            .iter()
            .map(|p| {
                OBJECT_LOOPS
                    .iter()
                    .filter_map(|f| {
                        let mut key = p.name.clone();
                        key.push(':');
                        key.push_str(f);
                        w.audio.sound(&key)
                    })
                    .collect()
            })
            .collect();
        Player {
            rng: 0x2545_f491,
            t: 0.0,
            pools: vec![(Vec::new(), f32::NEG_INFINITY); w.audio.sounds.len()],
            proto_loops,
            loops: Vec::new(),
            music: None,
            queued: None,
            fading: Vec::new(),
            steps: (Action::Stand, 0.0, None),
            paused: false,
            boost: 1.0,
        }
    }

    fn random(&mut self, n: usize) -> usize {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng as usize) % n.max(1)
    }

    /// Gain and pan of `def` at world `x` heard from `listener`.
    fn place(&self, def: &SoundDef, x: Option<f32>, listener: f32) -> (f32, f32) {
        let group = MASTER
            * self.boost
            * match def.group {
                SoundGroup::Music => MUSIC_VOLUME,
                _ => SOUND_VOLUME,
            };
        let Some(x) = x else {
            return (def.volume * group, 0.0);
        };
        let dx = x - listener;
        let gain = def.volume * group * def.attenuation(dx.abs());
        // A gentle pan: the original's listener hears full left/right a
        // step off centre.
        let pan = def.spatial * (dx / def.max_distance.max(1.0)).clamp(-1.0, 1.0) * 0.8;
        (gain, pan)
    }

    /// orig: `AudioPool.Play`.
    fn start<M: Mixer>(
        &mut self,
        w: &WorldData,
        m: &mut M,
        cue: Cue,
        listener: f32,
    ) -> Option<Voice> {
        let def = w.audio.sounds.get(cue.sound as usize)?;
        let (voices, last) = &mut self.pools[cue.sound as usize];
        if *last + def.min_retrigger > self.t {
            return None;
        }
        voices.retain(|&v| m.playing(v));
        if voices.len() >= def.max_simultaneous as usize {
            return None;
        }
        if def.in_range_only {
            if let Some(x) = cue.x {
                if (x - listener).abs() > def.max_distance {
                    return None;
                }
            }
        }
        let (gain, pan) = self.place(def, cue.x, listener);
        if gain <= 0.001 && !def.looping {
            return None;
        }
        let n = def.clips.len();
        let pick = match cue.clip {
            Some(i) => i % n,
            None if def.random => self.random(n),
            None => 0,
        };
        let clip = def.clips[pick];
        let v = m.play(clip, gain, pan, def.looping)?;
        let (voices, last) = &mut self.pools[cue.sound as usize];
        voices.push(v);
        *last = self.t;
        Some(v)
    }

    fn start_music<M: Mixer>(&mut self, w: &WorldData, m: &mut M, track: SoundId, listener: f32) {
        if let Some((v, t)) = self.music.take() {
            self.fading.push((
                v,
                w.audio.sounds[t as usize].volume * MUSIC_VOLUME * MASTER * self.boost,
                1.0,
            ));
        }
        let cue = Cue {
            sound: track,
            x: None,
            clip: None,
        };
        self.pools[track as usize].1 = f32::NEG_INFINITY;
        self.music = self.start(w, m, cue, listener).map(|v| (v, track));
    }

    /// One frame: start what the session queued, keep loops and music going.
    pub fn update<M: Mixer>(&mut self, s: &mut Session, sheet: &Sheet, m: &mut M, dt: f32) {
        if s.paused != self.paused {
            self.paused = s.paused;
            m.pause(s.paused);
        }
        let w = &s.world;
        let listener = s.camera_x;
        if s.paused {
            s.cues.clear();
            return;
        }
        self.t += dt;
        for cue in std::mem::take(&mut s.cues) {
            self.start(w, m, cue, listener);
        }

        // The soundtrack. orig: AudioManager.Update plays the queued track
        // once the current one ends; PlayTrack fades the current one out.
        for c in std::mem::take(&mut s.music_cues) {
            match c {
                MusicCue::Queue(t) => self.queued = Some(t),
                MusicCue::Play(t) => {
                    self.queued = None;
                    self.start_music(w, m, t, listener);
                }
                MusicCue::Fade => {
                    self.queued = None;
                    if let Some((v, t)) = self.music.take() {
                        self.fading.push((
                            v,
                            w.audio.sounds[t as usize].volume * MUSIC_VOLUME * MASTER * self.boost,
                            1.0,
                        ));
                    }
                }
            }
        }
        if self.music.is_none_or(|(v, _)| !m.playing(v)) {
            self.music = None;
            if let Some(t) = self.queued.take() {
                self.start_music(w, m, t, listener);
            }
        }
        // orig: AudioEmitter.FadeOut (linear over its duration).
        self.fading.retain_mut(|(v, gain, left)| {
            *left -= dt;
            if *left <= 0.0 || !m.playing(*v) {
                m.stop(*v);
                return false;
            }
            m.set(*v, *gain * *left, 0.0);
            true
        });

        self.footsteps(s, sheet, m);
        self.update_loops(s, m, dt);
    }

    /// orig: `Steed.OnAnimWalkStart` / `OnAnimGallopStart` / `OnAnimStandStart`,
    /// animation events at the start of each walk and gallop cycle.
    fn footsteps<M: Mixer>(&mut self, s: &Session, sheet: &Sheet, m: &mut M) {
        let w = &s.world;
        let steed = &w.steeds[s.game.steed as usize];
        let (action, t) = s.anim;
        let (last_action, last_t, voice) = self.steps;
        let clip = |a: Action| {
            let state = match a {
                Action::Walk => "walk",
                Action::Run => "gallop",
                _ => return None,
            };
            sheet
                .clip(&format!("{}_{state}", steed.clips))
                .map(|c| c.length.max(0.05))
        };
        let cycle_started = match clip(action) {
            Some(len) => action != last_action || (t / len).floor() > (last_t / len).floor(),
            None => false,
        };
        let mut voice = voice;
        if cycle_started {
            let field = if action == Action::Run {
                "Steed.gallopSound"
            } else {
                "Steed.walkSound"
            };
            if let Some(sound) = on(w, &steed.name, field) {
                if action != last_action {
                    if let Some(v) = voice.take() {
                        m.stop(v);
                    }
                }
                let x = (w.audio.sounds[sound as usize].spatial > 0.0).then_some(s.game.monarch.x);
                let started = self.start(
                    w,
                    m,
                    Cue {
                        sound,
                        x,
                        clip: None,
                    },
                    s.camera_x,
                );
                voice = started.or(voice);
            }
        } else if action != last_action && clip(action).is_none() {
            if let Some(v) = voice.take() {
                m.stop(v);
            }
        }
        self.steps = (action, t, voice);
    }

    /// Ambience beds and object loops: started, placed, faded, stopped.
    fn update_loops<M: Mixer>(&mut self, s: &Session, m: &mut M, dt: f32) {
        let w = &s.world;
        let g = &s.game;
        let listener = s.camera_x;
        for l in &mut self.loops {
            l.seen = false;
        }
        let mut want: Vec<(u64, SoundId, f32, f32)> = Vec::new();

        // orig: TimeBasedAudio in the AmbientManager's emitters, mixed by
        // region (forest or open ground) and the weather.
        let forest = forest_weight(g, listener);
        let season = g.season as u8;
        let rain = g.blender.state.floats[6];
        for (i, def) in w.audio.sounds.iter().enumerate() {
            let Some((start, end, seasons)) = def.schedule else {
                continue;
            };
            if def.group != SoundGroup::Ambient || seasons & season == 0 {
                continue;
            }
            let now = g.clock.time;
            let on = start == end
                || if start < end {
                    now >= start && now < end
                } else {
                    now >= start || now < end
                };
            if !on {
                continue;
            }
            let name = def.name.as_str();
            let region = if name.contains("Rain") {
                rain
            } else if name.contains("Wind") {
                // No wind yet: the original's calm-weather level.
                0.3
            } else if name.contains("Forest") {
                forest
            } else if name.contains("Meadow") {
                1.0 - forest
            } else {
                1.0
            };
            if region > 0.001 {
                want.push((
                    1 << 40 | i as u64,
                    i as SoundId,
                    def.volume * SOUND_VOLUME * MASTER * self.boost * region,
                    0.0,
                ));
            }
        }

        // Loops on objects near the listener.
        let mut near = |o: &game::level::Object| {
            for (k, &sound) in self.proto_loops[o.proto as usize].iter().enumerate() {
                let def = &w.audio.sounds[sound as usize];
                if (o.x - listener).abs() > def.max_distance + 1.0 {
                    continue;
                }
                let (gain, pan) = self.place(def, Some(o.x), listener);
                want.push(((o.id as u64) << 4 | k as u64, sound, gain, pan));
            }
        };
        for o in g.level.objects.iter().chain(&g.level.scenery) {
            near(o);
        }

        for (key, sound, gain, pan) in want {
            match self.loops.iter_mut().find(|l| l.key == key) {
                Some(l) => {
                    l.seen = true;
                    // orig: TimeBasedAudio fades over 2 s; object loops just follow.
                    l.gain = if key >> 40 == 1 {
                        let rate = w.audio.sounds[sound as usize].volume
                            * SOUND_VOLUME
                            * MASTER
                            * self.boost
                            / 2.0;
                        approach(l.gain, gain, rate * dt)
                    } else {
                        gain
                    };
                    m.set(l.voice, l.gain, pan);
                }
                None => {
                    let def = &w.audio.sounds[sound as usize];
                    let n = def.clips.len();
                    let clip = def.clips[if def.random { self.random(n) } else { 0 }];
                    // Beds fade in; object loops start at their level.
                    let start = if key >> 40 == 1 { 0.0 } else { gain };
                    if let Some(voice) = m.play(clip, start, pan, true) {
                        self.loops.push(Loop {
                            key,
                            sound,
                            voice,
                            gain: start,
                            seen: true,
                        });
                    }
                }
            }
        }
        // Gone or out of range: beds fade out, object loops stop.
        self.loops.retain_mut(|l| {
            if l.seen {
                return true;
            }
            if l.key >> 40 == 1 && l.gain > 0.001 {
                let rate =
                    w.audio.sounds[l.sound as usize].volume * SOUND_VOLUME * MASTER * self.boost
                        / 2.0;
                l.gain = approach(l.gain, 0.0, rate * dt);
                m.set(l.voice, l.gain, 0.0);
                return true;
            }
            m.stop(l.voice);
            false
        });
    }
}

/// Moves `v` toward `to` by at most `step`.
fn approach(v: f32, to: f32, step: f32) -> f32 {
    if (to - v).abs() <= step {
        to
    } else {
        v + step * (to - v).signum()
    }
}

/// How far into a forest the listener is, 0..1 over its 8-unit edges.
/// orig: `AmbientRegion` (Forest) weights.
fn forest_weight(g: &GameState, x: f32) -> f32 {
    g.forests
        .iter()
        .filter(|(a, b)| *a < x && x < *b)
        .map(|(a, b)| ((b - x).min(x - a) / 8.0).min(1.0))
        .fold(0.0, f32::max)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn test_world() -> WorldData {
        use game::data::{DayDef, DirectorDef, LevelConfigDef};
        WorldData {
            days: vec![DayDef {
                name: "day".into(),
                seasons: 7,
                weathers: 1,
                tracks: vec![],
            }],
            director: DirectorDef {
                regular_days: vec![0],
                boss_days: vec![0],
                recovery_days: vec![0],
                num_regular_days: (3, 4),
                ..Default::default()
            },
            levels: vec![LevelConfigDef {
                name: "island".into(),
                autumn_start: (40, 40),
                winter_start: (60, 60),
                days_before_game_over: -1,
                starting_coins: 0,
                min_level_width: 0,
                group_counts: vec![],
                waves: Default::default(),
                wave_timed_multiplier: 1.0,
                wave_multiplier_timeframe: (0, -1),
                season_change_days: 3.0,
                income_multiplier: 1.0,
                free_boat_parts: 0,
                wave_boat_multiplier: 1.0,
                force_all_unlocks: false,
                defence_waves: [None; 3],
                retaliation_waves: [None; 3],
            }],
            steeds: vec![Default::default()],
            ..Default::default()
        }
    }

    #[derive(Default)]
    struct MixerLog {
        plays: Vec<(AudioClipId, f32, f32)>,
        failed: Option<Voice>,
    }
    impl Mixer for MixerLog {
        fn play(&mut self, clip: AudioClipId, gain: f32, pan: f32, _: bool) -> Option<Voice> {
            self.plays.push((clip, gain, pan));
            Some(self.plays.len() as Voice)
        }
        fn set(&mut self, _: Voice, _: f32, _: f32) {}
        fn stop(&mut self, _: Voice) {}
        fn playing(&self, voice: Voice) -> bool {
            self.failed != Some(voice)
        }
        fn pause(&mut self, _: bool) {}
    }

    #[test]
    fn coin_impact_uses_event_position_for_symmetric_pan_and_distant_silence() {
        let mut world = test_world();
        world.audio.refs.push(("Coin.hitGroundSound".into(), 0));
        world.audio.sounds.push(SoundDef {
            clips: vec![0],
            volume: 1.0,
            spatial: 1.0,
            min_distance: 1.0,
            max_distance: 10.0,
            max_simultaneous: 8,
            ..Default::default()
        });
        let mut game = GameState::new(&world, 0, 1);
        game.monarch.x = 0.0;
        let mut player = Player::new(&world);
        let mut mixer = MixerLog::default();
        for x in [-30.0, -5.0, 5.0, 30.0] {
            game.coins.drop(
                Who::Unit(9),
                (x, 0.01),
                (0.0, -1.0),
                game::coins::Policy::Nobody,
                None,
                None,
            );
            let mut events = Vec::new();
            game.coins
                .update(game::state::TICK, &[], |e| events.push(GameEvent::Coin(e)));
            game.coins.list.clear(); // no entity lookup can supply its position now
            let mut cues = Vec::new();
            for event in events {
                on_event(&world, &game, &event, &mut cues, &mut Vec::new());
            }
            assert_eq!(cues.len(), 1);
            assert_eq!(cues[0].x, Some(x));
            player.start(&world, &mut mixer, cues[0], 0.0);
        }
        assert_eq!(mixer.plays.len(), 2);
        assert!(mixer.plays[0].1 > 0.0);
        assert_eq!(mixer.plays[0].1, mixer.plays[1].1);
        assert_eq!(mixer.plays[0].2, -mixer.plays[1].2);
        assert!(mixer.plays[0].2 < 0.0);
    }

    #[test]
    fn terminal_stream_failure_allows_the_next_queued_music_track() {
        use crate::{
            adpcm::{Adp, Channel},
            stream::Source,
        };
        use std::io::Cursor;
        let mut world = test_world();
        world.audio.sounds = (0..2)
            .map(|clip| SoundDef {
                clips: vec![clip],
                volume: 1.0,
                max_simultaneous: 1,
                group: SoundGroup::Music,
                ..Default::default()
            })
            .collect();
        let mut s = Session::new(world, 0, 1, game::progress::Progress::everything());
        let mut player = Player::new(&s.world);
        let mut mixer = MixerLog::default();
        s.music_cues.push(MusicCue::Play(0));
        player.update(&mut s, &Sheet::default(), &mut mixer, game::state::TICK);
        s.music_cues.push(MusicCue::Queue(1));
        let bytes = Adp {
            rate: 22050,
            samples: 14,
            gain: 1.0,
            channels: vec![Channel {
                coefs: [0; 16],
                data: vec![0; 8],
            }],
        }
        .to_bytes();
        let mut stream = Source::open(Cursor::new(bytes)).unwrap();
        let (mut left, mut right) = ([0; 8], [0; 8]);
        assert!(stream
            .fill_pair([&mut left, &mut right], 14, |_, _| Err(
                std::io::Error::other("injected DSP preparation failure")
            ))
            .is_err());
        assert!(!stream.playing(true));
        mixer.failed = Some(1); // the backend reports the terminal source as stopped
        player.update(&mut s, &Sheet::default(), &mut mixer, game::state::TICK);
        assert_eq!(mixer.plays.iter().map(|p| p.0).collect::<Vec<_>>(), [0, 1]);
    }

    #[test]
    fn positioned_worker_strokes_fade_out_at_the_emitters_range() {
        let player = Player::new(&WorldData::default());
        let def = SoundDef {
            volume: 0.351,
            spatial: 1.0,
            min_distance: 7.0,
            max_distance: 11.0,
            rolloff: game::data::Rolloff::Custom(vec![(0.18, 1.0), (0.59, 0.5), (1.0, 0.0)]),
            ..Default::default()
        };
        assert!(player.place(&def, Some(30.0), 30.0).0 > 0.0);
        assert_eq!(player.place(&def, Some(30.0), -30.0).0, 0.0);
        assert_eq!(player.place(&def, Some(-30.0), 30.0).0, 0.0);
    }
}
