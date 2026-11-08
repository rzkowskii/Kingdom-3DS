//! Runtime fire activation and point-light samples on the fixed scene clock.

use std::collections::BTreeMap;

use game::data::{sample_keys, FireDef, FireStyleDef, ProtoId, WorldData};
use game::level::Object;
use game::rng::Rng;
use game::state::{GameState, TICK};

const NIGHT_DELAY: f32 = 5.0;
const LIGHT_WAIT: f32 = 1.0;
const LIGHT_RAMP: f32 = 1.0;
const LIGHT_POLL_TICKS: u64 = 9; // Fire.SlowUpdate waits .15 s.

#[derive(Clone, Copy)]
pub(crate) struct FireSample {
    pub elapsed: f32,
    pub strength: f32,
    pub intensity: f32,
}

#[derive(Default)]
struct Fire {
    strength: f32,
    base_intensity: f32,
    intensity: f32,
    bonus: f32,
    started: Option<u64>,
    next_poll: u64,
    /// Fire's Invoke calls each have their own deadline and can coexist.
    pending: Vec<(u64, bool)>,
}

impl Fire {
    fn new(def: &FireDef, style: &FireStyleDef) -> Self {
        let strength = sample_keys(&style.strength, def.size.0.hypot(def.size.1));
        Self {
            strength,
            base_intensity: sample_keys(&style.intensity, strength),
            ..Self::default()
        }
    }

    fn start(&mut self, tick: u64) {
        if self.started.is_none() {
            self.next_poll = tick;
        }
        self.started = Some(tick);
        self.intensity = 0.0;
    }

    fn stop(&mut self) {
        self.started = None;
        self.intensity = 0.0;
    }

    fn set_active(&mut self, active: bool, tick: u64) {
        match (active, self.started.is_some()) {
            (true, false) => self.start(tick),
            (false, true) => self.stop(),
            _ => {}
        }
    }

    fn schedule(&mut self, tick: u64, start: bool, rng: &mut Rng) {
        let delay = (rng.range(0.0, NIGHT_DELAY) / TICK).ceil() as u64;
        let deadline = tick + delay;
        let at = self.pending.partition_point(|&(t, _)| t <= deadline);
        self.pending.insert(at, (deadline, start));
    }

    fn step(&mut self, tick: u64, ambient: f32, rng: &mut Rng) {
        while self.pending.first().is_some_and(|&(at, _)| at <= tick) {
            let (_, start) = self.pending.remove(0);
            if start {
                self.start(tick);
            } else {
                self.stop();
            }
        }
        if let Some(started) = self.started.filter(|_| tick >= self.next_poll) {
            self.next_poll = tick + LIGHT_POLL_TICKS;
            self.bonus = (self.bonus + rng.range(-0.3, 0.3)) * 0.8;
            let elapsed = (tick - started) as f32 * TICK;
            let fade = ((elapsed - LIGHT_WAIT) / LIGHT_RAMP).clamp(0.0, 1.0);
            self.intensity = (self.base_intensity + self.bonus * ambient) * fade;
        }
    }

    fn sample(&self, tick: u64) -> Option<FireSample> {
        self.started.map(|started| FireSample {
            elapsed: (tick - started) as f32 * TICK,
            strength: self.strength,
            intensity: self.intensity,
        })
    }
}

struct ObjectFires {
    proto: ProtoId,
    attachments: Vec<Fire>,
    tick: u64,
}

impl ObjectFires {
    fn new(o: &Object, world: &WorldData, daytime: bool, tick: u64) -> Self {
        let attachments = world.protos[usize::from(o.proto)]
            .fires
            .iter()
            .map(|def| {
                let mut fire = Fire::new(def, &world.fire);
                if def.auto_start && (!def.night_only || !daytime) {
                    fire.start(tick);
                }
                fire
            })
            .collect();
        Self {
            proto: o.proto,
            attachments,
            tick,
        }
    }

    fn step(
        &mut self,
        defs: &[FireDef],
        tick: u64,
        transition: Option<bool>,
        charged: bool,
        ambient: f32,
        rng: &mut Rng,
    ) {
        // New objects take Fire.Start's initial branch rather than receiving
        // a transition dispatched before their handlers were registered.
        let transition = transition.filter(|_| self.tick != tick);
        self.tick = tick;
        for (fire, def) in self.attachments.iter_mut().zip(defs) {
            if def.night_only {
                if let Some(daytime) = transition {
                    fire.schedule(tick, !daytime, rng);
                }
            } else if !def.auto_start {
                // The authored dormant attachments belong to statues.
                fire.set_active(charged, tick);
            }
            fire.step(tick, ambient, rng);
        }
    }
}

pub(crate) struct Fires {
    tick: u64,
    daytime: Option<bool>,
    objects: BTreeMap<u32, ObjectFires>,
    scenery: BTreeMap<u32, ObjectFires>,
    /// Changing mounts or rider model attaches a different torch.
    torch: Option<((u16, bool), Fire)>,
    rng: Rng,
}

impl Fires {
    pub fn new(seed: u32) -> Self {
        Self {
            tick: 0,
            daytime: None,
            objects: BTreeMap::new(),
            scenery: BTreeMap::new(),
            torch: None,
            rng: Rng::new(seed ^ 0x46b7_13e9),
        }
    }

    pub fn step(&mut self, world: &WorldData, game: &GameState) {
        self.tick += 1;
        let tick = self.tick;
        let daytime = game.days.is_daytime;
        let previous = self.daytime.replace(daytime);
        let transition = previous.filter(|&old| old != daytime).map(|_| daytime);
        let grayscale = game.blender.state.ambient_grayscale();
        let ambient = sample_keys(&world.fire.ambient, grayscale);

        for o in &game.level.objects {
            let proto = &world.protos[usize::from(o.proto)];
            if proto.fires.is_empty() || game::build::under_construction(world, o) {
                continue;
            }
            let state = self
                .objects
                .entry(o.id)
                .or_insert_with(|| ObjectFires::new(o, world, daytime, tick));
            if state.proto != o.proto {
                *state = ObjectFires::new(o, world, daytime, tick);
            }
            let charged = proto.statue.is_some()
                && game
                    .statues
                    .list
                    .iter()
                    .any(|s| s.object == o.id && s.charged);
            state.step(
                &proto.fires,
                tick,
                transition,
                charged,
                ambient,
                &mut self.rng,
            );
        }
        self.objects.retain(|_, o| o.tick == tick);

        // Scenery is created at island load and can only be removed after
        // that. Find just the tracked attachments on the per-tick path.
        if previous.is_none() {
            for o in &game.level.scenery {
                if !world.protos[usize::from(o.proto)].fires.is_empty() {
                    self.scenery
                        .insert(o.id, ObjectFires::new(o, world, daytime, tick));
                }
            }
        }
        self.scenery.retain(|&id, state| {
            let Ok(index) = game.level.scenery.binary_search_by_key(&id, |o| o.id) else {
                return false;
            };
            let o = &game.level.scenery[index];
            state.step(
                &world.protos[usize::from(o.proto)].fires,
                tick,
                transition,
                false,
                ambient,
                &mut self.rng,
            );
            true
        });

        let def = world.steeds[usize::from(game.steed)].torch.as_ref();
        if let Some(def) = def {
            let owner = (game.steed, game.look.queen);
            let (current, torch) = self
                .torch
                .get_or_insert_with(|| (owner, Fire::new(def, &world.fire)));
            if *current != owner {
                *current = owner;
                *torch = Fire::new(def, &world.fire);
            }
            torch.set_active(grayscale < world.fire.torch_threshold, tick);
            torch.step(tick, ambient, &mut self.rng);
        } else {
            self.torch = None;
        }
    }

    pub fn object(&self, o: &Object, attachment: usize, scenery: bool) -> Option<FireSample> {
        let objects = if scenery {
            &self.scenery
        } else {
            &self.objects
        };
        let state = objects.get(&o.id).filter(|state| state.proto == o.proto)?;
        state.attachments.get(attachment)?.sample(self.tick)
    }

    pub fn torch(&self) -> Option<FireSample> {
        self.torch.as_ref()?.1.sample(self.tick)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lamp_waits_then_ramps_only_on_the_slow_update_cadence() {
        let mut fire = Fire {
            base_intensity: 2.0,
            ..Default::default()
        };
        let mut rng = Rng::new(91);
        let mut expected_rng = rng.clone();
        fire.start(0);
        fire.step(0, 1.0, &mut rng);
        let expected_bonus = 0.8 * expected_rng.range(-0.3, 0.3);
        assert_eq!(fire.bonus, expected_bonus);
        assert_eq!(fire.sample(0).unwrap().intensity, 0.0);
        fire.step(8, 0.0, &mut rng);
        assert_eq!(fire.bonus, expected_bonus);
        fire.step(9, 0.0, &mut rng);
        let expected_bonus = (expected_bonus + expected_rng.range(-0.3, 0.3)) * 0.8;
        assert_eq!(fire.bonus, expected_bonus);

        for tick in 10..=62 {
            fire.step(tick, 0.0, &mut rng);
            assert_eq!(fire.sample(tick).unwrap().intensity, 0.0);
        }
        fire.step(63, 0.0, &mut rng);
        assert!((fire.sample(63).unwrap().intensity - 0.1).abs() < 1e-5);
        for tick in 64..=125 {
            fire.step(tick, 0.0, &mut rng);
        }
        assert!((fire.sample(125).unwrap().intensity - 1.9).abs() < 1e-5);
        fire.step(126, 0.0, &mut rng);
        assert_eq!(fire.sample(126).unwrap().intensity, 2.0);

        fire.stop();
        assert!(fire.sample(127).is_none());
        fire.start(127);
        fire.step(127, 0.0, &mut rng);
        assert_eq!(fire.sample(127).unwrap().elapsed, 0.0);
        assert_eq!(fire.sample(127).unwrap().intensity, 0.0);
    }

    #[test]
    fn night_callbacks_choose_independent_deadlines_and_fire_in_due_order() {
        let mut first = Fire::default();
        let mut second = Fire::default();
        let mut rng = Rng::new(413);
        first.schedule(0, true, &mut rng);
        second.schedule(0, true, &mut rng);
        first.schedule(1, false, &mut rng);

        let first_dusk = first.pending.iter().find(|&&(_, start)| start).unwrap().0;
        let first_dawn = first.pending.iter().find(|&&(_, start)| !start).unwrap().0;
        let second_dusk = second.pending[0].0;
        assert!((1..=300).contains(&first_dusk));
        assert!((2..=301).contains(&first_dawn));
        assert!((1..=300).contains(&second_dusk));
        assert_ne!(first_dusk, second_dusk);
        assert!(first_dusk < first_dawn);
        for tick in 0..=301 {
            first.step(tick, 0.0, &mut rng);
            second.step(tick, 0.0, &mut rng);
            assert_eq!(
                first.sample(tick).is_some(),
                (first_dusk..first_dawn).contains(&tick)
            );
            assert_eq!(second.sample(tick).is_some(), tick >= second_dusk);
        }
    }
}
