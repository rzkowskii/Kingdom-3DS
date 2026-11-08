//! The workshop's catapults: builders push one out to the wall, crank it,
//! and it lobs boulders at greed in front of it.
//!
//! orig: `PayableWorkshop`, `Catapult`, `Boulder`.

use alloc::vec::Vec;

use crate::animation::EventKind;
use crate::build::{under_construction, WALK_Y};
use crate::coins::Rect;
use crate::data::WorldData;
use crate::kingdom::Borders;
use crate::level::{self, Level};
use crate::rng::Rng;

/// orig: `Catapult.ArmMaxAngle`, `BorderDistance`.
const ARM_MAX: f32 = 68.0;
const BORDER_DISTANCE: f32 = 2.5;
/// orig: `Update()` while Moving: 0.15 per pushing builder.
const PUSH_SPEED: f32 = 0.15;
/// The fire clip: the boulder leaves partway, the arm settles after.
const RELEASE_AT: f32 = 0.15;
const ARM_IMPACT_AT: f32 = 0.16666667;
const FIRE_TIME: f32 = 1.1666666;

/// orig: `Catapult.State` (stored on the level object as `mode`).
pub mod mode {
    pub const MOVING: u8 = 0;
    pub const RELOADING: u8 = 1;
    pub const READY: u8 = 2;
    pub const FIRING: u8 = 3;
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct Catapult {
    pub object: u32,
    pub side: i8,
    /// The workshop counter that built it. orig: `PayableWorkshop._catapult`.
    pub counter: u32,
    /// Time into the firing animation.
    pub fire_t: f32,
    released: bool,
}

/// A boulder in flight. orig: `Boulder` launched by a catapult.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct Boulder {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub spin: f32,
    damage: i32,
    stuck_probability: f32,
    /// orig: `ghostTime`: passes through things at first.
    ghost: f32,
    bounces: u8,
    hit: Vec<u32>,
    /// Lodged in the ground. The float is the old save format's age field.
    pub stuck: Option<f32>,
    /// Saved separately to preserve the legacy projectile layout.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub runtime: BoulderRuntime,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoulderRuntime {
    /// Runtime identity for a cached projectile scanner result.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub id: u64,
    pub held_by: Option<u32>,
    pub hostile: bool,
    fixed_pending: f32,
    angular_velocity: f32,
    launch_side: i8,
    on_ground: bool,
    after_release: Option<f32>,
    returned_hits: Vec<BoulderTargetKind>,
    /// Time since the last admitted physical callback. None is ready.
    /// The v12 tail carries this field separately from the v11 state.
    #[cfg_attr(feature = "serde", serde(skip))]
    contact_age: Option<f32>,
}

impl Boulder {
    #[cfg(test)]
    pub(crate) fn test_lodged(x: f32) -> Self {
        Self {
            x,
            y: 0.9375 - WALK_Y,
            vx: 0.0,
            vy: 0.0,
            spin: 0.0,
            damage: 15,
            stuck_probability: 1.0,
            ghost: 0.0,
            bounces: 1,
            hit: Vec::new(),
            stuck: Some(0.0),
            runtime: BoulderRuntime::default(),
        }
    }

    /// The PC scanner checks layer Projectiles before testing the object's state.
    pub fn scan_eligible(&self) -> bool {
        self.ghost <= 0.0
    }

    /// `Boss.Throw` parents the lodged boulder to its hand and calls SetFake.
    pub fn hold_by(&mut self, boss: u32) -> bool {
        if self.stuck.is_none() || self.runtime.held_by.is_some() || !self.scan_eligible() {
            return false;
        }
        self.stuck = None;
        self.vx = 0.0;
        self.vy = 0.0;
        self.runtime.angular_velocity = 0.0;
        self.runtime.on_ground = false;
        self.runtime.held_by = Some(boss);
        true
    }

    /// `OnAnimBoulderRelease`: an unnormalized (side, 1) times throwForce.Random.
    /// The previous SetFake zeroed velocity, so mass-scaled impulse sets velocity.
    pub fn release_from_boss(
        &mut self,
        at: [f32; 2],
        side: i8,
        speed: f32,
        after_release: f32,
        world: &WorldData,
        rng: &mut Rng,
    ) {
        debug_assert!(self.runtime.held_by.is_some());
        self.x = at[0];
        self.y = at[1];
        self.vx = side as f32 * speed;
        self.vy = speed;
        self.damage = world.boss_throw.boulder_damage;
        self.ghost = world.boss_throw.boulder_ghost;
        self.bounces = 0;
        self.hit.clear();
        self.runtime = BoulderRuntime {
            id: self.runtime.id,
            hostile: true,
            // World-space rotation. The scene converts it to screen-space.
            angular_velocity: -side as f32 * 180.0 * (rng.value() + 1.0),
            launch_side: side,
            after_release: Some(after_release),
            contact_age: self.runtime.contact_age,
            ..Default::default()
        };
    }

    fn age_contact(&mut self, dt: f32) {
        self.runtime.contact_age = self
            .runtime
            .contact_age
            .map(|age| age + dt)
            .filter(|&age| age <= 0.1);
    }

    fn admit_contact(&mut self) -> bool {
        if self.runtime.contact_age.is_some() {
            return false;
        }
        self.runtime.contact_age = Some(0.0);
        true
    }

    fn effect_side(&self) -> i8 {
        if self.vx > 0.0 {
            1
        } else {
            -1
        }
    }

    fn impact(&self, y: f32, side: i8, collision_sound: bool) -> BoulderHit {
        BoulderHit::Impact {
            x: self.x,
            y,
            shake_y: self.y,
            side,
            collision_sound,
        }
    }

    fn burst(&self, side: i8, impact: bool) -> BoulderHit {
        BoulderHit::Burst {
            x: self.x,
            y: self.y,
            side,
            impact,
        }
    }
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Siege {
    pub catapults: Vec<Catapult>,
    pub boulders: Vec<Boulder>,
    /// Saved separately in the versioned presentation tail.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub recoils: Vec<crate::recoil::State>,
    #[cfg_attr(feature = "serde", serde(skip))]
    next_boulder_id: u64,
}

/// Authored markers from the catapult's simulation-owned firing timeline.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CatapultEvent {
    Fired {
        object: u32,
        x: f32,
    },
    Released(u32),
    /// PC applies the chassis angular impulse at this marker.
    ArmImpact(u32),
    Finished(u32),
}

/// Distinct collision objects count against the returned boulder's hit limit.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoulderTargetKind {
    Monarch,
    Unit(u32),
    Object(u32),
    Enemy(u32),
    Wildlife(u32),
}

#[derive(Clone, Copy, Debug)]
pub struct BoulderTarget {
    pub target: BoulderTargetKind,
    pub rect: Rect,
    pub physical: bool,
    pub takes_damage: bool,
    pub huge: bool,
}

pub struct BoulderWall {
    pub object: u32,
    /// Ground-relative, world-space vertices of the authored wall collider.
    pub polygon: crate::data::ObstacleDef,
}

#[derive(Clone, Copy, Debug)]
pub struct BoulderEnemy {
    pub id: u32,
    pub x: f32,
    /// Huge damage trigger. Its admission shatters even a friendly boulder.
    pub huge: Option<Rect>,
}

/// Something a boulder hit, or a returned boulder breaking.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BoulderHit {
    Troll(u32, i32),
    Returned {
        target: BoulderTargetKind,
        damage: i32,
        direction: f32,
    },
    Impact {
        x: f32,
        y: f32,
        shake_y: f32,
        side: i8,
        collision_sound: bool,
    },
    Burst {
        x: f32,
        y: f32,
        side: i8,
        impact: bool,
    },
}

impl Siege {
    pub fn identify_boulders(&mut self) {
        for b in &mut self.boulders {
            if b.runtime.id == 0 {
                self.next_boulder_id += 1;
                b.runtime.id = self.next_boulder_id;
            }
        }
    }

    #[cfg(feature = "save")]
    pub fn boulder_states(&self) -> Vec<BoulderRuntime> {
        self.boulders.iter().map(|b| b.runtime.clone()).collect()
    }

    #[cfg(feature = "save")]
    pub(crate) fn boulder_contact_ages(&self) -> Vec<Option<f32>> {
        self.boulders
            .iter()
            .map(|b| b.runtime.contact_age)
            .collect()
    }

    #[cfg(feature = "save")]
    pub(crate) fn restore_boulder_contact_ages(&mut self, ages: Vec<Option<f32>>) -> bool {
        if ages.len() != self.boulders.len()
            || ages.iter().flatten().any(|age| !(0.0..=0.1).contains(age))
        {
            return false;
        }
        for (boulder, age) in self.boulders.iter_mut().zip(ages) {
            boulder.runtime.contact_age = age;
        }
        true
    }

    #[cfg(feature = "save")]
    pub fn restore_boulder_states(
        &mut self,
        states: Vec<BoulderRuntime>,
        world: &WorldData,
    ) -> bool {
        let d = &world.boss_throw;
        if !d.valid()
            || states.len() != self.boulders.len()
            || self.boulders.iter().zip(&states).any(|(b, s)| {
                ![b.x, b.y, b.vx, b.vy, b.spin, b.ghost]
                    .into_iter()
                    .all(|v| v.is_finite() && v.abs() <= 1_000_000.0)
                    || !b.stuck.is_none_or(|t| t.is_finite() && t >= 0.0)
                    || !(0.0..d.fixed_step).contains(&s.fixed_pending)
                    || !s.angular_velocity.is_finite()
                    || s.angular_velocity.abs() > 360.0
                    || s.after_release.is_some_and(|t| !(0.0..=60.0).contains(&t))
                    || s.returned_hits.len() > d.boulder_max_hits as usize
                    || s.returned_hits
                        .iter()
                        .enumerate()
                        .any(|(i, hit)| s.returned_hits[..i].contains(hit))
                    || if s.hostile {
                        s.held_by.is_some()
                            || b.stuck.is_some()
                            || !matches!(s.launch_side, -1 | 1)
                            || !(0.0..=d.boulder_ghost).contains(&b.ghost)
                            || b.damage != d.boulder_damage
                    } else {
                        s.launch_side != 0
                            || s.fixed_pending != 0.0
                            || s.angular_velocity != 0.0
                            || s.on_ground && (s.held_by.is_some() || b.stuck.is_some())
                            || s.after_release.is_some()
                            || !s.returned_hits.is_empty()
                            || s.held_by.is_some()
                                && (b.stuck.is_some()
                                    || b.vx != 0.0
                                    || b.vy != 0.0
                                    || !b.scan_eligible())
                    }
            })
        {
            return false;
        }
        self.next_boulder_id = 0;
        for (b, mut runtime) in self.boulders.iter_mut().zip(states) {
            runtime.id = 0;
            b.runtime = runtime;
        }
        self.identify_boulders();
        true
    }

    /// Death removes presentation state immediately, before a same-tick save.
    pub fn remove(&mut self, object: u32) {
        self.catapults.retain(|c| c.object != object);
        self.recoils.retain(|r| r.object != object);
    }

    pub fn recoil_pose(
        &self,
        object: u32,
        definition: &crate::recoil::Definition,
    ) -> crate::data::Affine2 {
        self.recoils
            .iter()
            .find(|r| r.object == object)
            .map_or(crate::data::Affine2::IDENTITY, |r| r.pose(definition))
    }

    pub fn wheel_pose(
        &self,
        object: u32,
        wheel: usize,
        definition: &crate::recoil::Definition,
    ) -> crate::data::Affine2 {
        self.recoils
            .iter()
            .find(|r| r.object == object)
            .map_or(crate::data::Affine2::IDENTITY, |r| {
                r.wheel_pose(wheel, definition)
            })
    }

    pub fn recoils_valid(&self, world: &WorldData, level: &Level) -> bool {
        self.recoils.iter().enumerate().all(|(i, r)| {
            !self.recoils[..i].iter().any(|p| p.object == r.object)
                && self
                    .catapults
                    .iter()
                    .any(|c| c.object == r.object && c.side == r.side)
                && level.index_of(r.object).is_some_and(|i| {
                    let o = &level.objects[i];
                    world
                        .protos
                        .get(o.proto as usize)
                        .and_then(|p| p.catapult.as_ref())
                        .and_then(|c| c.recoil.as_ref())
                        .is_some_and(|d| {
                            r.valid(d) && (r.ground_y + o.y - d.ground_height).abs() < 0.001
                        })
                })
        })
    }

    pub fn restore_recoils(
        &mut self,
        states: Vec<crate::recoil::State>,
        world: &WorldData,
        level: &Level,
    ) -> bool {
        let old = core::mem::replace(&mut self.recoils, states);
        if self.recoils_valid(world, level) {
            true
        } else {
            self.recoils = old;
            false
        }
    }

    /// orig: the Workshop's `PayableWorkshop` children: one counter per side.
    pub fn place_counters(world: &WorldData, level: &mut Level, workshop: u32) {
        let Some(i) = level.index_of(workshop) else {
            return;
        };
        let o = &level.objects[i];
        let (layer, x, y, z) = (o.layer, o.x, o.y, o.z);
        let counters = world.protos[o.proto as usize].workshop.clone();
        for (proto, dx) in counters {
            let c = level::instantiate(world, proto, layer, x + dx, y, z, 1.0);
            level.add(c);
        }
    }

    /// orig: `PayableWorkshop.CanPay()`: no catapult of its own yet.
    pub fn counter_can_pay(&self, counter: u32) -> bool {
        !self.catapults.iter().any(|c| c.counter == counter)
    }

    /// orig: `PayableWorkshop.Pay()`: a catapult (to be built) beside it.
    pub fn order(&mut self, world: &WorldData, level: &mut Level, counter: u32) {
        let Some(i) = level.index_of(counter) else {
            return;
        };
        let o = &level.objects[i];
        let Some((side, proto)) = world.protos[o.proto as usize].workshop_side else {
            return;
        };
        let p = &world.protos[proto as usize];
        let z = p.fixed.and_then(|f| f.fix_z).unwrap_or(p.root_z);
        let x = o.x + side as f32 * 1.2;
        let mut c = level::instantiate(world, proto, o.layer, x, 1.0, z, side as f32);
        if p.building.as_ref().is_some_and(|b| b.build_points > 0) {
            c.build = Some(0.0);
        }
        c.mode = mode::MOVING;
        let object = level.add(c);
        self.catapults.push(Catapult {
            object,
            side,
            counter,
            fire_t: 0.0,
            released: false,
        });
    }

    /// orig: `Catapult.OnWorkFinish()` while reloading: a turn of the crank.
    pub fn crank(world: &WorldData, level: &mut Level, object: u32) {
        let Some(i) = level.index_of(object) else {
            return;
        };
        let o = &mut level.objects[i];
        let Some(d) = world.protos[o.proto as usize].catapult else {
            return;
        };
        if o.mode == mode::RELOADING {
            o.aim += d.crank_rate * ARM_MAX;
        }
    }

    /// `pushers` counts the builders at each catapult. Returns boulder hits.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        dt: f32,
        world: &WorldData,
        level: &mut Level,
        borders: Borders,
        pushers: &[(u32, usize)],
        enemies: &[BoulderEnemy],
        rng: &mut Rng,
        mut events: impl FnMut(CatapultEvent),
    ) -> Vec<BoulderHit> {
        let existing_boulders = self.boulders.len();
        let mut new_flight_times = Vec::new();
        self.catapults
            .retain(|c| level.index_of(c.object).is_some());
        self.recoils
            .retain(|r| self.catapults.iter().any(|c| c.object == r.object));
        for c in &mut self.catapults {
            let Some(i) = level.index_of(c.object) else {
                continue;
            };
            let o = &level.objects[i];
            let Some(d) = world.protos[o.proto as usize].catapult else {
                continue;
            };
            if under_construction(world, o) {
                continue;
            }
            let mut recoil = d.recoil.as_ref().map(|physics| {
                let ri = match self.recoils.iter().position(|r| r.object == c.object) {
                    Some(i) => i,
                    None => {
                        let mut state = crate::recoil::State::new(c.object, c.side, physics);
                        state.ground_y = physics.ground_height - o.y;
                        self.recoils.push(state);
                        self.recoils.len() - 1
                    }
                };
                &mut self.recoils[ri]
            });
            let mut advanced = 0.0;
            let s = c.side as f32;
            // orig: IsInPosition: within 2.5 of its side's border.
            let in_position = (o.x - (borders.side(c.side) - s * BORDER_DISTANCE)) * s > 0.0;
            let workers = pushers.iter().find(|p| p.0 == c.object).map_or(0, |p| p.1);
            let o = &mut level.objects[i];
            match o.mode {
                mode::MOVING => {
                    if in_position {
                        o.mode = mode::RELOADING;
                        if let Some(r) = recoil.as_mut() {
                            r.stop();
                        }
                    } else {
                        // PC Update sets chassis velocity; wheel contacts and
                        // hinges then determine its actual displacement.
                        let speed = s * PUSH_SPEED * workers as f32;
                        if let Some(r) = recoil.as_mut() {
                            r.drive(speed);
                        } else {
                            o.x += speed * dt;
                        }
                    }
                }
                mode::RELOADING => {
                    if o.aim > ARM_MAX {
                        o.mode = mode::READY;
                    }
                }
                mode::READY => {
                    // orig: the scanner sees scanRange.max ahead and nothing nearer than min.
                    let target = enemies.iter().any(|enemy| {
                        let ex = enemy.x;
                        let dd = (ex - o.x) * s;
                        dd >= d.scan.0 && dd <= d.scan.1
                    });
                    if target {
                        o.mode = mode::FIRING;
                        c.fire_t = 0.0;
                        c.released = false;
                        events(CatapultEvent::Fired {
                            object: c.object,
                            x: o.x,
                        });
                    }
                    // PC checks this even when Fire() was called above. A
                    // moving border can cancel firing before its first marker.
                    if !in_position {
                        o.mode = mode::MOVING;
                        o.aim = 0.0;
                    }
                }
                _ => {
                    let clip = world.animations.clip("catapult_fire");
                    let speed = world
                        .animations
                        .state_speed("catapult", "Fire")
                        .unwrap_or(1.0);
                    let before = c.fire_t;
                    c.fire_t += dt * speed;
                    let crossed = |kind, fallback| {
                        clip.map_or_else(
                            || {
                                world.animations.clips.is_empty()
                                    && before < fallback
                                    && c.fire_t >= fallback
                            },
                            |clip| clip.event_count(kind, before, c.fire_t, before == 0.0) > 0,
                        )
                    };
                    let marker_dt = |kind, fallback| {
                        let at = clip
                            .and_then(|c| c.first_event_time(kind))
                            .unwrap_or(fallback);
                        if speed > 0.0 {
                            ((at - before) / speed).clamp(0.0, dt)
                        } else {
                            0.0
                        }
                    };
                    if !c.released && crossed(EventKind::ProjectileRelease, RELEASE_AT) {
                        let until = marker_dt(EventKind::ProjectileRelease, RELEASE_AT);
                        advance_recoil(&mut recoil, d.recoil.as_ref(), until - advanced);
                        advanced = until;
                        c.released = true;
                        events(CatapultEvent::Released(c.object));
                        // orig: ReleaseProjectileFromAnimation: along (dir, 1.5) at projectileForce.
                        let speed = rng.range(d.force.0, d.force.1);
                        let len = libm::sqrtf(1.0 + 1.5 * 1.5);
                        let release_time = clip
                            .and_then(|c| c.first_event_time(EventKind::ProjectileRelease))
                            .unwrap_or(RELEASE_AT);
                        let spoon = clip
                            .and_then(|clip| {
                                world.protos[o.proto as usize].clip_transform(
                                    "Flippable/Arm/Spoon",
                                    "",
                                    clip,
                                    release_time,
                                )
                            })
                            .map(|a| (a.translation[0], a.translation[1]))
                            .unwrap_or(d.spoon);
                        let spoon = recoil
                            .as_ref()
                            .zip(d.recoil.as_ref())
                            .map_or([spoon.0 * s, spoon.1], |(r, d)| {
                                r.pose(d).point([spoon.0 * s, spoon.1])
                            });
                        self.boulders.push(Boulder {
                            x: o.x + spoon[0],
                            y: o.y + spoon[1] - WALK_Y,
                            vx: s / len * speed,
                            vy: 1.5 / len * speed,
                            spin: 0.0,
                            damage: d.boulder_damage,
                            stuck_probability: d.stuck_probability,
                            ghost: 0.3,
                            bounces: 0,
                            hit: Vec::new(),
                            stuck: None,
                            runtime: BoulderRuntime::default(),
                        });
                        new_flight_times.push(dt - until);
                    }
                    if crossed(EventKind::ArmImpact, ARM_IMPACT_AT) {
                        let until = marker_dt(EventKind::ArmImpact, ARM_IMPACT_AT);
                        advance_recoil(&mut recoil, d.recoil.as_ref(), until - advanced);
                        advanced = until;
                        if let (Some(r), Some(d)) = (recoil.as_mut(), d.recoil.as_ref()) {
                            r.impact(d);
                        }
                        events(CatapultEvent::ArmImpact(c.object));
                    }
                    if crossed(EventKind::FiringDone, FIRE_TIME) {
                        let until = marker_dt(EventKind::FiringDone, FIRE_TIME);
                        advance_recoil(&mut recoil, d.recoil.as_ref(), until - advanced);
                        advanced = until;
                        events(CatapultEvent::Finished(c.object));
                        // orig: FiringDoneFromAnimation.
                        o.aim = 0.0;
                        let physical_x = o.x
                            + recoil
                                .as_ref()
                                .zip(d.recoil.as_ref())
                                .map_or(0.0, |(r, d)| r.pose(d).translation[0]);
                        let in_position =
                            (physical_x - (borders.side(c.side) - s * BORDER_DISTANCE)) * s > 0.0;
                        o.mode = if in_position {
                            if let Some(r) = recoil.as_mut() {
                                r.stop();
                            }
                            mode::RELOADING
                        } else {
                            mode::MOVING
                        };
                    }
                }
            }
            advance_recoil(&mut recoil, d.recoil.as_ref(), dt - advanced);
            if let (Some(r), Some(d)) = (recoil.as_mut(), d.recoil.as_ref()) {
                // Gameplay targeting, workers, saves and rendering all use the
                // physical root. Keep solver coordinates local for ARM precision.
                let shift = r.pose(d).translation;
                o.x += shift[0];
                o.y += shift[1];
                r.shift_origin(shift);
            }
        }
        let hits = self.update_boulders(
            dt,
            existing_boulders,
            &new_flight_times,
            enemies,
            &world.boss_throw,
            rng,
        );
        self.identify_boulders();
        hits
    }

    /// An old in-flight boulder already belongs to the save. If release had
    /// not happened yet, restart that firing phase under the authored timing.
    #[cfg(feature = "save")]
    pub(crate) fn migrate_animation_clocks(&mut self) {
        for c in &mut self.catapults {
            if !c.released {
                c.fire_t = 0.0;
            }
        }
    }

    /// orig: `Boulder.HitObject()`: a friendly boulder hurts every greed it
    /// touches; on the ground it bounces once, then sticks or bursts.
    fn update_boulders(
        &mut self,
        dt: f32,
        existing: usize,
        new_flight_times: &[f32],
        enemies: &[BoulderEnemy],
        def: &crate::data::BossThrowDef,
        rng: &mut Rng,
    ) -> Vec<BoulderHit> {
        let g = crate::coins::GRAVITY;
        let radius = def.boulder_radius;
        let floor = def.ground_y - WALK_Y + radius;
        let mut hits = Vec::new();
        let mut gone = Vec::new();
        for (i, b) in self.boulders.iter_mut().enumerate() {
            // A new payload only receives time after its release marker.
            let dt = if i < existing {
                dt
            } else {
                new_flight_times[i - existing]
            };
            if dt <= 0.0 || b.runtime.hostile {
                continue;
            }
            b.age_contact(dt);
            if b.runtime.held_by.is_some() {
                continue;
            }
            if b.stuck.is_some() {
                // Old snapshots could lodge a boulder before its grace ended.
                b.ghost = (b.ghost - dt).max(0.0);
                continue;
            }
            let ghost = b.ghost > 0.0;
            b.ghost -= dt;
            b.vy -= g * dt;
            b.x += b.vx * dt;
            b.y += b.vy * dt;
            b.spin -= b.vx.signum() * 360.0 * dt;
            let side = b.effect_side();
            let entered_ground = b.y <= floor && !b.runtime.on_ground;
            b.runtime.on_ground = b.y <= floor;
            let mut physical_burst = false;
            if b.runtime.on_ground {
                b.y = floor;
                if !ghost && entered_ground && b.admit_contact() {
                    if b.bounces < 1 {
                        b.bounces += 1;
                        let k = rng.range(0.7, 1.0);
                        b.vx *= k;
                        b.vy = -b.vy * 0.5 * k;
                    } else if rng.value() < b.stuck_probability {
                        b.stuck = Some(0.0);
                        b.y = 0.9375 - WALK_Y;
                        b.vx = 0.0;
                        b.vy = 0.0;
                        b.runtime.on_ground = false;
                    } else {
                        physical_burst = true;
                    }
                    hits.push(b.impact(floor, side, !physical_burst));
                } else {
                    b.vy = -b.vy * 0.5;
                }
            }
            let mut burst = physical_burst;
            if !ghost && b.stuck.is_none() && !physical_burst {
                for enemy in enemies {
                    let overlaps = enemy.huge.map_or_else(
                        || (enemy.x - b.x).abs() < 0.4 && b.y < 0.8,
                        |rect| circle_hits_rect([b.x, b.y], radius, rect),
                    );
                    if overlaps && !b.hit.contains(&enemy.id) {
                        b.hit.push(enemy.id);
                        hits.push(BoulderHit::Troll(enemy.id, b.damage));
                        if enemy.huge.is_some() {
                            burst = true;
                            break;
                        }
                    }
                }
            }
            if burst {
                hits.push(b.burst(side, physical_burst));
                gone.push(i);
            }
        }
        for i in gone.into_iter().rev() {
            self.boulders.remove(i);
        }
        hits
    }

    /// Advance enemy boulders after the boss animation markers. Each new shot
    /// receives only the remainder after its release marker.
    pub fn update_returned_boulders(
        &mut self,
        dt: f32,
        world: &WorldData,
        targets: &[BoulderTarget],
        walls: &[BoulderWall],
    ) -> Vec<BoulderHit> {
        let mut hits = Vec::new();
        let d = &world.boss_throw;
        if !(0.0..=60.0).contains(&dt) || !d.valid() {
            return hits;
        }
        let step = d.fixed_step;
        let radius = d.boulder_radius;
        let floor = d.ground_y - WALK_Y + radius;
        self.boulders.retain_mut(|b| {
            if !b.runtime.hostile {
                return true;
            }
            let time = b.runtime.after_release.take().unwrap_or(dt).min(dt);
            let total = time + b.runtime.fixed_pending;
            let steps = libm::floorf(total / step) as u32;
            b.runtime.fixed_pending =
                (total - steps as f32 * step).clamp(0.0, step - f32::EPSILON * step);
            for _ in 0..steps {
                // Ghost switches layer, not body type. Ground contacts still
                // bounce, but HitObject has no effect until the grace ends.
                b.age_contact(step);
                let ghost = b.ghost > 0.0;
                b.ghost = if b.ghost <= step + f32::EPSILON {
                    0.0
                } else {
                    b.ghost - step
                };
                let damping = 1.0 / (1.0 + step * d.boulder_linear_drag);
                b.vx *= damping;
                b.vy = (b.vy - d.gravity * step) * damping;
                b.runtime.angular_velocity /= 1.0 + step * d.boulder_angular_drag;
                b.x += b.vx * step;
                b.y += b.vy * step;
                b.spin = (b.spin - b.runtime.angular_velocity * step) % 360.0;
                let entered_ground = b.y <= floor && !b.runtime.on_ground;
                b.runtime.on_ground = b.y <= floor;
                if b.runtime.on_ground {
                    b.y = floor;
                    bounce_boulder_ground(b, radius, d.boulder_restitution, d.boulder_friction);
                }
                if ghost {
                    continue;
                }
                let direction = if b.vx.abs() > 0.2 {
                    b.vx.signum()
                } else {
                    b.runtime.launch_side as f32
                };
                let side = b.effect_side();
                let mut impact = entered_ground && b.admit_contact();
                if impact {
                    hits.push(b.impact(b.y, side, false));
                }
                let mut burst = impact;
                for target in targets {
                    if !circle_hits_rect([b.x, b.y], radius, target.rect) {
                        continue;
                    }
                    let physical = target.physical && b.admit_contact();
                    if physical {
                        hits.push(b.impact(b.y, side, false));
                    }
                    let admitted = record_returned_hit(
                        b,
                        target.target,
                        target.takes_damage,
                        d.boulder_max_hits,
                        direction,
                        &mut hits,
                    );
                    if physical || (admitted && target.huge) {
                        burst = true;
                        impact |= physical;
                        break;
                    }
                }
                if !burst {
                    if let Some(wall) = walls
                        .iter()
                        .find(|w| circle_hits_polygon([b.x, b.y], radius, &w.polygon.points))
                    {
                        impact = b.admit_contact();
                        if impact {
                            hits.push(b.impact(b.y, side, false));
                        }
                        record_returned_hit(
                            b,
                            BoulderTargetKind::Object(wall.object),
                            true,
                            d.boulder_max_hits,
                            direction,
                            &mut hits,
                        );
                        burst = impact;
                    }
                }
                if burst {
                    hits.push(b.burst(side, impact));
                    return false;
                }
            }
            true
        });
        hits
    }
}

fn record_returned_hit(
    boulder: &mut Boulder,
    target: BoulderTargetKind,
    takes_damage: bool,
    max_hits: u8,
    direction: f32,
    hits: &mut Vec<BoulderHit>,
) -> bool {
    let seen = &mut boulder.runtime.returned_hits;
    if seen.len() >= max_hits as usize || seen.contains(&target) {
        return false;
    }
    // The PC HashSet counts the collider object even if its damage mask
    // rejects the hit. A Huge surface still shatters an immune boulder.
    seen.push(target);
    if takes_damage {
        hits.push(BoulderHit::Returned {
            target,
            damage: boulder.damage,
            direction,
        });
    }
    true
}

fn circle_hits_rect([x, y]: [f32; 2], radius: f32, rect: Rect) -> bool {
    let dx = x - x.clamp(rect.x, rect.x + rect.w);
    let dy = y - y.clamp(rect.y, rect.y + rect.h);
    dx * dx + dy * dy <= radius * radius
}

fn circle_hits_polygon([x, y]: [f32; 2], radius: f32, points: &[[f32; 2]]) -> bool {
    let mut inside = false;
    for i in 0..points.len() {
        let [ax, ay] = points[i];
        let [bx, by] = points[(i + 1) % points.len()];
        let dx = bx - ax;
        let dy = by - ay;
        let length_sq = dx * dx + dy * dy;
        if length_sq > 0.0 {
            let t = (((x - ax) * dx + (y - ay) * dy) / length_sq).clamp(0.0, 1.0);
            let sx = x - ax - dx * t;
            let sy = y - ay - dy * t;
            if sx * sx + sy * sy <= radius * radius {
                return true;
            }
        }
        if (ay > y) != (by > y) && x < ax + (y - ay) * dx / dy {
            inside = !inside;
        }
    }
    inside
}

/// One circular body against a horizontal plane. Normal restitution uses the
/// shipped Physics2D velocity threshold. Coulomb friction uses Box2D's material
/// mix and the disk inertia (I/m = r²/2); this resolves rolling during ghosting.
fn bounce_boulder_ground(b: &mut Boulder, radius: f32, restitution: f32, friction: f32) {
    if b.vy >= 0.0 {
        return;
    }
    let incoming = -b.vy;
    b.vy = if incoming > 1.0 {
        incoming * restitution
    } else {
        0.0
    };
    let cap = libm::sqrtf(friction * 0.4) * (incoming + b.vy);
    let omega = b.runtime.angular_velocity.to_radians();
    let impulse = (-(b.vx + radius * omega) / 3.0).clamp(-cap, cap);
    b.vx += impulse;
    b.runtime.angular_velocity = (omega + 2.0 * impulse / radius)
        .to_degrees()
        .clamp(-360.0, 360.0);
}

#[cfg(test)]
mod boulder_tests {
    use super::*;

    fn returned(world: &WorldData, side: i8, at: [f32; 2], remainder: f32) -> Siege {
        let mut siege = Siege::default();
        siege.boulders.push(Boulder::test_lodged(0.0));
        siege.identify_boulders();
        let b = &mut siege.boulders[0];
        assert!(b.hold_by(7));
        b.release_from_boss(at, side, 5.0, remainder, world, &mut Rng::new(9));
        siege
    }

    fn trigger(target: BoulderTargetKind, takes_damage: bool, huge: bool) -> BoulderTarget {
        BoulderTarget {
            target,
            rect: Rect {
                x: -3.0,
                y: 0.0,
                w: 6.0,
                h: 3.0,
            },
            physical: false,
            takes_damage,
            huge,
        }
    }

    #[test]
    fn lodged_boulders_keep_authored_height_and_remain_catchable() {
        let mut siege = Siege::default();
        let mut b = Boulder::test_lodged(2.0);
        b.stuck = None;
        b.y = 0.01;
        b.vy = -1.0;
        siege.boulders.push(b);
        let mut rng = Rng::new(2);
        siege.update_boulders(0.1, 1, &[], &[], &Default::default(), &mut rng);
        assert_eq!(siege.boulders[0].stuck, Some(0.0));
        assert_eq!(siege.boulders[0].y, 0.9375 - WALK_Y);
        siege.update_boulders(20.0, 1, &[], &[], &Default::default(), &mut rng);
        assert!(siege.boulders[0].hold_by(8));
        assert!(
            siege.boulders[0].scan_eligible(),
            "held object remains on Projectiles"
        );
        assert!(!siege.boulders[0].hold_by(9));
        siege.update_boulders(20.0, 1, &[], &[], &Default::default(), &mut rng);
        assert_eq!(siege.boulders[0].runtime.held_by, Some(8));
        assert_eq!(siege.boulders[0].y, 0.9375 - WALK_Y);
    }

    #[test]
    fn return_uses_unnormalized_velocity_and_only_post_marker_fixed_time() {
        let world = WorldData::default();
        let d = &world.boss_throw;
        for side in [-1, 1] {
            let mut s = returned(&world, side, [2.0, 1.0], 0.005);
            let b = &s.boulders[0];
            assert_eq!([b.vx, b.vy], [side as f32 * 5.0, 5.0]);
            assert_eq!(b.runtime.id, 1, "relaunch reuses the same boulder");
            assert_eq!(b.runtime.held_by, None);
            assert!(b.runtime.hostile);
            assert!(!b.scan_eligible());
            let omega = -side as f32 * 180.0 * (Rng::new(9).value() + 1.0);
            assert_eq!(b.runtime.angular_velocity, omega);
            s.update_returned_boulders(0.1, &world, &[], &[]);
            assert_eq!(s.boulders[0].x, 2.0);
            assert_eq!(s.boulders[0].ghost, d.boulder_ghost);
            s.update_returned_boulders(0.016, &world, &[], &[]);
            let b = &s.boulders[0];
            let vx = side as f32 * 5.0 / (1.0 + d.fixed_step * d.boulder_linear_drag);
            let vy =
                (5.0 - d.gravity * d.fixed_step) / (1.0 + d.fixed_step * d.boulder_linear_drag);
            assert!((b.x - (2.0 + vx * d.fixed_step)).abs() < 1e-6);
            assert!((b.y - (1.0 + vy * d.fixed_step)).abs() < 1e-6);
            assert!(
                (b.spin + omega * d.fixed_step / (1.0 + d.fixed_step * d.boulder_angular_drag))
                    .abs()
                    < 1e-5
            );
        }
    }

    #[test]
    fn return_ghost_ignores_walls_and_citizens_then_damages_and_shatters() {
        let world = WorldData::default();
        let h = world.boss_throw.fixed_step;
        let mut siege = returned(&world, 1, [0.0, 1.0], h);
        let targets = [trigger(BoulderTargetKind::Unit(1), true, false)];
        let walls = [BoulderWall {
            object: 4,
            polygon: crate::data::ObstacleDef {
                origin: [0.0, 0.0],
                points: vec![[-2.0, 0.0], [2.0, 0.0], [2.0, 3.0], [-2.0, 3.0]],
            },
        }];
        for _ in 0..15 {
            assert!(siege
                .update_returned_boulders(h, &world, &targets, &walls)
                .is_empty());
            assert_eq!(siege.boulders.len(), 1);
        }
        assert!(siege.boulders[0].scan_eligible());
        assert!(matches!(
            siege
                .update_returned_boulders(h, &world, &targets, &walls)
                .as_slice(),
            [
                BoulderHit::Returned {
                    target: BoulderTargetKind::Unit(1),
                    damage: 15,
                    direction: 1.0
                },
                BoulderHit::Impact {
                    side: 1,
                    collision_sound: false,
                    ..
                },
                BoulderHit::Returned {
                    target: BoulderTargetKind::Object(4),
                    damage: 15,
                    direction: 1.0
                },
                BoulderHit::Burst { impact: true, .. }
            ]
        ));
        assert!(siege.boulders.is_empty());
    }

    #[test]
    fn returned_damage_is_deduplicated_and_immunity_still_counts_toward_cap() {
        let world = WorldData::default();
        let h = world.boss_throw.fixed_step;
        let mut siege = returned(&world, -1, [0.0, 1.0], h);
        siege.boulders[0].ghost = 0.0;
        let targets = [
            trigger(BoulderTargetKind::Unit(1), true, false),
            trigger(BoulderTargetKind::Unit(1), true, false),
            trigger(BoulderTargetKind::Enemy(1), false, false),
            trigger(BoulderTargetKind::Unit(2), true, false),
            trigger(BoulderTargetKind::Unit(3), true, false),
            trigger(BoulderTargetKind::Unit(4), true, false),
            trigger(BoulderTargetKind::Unit(5), true, false),
        ];
        let hits = siege.update_returned_boulders(h, &world, &targets, &[]);
        assert_eq!(hits.len(), 4);
        for (i, hit) in hits.iter().enumerate() {
            assert_eq!(
                *hit,
                BoulderHit::Returned {
                    target: BoulderTargetKind::Unit(i as u32 + 1),
                    damage: 15,
                    direction: -1.0,
                }
            );
        }
        assert!(siege
            .update_returned_boulders(h, &world, &targets, &[])
            .is_empty());
        assert_eq!(siege.boulders.len(), 1);
        let huge = [trigger(BoulderTargetKind::Enemy(7), false, true)];
        assert!(siege
            .update_returned_boulders(h, &world, &huge, &[])
            .is_empty());
        let mut fresh = returned(&world, -1, [0.0, 1.0], h);
        fresh.boulders[0].ghost = 0.0;
        assert!(matches!(
            fresh
                .update_returned_boulders(h, &world, &huge, &[])
                .as_slice(),
            [BoulderHit::Burst { impact: false, .. }]
        ));
        assert!(
            fresh.boulders.is_empty(),
            "a Huge surface bursts even when immune"
        );
    }

    #[test]
    fn returned_boulder_bounces_on_ground_during_grace_and_bursts_after_it() {
        let world = WorldData::default();
        let d = &world.boss_throw;
        let h = d.fixed_step;
        let floor = d.ground_y - WALK_Y + d.boulder_radius;
        let mut siege = returned(&world, 1, [0.0, floor + 0.01], h);
        siege.boulders[0].vy = -3.0;
        assert!(siege
            .update_returned_boulders(h, &world, &[], &[])
            .is_empty());
        let b = &siege.boulders[0];
        assert_eq!(b.y, floor);
        assert!(
            (b.vy - 3.3 * d.boulder_restitution / (1.0 + h * d.boulder_linear_drag)).abs() < 1e-6
        );
        assert!(b.vx < 5.0, "disk friction is active while ghosting");
        let mut solid = returned(&world, 1, [0.0, floor + 0.01], h);
        solid.boulders[0].vy = -3.0;
        solid.boulders[0].ghost = 0.0;
        assert!(matches!(
            solid
                .update_returned_boulders(h, &world, &[], &[])
                .as_slice(),
            [
                BoulderHit::Impact {
                    collision_sound: false,
                    ..
                },
                BoulderHit::Burst { impact: true, .. }
            ]
        ));
        assert!(solid.boulders.is_empty());
    }

    #[test]
    fn returned_boulder_checks_the_authored_polygon_not_the_wall_aabb() {
        let world = WorldData::default();
        let h = world.boss_throw.fixed_step;
        let mut siege = returned(&world, 1, [1.0, 1.0], h);
        siege.boulders[0].ghost = 0.0;
        let walls = [BoulderWall {
            object: 4,
            polygon: crate::data::ObstacleDef {
                origin: [0.0, 0.0],
                points: vec![
                    [0.0, 0.0],
                    [2.0, 0.0],
                    [2.0, 2.0],
                    [1.8, 2.0],
                    [1.8, 0.2],
                    [0.2, 0.2],
                    [0.2, 2.0],
                    [0.0, 2.0],
                ],
            },
        }];
        assert!(siege
            .update_returned_boulders(h, &world, &[], &walls)
            .is_empty());
        siege.boulders[0].x = 1.56;
        assert!(matches!(
            siege
                .update_returned_boulders(h, &world, &[], &walls)
                .as_slice(),
            [
                BoulderHit::Impact {
                    collision_sound: false,
                    ..
                },
                BoulderHit::Returned {
                    target: BoulderTargetKind::Object(4),
                    ..
                },
                BoulderHit::Burst { impact: true, .. }
            ]
        ));
    }

    #[test]
    fn friendly_bounce_and_lodge_use_distinct_dust_and_shake_heights() {
        let def = crate::data::BossThrowDef::default();
        let floor = def.ground_y - WALK_Y + def.boulder_radius;
        let mut siege = Siege::default();
        let mut b = Boulder::test_lodged(0.0);
        b.stuck = None;
        b.bounces = 0;
        b.y = floor;
        b.vx = 1.0;
        b.vy = -1.0;
        siege.boulders.push(b);
        let mut rng = Rng::new(2);
        let hits = siege.update_boulders(0.02, 1, &[], &[], &def, &mut rng);
        assert!(matches!(hits.as_slice(), [BoulderHit::Impact {
            y, shake_y, side: 1, collision_sound: true, ..
        }] if *y == floor && *shake_y == floor));

        // Staying in contact cannot emit again once the cooldown expires.
        for _ in 0..12 {
            siege.boulders[0].vy = 0.0;
            assert!(siege
                .update_boulders(0.01, 1, &[], &[], &def, &mut rng)
                .is_empty());
        }
        siege.boulders[0].y = floor + 0.1;
        assert!(siege
            .update_boulders(0.01, 1, &[], &[], &def, &mut rng)
            .is_empty());
        siege.boulders[0].y = floor;
        siege.boulders[0].vx = -1.0;
        siege.boulders[0].vy = -1.0;
        let hits = siege.update_boulders(0.02, 1, &[], &[], &def, &mut rng);
        assert!(matches!(hits.as_slice(), [BoulderHit::Impact {
            y, shake_y, side: -1, collision_sound: true, ..
        }] if *y == floor && *shake_y == 0.9375 - WALK_Y));
        assert!(siege.boulders[0].stuck.is_some());
    }

    #[test]
    fn ghost_and_strict_physical_cooldown_suppress_dust_and_burst() {
        let def = crate::data::BossThrowDef::default();
        let floor = def.ground_y - WALK_Y + def.boulder_radius;
        for (ghost, age, dt, impact) in [
            (0.01, None, 0.02, false),
            (0.0, Some(0.0), 0.1, false),
            (0.0, Some(0.0), 0.101, true),
        ] {
            let mut siege = Siege::default();
            let mut b = Boulder::test_lodged(0.0);
            b.stuck = None;
            b.y = floor;
            b.vy = -1.0;
            b.stuck_probability = 0.0;
            b.ghost = ghost;
            b.runtime.contact_age = age;
            siege.boulders.push(b);
            let hits = siege.update_boulders(dt, 1, &[], &[], &def, &mut Rng::new(4));
            if impact {
                assert!(matches!(
                    hits.as_slice(),
                    [
                        BoulderHit::Impact {
                            side: -1,
                            collision_sound: false,
                            ..
                        },
                        BoulderHit::Burst {
                            side: -1,
                            impact: true,
                            ..
                        }
                    ]
                ));
                assert!(siege.boulders.is_empty());
            } else {
                assert!(hits.is_empty());
                assert_eq!(siege.boulders.len(), 1);
            }
        }
    }

    #[test]
    fn friendly_huge_trigger_shatters_in_the_air_without_physical_impact() {
        let def = crate::data::BossThrowDef::default();
        let mut siege = Siege::default();
        let mut b = Boulder::test_lodged(0.0);
        b.stuck = None;
        b.y = 1.0;
        b.vx = 1.0;
        siege.boulders.push(b);
        let targets = [BoulderEnemy {
            id: 2,
            x: 0.0,
            huge: Some(Rect::centered(0.75, 0.9, 1.5)),
        }];
        assert!(matches!(
            siege
                .update_boulders(0.02, 1, &[], &targets, &def, &mut Rng::new(5))
                .as_slice(),
            [
                BoulderHit::Troll(2, 15),
                BoulderHit::Burst {
                    side: 1,
                    impact: false,
                    ..
                }
            ]
        ));
        assert!(siege.boulders.is_empty());
    }

    #[cfg(feature = "save")]
    #[test]
    fn returned_boulder_phase_restores_without_replaying_earlier_hits() {
        let world = WorldData::default();
        let h = world.boss_throw.fixed_step;
        let mut original = returned(&world, 1, [0.0, 1.0], h);
        original.boulders[0].ghost = 0.0;
        let targets = [trigger(BoulderTargetKind::Unit(1), true, false)];
        assert_eq!(
            original
                .update_returned_boulders(h, &world, &targets, &[])
                .len(),
            1
        );
        original.update_returned_boulders(h * 0.5, &world, &targets, &[]);
        let bytes = postcard::to_allocvec(&original).unwrap();
        let tail = postcard::to_allocvec(&original.boulder_states()).unwrap();
        let mut resumed: Siege = postcard::from_bytes(&bytes).unwrap();
        assert!(
            !resumed.boulders[0].runtime.hostile,
            "legacy prefix omits runtime"
        );
        assert!(resumed.restore_boulder_states(postcard::from_bytes(&tail).unwrap(), &world));
        assert_eq!(original, resumed);
        let mut corrupt = resumed.boulder_states();
        corrupt[0].fixed_pending = f32::NAN;
        assert!(!resumed.restore_boulder_states(corrupt, &world));
        assert!(!resumed.restore_boulder_states(vec![], &world));
        assert_eq!(original, resumed, "invalid state is rejected atomically");
        assert!(resumed
            .update_returned_boulders(h, &world, &targets, &[])
            .is_empty());
        original.update_returned_boulders(h, &world, &targets, &[]);
        assert_eq!(original, resumed);
    }
}

fn advance_recoil(
    state: &mut Option<&mut crate::recoil::State>,
    definition: Option<&crate::recoil::Definition>,
    dt: f32,
) {
    if let (Some(state), Some(definition)) = (state.as_mut(), definition) {
        state.advance(dt, definition);
    }
}

#[cfg(test)]
mod animation_tests {
    use super::*;
    use crate::animation::{AnimatorState, Clip, Event, Key, Pose, Tangent, TransformTrack};
    use crate::data::{CatapultDef, Layer, Proto, TransformNode};

    fn fixture() -> (WorldData, Level, Siege, Rng) {
        let mut world = WorldData::default();
        let mut physics = crate::recoil::fixture();
        // This synthetic fixture's level origin is the ground surface.
        physics.ground_height = 0.0;
        let node = |path: &str, parent, position| TransformNode {
            path: path.into(),
            parent,
            local: Pose {
                position,
                ..Default::default()
            },
            animator: None,
        };
        world.protos.push(Proto {
            catapult: Some(CatapultDef {
                crank_rate: 0.1,
                force: (7.0, 7.0),
                scan: (2.0, 8.0),
                hit_points: 4,
                boulder_damage: 15,
                stuck_probability: 0.5,
                spoon: (99.0, 99.0),
                recoil: Some(physics),
            }),
            transforms: vec![
                node("", None, [0.0; 3]),
                node("Flippable", Some(0), [2.0, 0.0, 0.0]),
                node("Flippable/Arm", Some(1), [0.0; 3]),
                node("Flippable/Arm/Spoon", Some(2), [1.0, 0.0, 0.0]),
            ],
            ..Default::default()
        });
        world.animations.clips.push((
            "catapult_fire".into(),
            Clip {
                length: 2.1333334,
                looping: false,
                tracks: vec![TransformTrack {
                    path: "Flippable/Arm".into(),
                    euler_z: vec![Key {
                        time: 0.0,
                        value: 90.0,
                        in_tangent: Tangent::Slope(0.0),
                        out_tangent: Tangent::Slope(0.0),
                    }],
                    ..Default::default()
                }],
                events: [
                    (0.15, EventKind::ProjectileRelease),
                    (0.16666667, EventKind::ArmImpact),
                    (1.1666666, EventKind::FiringDone),
                ]
                .into_iter()
                .map(|(time, kind)| Event {
                    time,
                    kind,
                    float_parameter: 0.0,
                    int_parameter: 0,
                    string_parameter: Default::default(),
                })
                .collect(),
            },
        ));
        world.animations.states.push(AnimatorState {
            controller: "catapult".into(),
            name: "Fire".into(),
            clip: Some("catapult_fire".into()),
            speed: 1.0,
            speed_parameter: None,
            cycle_offset: 0.0,
        });
        let mut level = Level::empty(-20.0, 40.0);
        let object = level.add(level::instantiate(
            &world,
            0,
            Layer::Game,
            8.0,
            0.0,
            0.0,
            1.0,
        ));
        level.objects[0].mode = mode::READY;
        let siege = Siege {
            catapults: vec![Catapult {
                object,
                side: 1,
                counter: 0,
                fire_t: 0.0,
                released: false,
            }],
            boulders: vec![],
            recoils: vec![],
            next_boulder_id: 0,
        };
        (world, level, siege, Rng::new(80))
    }

    fn advance(
        s: &mut Siege,
        w: &WorldData,
        l: &mut Level,
        rng: &mut Rng,
        dt: f32,
    ) -> Vec<CatapultEvent> {
        let mut events = Vec::new();
        s.update(
            dt,
            w,
            l,
            Borders {
                left: -10.0,
                right: 10.0,
            },
            &[],
            &[BoulderEnemy {
                id: 100,
                x: 12.0,
                huge: None,
            }],
            rng,
            |e| events.push(e),
        );
        events
    }

    #[test]
    fn catapult_markers_release_at_animated_spoon_and_finish_before_clip_end() {
        let (mut w, mut level, mut s, mut rng) = fixture();
        // Isolate marker/socket animation from falling/settling in this test.
        w.protos[0].catapult.as_mut().unwrap().recoil = None;
        assert!(matches!(
            advance(&mut s, &w, &mut level, &mut rng, 0.0).as_slice(),
            [CatapultEvent::Fired { .. }]
        ));
        assert!(advance(&mut s, &w, &mut level, &mut rng, 0.14).is_empty());
        let released = advance(&mut s, &w, &mut level, &mut rng, 0.02);
        assert_eq!(
            released,
            vec![CatapultEvent::Released(s.catapults[0].object)]
        );
        let b = &s.boulders[0];
        assert!(
            (b.x - b.vx * 0.01 - 10.0).abs() < 1e-5,
            "uses composed spoon, not static fallback"
        );
        assert!((b.y - b.vy * 0.01 - (1.0 - WALK_Y)).abs() < 1e-5);
        assert_eq!(
            advance(&mut s, &w, &mut level, &mut rng, 0.01),
            vec![CatapultEvent::ArmImpact(s.catapults[0].object)]
        );
        let done = advance(&mut s, &w, &mut level, &mut rng, 1.0);
        assert_eq!(done, vec![CatapultEvent::Finished(s.catapults[0].object)]);
        assert_eq!(level.objects[0].mode, mode::RELOADING);
        assert!(s.catapults[0].fire_t < w.animations.clip("catapult_fire").unwrap().length);
        assert!(advance(&mut s, &w, &mut level, &mut rng, 1.0).is_empty());
    }

    #[test]
    fn catapult_catchup_uses_controller_speed_once_and_removal_cancels_pending_markers() {
        let (mut w, mut level, mut s, mut rng) = fixture();
        w.animations.states[0].speed = 2.0;
        advance(&mut s, &w, &mut level, &mut rng, 0.0);
        let id = s.catapults[0].object;
        assert_eq!(
            advance(&mut s, &w, &mut level, &mut rng, 0.6),
            vec![
                CatapultEvent::Released(id),
                CatapultEvent::ArmImpact(id),
                CatapultEvent::Finished(id)
            ]
        );
        assert!(advance(&mut s, &w, &mut level, &mut rng, 0.6).is_empty());
        let (w, mut level, mut s, mut rng) = fixture();
        advance(&mut s, &w, &mut level, &mut rng, 0.0);
        advance(&mut s, &w, &mut level, &mut rng, 0.1);
        level.objects.clear();
        assert!(advance(&mut s, &w, &mut level, &mut rng, 2.0).is_empty());
        assert!(s.boulders.is_empty());
    }

    #[cfg(feature = "save")]
    #[test]
    fn catapult_marker_phase_roundtrips_before_and_after_release() {
        let (w, mut level, mut s, mut rng) = fixture();
        advance(&mut s, &w, &mut level, &mut rng, 0.0);
        for prefix in [0.14, 0.03] {
            advance(&mut s, &w, &mut level, &mut rng, prefix);
            let mut resumed: Siege =
                postcard::from_bytes(&postcard::to_allocvec(&s).unwrap()).unwrap();
            assert!(resumed.restore_recoils(s.recoils.clone(), &w, &level));
            let mut other_level = level.clone();
            let mut other_rng = rng.clone();
            for dt in [0.02, 0.04, 1.0] {
                let mut original = s.clone();
                let mut l = level.clone();
                let mut r = rng.clone();
                assert_eq!(
                    advance(&mut original, &w, &mut l, &mut r, dt),
                    advance(&mut resumed, &w, &mut other_level, &mut other_rng, dt)
                );
                assert_eq!(original, resumed);
                resumed = postcard::from_bytes(&postcard::to_allocvec(&s).unwrap()).unwrap();
                assert!(resumed.restore_recoils(s.recoils.clone(), &w, &level));
                other_level = level.clone();
                other_rng = rng.clone();
            }
        }
        assert!(s.catapults[0].released);
        let at = s.catapults[0].fire_t;
        s.migrate_animation_clocks();
        assert_eq!(
            s.catapults[0].fire_t, at,
            "migration must not replay released payloads"
        );
        s.catapults[0].released = false;
        s.migrate_animation_clocks();
        assert_eq!(s.catapults[0].fire_t, 0.0);
    }

    #[test]
    fn recoil_starts_at_impact_continues_after_interruption_and_removes_with_body() {
        let (w, mut level, mut s, mut rng) = fixture();
        let d = w.protos[0].catapult.unwrap().recoil.unwrap();
        advance(&mut s, &w, &mut level, &mut rng, 0.0);
        let mut unforced = s.recoils[0];
        unforced.advance(0.15, &d);
        unforced.advance(0.16 - 0.15, &d);
        advance(&mut s, &w, &mut level, &mut rng, 0.16);
        assert!(
            (s.recoils[0].bodies[0].angular_velocity - unforced.bodies[0].angular_velocity).abs()
                < 1e-6,
            "release does not recoil the chassis"
        );
        let events = advance(&mut s, &w, &mut level, &mut rng, 0.04);
        assert_eq!(
            events,
            vec![CatapultEvent::ArmImpact(s.catapults[0].object)]
        );
        assert!(
            s.recoils[0].bodies[0].angle < -0.01 && s.recoils[0].bodies[0].angular_velocity < -0.01
        );
        let impulse_pose = s.recoil_pose(s.catapults[0].object, &d);
        assert!(impulse_pose.basis[2] < -0.01);
        let paused = s.recoils.clone();
        assert!(advance(&mut s, &w, &mut level, &mut rng, 0.0).is_empty());
        assert_eq!(s.recoils, paused);
        // Cancel the animation; an already-applied physical impulse still settles.
        level.objects[0].mode = mode::RELOADING;
        assert!(advance(&mut s, &w, &mut level, &mut rng, 2.0).is_empty());
        assert!(s.recoils[0].bodies[0].angle.abs() < 0.02);
        level.objects.clear();
        advance(&mut s, &w, &mut level, &mut rng, 0.0);
        assert!(s.recoils.is_empty());
    }

    #[test]
    fn interrupted_preimpact_animation_cannot_leave_a_delayed_recoil() {
        let (w, mut level, mut s, mut rng) = fixture();
        advance(&mut s, &w, &mut level, &mut rng, 0.0);
        advance(&mut s, &w, &mut level, &mut rng, 0.1);
        level.objects[0].mode = mode::RELOADING;
        let mut unforced = s.recoils[0];
        unforced.advance(1.0, &w.protos[0].catapult.unwrap().recoil.unwrap());
        advance(&mut s, &w, &mut level, &mut rng, 1.0);
        assert_eq!(s.recoils[0].bodies[0].angle, unforced.bodies[0].angle);
        assert_eq!(
            s.recoils[0].bodies[0].angular_velocity,
            unforced.bodies[0].angular_velocity
        );
    }

    #[test]
    fn boulder_release_uses_live_body_pose_and_recoil_restore_rejects_wrong_identity() {
        let (w, mut level, mut s, mut rng) = fixture();
        let d = w.protos[0].catapult.unwrap().recoil.unwrap();
        advance(&mut s, &w, &mut level, &mut rng, 0.0);
        s.recoils[0].impact(&d);
        let mut expected = s.recoils[0];
        expected.advance(0.15, &d);
        let spoon = expected.pose(&d).point([2.0, 1.0]);
        advance(&mut s, &w, &mut level, &mut rng, 0.15);
        let b = &s.boulders[0];
        assert!((b.x - (8.0 + spoon[0])).abs() < 1e-5);
        assert!((b.y - (spoon[1] - WALK_Y)).abs() < 1e-5);
        assert_eq!(b.ghost, 0.3);
        assert!(s.recoils_valid(&w, &level));
        let original = s.recoils.clone();
        let mut invalid = original.clone();
        invalid[0].side = -1;
        assert!(!s.restore_recoils(invalid, &w, &level));
        let mut duplicate = original.clone();
        duplicate.push(original[0]);
        assert!(!s.restore_recoils(duplicate, &w, &level));
        let mut invalid = original.clone();
        invalid[0].bodies[0].angle = f32::NAN;
        assert!(!s.restore_recoils(invalid, &w, &level));
        assert_eq!(s.recoils, original);
    }

    #[test]
    fn builders_drive_chassis_through_wheel_contacts_and_reload_stops_only_chassis() {
        let (w, mut level, mut siege, mut rng) = fixture();
        let id = siege.catapults[0].object;
        let d = w.protos[0].catapult.unwrap().recoil.unwrap();
        level.objects[0].mode = mode::MOVING;
        for _ in 0..180 {
            siege.update(
                crate::state::TICK,
                &w,
                &mut level,
                Borders {
                    left: -20.,
                    right: 20.,
                },
                &[(id, 2)],
                &[],
                &mut rng,
                |_| {},
            );
        }
        let o = &level.objects[0];
        assert!(
            o.x > 8.1 && o.x < 8.91,
            "drive displacement must come from physics: {}",
            o.x
        );
        assert!((o.y - d.ground_height).abs() < 0.03);
        let r = &siege.recoils[0];
        assert!(
            r.bodies[1].angular_velocity < -0.05 && r.bodies[2].angular_velocity < -0.05,
            "ground friction rolls both wheels"
        );
        assert!(
            r.pose(&d).translation.iter().all(|x| x.abs() < 1e-6),
            "root displacement is committed once"
        );
        assert!((r.ground_y + o.y - d.ground_height).abs() < 1e-6);
        let wheel_velocity = r.bodies[1].velocity;
        siege.update(
            0.,
            &w,
            &mut level,
            Borders {
                left: -10.,
                right: 10.,
            },
            &[(id, 2)],
            &[],
            &mut rng,
            |_| {},
        );
        assert_eq!(level.objects[0].mode, mode::RELOADING);
        assert_eq!(siege.recoils[0].bodies[0].velocity, [0., 0.]);
        assert_eq!(
            siege.recoils[0].bodies[1].velocity, wheel_velocity,
            "SetState Reloading does not freeze wheel bodies"
        );
    }

    #[test]
    fn border_movement_cancels_ready_fire_before_any_animation_marker() {
        let (w, mut level, mut siege, mut rng) = fixture();
        let id = siege.catapults[0].object;
        let mut events = Vec::new();
        siege.update(
            0.,
            &w,
            &mut level,
            Borders {
                left: -20.,
                right: 20.,
            },
            &[],
            &[BoulderEnemy {
                id: 100,
                x: 12.0,
                huge: None,
            }],
            &mut rng,
            |e| events.push(e),
        );
        assert!(matches!(events.as_slice(), [CatapultEvent::Fired { .. }]));
        assert_eq!(level.objects[0].mode, mode::MOVING);
        events.clear();
        siege.update(
            2.,
            &w,
            &mut level,
            Borders {
                left: -20.,
                right: 20.,
            },
            &[],
            &[BoulderEnemy {
                id: 100,
                x: 12.0,
                huge: None,
            }],
            &mut rng,
            |e| events.push(e),
        );
        assert!(events.is_empty());
        assert!(siege.boulders.is_empty());
        assert_eq!(siege.catapults[0].object, id);
        assert_eq!(siege.catapults[0].fire_t, 0.);
    }

    #[test]
    fn released_boulders_only_fly_after_marker_while_existing_boulders_get_full_tick() {
        for dt in [0.2, 0.6] {
            let (mut w, mut level, mut siege, mut rng) = fixture();
            w.protos[0].catapult.as_mut().unwrap().recoil = None;
            level.objects[0].y = 10.; // isolate flight timing from ground contacts
            advance(&mut siege, &w, &mut level, &mut rng, 0.);
            advance(&mut siege, &w, &mut level, &mut rng, 0.15);
            let previous = siege.boulders[0].clone();
            assert_eq!(previous.x, 10.);
            assert_eq!(previous.ghost, 0.3);
            level.objects[0].mode = mode::READY;
            advance(&mut siege, &w, &mut level, &mut rng, 0.);
            advance(&mut siege, &w, &mut level, &mut rng, dt);
            assert_eq!(siege.boulders.len(), 2);
            let old = &siege.boulders[0];
            let new = &siege.boulders[1];
            assert!((old.x - previous.x - previous.vx * dt).abs() < 1e-5);
            assert!((new.x - 10. - new.vx * (dt - 0.15)).abs() < 1e-5);
            assert!((old.ghost - (0.3 - dt)).abs() < 1e-6);
            assert!((new.ghost - (0.3 - (dt - 0.15))).abs() < 1e-6);
        }
    }
}
