//! The monarch on their steed: walk/gallop/eat/rear and stamina.
//!
//! orig: `Player.UpdateActionState()`, `Player.SetActionState()`,
//! `Player.TryToGallop()`, `Steed` (stats and timers), `Mover` (velocity).

use crate::controls::Controls;
use crate::rng::Rng;

/// Ground friction deceleration (units/s²) when the target speed is below
/// the current speed. `Mover` with `maxAccelerationPerFrame = 0` snaps
/// velocity up instantly but never sets it down; Box2D friction slows the
/// body instead: √(Player material 1.0 × default 0.4) × gravity 15.
const FRICTION_DECEL: f32 = 9.487;
/// Unity's fixed physics step (`Time.fixedDeltaTime`, 50 Hz).
const PHYSICS_STEP: f32 = 0.02;
/// Speed lost to friction while `Mover` holds a speed. Every physics step,
/// Box2D takes friction off the velocity *before* it moves the body, then
/// `Mover.Update()` snaps the velocity back up to the target. So the body
/// always moves this much slower than its speed setting. Measured in the
/// original: walk 2.0 → 1.8103, gallop 3.6 → 3.4103 (trace fixtures).
const HELD_SPEED_LOSS: f32 = FRICTION_DECEL * PHYSICS_STEP;

/// Per-steed tuning. orig: `Steed` fields (values from the steed prefabs).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SteedStats {
    pub walk_speed: f32,
    pub run_speed: f32,
    pub forest_speed_multiplier: f32,
    pub walk_stamina_rate: f32,
    pub run_stamina_rate: f32,
    pub stand_stamina_rate: f32,
    pub eat_delay: f32,
    pub eat_duration: f32,
    pub rearing_cooldown: f32,
    pub rearing_duration: f32,
    pub well_fed_duration: f32,
    pub tired_duration: f32,
    pub reserve_stamina: f32,
    pub reserve_probability: f32,
    pub does_not_need_grass: bool,
}

impl SteedStats {
    /// orig: the `Steed` prefab's movement and stamina values.
    pub fn from_def(d: &crate::data::SteedDef) -> Self {
        Self {
            walk_speed: d.walk_speed,
            run_speed: d.run_speed,
            forest_speed_multiplier: d.forest_speed_multiplier,
            walk_stamina_rate: d.walk_stamina_rate,
            run_stamina_rate: d.run_stamina_rate,
            stand_stamina_rate: d.stand_stamina_rate,
            eat_delay: d.eat_delay,
            eat_duration: d.eat_duration,
            rearing_cooldown: d.rearing_cooldown,
            rearing_duration: d.rearing_duration,
            well_fed_duration: d.well_fed_duration,
            tired_duration: d.tired_duration,
            reserve_stamina: d.reserve_stamina,
            reserve_probability: d.reserve_probability,
            does_not_need_grass: d.does_not_need_grass,
        }
    }

    /// The default steed. orig: `Player.standardSteedPrefab` → "Horse Regular".
    pub const HORSE_REGULAR: Self = Self {
        walk_speed: 2.0,
        run_speed: 3.6,
        forest_speed_multiplier: 0.85,
        walk_stamina_rate: 0.05,
        run_stamina_rate: -0.08,
        stand_stamina_rate: 0.15,
        eat_delay: 0.5,
        eat_duration: 3.0,
        rearing_cooldown: 10.0,
        rearing_duration: 0.8,
        well_fed_duration: 48.0,
        tired_duration: 20.0,
        reserve_stamina: 0.25,
        reserve_probability: 0.7,
        does_not_need_grass: false,
    };
}

/// orig: `Player.ActionState`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Stand,
    Walk,
    Run,
    Eat,
    Rear,
}

/// What's under the steed's front hooves. orig: `Player.GroundType`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ground {
    Normal,
    Grass,
    Forest,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MonarchEvent {
    /// orig: `Steed.OnStartGallop()`.
    StartedGallop,
    /// Out of stamina mid-gallop. orig: `Steed.BecomeTired()`.
    BecameTired,
    /// Tried to gallop while tired. orig: `Steed.Rear()`.
    Reared,
    /// orig: `Steed.Eat()`.
    StartedEating,
    /// Stamina refilled. orig: `Steed.EatingDone()`.
    AteDone,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug)]
pub struct Monarch {
    pub x: f32,
    /// -1 or 1. orig: `transform.localScale.x`.
    pub facing: i8,
    pub velocity: f32,
    pub steed: SteedStats,
    /// 0..=1. orig: `Steed.stamina`.
    pub stamina: f32,
    /// orig: `Steed.tiredTimer`.
    pub tired_timer: f32,
    /// orig: `Steed.wellFedTimer`.
    pub well_fed_timer: f32,
    action: Action,
    eat_timer: f32,
    rearing_timer: f32,
    prev_dir: i8,
    ground: Ground,
    move_speed: f32,
    speed_multiplier: f32,
}

impl Monarch {
    pub fn new(x: f32, steed: SteedStats) -> Self {
        Self {
            x,
            facing: 1,
            velocity: 0.0,
            steed,
            stamina: 1.0,
            tired_timer: 0.0,
            well_fed_timer: 0.0,
            action: Action::Stand,
            eat_timer: 0.0,
            rearing_timer: 0.0,
            prev_dir: 0,
            ground: Ground::Normal,
            move_speed: 0.0,
            speed_multiplier: 1.0,
        }
    }

    pub fn action(&self) -> Action {
        self.action
    }

    /// orig: `Player.isRunning`. Payables can't be targeted while galloping.
    pub fn is_running(&self) -> bool {
        self.action == Action::Run
    }

    /// Steed is out of breath (puffing). orig: `Player.Update()` → `Steed.puffActive`.
    pub fn is_puffing(&self) -> bool {
        matches!(self.action, Action::Walk | Action::Stand | Action::Run)
            && (self.tired_timer > 0.0 || self.stamina <= 0.2)
    }

    pub fn update(
        &mut self,
        dt: f32,
        controls: &Controls,
        ground: Ground,
        rng: &mut Rng,
        mut emit: impl FnMut(MonarchEvent),
    ) {
        // orig: Player.Update
        self.rearing_timer += dt;
        self.eat_timer += dt;
        // orig: Steed.Update
        self.tired_timer -= dt;
        self.well_fed_timer -= dt;
        self.set_ground(ground);
        self.update_action(
            dt,
            controls.direction,
            controls.start_sprint,
            controls.stop_sprint,
            rng,
            &mut emit,
        );
        let held = self.update_velocity(dt);
        let moved = if held {
            self.velocity - self.velocity.signum() * HELD_SPEED_LOSS.min(self.velocity.abs())
        } else {
            self.velocity
        };
        self.x += moved * dt;
    }

    /// orig: `Player.SetGroundType()`.
    fn set_ground(&mut self, ground: Ground) {
        if ground != self.ground {
            self.ground = ground;
            self.speed_multiplier = if ground == Ground::Forest {
                self.steed.forest_speed_multiplier
            } else {
                1.0
            };
        }
    }

    /// orig: `Player.SetActionState()`.
    fn set_action(&mut self, new: Action, direction: i8, emit: &mut impl FnMut(MonarchEvent)) {
        if new != self.action && new == Action::Run {
            emit(MonarchEvent::StartedGallop);
        }
        match new {
            Action::Stand => {
                self.eat_timer = 0.0;
                self.move_speed = 0.0;
            }
            Action::Walk => self.set_speed(self.steed.walk_speed, direction),
            Action::Run => self.set_speed(self.steed.run_speed, direction),
            Action::Eat => {
                self.eat_timer = 0.0;
                emit(MonarchEvent::StartedEating);
            }
            Action::Rear => {
                self.rearing_timer = 0.0;
                self.move_speed = 0.0;
                emit(MonarchEvent::Reared);
            }
        }
        self.action = new;
    }

    /// orig: `Mover.SetSpeed(speed, direction)`.
    fn set_speed(&mut self, speed: f32, direction: i8) {
        if direction != 0 {
            self.facing = direction;
        }
        self.move_speed = speed;
    }

    /// orig: `Player.Rear()`.
    fn rear(&mut self, emit: &mut impl FnMut(MonarchEvent)) {
        if self.rearing_timer > self.steed.rearing_cooldown {
            self.set_action(Action::Rear, 0, emit);
        }
    }

    /// orig: `Player.TryToGallop()`.
    fn try_gallop(&mut self, direction: i8, emit: &mut impl FnMut(MonarchEvent)) {
        if self.tired_timer > 0.0 {
            self.rear(emit);
        } else {
            self.set_action(Action::Run, direction, emit);
        }
    }

    /// orig: `Player.UpdateActionState()`.
    fn update_action(
        &mut self,
        dt: f32,
        direction: i8,
        start_sprint: bool,
        stop_sprint: bool,
        rng: &mut Rng,
        emit: &mut impl FnMut(MonarchEvent),
    ) {
        let s = self.steed;
        match self.action {
            Action::Stand => {
                self.stamina += s.stand_stamina_rate * dt;
                if (self.ground == Ground::Grass || s.does_not_need_grass)
                    && self.eat_timer > s.eat_delay
                    && self.well_fed_timer < 0.75 * s.well_fed_duration
                {
                    self.set_action(Action::Eat, 0, emit);
                }
                if direction != 0 {
                    if start_sprint {
                        self.try_gallop(direction, emit);
                    } else {
                        self.set_action(Action::Walk, direction, emit);
                    }
                }
            }
            Action::Walk => {
                self.stamina += s.walk_stamina_rate * dt;
                if direction == 0 || direction != self.prev_dir {
                    self.set_action(Action::Stand, 0, emit);
                } else if start_sprint {
                    self.try_gallop(direction, emit);
                }
            }
            Action::Run => {
                if self.well_fed_timer <= 0.0 {
                    self.stamina += s.run_stamina_rate * dt;
                }
                if direction == 0 || direction != self.prev_dir {
                    self.set_action(Action::Stand, 0, emit);
                } else if stop_sprint {
                    self.set_action(Action::Walk, direction, emit);
                } else if self.stamina <= 0.0 {
                    if rng.value() < s.reserve_probability {
                        self.stamina = s.reserve_stamina;
                    } else {
                        self.set_action(Action::Walk, direction, emit);
                        self.tired_timer = s.tired_duration;
                        emit(MonarchEvent::BecameTired);
                    }
                }
            }
            Action::Rear => {
                if self.rearing_timer > s.rearing_duration {
                    self.set_action(Action::Stand, 0, emit);
                }
            }
            Action::Eat => {
                if self.eat_timer > s.eat_duration {
                    // orig: Steed.EatingDone
                    self.stamina = 1.0;
                    self.well_fed_timer = s.well_fed_duration;
                    self.tired_timer = 0.0;
                    emit(MonarchEvent::AteDone);
                    self.set_action(Action::Stand, 0, emit);
                }
                if direction != 0 {
                    self.set_action(Action::Walk, direction, emit);
                }
            }
        }
        self.stamina = self.stamina.clamp(0.0, 1.0);
        self.prev_dir = direction;
    }

    /// orig: `Mover.Update()` with `maxAccelerationPerFrame = 0` (Player prefab),
    /// plus ground friction from the physics engine. `velocity` is the speed
    /// setting `Mover` writes. Returns whether `Mover` is holding it at a
    /// non-zero target this frame, i.e. the case where friction keeps eating
    /// [`HELD_SPEED_LOSS`] out of every physics step.
    fn update_velocity(&mut self, dt: f32) -> bool {
        let target = self.move_speed * self.speed_multiplier * self.facing as f32;
        // In the original, friction has always just dragged the body below
        // the target, so an equal speed counts as speeding up too.
        let speeding_up =
            (target > 0.0 && target >= self.velocity) || (target < 0.0 && target <= self.velocity);
        if speeding_up {
            self.velocity = target;
            true
        } else {
            let step = FRICTION_DECEL * dt;
            self.velocity = if self.velocity > target {
                (self.velocity - step).max(target)
            } else {
                (self.velocity + step).min(target)
            };
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    const DT: f32 = 1.0 / 60.0;

    struct Sim {
        m: Monarch,
        rng: Rng,
        events: Vec<MonarchEvent>,
    }

    impl Sim {
        fn new() -> Self {
            Self {
                m: Monarch::new(0.0, SteedStats::HORSE_REGULAR),
                rng: Rng::new(1),
                events: Vec::new(),
            }
        }

        fn run(&mut self, dir: i8, sprint: bool, ground: Ground, frames: u32) {
            for i in 0..frames {
                let ev = &mut self.events;
                let c = Controls {
                    direction: dir,
                    start_sprint: sprint && i == 0,
                    ..Default::default()
                };
                self.m.update(DT, &c, ground, &mut self.rng, |e| ev.push(e));
            }
        }
    }

    #[test]
    fn walks_at_walk_speed() {
        let mut s = Sim::new();
        s.run(1, false, Ground::Normal, 60);
        assert_eq!(s.m.action(), Action::Walk);
        assert_eq!(s.m.velocity, 2.0);
        // The setting is 2.0, but friction costs 0.1897 per physics step.
        assert!((s.m.x - 1.8103).abs() < 0.05, "{}", s.m.x);
    }

    #[test]
    fn held_speed_loses_one_physics_step_of_friction() {
        let mut s = Sim::new();
        s.run(1, false, Ground::Normal, 1);
        s.run(1, true, Ground::Normal, 1);
        let x0 = s.m.x;
        s.run(1, false, Ground::Normal, 600);
        let speed = (s.m.x - x0) / (600.0 * DT);
        assert!((speed - 3.4103).abs() < 1e-3, "{speed}");
    }

    #[test]
    fn forest_slows_walking() {
        let mut s = Sim::new();
        s.run(-1, false, Ground::Forest, 10);
        assert!((s.m.velocity + 1.7).abs() < 1e-4);
    }

    #[test]
    fn gallop_drains_stamina_then_tires_or_uses_reserve() {
        let mut s = Sim::new();
        s.run(1, false, Ground::Normal, 1);
        s.run(1, true, Ground::Normal, 1);
        assert_eq!(s.m.action(), Action::Run);
        assert!(s.events.contains(&MonarchEvent::StartedGallop));
        assert_eq!(s.m.velocity, 3.6);
        // At -0.08/s a full bar lasts 12.5 s; reserve draws can extend it.
        s.run(1, false, Ground::Normal, 60 * 60);
        assert!(s.events.contains(&MonarchEvent::BecameTired));
        assert_eq!(s.m.action(), Action::Walk);
        assert!(s.m.tired_timer > 0.0);
    }

    #[test]
    fn galloping_while_tired_rears() {
        let mut s = Sim::new();
        s.m.tired_timer = 5.0;
        s.m.rearing_timer = 100.0;
        s.run(1, false, Ground::Normal, 1);
        s.run(1, true, Ground::Normal, 1);
        assert_eq!(s.m.action(), Action::Rear);
        s.run(0, false, Ground::Normal, 60);
        assert_eq!(s.m.action(), Action::Stand);
    }

    #[test]
    fn standing_on_grass_eats_and_refills() {
        let mut s = Sim::new();
        s.m.stamina = 0.1;
        s.run(0, false, Ground::Grass, 60 * 4);
        assert!(s.events.contains(&MonarchEvent::StartedEating));
        assert!(s.events.contains(&MonarchEvent::AteDone));
        assert_eq!(s.m.stamina, 1.0);
        assert!(s.m.well_fed_timer > 40.0);
    }

    #[test]
    fn reversing_stops_first_then_slides_to_a_halt() {
        let mut s = Sim::new();
        s.run(1, false, Ground::Normal, 30);
        s.run(-1, false, Ground::Normal, 1);
        assert_eq!(s.m.action(), Action::Stand);
        assert!(s.m.velocity > 0.0, "friction, not an instant stop");
        s.run(0, false, Ground::Normal, 30);
        assert_eq!(s.m.velocity, 0.0);
    }
}
