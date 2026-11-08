//! The greed: wave scheduling, portals spawning trolls, troll behaviour,
//! arrows, and the monarch's dropped crown.
//!
//! orig: `Director.ScheduleWaveToday()`, `EnemyManager` (`SpawnWave`,
//! `GetEnemies`, `CheckDanger`, wave cohesion), `Portal` (spawn queue),
//! `Troll`, `Arrow`, `Crown`, portal defence / destruction / rebuilding
//! and retaliation waves, squid captives and bosses.

use alloc::vec::Vec;

use crate::coins::Rect;
use crate::data::{ToolKind, WorldData};
use crate::director::{Season, WaveKind};
use crate::kingdom::Borders;
use crate::level::Level;
use crate::rng::Rng;
use crate::time::{SECONDS_PER_HOUR, WAVE_AT_CASTLE};
use crate::units::Mover;

/// orig: Troll prefab.
mod troll {
    pub const WALK: f32 = 2.0;
    pub const RUN: f32 = 2.8;
    pub const ANGRY_COOLDOWN: f32 = 12.0;
    pub const CHARGE_RANGE: f32 = 2.0;
    pub const CHARGE_SPEED: f32 = 3.0;
    pub const CHARGE_JUMP: (f32, f32) = (8.0, 2.3);
    pub const WALL_BOUNCE: (f32, f32) = (-3.0, 1.0);
    pub const ATTACK_COOLDOWN: f32 = 1.0;
    pub const HIT_DAMAGE: i32 = 1;
    pub const LOOT_RANGE: f32 = 3.0;
    pub const TOUGH_HEALTH: i32 = 5;
}
/// orig: Troll prefab z.
pub const TROLL_Z: f32 = 0.6;
/// Kingdom stays "in danger" while attackers are within this of the borders.
/// orig: `EnemyManager.safeBorderDistance`.
const SAFE_BORDER_DISTANCE: f32 = 20.0;
/// orig: `EnemyManager.retaliationDelayHours` (scene).
const RETALIATION_DELAY_HOURS: f32 = 2.0;
/// orig: `Director.ScheduleWaveToday`: the portal opens this many hours before the spawn.
const PORTAL_OPEN_LEAD: f32 = 6.0;
/// orig: Physics2DSettings gravity.
const GRAVITY: f32 = 15.0;
/// orig: Troll prefab trigger: offset (0, 0.35), size (0.3, 0.6).
const TROLL_BOX: Rect = Rect::centered(0.35, 0.3, 0.6);
/// orig: `Archer.maxForce` (prefab) on a 1 kg arrow.
pub const ARROW_SPEED: f32 = 8.5;
/// orig: Crown prefab gravity scale.
const CROWN_GRAVITY: f32 = 0.7 * GRAVITY;

/// orig: `EnemyType` (the ones ported).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnemyKind {
    Troll,
    ToughTroll,
    /// Flies over the walls and carries citizens off. orig: `Squid`.
    Squid,
    /// Smashes walls, spits out trolls. orig: `Boss`.
    Boss,
    /// A boss with 5000 hit points. orig: `EnemyType.KillerBoss`.
    KillerBoss,
}

impl EnemyKind {
    pub fn is_boss(self) -> bool {
        matches!(self, EnemyKind::Boss | EnemyKind::KillerBoss)
    }
}

/// orig: Squid prefab.
mod squid {
    pub const SCAN_INTERVAL: f32 = 0.5;
    pub const SCAN_CAPACITY: usize = 12;
    pub const HP: i32 = 22;
}

/// orig: Boss prefab.
mod boss {
    pub const WALK: f32 = 1.2;
    pub const HP: i32 = 90;
    pub const KILLER_HP: i32 = 5000;
    pub const HIT_DAMAGE: i32 = 22;
    /// orig: `hitBox` (x 0, y 0.4, 1.6 × 0.7) in front.
    pub const REACH: f32 = 1.6;
    pub const BASH_TIME: f32 = 1.2;
    /// The punch lands partway into the bash clip.
    pub const BASH_IMPACT: f32 = 0.6;
    pub const THROW_PAUSE: f32 = 2.0;
    pub const THROW_TIME: f32 = 2.6;
    pub const RELEASE_AT: f32 = 0.8333333;
    pub const SCAN_INTERVAL: f32 = 1.0 / 6.0;
    pub const PUKE_RANGE: f32 = 8.0;
    pub const PUKE_INTERVAL: f32 = 10.0;
    pub const PUKE_AMOUNT: usize = 4;
    pub const PUKE_OFFSET: (f32, f32) = (0.8, 1.2);
}

/// What a troll carries off. orig: `Troll._loot`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loot {
    Coin,
    Tool(ToolKind),
    Crown,
    /// A part stacked at the wharf (index into `WharfDef::parts`).
    /// orig: the `BoatPart` droppable.
    BoatPart(u16),
    /// A stolen tool retains its identity and shield allegiance while carried.
    /// Appended after the v5 variants so existing saves remain decodable.
    CarriedTool {
        kind: ToolKind,
        id: u32,
        shield_shop: Option<u32>,
    },
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TrollState {
    /// orig: `Troll.Chase()`: walk at the monarch (or crown) for 3–5 s at a time.
    Chase { t: f32 },
    /// orig: `Troll.GrabLoot()`.
    GrabLoot { x: f32 },
    /// orig: `Troll.Charge()`: leap at a target; `hit` once something was struck.
    Charge {
        t: f32,
        landed: bool,
        hit: bool,
        recover: f32,
    },
    /// orig: `Troll.Retreat()`.
    Retreat { t: f32 },
    /// Knocked out, fading. orig: Damageable death + `troll_die`.
    Dead { t: f32 },
    /// Squid has already grabbed a citizen; wait for the authored completion
    /// marker and its 0.1-second carry-position settle before another grab.
    /// Appended in save v7; older enum discriminants remain unchanged.
    GrabCitizen { t: f32 },
    /// Boss throw coroutine. Appended in v11 to preserve the earlier tags.
    ThrowBoulder { t: f32 },
}

#[derive(Clone, Copy, Debug)]
enum ProjectileRef {
    Boulder(u64),
    Arrow(u64),
    Bolt(u64),
}

#[derive(Clone, Debug)]
struct BossSight {
    elapsed: f32,
    projectiles: Vec<ProjectileRef>,
}

impl Default for BossSight {
    fn default() -> Self {
        Self {
            elapsed: boss::SCAN_INTERVAL,
            projectiles: Vec::new(),
        }
    }
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug)]
pub struct Troll {
    pub id: u32,
    pub kind: EnemyKind,
    pub x: f32,
    pub y: f32,
    pub vy: f32,
    pub mover: Mover,
    pub hp: i32,
    pub state: TrollState,
    pub loot: Option<Loot>,
    pub wave: u32,
    pub boss_wave: bool,
    should_retreat: bool,
    /// Still part of an attack. orig: `Enemy.isAttacking`.
    attacking: bool,
    angry: f32,
    born: f32,
    wave_target_offset: f32,
    distance_from_wave_center: f32,
    speed_mult: f32,
    speed_mult_t: f32,
    target: Option<Target>,
    pub anim_t: f32,
    /// Squid: citizens it carries. orig: `_victims`.
    pub victims: u8,
    /// Boss: punching (time into it), puke timers. orig: `Boss` state.
    pub bash: Option<(f32, bool)>,
    pub puke: Option<f32>,
    since_puke: f32,
    puke_now: bool,
    reached_center: bool,
    #[cfg_attr(feature = "serde", serde(skip))]
    boss_sight: BossSight,
}

impl Troll {
    pub fn boss_throw_animation_time(&self, world: &WorldData) -> Option<f32> {
        let TrollState::ThrowBoulder { t } = self.state else {
            return None;
        };
        let time = t * boss_throw_speed(world);
        (time
            < world
                .animations
                .clip("boss_throw")
                .map_or(2.0, |c| c.length))
        .then_some(time)
    }

    pub fn boss_hand(&self, world: &WorldData) -> [f32; 2] {
        self.hand_at(world, self.boss_throw_animation_time(world))
    }

    fn hand_at(&self, world: &WorldData, clip_time: Option<f32>) -> [f32; 2] {
        let rest = world.boss_throw.hand;
        let p = clip_time.map_or(rest, |time| {
            world.animations.clip("boss_throw").map_or_else(
                || {
                    if time >= boss::RELEASE_AT {
                        [0.963, 0.876, 0.0]
                    } else {
                        rest
                    }
                },
                |clip| {
                    clip.pose(
                        "Hand",
                        time,
                        crate::animation::Pose {
                            position: rest,
                            ..Default::default()
                        },
                    )
                    .position
                },
            )
        });
        [self.x + p[0] * self.mover.facing as f32, self.y + p[1]]
    }

    pub fn grab_animation_time(&self) -> Option<f32> {
        match self.state {
            TrollState::GrabCitizen { t } => Some(t),
            _ => None,
        }
    }

    pub fn box_at(&self) -> Rect {
        match self.kind {
            EnemyKind::Boss | EnemyKind::KillerBoss => {
                Rect::centered(0.8, 1.0, 1.6).at(self.x, self.y, false)
            }
            EnemyKind::Squid => Rect::centered(0.3, 0.6, 0.6).at(self.x, self.y, false),
            _ => TROLL_BOX.at(self.x, self.y, false),
        }
    }

    pub fn is_dead(&self) -> bool {
        matches!(self.state, TrollState::Dead { .. })
    }

    fn airborne(&self) -> bool {
        self.y > 0.0 || self.vy > 0.0
    }
}

/// Something a troll can hit.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Monarch,
    Unit(u32),
    /// A wall object.
    Object(u32),
}

/// A potential troll target this tick, provided by the game state.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug)]
pub struct TargetInfo {
    pub target: Target,
    pub x: f32,
    pub rect: Rect,
    /// orig: `TargetCacher` priority list (archers, knights).
    pub priority: bool,
    pub wall: bool,
}

/// Citizens-layer colliders for the squid's independent scanner. Ground greed
/// immunity (tower slots or stun) does not remove a citizen from this layer.
#[derive(Clone, Copy, Debug)]
pub struct CitizenInfo {
    pub unit: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub rect: Rect,
    pub grabbable: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct ObstacleInfo {
    pub object: u32,
    pub x: f32,
    pub rect: Rect,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub enum SquidPhase {
    Chase,
    Retreat {
        wait: f32,
    },
    NoPortal {
        wait: f32,
    },
    /// PC discards the Despawn IEnumerator after the no-portal ten-second wait.
    Stranded,
    Despawning {
        elapsed: f32,
    },
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SquidAnimation {
    Move,
    DiveIn,
    DiveOut,
    Attack,
    Die,
}

impl SquidAnimation {
    fn state(self) -> &'static str {
        match self {
            Self::Move => "Move",
            Self::DiveIn => "Dive In",
            Self::DiveOut => "Dive Out",
            Self::Attack => "Attack",
            Self::Die => "Die",
        }
    }
    fn clip(self) -> &'static str {
        match self {
            Self::Move => "squid_move",
            Self::DiveIn => "squid_dive_in",
            Self::DiveOut => "squid_dive_out",
            Self::Attack => "squid_attack",
            Self::Die => "squid_death",
        }
    }
    fn length(self, world: &WorldData) -> f32 {
        world.animations.clip(self.clip()).map_or(
            match self {
                Self::Move => 1.0,
                Self::DiveIn => 1.0 / 3.0,
                Self::DiveOut => 0.375,
                Self::Attack => 0.75,
                Self::Die => 3.125,
            },
            |c| c.length,
        )
    }
    fn speed(self, world: &WorldData) -> f32 {
        world
            .animations
            .state_speed("squid", self.state())
            .unwrap_or(1.0)
    }
}

/// Saved outside the legacy Troll/Greed prefix. Physics and coroutine clocks
/// must survive a save without repeating a flap, transition or despawn effect.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct SquidRuntime {
    pub enemy: u32,
    pub flight: crate::flight::State,
    pub phase: SquidPhase,
    pub initialized: bool,
    pub altitude: f32,
    pub current_speed: f32,
    pub target: Option<u32>,
    pub transition_elapsed: f32,
    pub chase_wait: f32,
    pub chase_running: bool,
    pub animation: SquidAnimation,
    pub animation_elapsed: f32,
    pub dive_blend: Option<f32>,
    pub attack_pending: bool,
    pub grab_pending: bool,
    pub grab_settle: Option<f32>,
}

impl SquidRuntime {
    pub fn new(enemy: u32) -> Self {
        Self {
            enemy,
            flight: crate::flight::State::new(enemy),
            phase: SquidPhase::Chase,
            initialized: false,
            // _flyAltitude is initially zero in PC world coordinates.
            altitude: -crate::build::WALK_Y,
            current_speed: 0.0,
            target: None,
            transition_elapsed: 0.0,
            chase_wait: 0.0,
            chase_running: true,
            animation: SquidAnimation::Move,
            animation_elapsed: 0.0,
            dive_blend: None,
            attack_pending: false,
            grab_pending: false,
            grab_settle: None,
        }
    }

    fn valid(&self, def: &crate::flight::Definition) -> bool {
        self.flight.enemy == self.enemy
            && self.flight.valid(def)
            && self.altitude.is_finite()
            && self.altitude.abs() <= 10000.0
            && self.current_speed.is_finite()
            && ((!self.initialized && self.current_speed == 0.0)
                || (def.move_speed - 0.4..=def.move_speed).contains(&self.current_speed))
            && self.transition_elapsed.is_finite()
            && (0.0..=2.0).contains(&self.transition_elapsed)
            && self.chase_wait.is_finite()
            && (0.0..=5.0).contains(&self.chase_wait)
            && self.animation_elapsed.is_finite()
            && (0.0..=60.0).contains(&self.animation_elapsed)
            && self.dive_blend.is_none_or(|t| {
                self.animation == SquidAnimation::Move && t.is_finite() && (0.0..0.25).contains(&t)
            })
            && self
                .grab_settle
                .is_none_or(|t| t.is_finite() && (0.0..=0.1).contains(&t))
            && match self.phase {
                SquidPhase::Chase | SquidPhase::Stranded => true,
                SquidPhase::Retreat { wait } => wait.is_finite() && (0.0..=1.0).contains(&wait),
                SquidPhase::NoPortal { wait } => wait.is_finite() && (0.0..=10.0).contains(&wait),
                SquidPhase::Despawning { elapsed } => {
                    elapsed.is_finite() && (0.0..1.0).contains(&elapsed)
                }
            }
    }

    fn attack(&mut self) -> bool {
        if self.animation == SquidAnimation::Attack {
            self.attack_pending = true;
            false
        } else {
            self.animation = SquidAnimation::Attack;
            self.animation_elapsed = 0.0;
            self.dive_blend = None;
            true
        }
    }

    fn migrate_grab(&mut self, t: &Troll, world: &WorldData) {
        if let TrollState::GrabCitizen { t } = t.state {
            self.animation = SquidAnimation::Attack;
            self.animation_elapsed = t.min(self.animation.length(world));
            let marker = world
                .animations
                .clip("squid_attack")
                .and_then(|c| c.first_event_time(crate::animation::EventKind::GrabComplete))
                .unwrap_or(0.5);
            self.grab_pending = t < marker;
            self.grab_settle = (!self.grab_pending)
                .then(|| (0.1 - (t - marker) / self.animation.speed(world)).max(0.0));
        }
    }

    fn attack_marker(
        &mut self,
        world: &WorldData,
        before: f32,
        after: f32,
        remaining: f32,
    ) -> bool {
        if self.animation != SquidAnimation::Attack || !self.grab_pending {
            return false;
        }
        let marker = world
            .animations
            .clip("squid_attack")
            .and_then(|c| c.first_event_time(crate::animation::EventKind::GrabComplete))
            .or_else(|| world.animations.clips.is_empty().then_some(0.5));
        let Some(at) = marker.filter(|at| before < *at && after >= *at) else {
            return false;
        };
        self.grab_pending = false;
        self.grab_settle =
            Some((0.1 - remaining - (after - at) / self.animation.speed(world)).max(0.0));
        true
    }

    fn animate(&mut self, world: &WorldData, mut dt: f32, vy: f32, death: Option<f32>) -> bool {
        let mut complete = false;
        if let Some(time) = death {
            self.animation = SquidAnimation::Die;
            self.animation_elapsed = time.min(SquidAnimation::Die.length(world));
            self.dive_blend = None;
            self.attack_pending = false;
            self.grab_pending = false;
            self.grab_settle = None;
            return false;
        }
        for _ in 0..8 {
            let speed = self.animation.speed(world);
            let length = self.animation.length(world);
            if speed <= 0.0 || length <= 0.0 {
                return complete;
            }
            if let Some(blend) = self.dive_blend {
                let step = dt.min(0.25 - blend);
                self.animation_elapsed = (self.animation_elapsed + step * speed) % length;
                dt -= step;
                if blend + step < 0.25 {
                    self.dive_blend = Some(blend + step);
                    return complete;
                }
                self.dive_blend = None;
                self.animation = SquidAnimation::DiveIn;
                // Both Unity states run during the fixed-duration blend. Sprite
                // reference blending is discrete here: outgoing until handoff.
                self.animation_elapsed = 0.25 * self.animation.speed(world);
                continue;
            }
            let until = match self.animation {
                SquidAnimation::Move if vy < -0.3 => {
                    let exit = 0.75 * length;
                    let at = if self.animation_elapsed < exit {
                        exit
                    } else {
                        exit + length
                    };
                    (at - self.animation_elapsed) / speed
                }
                SquidAnimation::DiveIn if vy > -0.3 => {
                    ((length - self.animation_elapsed) / speed).max(0.0)
                }
                SquidAnimation::DiveOut | SquidAnimation::Attack => {
                    ((length - self.animation_elapsed) / speed).max(0.0)
                }
                _ => f32::INFINITY,
            };
            if dt < until {
                complete |= self.attack_marker(
                    world,
                    self.animation_elapsed,
                    self.animation_elapsed + dt * speed,
                    0.0,
                );
                self.animation_elapsed += dt * speed;
                if self.animation == SquidAnimation::Move {
                    self.animation_elapsed %= length;
                } else {
                    self.animation_elapsed = self.animation_elapsed.min(length);
                }
                return complete;
            }
            complete |= self.attack_marker(
                world,
                self.animation_elapsed,
                self.animation_elapsed + until * speed,
                dt - until,
            );
            self.animation_elapsed += until * speed;
            dt -= until;
            match self.animation {
                SquidAnimation::Move => self.dive_blend = Some(0.0),
                SquidAnimation::DiveIn => {
                    self.animation = SquidAnimation::DiveOut;
                    self.animation_elapsed = 0.0;
                }
                SquidAnimation::DiveOut | SquidAnimation::Attack => {
                    self.animation = if self.attack_pending {
                        self.attack_pending = false;
                        SquidAnimation::Attack
                    } else {
                        SquidAnimation::Move
                    };
                    self.animation_elapsed = 0.0;
                }
                _ => return complete,
            }
        }
        complete
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SquidVisual {
    pub overlay_elapsed: Option<f32>,
    pub vertical_speed: f32,
    pub death_alpha: f32,
    pub clip: &'static str,
    pub time: f32,
}

/// Scanner contents are transient PC object references. The independently
/// saved runtime holds the movement/retreat timeline, not these caches.
#[derive(Clone, Debug)]
struct SquidMind {
    enemy: u32,
    scan_elapsed: f32,
    citizens: Vec<u32>,
    obstacle_elapsed: f32,
    obstacles: Vec<u32>,
    runtime: SquidRuntime,
}

impl SquidMind {
    fn new(enemy: u32, rng: &mut Rng) -> Self {
        Self {
            enemy,
            // Scanner..ctor: lastRefresh = Time.time - Random.Range(0, interval).
            scan_elapsed: rng.range(0.0, squid::SCAN_INTERVAL),
            citizens: Vec::new(),
            obstacle_elapsed: rng.range(0.0, squid::SCAN_INTERVAL),
            obstacles: Vec::new(),
            runtime: SquidRuntime::new(enemy),
        }
    }

    fn closest(
        &mut self,
        t: &Troll,
        citizens: &[CitizenInfo],
        grabbed: &[u32],
        range: f32,
    ) -> Option<CitizenInfo> {
        if self.scan_elapsed >= squid::SCAN_INTERVAL {
            // Refresh on demand, at most once per interval; no catch-up scans.
            self.scan_elapsed = 0.0;
            let scan = Rect {
                x: t.x - range,
                y: 0.5 - crate::build::WALK_Y,
                w: 2.0 * range,
                h: t.y + 1.0 - (0.5 - crate::build::WALK_Y),
            };
            self.citizens.clear();
            // Unity supplies at most 12 overlap results before GetClosest.
            // Preserve snapshot order; equal-depth Unity broadphase ordering
            // is not recoverable from the managed assembly.
            self.citizens.extend(
                citizens
                    .iter()
                    .filter(|c| c.rect.overlaps(&scan) && !grabbed.contains(&c.unit))
                    .take(squid::SCAN_CAPACITY)
                    .map(|c| c.unit),
            );
        }
        // Cached references retain live positions even outside the old bounds.
        // GetClosest removes inactive/destroyed objects before selecting one.
        self.citizens
            .retain(|id| citizens.iter().any(|c| c.unit == *id));
        self.citizens
            .iter()
            .filter_map(|id| citizens.iter().find(|c| c.unit == *id))
            .min_by(|a, b| (a.x - t.x).abs().total_cmp(&(b.x - t.x).abs()))
            // CanBeGrabbedNow is checked only after selection, including a
            // citizen claimed earlier in the same update by another squid.
            .filter(|c| c.grabbable && !grabbed.contains(&c.unit))
            .copied()
    }

    fn obstacle(&mut self, t: &Troll, ctx: &GreedCtx) -> Option<f32> {
        if self.obstacle_elapsed >= squid::SCAN_INTERVAL {
            self.obstacle_elapsed = 0.0;
            let dir = t.mover.facing as f32;
            let a = t.x + 4.0 * dir;
            let b = t.x - dir;
            let scan = Rect {
                x: a.min(b),
                y: 0.5 - crate::build::WALK_Y,
                w: (a - b).abs(),
                h: t.y + 1.0 - (0.5 - crate::build::WALK_Y),
            };
            self.obstacles.clear();
            self.obstacles.extend(
                ctx.obstacles
                    .iter()
                    .enumerate()
                    .filter(|(i, o)| {
                        o.rect.overlaps(&scan)
                            && ctx
                                .solid_obstacles
                                .get(*i)
                                .is_none_or(|p| polygon_overlaps_rect(&p.points, scan))
                    })
                    .take(squid::SCAN_CAPACITY)
                    .map(|(_, o)| o.object),
            );
        }
        // Scanner.GetAny selects the first still-active cached result; it does
        // not choose the nearest obstacle or re-check the old overlap bounds.
        for id in &self.obstacles {
            if let Some(o) = ctx.obstacles.iter().find(|o| o.object == *id) {
                return Some(o.x);
            }
        }
        self.obstacles.clear();
        None
    }
}

fn polygon_overlaps_rect(points: &[[f32; 2]], rect: Rect) -> bool {
    if points.len() < 3 {
        return false;
    }
    if points.iter().any(|p| {
        p[0] >= rect.x && p[0] <= rect.x + rect.w && p[1] >= rect.y && p[1] <= rect.y + rect.h
    }) {
        return true;
    }
    let corners = [
        [rect.x, rect.y],
        [rect.x + rect.w, rect.y],
        [rect.x + rect.w, rect.y + rect.h],
        [rect.x, rect.y + rect.h],
    ];
    // General polygon overlap, including concave wall outlines.
    for corner in corners {
        let mut inside = false;
        for (a, b) in points
            .iter()
            .zip(points.iter().cycle().skip(1))
            .take(points.len())
        {
            if (a[1] > corner[1]) != (b[1] > corner[1])
                && corner[0] < (b[0] - a[0]) * (corner[1] - a[1]) / (b[1] - a[1]) + a[0]
            {
                inside = !inside;
            }
        }
        if inside {
            return true;
        }
    }
    let cross = |a: &[f32; 2], b: &[f32; 2], c: &[f32; 2]| {
        (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
    };
    for (a, b) in points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .take(points.len())
    {
        for (c, d) in corners.iter().zip(corners.iter().cycle().skip(1)).take(4) {
            if a[0].min(b[0]) <= c[0].max(d[0])
                && c[0].min(d[0]) <= a[0].max(b[0])
                && a[1].min(b[1]) <= c[1].max(d[1])
                && c[1].min(d[1]) <= a[1].max(b[1])
                && cross(a, b, c) * cross(a, b, d) <= 0.0
                && cross(c, d, a) * cross(c, d, b) <= 0.0
            {
                return true;
            }
        }
    }
    false
}

/// Loot lying around for trolls to take.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug)]
pub struct LootInfo {
    pub loot: Loot,
    pub x: f32,
    pub y: f32,
    /// Index in its owner's list, so the caller can remove it.
    pub index: usize,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Arrow {
    #[cfg_attr(feature = "serde", serde(skip))]
    scan_id: u64,
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    /// Falls into the water and can't hit. orig: `Arrow.DropInWater()`.
    pub missed: bool,
    pub stuck: f32,
    /// Where it was loosed from (animals run from there).
    pub from_x: f32,
    /// orig: `hitDamage` (1), ×`perfectDamageMultiplier` (2) for a perfect shot.
    pub damage: i32,
}

/// A ballista bolt: flies flatter than an arrow and goes through up to
/// `maxHits` greed. orig: `Bolt`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct Bolt {
    #[cfg_attr(feature = "serde", serde(skip))]
    scan_id: u64,
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub damage: i32,
    max_hits: u8,
    hit: Vec<u32>,
    pub stuck: f32,
}

/// orig: Bolt prefab gravity scale 0.7.
const BOLT_GRAVITY: f32 = 0.7 * GRAVITY;

/// The crown when it's off the monarch's head. orig: `Crown`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crown {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub t: f32,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug)]
struct ScheduledWave {
    /// Hours since day 0 when the portal opens / the wave spawns.
    open_at: f32,
    spawn_at: f32,
    portal: u32,
    wave: u16,
    day: u32,
    opened: bool,
}

/// orig: `Portal.State`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PortalPhase {
    Closed,
    Open,
    /// A regular portal fell; it lingers `t` more seconds as rubble.
    Crumbled {
        t: f32,
    },
    /// A cliff portal grows back over `left` seconds.
    Rebuilding {
        left: f32,
    },
}

/// The four states of the PC `portal.controller`. Gameplay Open/Closed is
/// immediate; its visible opening/closing animation has its own saved clock.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortalVisualState {
    Close,
    Open,
    Idle,
    Destroy,
}

impl PortalVisualState {
    pub fn name(self) -> &'static str {
        match self {
            Self::Close => "Close",
            Self::Open => "Open",
            Self::Idle => "Idle",
            Self::Destroy => "Destroy",
        }
    }

    fn length(self, world: &WorldData) -> f32 {
        world
            .animations
            .state_clip("portal", self.name())
            .map_or_else(
                || {
                    if world.animations.clips.is_empty() {
                        match self {
                            Self::Close => 1.2,
                            Self::Open => 1.1000001,
                            Self::Idle => 2.0,
                            Self::Destroy => 1.6875,
                        }
                    } else {
                        0.0
                    }
                },
                |clip| clip.length,
            )
    }
}

const PORTAL_OPEN: u8 = 1;
const PORTAL_CLOSE: u8 = 2;
const PORTAL_DESTROY: u8 = 4;

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PortalVisual {
    pub object: u32,
    pub state: PortalVisualState,
    /// Clip seconds, including the state's speed, independent of global time.
    pub elapsed: f32,
    /// Unity trigger parameters remain set until a matching edge consumes them.
    pub triggers: u8,
}

impl PortalVisual {
    fn new(object: u32) -> Self {
        Self {
            object,
            state: PortalVisualState::Close,
            elapsed: 0.0,
            triggers: 0,
        }
    }

    fn trigger(&mut self, trigger: u8) {
        self.triggers |= trigger;
        self.consume_trigger();
    }

    fn consume_trigger(&mut self) {
        // The authored graph has no AnyState transitions. A Close/Destroy
        // requested during Open waits until Open reaches Idle. Idle's Destroy
        // edge precedes Close. Every edge has zero duration and offset.
        let next = match self.state {
            PortalVisualState::Close | PortalVisualState::Destroy
                if self.triggers & PORTAL_OPEN != 0 =>
            {
                Some((PORTAL_OPEN, PortalVisualState::Open))
            }
            PortalVisualState::Idle if self.triggers & PORTAL_DESTROY != 0 => {
                Some((PORTAL_DESTROY, PortalVisualState::Destroy))
            }
            PortalVisualState::Idle if self.triggers & PORTAL_CLOSE != 0 => {
                Some((PORTAL_CLOSE, PortalVisualState::Close))
            }
            _ => None,
        };
        if let Some((trigger, next)) = next {
            self.triggers &= !trigger;
            self.state = next;
            self.elapsed = 0.0;
        }
    }

    fn advance(&mut self, world: &WorldData, mut dt: f32) {
        if !dt.is_finite() || dt < 0.0 {
            return;
        }
        // At most three queued triggers plus Open's automatic exit can fire.
        // A bounded loop also protects callers from malformed fixture data.
        for _ in 0..8 {
            let before = self.state;
            self.consume_trigger();
            if self.state != before {
                continue;
            }
            let speed = world
                .animations
                .state_speed("portal", self.state.name())
                .unwrap_or(1.0);
            if !speed.is_finite() || speed <= 0.0 {
                return;
            }
            let length = self.state.length(world);
            if self.state == PortalVisualState::Open {
                let exit = world
                    .animations
                    .transitions
                    .iter()
                    .find(|t| {
                        t.controller == "portal"
                            && t.from == "Open"
                            && t.to.as_deref() == Some("Idle")
                    })
                    .and_then(|t| t.exit_time)
                    .unwrap_or(1.0)
                    * length;
                let remaining = ((exit - self.elapsed) / speed).max(0.0);
                if dt >= remaining {
                    dt -= remaining;
                    self.state = PortalVisualState::Idle;
                    self.elapsed = 0.0;
                    continue;
                }
            }
            self.elapsed += dt * speed;
            if self.state == PortalVisualState::Idle && length > 0.0 {
                self.elapsed %= length;
            } else {
                self.elapsed = self.elapsed.min(length);
            }
            return;
        }
    }
}

/// orig: `Portal.SpawnCheck()` coroutine.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
enum Defense {
    /// Looking for intruders once a second.
    Watch { t: f32 },
    /// Provoked; the first defenders come after `defenseSpawnDelay`.
    Provoked { t: f32 },
    /// Spawning a defence wave every `defenseSpawnCooldown` while intruders stay.
    Spawning { t: f32 },
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug)]
struct PortalState {
    object: u32,
    queue: Vec<(EnemyKind, u32, bool)>,
    next_in: f32,
    interval: f32,
    phase: PortalPhase,
    /// orig: `_closeAfterSpawn`.
    close_after: bool,
    defense: Defense,
}

impl PortalState {
    /// orig: `Portal.canSpawnWave`.
    fn can_spawn(&self) -> bool {
        matches!(self.phase, PortalPhase::Open | PortalPhase::Closed)
    }
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GreedEvent {
    PortalOpened(u32),
    Spawned(u32),
    Charge(u32),
    /// A troll struck something.
    Hit {
        by: u32,
        target: Target,
        damage: i32,
    },
    /// A troll grabbed loot (`index` into the list it was given).
    TookLoot {
        by: u32,
        loot: Loot,
        index: usize,
    },
    Killed(u32),
    /// A troll got away with loot.
    Escaped {
        by: u32,
        loot: Option<Loot>,
    },
    /// The greed took the crown. orig: `Troll.PickupCrown()` → `Game.EndGame(Lost)`.
    CrownTaken,
    /// An arrow struck a portal. orig: `Arrow.HitObject()` on the portal's Damageable.
    PortalHit {
        object: u32,
        damage: i32,
    },
    /// A portal fell. orig: `Portal.Crumble()`, `EnemyManager.PortalDestroyed()`.
    PortalDestroyed {
        object: u32,
        cliff: bool,
    },
    /// The rubble of a regular portal is gone. orig: `Destroy(gameObject, 10)`.
    PortalRemoved(u32),
    /// A cliff portal stands again. orig: `Portal.Update()` (Rebuilding done).
    PortalRebuilt(u32),
    /// A squid snatched a citizen. orig: `Squid.GrabCitizen()` → `Character.Grab()`.
    Grabbed {
        by: u32,
        unit: u32,
    },
    /// A squid died and dropped its catch. orig: `Squid.HandleOnDeath()` → `Release()`.
    Released {
        x: f32,
        count: u8,
    },
    /// Two cliff portals are down at once. orig: `Director.OnCliffPortalDestroyed()` → `EndGame(Won)`.
    CliffsDestroyed,
    /// A retaliation wave came out. orig: `EnemyManager.SpawnRetaliation()`.
    Retaliation,
    /// Recoverable loot released exactly once by a defeated carrier.
    /// orig: `Troll.HandleOnDeath()` → `DropLoot()`.
    DroppedLoot {
        by: u32,
        loot: Loot,
        x: f32,
        y: f32,
    },
    /// Starts the short hand-to-carry-position settle, not the initial capture.
    GrabComplete(u32),
    BoulderThrown {
        x: f32,
    },
    BossBashImpact {
        x: f32,
        y: f32,
    },
    /// EnemyManager accepted a wave whose authored bossWave flag is set.
    /// This happens even if that day's enemy count is zero.
    BossWaveSpawned {
        portal: u32,
    },
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default)]
pub struct Greed {
    pub trolls: Vec<Troll>,
    pub arrows: Vec<Arrow>,
    pub bolts: Vec<Bolt>,
    pub crown: Option<Crown>,
    scheduled: Vec<ScheduledWave>,
    portals: Vec<PortalState>,
    next_id: u32,
    next_wave: u32,
    cohesion_t: f32,
    /// orig: `EnemyManager._retaliationsQueued` / `_retaliationCountdown` /
    /// `_numRetaliationWavesSpawned`.
    retaliations_queued: u32,
    retaliation_t: f32,
    retaliations_spawned: u32,
    /// orig: `Director._cliffsCurrentlyDestroyed`.
    cliffs_down: u32,
    /// Saved in the mandatory v8 extension; keep the v5-v7 prefix layout intact.
    #[cfg_attr(feature = "serde", serde(skip))]
    portal_visuals: Vec<PortalVisual>,
    #[cfg_attr(feature = "serde", serde(skip))]
    squid_minds: Vec<SquidMind>,
    #[cfg_attr(feature = "serde", serde(skip))]
    next_projectile_id: u64,
}

/// A portal as the greed module sees it this tick.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug)]
pub struct PortalInfo {
    pub object: u32,
    pub x: f32,
    /// Its trigger in ground-relative world coordinates: (x0, x1, y0, y1).
    pub hit_box: (f32, f32, f32, f32),
    pub by_arrows: bool,
    pub defense_range: f32,
    pub defense_delay: (f32, f32),
    pub defense_cooldown: f32,
    pub cliff: bool,
}

/// Inputs from the rest of the game.
pub struct GreedCtx<'a> {
    pub world: &'a WorldData,
    pub level_index: usize,
    pub campfire: f32,
    pub borders: Borders,
    pub monarch_x: f32,
    pub monarch_has_crown: bool,
    pub targets: &'a [TargetInfo],
    pub citizens: &'a [CitizenInfo],
    pub obstacles: &'a [ObstacleInfo],
    /// Ground-relative world polygons, in the same order as `obstacles`.
    pub solid_obstacles: &'a [crate::data::ObstacleDef],
    pub loot: &'a [LootInfo],
    /// (object id, x) of every portal.
    pub portals: &'a [(u32, f32)],
    pub portal_infos: &'a [PortalInfo],
    /// What provokes portals: the monarch, knights and archers.
    /// orig: `Portal._threatScanner` (tags Player, Knight, Archer).
    pub threats: &'a [f32],
    pub season: Season,
    pub day: u32,
    /// orig: `Kingdom.boat != null`.
    pub boat: bool,
}

impl Greed {
    pub fn squid_states(&self) -> Vec<SquidRuntime> {
        self.trolls
            .iter()
            .filter(|t| t.kind == EnemyKind::Squid)
            .map(|t| {
                self.squid_minds
                    .iter()
                    .find(|m| m.enemy == t.id)
                    .map_or_else(|| SquidRuntime::new(t.id), |m| m.runtime.clone())
            })
            .collect()
    }

    pub fn restore_squid_states(&mut self, states: Vec<SquidRuntime>, world: &WorldData) -> bool {
        if states.len()
            != self
                .trolls
                .iter()
                .filter(|t| t.kind == EnemyKind::Squid)
                .count()
            || states.iter().enumerate().any(|(i, s)| {
                !s.valid(&world.squid)
                    || states[..i].iter().any(|old| old.enemy == s.enemy)
                    || !self.trolls.iter().any(|t| {
                        t.id == s.enemy
                            && t.kind == EnemyKind::Squid
                            && crate::flight::Motion {
                                x: t.x,
                                y: t.y,
                                vx: t.mover.vx,
                                vy: t.vy,
                            }
                            .valid()
                    })
            })
        {
            return false;
        }
        self.squid_minds = states
            .into_iter()
            .map(|runtime| {
                let mut m = SquidMind::new(
                    runtime.enemy,
                    &mut Rng::new(runtime.enemy.wrapping_add(0x3ab174d3)),
                );
                m.runtime = runtime;
                m
            })
            .collect();
        true
    }

    pub fn migrate_squid_states(&mut self, world: &WorldData) {
        self.squid_minds = self
            .trolls
            .iter()
            .filter(|t| t.kind == EnemyKind::Squid)
            .map(|t| {
                let mut m = SquidMind::new(t.id, &mut Rng::new(t.id.wrapping_add(0x3ab174d3)));
                if matches!(t.state, TrollState::Retreat { .. }) || (!t.is_dead() && !t.attacking) {
                    m.runtime.phase = SquidPhase::Retreat { wait: 0.0 };
                }
                m.runtime.migrate_grab(t, world);
                m
            })
            .collect();
    }

    pub fn squid_visual(&self, id: u32) -> Option<SquidVisual> {
        let t = self
            .trolls
            .iter()
            .find(|t| t.id == id && t.kind == EnemyKind::Squid)?;
        let overlay_elapsed = self
            .squid_minds
            .iter()
            .find(|m| m.enemy == id)
            .and_then(|m| match m.runtime.phase {
                SquidPhase::Despawning { elapsed } => Some(elapsed),
                _ => None,
            });
        Some(SquidVisual {
            overlay_elapsed,
            vertical_speed: t.vy,
            death_alpha: match t.state {
                TrollState::Dead { t } => (3.0 - t).clamp(0.0, 1.0),
                _ => 1.0,
            },
            clip: if t.is_dead() {
                "squid_death"
            } else {
                self.squid_minds
                    .iter()
                    .find(|m| m.enemy == id)
                    .map_or("squid_move", |m| m.runtime.animation.clip())
            },
            time: if let TrollState::Dead { t } = t.state {
                t
            } else {
                self.squid_minds
                    .iter()
                    .find(|m| m.enemy == id)
                    .map_or(t.anim_t, |m| m.runtime.animation_elapsed)
            },
        })
    }

    pub fn portal_visuals(&self) -> &[PortalVisual] {
        &self.portal_visuals
    }

    pub fn portal_animation(&self, object: u32) -> Option<(&'static str, f32)> {
        self.portal_visuals
            .iter()
            .find(|v| v.object == object)
            .map(|v| (v.state.name(), v.elapsed))
    }

    fn portal_visual_mut(&mut self, object: u32) -> &mut PortalVisual {
        if let Some(i) = self.portal_visuals.iter().position(|v| v.object == object) {
            return &mut self.portal_visuals[i];
        }
        self.portal_visuals.push(PortalVisual::new(object));
        self.portal_visuals.last_mut().expect("just pushed")
    }

    /// Validate before replacing live clocks; invalid tails leave state intact.
    pub fn restore_portal_visuals(
        &mut self,
        visuals: Vec<PortalVisual>,
        world: &WorldData,
        level: &Level,
    ) -> bool {
        let valid = visuals.iter().enumerate().all(|(i, v)| {
            v.elapsed.is_finite()
                && v.elapsed >= 0.0
                && v.elapsed <= v.state.length(world)
                && v.triggers & !(PORTAL_OPEN | PORTAL_CLOSE | PORTAL_DESTROY) == 0
                && !visuals[..i].iter().any(|other| other.object == v.object)
                && level.index_of(v.object).is_some_and(|index| {
                    world
                        .protos
                        .get(level.objects[index].proto as usize)
                        .is_some_and(|p| p.portal.is_some())
                })
        });
        if valid {
            self.portal_visuals = visuals;
        }
        valid
    }

    /// Older saves recorded gameplay phase but no visible transition phase.
    /// Preserve their prior endpoint instead of replaying an opening on load.
    pub fn migrate_portal_visuals(&mut self, world: &WorldData, level: &Level) {
        self.portal_visuals = level
            .objects
            .iter()
            .filter_map(|o| {
                world.protos.get(o.proto as usize)?.portal?;
                let state = match self.portal_phase(o.id) {
                    PortalPhase::Open => PortalVisualState::Idle,
                    PortalPhase::Closed => PortalVisualState::Close,
                    PortalPhase::Crumbled { .. } | PortalPhase::Rebuilding { .. } => {
                        PortalVisualState::Destroy
                    }
                };
                Some(PortalVisual {
                    object: o.id,
                    state,
                    elapsed: if state == PortalVisualState::Idle {
                        0.0
                    } else {
                        state.length(world)
                    },
                    triggers: 0,
                })
            })
            .collect();
    }

    /// PC HandleOnReceiveHit calls Open only while the portal survives damage.
    pub fn portal_hit(&mut self, object: u32, events: &mut impl FnMut(GreedEvent)) {
        if self.portal_mut(object).phase == PortalPhase::Closed {
            self.portal_mut(object).phase = PortalPhase::Open;
            self.portal_visual_mut(object).trigger(PORTAL_OPEN);
            events(GreedEvent::PortalOpened(object));
        }
    }

    fn advance_portal_visuals(&mut self, world: &WorldData, level: &Level, dt: f32) {
        let active: Vec<u32> = level
            .objects
            .iter()
            .filter_map(|o| {
                world.protos.get(o.proto as usize)?.portal?;
                (!matches!(self.portal_phase(o.id), PortalPhase::Crumbled { t } if t <= 0.0))
                    .then_some(o.id)
            })
            .collect();
        self.portal_visuals.retain(|v| active.contains(&v.object));
        for object in active {
            self.portal_visual_mut(object).advance(world, dt);
        }
    }
    pub fn portals_in(world: &WorldData, level: &Level) -> Vec<(u32, f32)> {
        level
            .objects
            .iter()
            .filter(|o| world.protos[o.proto as usize].portal.is_some())
            .map(|o| (o.id, o.x))
            .collect()
    }

    pub fn portal_open(&self, object: u32) -> bool {
        self.portals
            .iter()
            .any(|p| p.object == object && p.phase == PortalPhase::Open)
    }

    pub fn portal_phase(&self, object: u32) -> PortalPhase {
        self.portals
            .iter()
            .find(|p| p.object == object)
            .map_or(PortalPhase::Closed, |p| p.phase)
    }

    /// orig: `Portal.canSpawnWave`.
    pub fn portal_can_spawn(&self, object: u32) -> bool {
        self.portals
            .iter()
            .find(|p| p.object == object)
            .is_none_or(|p| p.can_spawn())
    }

    /// A portal's hit points ran out. orig: `Portal.HandleOnDeath()` →
    /// `Crumble()`, `EnemyManager.PortalDestroyed()`.
    pub fn portal_destroyed(
        &mut self,
        object: u32,
        cliff: bool,
        rebuild_days: f32,
        events: &mut impl FnMut(GreedEvent),
    ) {
        let p = self.portal_mut(object);
        if !p.can_spawn() {
            return;
        }
        p.queue.clear();
        p.phase = if cliff {
            PortalPhase::Rebuilding {
                left: rebuild_days * 24.0 * SECONDS_PER_HOUR,
            }
        } else {
            PortalPhase::Crumbled { t: 10.0 }
        };
        self.portal_visual_mut(object).trigger(PORTAL_DESTROY);
        events(GreedEvent::PortalDestroyed { object, cliff });
        if cliff {
            self.cliffs_down += 1;
            if self.cliffs_down >= 2 {
                events(GreedEvent::CliffsDestroyed);
            }
        }
        if self.retaliations_queued == 0 {
            self.retaliation_t = RETALIATION_DELAY_HOURS * SECONDS_PER_HOUR;
        }
        self.retaliations_queued += 1;
    }

    /// The enemies a wave sends, queued at `portal` (opening it).
    /// orig: `Portal.SpawnEnemies()`.
    fn queue_wave(&mut self, portal: u32, enemies: Vec<EnemyKind>, boss: bool, close_after: bool) {
        if enemies.is_empty() {
            return;
        }
        self.next_wave += 1;
        let wave = self.next_wave;
        let p = self.portal_mut(portal);
        let opened = p.phase != PortalPhase::Open;
        if opened {
            p.phase = PortalPhase::Open;
            p.next_in = 0.5;
        }
        p.interval = (15.0 / enemies.len() as f32).min(1.0);
        p.queue.extend(enemies.into_iter().map(|e| (e, wave, boss)));
        p.close_after = close_after;
        if opened {
            self.portal_visual_mut(portal).trigger(PORTAL_OPEN);
        }
    }

    /// orig: EnemyManager.SpawnWave. Defense waves call the portal directly.
    fn admit_wave(
        &mut self,
        portal: u32,
        wave: u16,
        day: u32,
        ctx: &GreedCtx,
        events: &mut impl FnMut(GreedEvent),
    ) -> bool {
        if !ctx.portals.iter().any(|&(id, _)| id == portal) || !self.portal_can_spawn(portal) {
            return false;
        }
        let enemies = Self::enemies(ctx.world, ctx.level_index, wave, day, ctx.boat);
        let boss = ctx.world.waves[wave as usize].boss_wave;
        self.queue_wave(portal, enemies, boss, true);
        if boss {
            events(GreedEvent::BossWaveSpawned { portal });
        }
        true
    }

    /// orig: `Kingdom.ClosestSpawner(side, checkOtherSideIfNoneFound: true)`.
    fn closest_portal(
        world: &WorldData,
        level: &Level,
        campfire: f32,
        side: i8,
    ) -> Option<(u32, f32)> {
        let all = Self::portals_in(world, level);
        let on = |s: i8| {
            all.iter()
                .copied()
                .filter(|p| {
                    if s < 0 {
                        p.1 < campfire
                    } else {
                        p.1 > campfire
                    }
                })
                .min_by(|a, b| (a.1 - campfire).abs().total_cmp(&(b.1 - campfire).abs()))
        };
        on(side).or_else(|| on(-side))
    }

    /// The enemies a wave sends on `day`. orig: `EnemyManager.GetEnemies()`.
    /// `boat`: the boat is in the water (more greed comes, except in boss waves).
    pub fn enemies(
        world: &WorldData,
        level_index: usize,
        wave: u16,
        day: u32,
        boat: bool,
    ) -> Vec<EnemyKind> {
        let w = &world.waves[wave as usize];
        let cfg = &world.levels[level_index];
        let mut mult = 1.0;
        let d = day as i32;
        if cfg.wave_multiplier_timeframe.0 <= d && d <= cfg.wave_multiplier_timeframe.1 {
            mult *= cfg.wave_timed_multiplier;
        }
        if !w.boss_wave && boat {
            mult *= cfg.wave_boat_multiplier;
        }
        let count =
            |c: &crate::data::Curve| libm::floorf(c.eval(day as f32) * mult).max(0.0) as usize;
        let mut out = Vec::new();
        out.extend(core::iter::repeat_n(EnemyKind::Troll, count(&w.trolls)));
        out.extend(core::iter::repeat_n(
            EnemyKind::ToughTroll,
            count(&w.tough_trolls),
        ));
        out.extend(core::iter::repeat_n(EnemyKind::Squid, count(&w.squids)));
        out.extend(core::iter::repeat_n(EnemyKind::Boss, count(&w.bosses)));
        out.extend(core::iter::repeat_n(
            EnemyKind::KillerBoss,
            count(&w.killer_bosses),
        ));
        out
    }

    fn wave_for(
        world: &WorldData,
        level_index: usize,
        kind: WaveKind,
        season: Season,
    ) -> Option<u16> {
        let k = match kind {
            WaveKind::Regular => 0,
            WaveKind::Boss => 1,
            WaveKind::Recovery => 2,
        };
        world.levels[level_index].waves[k][season_index(season)]
    }

    /// A wave is due today: work out when it must spawn to reach the border
    /// at `waveAtCastle`. orig: `Director.AddWaveEvents()` + `ScheduleWaveToday()`.
    #[allow(clippy::too_many_arguments)]
    pub fn schedule(
        &mut self,
        world: &WorldData,
        level: &Level,
        level_index: usize,
        kind: WaveKind,
        season: Season,
        day: u32,
        campfire: f32,
        borders: Borders,
        rng: &mut Rng,
    ) {
        let Some(wave) = Self::wave_for(world, level_index, kind, season) else {
            return;
        };
        let both = world.waves[wave as usize].both_sides;
        let sides: Vec<i8> = if both {
            alloc::vec![-1, 1]
        } else if rng.value() < 0.5 {
            alloc::vec![-1]
        } else {
            alloc::vec![1]
        };
        for side in sides {
            let Some((portal, px)) = Self::closest_portal(world, level, campfire, side) else {
                continue;
            };
            // orig: arrival at waveAtCastle, which is ≤ 12 so it means the next night.
            let arrival = if WAVE_AT_CASTLE > 12.0 {
                WAVE_AT_CASTLE
            } else {
                WAVE_AT_CASTLE + 24.0
            };
            // orig: EnemyManager.GetWaveTravelTime: distance to the border at the slowest member's speed.
            let w = &world.waves[wave as usize];
            // orig: GetSlowestInWave.
            let d = day as f32;
            let mut slowest: f32 = 5.0;
            if w.trolls.eval(d) >= 1.0 || w.tough_trolls.eval(d) >= 1.0 {
                slowest = troll::WALK;
            }
            if w.bosses.eval(d) >= 1.0 || w.killer_bosses.eval(d) >= 1.0 {
                slowest = slowest.min(boss::WALK);
            }
            if w.squids.eval(d) >= 1.0 {
                slowest = slowest.min(world.squid.move_speed * 1.2);
            }
            let border = borders.side(if px < campfire { -1 } else { 1 });
            let travel = (border - px).abs() / slowest / SECONDS_PER_HOUR;
            let spawn = day as f32 * 24.0 + arrival - travel;
            self.scheduled.push(ScheduledWave {
                open_at: spawn - PORTAL_OPEN_LEAD,
                spawn_at: spawn,
                portal,
                wave,
                day,
                opened: false,
            });
        }
    }

    /// orig: `EnemyManager.isDangerous`.
    pub fn is_dangerous(&self, borders: Borders) -> bool {
        self.retaliations_queued > 0
            || self.trolls.iter().any(|t| {
                t.attacking
                    && (t.boss_wave
                        || (t.x > borders.left - SAFE_BORDER_DISTANCE
                            && t.x < borders.right + SAFE_BORDER_DISTANCE))
            })
    }

    /// Dawn: trolls that retreat at dawn head home. orig: `EnemyManager.HandleOnDayStart()`.
    pub fn dawn(&mut self) {
        for t in &mut self.trolls {
            if !t.boss_wave && t.kind != EnemyKind::Squid {
                t.should_retreat = true;
            }
        }
    }

    /// orig: `EnemyManager.Retreat()` (game over).
    pub fn retreat_all(&mut self) {
        for t in &mut self.trolls {
            t.should_retreat = true;
        }
    }

    pub fn spawn_troll(
        &mut self,
        kind: EnemyKind,
        x: f32,
        wave: u32,
        boss_wave: bool,
        rng: &mut Rng,
    ) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        let mut mover = Mover::default();
        mover.facing = 1;
        self.trolls.push(Troll {
            id,
            kind,
            x,
            y: 0.0,
            vy: 0.0,
            mover,
            hp: match kind {
                EnemyKind::ToughTroll => troll::TOUGH_HEALTH,
                EnemyKind::Squid => squid::HP,
                EnemyKind::Boss => boss::HP,
                EnemyKind::KillerBoss => boss::KILLER_HP,
                EnemyKind::Troll => 1,
            },
            state: TrollState::Chase {
                t: if kind.is_boss() { -1.0 } else { 0.0 },
            },
            loot: None,
            wave,
            boss_wave,
            should_retreat: false,
            attacking: true,
            angry: 0.0,
            born: 0.0,
            wave_target_offset: rng.range(if kind == EnemyKind::Squid { -5.0 } else { 0.0 }, 10.0),
            distance_from_wave_center: 0.0,
            // orig: OnEnable: SetSpeedMultiplier(Random.Range(0.2, 0.7), 2).
            speed_mult: rng.range(0.2, 0.7),
            speed_mult_t: 2.0,
            target: None,
            anim_t: 0.0,
            victims: 0,
            bash: None,
            puke: None,
            since_puke: boss::PUKE_INTERVAL,
            puke_now: false,
            reached_center: false,
            boss_sight: if kind.is_boss() {
                BossSight {
                    elapsed: rng.range(0.0, boss::SCAN_INTERVAL),
                    ..Default::default()
                }
            } else {
                BossSight::default()
            },
        });
        if kind == EnemyKind::Squid {
            self.squid_minds.push(SquidMind::new(id, rng));
        }
        id
    }

    /// Damages a troll (arrows). orig: `Damageable.ReceiveDamage()`.
    pub fn hurt(&mut self, i: usize, damage: i32, events: &mut impl FnMut(GreedEvent)) {
        let t = &mut self.trolls[i];
        // orig: PickupCrown makes its carrier invulnerable before ending the run.
        if t.is_dead() || t.loot == Some(Loot::Crown) {
            return;
        }
        t.hp -= damage;
        // orig: Boss.HandleOnReceiveDamage: puke soon.
        t.puke_now = true;
        if t.hp <= 0 {
            t.state = TrollState::Dead { t: 0.0 };
            t.mover.stop();
            t.attacking = false;
            if t.kind == EnemyKind::Squid {
                if let Some(m) = self.squid_minds.iter_mut().find(|m| m.enemy == t.id) {
                    m.runtime.target = None;
                    m.runtime.altitude = -crate::build::WALK_Y;
                }
            }
            events(GreedEvent::Killed(t.id));
            if let Some(loot) = t.loot.take() {
                // Wharf.HandleOnPartPickedUp already made a stolen plank
                // available to buy again; its dropped debris is not stock.
                if matches!(loot, Loot::Coin | Loot::Tool(_) | Loot::CarriedTool { .. }) {
                    events(GreedEvent::DroppedLoot {
                        by: t.id,
                        loot,
                        x: t.x + 0.1875 * t.mover.facing as f32,
                        y: t.y + 0.25,
                    });
                }
            }
            if t.victims > 0 {
                events(GreedEvent::Released {
                    x: t.x,
                    count: t.victims,
                });
                t.victims = 0;
            }
        }
    }

    /// Fires an arrow. orig: `Archer.Shoot()` → `Arrow` with an impulse.
    pub fn shoot(&mut self, x: f32, y: f32, vx: f32, vy: f32, missed: bool, perfect: bool) {
        self.arrows.push(Arrow {
            scan_id: 0,
            x,
            y,
            vx,
            vy,
            missed,
            stuck: 0.0,
            from_x: x,
            damage: if perfect { 2 } else { 1 },
        });
    }

    /// orig: `Ballista.Fire()` → `Bolt.Launch()`.
    pub fn fire_bolt(&mut self, x: f32, y: f32, vx: f32, vy: f32, damage: i32, max_hits: u8) {
        self.bolts.push(Bolt {
            scan_id: 0,
            x,
            y,
            vx,
            vy,
            damage,
            max_hits,
            hit: Vec::new(),
            stuck: 0.0,
        });
    }

    fn update_bolts(&mut self, dt: f32, events: &mut impl FnMut(GreedEvent)) {
        let mut hits: Vec<(usize, i32)> = Vec::new();
        for b in &mut self.bolts {
            if b.stuck > 0.0 {
                b.stuck += dt;
                continue;
            }
            b.vy -= BOLT_GRAVITY * dt;
            b.x += b.vx * dt;
            b.y += b.vy * dt;
            if b.y <= 0.0 {
                b.y = 0.0;
                b.stuck = dt;
                continue;
            }
            // orig: Bolt.HitObject: each troll once, up to maxHits.
            for (i, t) in self.trolls.iter().enumerate() {
                if b.hit.len() >= b.max_hits as usize {
                    break;
                }
                if !t.is_dead() && !b.hit.contains(&t.id) && point_in(&t.box_at(), b.x, b.y) {
                    b.hit.push(t.id);
                    hits.push((i, b.damage));
                }
            }
        }
        for (i, d) in hits {
            self.hurt(i, d, events);
        }
        // orig: Pool.Despawn(gameObject, 10) once stuck.
        self.bolts.retain(|b| b.stuck < 10.0);
    }

    /// Drops the crown off the monarch's head. orig: `Player.LoseCrown()`.
    pub fn drop_crown(&mut self, x: f32, vx: f32, vy: f32) {
        self.crown = Some(Crown {
            x,
            y: 1.3,
            vx,
            vy,
            t: 0.0,
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        dt: f32,
        hours_now: f32,
        ctx: &GreedCtx,
        level: &Level,
        boulders: &mut [crate::siege::Boulder],
        rng: &mut Rng,
        mut events: impl FnMut(GreedEvent),
    ) {
        // Scheduled waves: open the portal, then queue the trolls.
        let mut i = 0;
        while i < self.scheduled.len() {
            let s = self.scheduled[i];
            if !s.opened && hours_now >= s.open_at {
                self.scheduled[i].opened = true;
                // orig: Portal.Open() only from Closed.
                if ctx.portals.iter().any(|&(id, _)| id == s.portal) {
                    self.portal_hit(s.portal, &mut events);
                }
            }
            if hours_now >= s.spawn_at {
                self.scheduled.remove(i);
                self.admit_wave(s.portal, s.wave, s.day, ctx, &mut events);
                continue;
            }
            i += 1;
        }
        self.update_portals(dt, ctx, rng, &mut events);
        // Portal spawn queues. orig: Portal.Update.
        let mut spawns: Vec<(EnemyKind, f32, u32, bool)> = Vec::new();
        let mut closed = Vec::new();
        for p in &mut self.portals {
            if p.phase != PortalPhase::Open {
                continue;
            }
            if p.queue.is_empty() {
                // orig: Close() after a wave (regular portals only).
                if p.close_after {
                    p.close_after = false;
                    let cliff = ctx
                        .portal_infos
                        .iter()
                        .any(|i| i.object == p.object && i.cliff);
                    if !cliff {
                        p.phase = PortalPhase::Closed;
                        closed.push(p.object);
                    }
                }
                continue;
            }
            p.next_in -= dt;
            if p.next_in <= 0.0 {
                let Some(x) = level.index_of(p.object).map(|i| level.objects[i].x) else {
                    p.queue.clear();
                    continue;
                };
                // orig: a random entry of the queue.
                let k = rng.range_i32(0, p.queue.len() as i32) as usize;
                let (kind, wave, boss) = p.queue.remove(k);
                spawns.push((kind, x, wave, boss));
                p.next_in = p.interval;
            }
        }
        for object in closed {
            self.portal_visual_mut(object).trigger(PORTAL_CLOSE);
        }
        for (kind, x, wave, boss) in spawns {
            let id = self.spawn_troll(kind, x, wave, boss, rng);
            events(GreedEvent::Spawned(id));
        }

        self.update_cohesion(dt);
        self.update_trolls(dt, ctx, boulders, rng, &mut events);
        self.update_arrows(dt, ctx, &mut events);
        self.update_bolts(dt, &mut events);
        self.update_crown(dt);
        self.advance_portal_visuals(ctx.world, level, dt);
    }

    fn portal_mut(&mut self, object: u32) -> &mut PortalState {
        if let Some(i) = self.portals.iter().position(|p| p.object == object) {
            return &mut self.portals[i];
        }
        self.portals.push(PortalState {
            object,
            queue: Vec::new(),
            next_in: 0.0,
            interval: 1.0,
            phase: PortalPhase::Closed,
            close_after: false,
            defense: Defense::Watch { t: 0.0 },
        });
        self.portals.last_mut().expect("just pushed")
    }

    /// Rubble, rebuilding, defence waves and retaliation.
    /// orig: `Portal.Update()` / `SpawnCheck()`, `EnemyManager.Update()`.
    fn update_portals(
        &mut self,
        dt: f32,
        ctx: &GreedCtx,
        rng: &mut Rng,
        events: &mut impl FnMut(GreedEvent),
    ) {
        for info in ctx.portal_infos {
            self.portal_mut(info.object);
        }
        let mut defence: Vec<u32> = Vec::new();
        let mut rebuilt = Vec::new();
        for p in &mut self.portals {
            match p.phase {
                PortalPhase::Crumbled { t } => {
                    let t = t - dt;
                    if t <= 0.0 {
                        events(GreedEvent::PortalRemoved(p.object));
                    }
                    p.phase = PortalPhase::Crumbled { t };
                    continue;
                }
                PortalPhase::Rebuilding { left } => {
                    let left = left - dt;
                    if left <= 0.0 {
                        // orig: Damageable.Reset, Closed, then Open().
                        p.phase = PortalPhase::Open;
                        rebuilt.push(p.object);
                        p.defense = Defense::Watch { t: 0.0 };
                        self.cliffs_down = self.cliffs_down.saturating_sub(1);
                        events(GreedEvent::PortalRebuilt(p.object));
                    } else {
                        p.phase = PortalPhase::Rebuilding { left };
                    }
                    continue;
                }
                _ => {}
            }
            let Some(info) = ctx.portal_infos.iter().find(|i| i.object == p.object) else {
                continue;
            };
            let threatened = ctx
                .threats
                .iter()
                .any(|&x| (x - info.x).abs() <= info.defense_range);
            p.defense = match p.defense {
                Defense::Watch { t } => {
                    let t = t - dt;
                    if t > 0.0 {
                        Defense::Watch { t }
                    } else if p.phase == PortalPhase::Open && threatened {
                        Defense::Provoked {
                            t: rng.range(info.defense_delay.0, info.defense_delay.1),
                        }
                    } else {
                        Defense::Watch { t: 1.0 }
                    }
                }
                Defense::Provoked { t } | Defense::Spawning { t } => {
                    let t = t - dt;
                    if t > 0.0 {
                        if matches!(p.defense, Defense::Provoked { .. }) {
                            Defense::Provoked { t }
                        } else {
                            Defense::Spawning { t }
                        }
                    } else if threatened {
                        defence.push(p.object);
                        Defense::Spawning {
                            t: info.defense_cooldown,
                        }
                    } else {
                        Defense::Watch { t: 1.0 }
                    }
                }
            };
        }
        for object in rebuilt {
            self.portal_visual_mut(object).trigger(PORTAL_OPEN);
        }
        // orig: Portal.SpawnDefenseWave: the season's defence wave, today's size.
        let s = season_index(ctx.season);
        if let Some(wave) = ctx.world.levels[ctx.level_index].defence_waves[s] {
            for object in defence {
                let enemies = Self::enemies(ctx.world, ctx.level_index, wave, ctx.day, ctx.boat);
                self.queue_wave(object, enemies, false, false);
            }
        }
        // orig: EnemyManager.Update → SpawnRetaliation every retaliationDelayHours.
        if self.retaliations_queued > 0 {
            self.retaliation_t -= dt;
            if self.retaliation_t <= 0.0 {
                self.retaliation_t = RETALIATION_DELAY_HOURS * SECONDS_PER_HOUR;
                self.retaliations_queued -= 1;
                let side = if rng.value() < 0.5 { 1 } else { -1 };
                if let Some(wave) = ctx.world.levels[ctx.level_index].retaliation_waves[s] {
                    if self.closest_live_portal(ctx, side).is_some_and(|portal| {
                        self.admit_wave(portal, wave, self.retaliations_spawned, ctx, events)
                    }) {
                        events(GreedEvent::Retaliation);
                    }
                }
                self.retaliations_spawned += 1;
            }
        }
    }

    /// orig: `Kingdom.ClosestSpawner(side, checkOtherSideIfNoneFound: true)` + `canSpawnWave`.
    fn closest_live_portal(&self, ctx: &GreedCtx, side: i8) -> Option<u32> {
        let on = |s: i8| {
            ctx.portals
                .iter()
                .filter(|p| (p.1 < ctx.campfire) == (s < 0))
                .min_by(|a, b| {
                    (a.1 - ctx.campfire)
                        .abs()
                        .total_cmp(&(b.1 - ctx.campfire).abs())
                })
                .map(|p| p.0)
        };
        on(side)
            .or_else(|| on(-side))
            .filter(|&p| self.portal_can_spawn(p))
    }

    /// Every 3 s, how far each troll is ahead of its wave's middle.
    /// orig: `EnemyManager.UpdateWavePositions()`.
    fn update_cohesion(&mut self, dt: f32) {
        self.cohesion_t -= dt;
        if self.cohesion_t > 0.0 {
            return;
        }
        self.cohesion_t = 3.0;
        let mut waves: Vec<(u32, f32, f32)> = Vec::new();
        let in_wave = |t: &Troll| {
            t.kind != EnemyKind::Squid
                || self
                    .squid_minds
                    .iter()
                    .find(|m| m.enemy == t.id)
                    .map_or(!matches!(t.state, TrollState::Retreat { .. }), |m| {
                        matches!(m.runtime.phase, SquidPhase::Chase)
                    })
        };
        for t in self.trolls.iter().filter(|t| in_wave(t)) {
            match waves.iter_mut().find(|w| w.0 == t.wave) {
                Some(w) => {
                    w.1 = w.1.min(t.x);
                    w.2 = w.2.max(t.x);
                }
                None => waves.push((t.wave, t.x, t.x)),
            }
        }
        for t in self.trolls.iter_mut().filter(|t| in_wave(t)) {
            if let Some(w) = waves.iter().find(|w| w.0 == t.wave) {
                t.distance_from_wave_center = t.x - (w.1 + w.2) / 2.0;
            }
        }
    }

    fn update_trolls(
        &mut self,
        dt: f32,
        ctx: &GreedCtx,
        boulders: &mut [crate::siege::Boulder],
        rng: &mut Rng,
        events: &mut impl FnMut(GreedEvent),
    ) {
        let mut taken: Vec<usize> = Vec::new();
        let mut gone: Vec<usize> = Vec::new();
        let mut pukes: Vec<(f32, i8, u32)> = Vec::new();
        let mut grabbed_citizens = Vec::new();
        // Restoring omitted scanner phases must not consume the persisted
        // gameplay RNG. A local seeded stream still staggers loaded squids.
        let mut scanner_rng = rng.clone();
        if self.trolls.iter().any(|t| t.kind.is_boss()) {
            for id in self
                .arrows
                .iter_mut()
                .map(|a| &mut a.scan_id)
                .chain(self.bolts.iter_mut().map(|b| &mut b.scan_id))
            {
                if *id == 0 {
                    self.next_projectile_id += 1;
                    *id = self.next_projectile_id;
                }
            }
        }
        let mut projectiles = BossProjectiles {
            boulders,
            arrows: &self.arrows,
            bolts: &self.bolts,
        };
        for (ti, t) in self.trolls.iter_mut().enumerate() {
            if t.kind != EnemyKind::Troll && t.kind != EnemyKind::ToughTroll {
                t.anim_t += dt;
                let done = if t.kind == EnemyKind::Squid {
                    let mind = match self.squid_minds.iter().position(|m| m.enemy == t.id) {
                        Some(i) => &mut self.squid_minds[i],
                        None => {
                            // SquidData does not restore scanner/coroutine state.
                            self.squid_minds
                                .push(SquidMind::new(t.id, &mut scanner_rng));
                            self.squid_minds.last_mut().expect("just pushed")
                        }
                    };
                    update_squid(t, mind, dt, ctx, &mut grabbed_citizens, events)
                } else {
                    update_boss(t, dt, ctx, &mut projectiles, rng, &mut pukes, events)
                };
                if done {
                    gone.push(ti);
                }
                continue;
            }
            t.born += dt;
            t.angry -= dt;
            t.anim_t += dt;
            if t.speed_mult_t > 0.0 {
                t.speed_mult_t -= dt;
                if t.speed_mult_t <= 0.0 {
                    t.speed_mult = 1.0;
                }
            }
            // Transitions. orig: Troll FSM (checked every tick while chasing).
            if let TrollState::Chase { .. } | TrollState::GrabLoot { .. } = t.state {
                if t.loot.is_some() || t.should_retreat {
                    // orig: ShouldRetreat
                    t.state = TrollState::Retreat { t: 0.0 };
                    t.attacking = false;
                } else if let Some(target) = pick_target(t, ctx, true) {
                    start_charge(t, target, events);
                } else if let (TrollState::Chase { .. }, Some(l)) =
                    (t.state, nearest_loot(t, ctx, &taken))
                {
                    t.state = TrollState::GrabLoot { x: l.x };
                } else if let (TrollState::Chase { .. }, Some(target)) =
                    (t.state, pick_target(t, ctx, false))
                {
                    start_charge(t, target, events);
                }
            }
            match t.state {
                TrollState::Chase { t: tt } => {
                    if tt <= 0.0 {
                        // orig: Troll.Chase: head for the monarch, or the dropped crown.
                        let goal = if ctx.monarch_has_crown {
                            Some(ctx.monarch_x)
                        } else {
                            None
                        };
                        let dist = goal.map_or(rng.value() - 0.5, |g| g - t.x);
                        let dir: i8 = if dist > 0.0 { 1 } else { -1 };
                        let walk =
                            if t.distance_from_wave_center * dir as f32 > t.wave_target_offset {
                                troll::WALK * 0.3
                            } else {
                                troll::WALK
                            };
                        let speed = if t.angry > 0.0 { troll::RUN } else { walk };
                        t.mover.set_speed(speed * t.speed_mult, dir);
                        if t.born > 5.0 && rng.value() < 0.3 {
                            // orig: Mover.Jump((4, 3), dir) + half speed for 1.2 s.
                            t.mover.vx = 4.0 * dir as f32;
                            t.vy = 3.0;
                            t.speed_mult = 0.5;
                            t.speed_mult_t = 1.2;
                        }
                        t.state = TrollState::Chase {
                            t: rng.range(3.0, 5.0),
                        };
                    } else {
                        t.state = TrollState::Chase { t: tt - dt };
                    }
                }
                TrollState::GrabLoot { x } => {
                    t.mover.set_goal(x, troll::RUN);
                    if !t.mover.moving_to_goal {
                        t.state = TrollState::Chase { t: 0.0 };
                    }
                }
                TrollState::Charge {
                    t: ct,
                    landed,
                    hit,
                    recover,
                } => {
                    let ct = ct + dt;
                    let mut hit = hit;
                    let mut landed = landed;
                    if !landed && !hit {
                        // orig: OnTriggerStay2D / FixedUpdate cast while charging.
                        if let Some(info) =
                            ctx.targets.iter().find(|i| t.box_at().overlaps(&i.rect))
                        {
                            hit = true;
                            events(GreedEvent::Hit {
                                by: t.id,
                                target: info.target,
                                damage: troll::HIT_DAMAGE,
                            });
                            let dir = t.mover.facing as f32;
                            if info.wall {
                                // orig: PauseMotion then wallBounceForce.
                                t.mover.vx = troll::WALL_BOUNCE.0 * dir * rng.range(0.8, 1.2);
                                t.vy = troll::WALL_BOUNCE.1;
                            } else {
                                t.mover.vx *= 0.8;
                            }
                        }
                    }
                    if !landed && ct > 0.1 && !t.airborne() {
                        // orig: touching the ground ends the leap.
                        landed = true;
                        t.mover.vx *= 0.3;
                        t.mover.set_speed(0.0, 0);
                    }
                    if hit && !landed && !t.airborne() {
                        landed = true;
                        t.mover.set_speed(0.0, 0);
                    }
                    if landed {
                        let recover = recover + dt;
                        if recover >= troll::ATTACK_COOLDOWN {
                            t.state = TrollState::Chase { t: 0.0 };
                        } else {
                            t.state = TrollState::Charge {
                                t: ct,
                                landed,
                                hit,
                                recover,
                            };
                        }
                    } else {
                        t.state = TrollState::Charge {
                            t: ct,
                            landed,
                            hit,
                            recover,
                        };
                    }
                }
                TrollState::Retreat { t: rt } => {
                    // orig: Troll.Retreat: back to the nearest portal on this side, then vanish.
                    let side = if t.x < ctx.campfire { -1 } else { 1 };
                    match retreat_portal(ctx, t.x, side) {
                        Some(px) => {
                            t.mover.set_goal(px, troll::RUN);
                            if !t.mover.moving_to_goal {
                                gone.push(ti);
                            }
                        }
                        None => {
                            t.mover.set_speed(troll::RUN, side);
                            if rt > 9.0 {
                                gone.push(ti);
                            }
                        }
                    }
                    t.state = TrollState::Retreat { t: rt + dt };
                }
                TrollState::GrabCitizen { .. } | TrollState::ThrowBoulder { .. } => {
                    // Special enemies handle these states above.
                    t.state = TrollState::Chase { t: 0.0 };
                }
                TrollState::Dead { t: dt0 } => {
                    t.mover.stop();
                    if dt0 > 1.5 {
                        gone.push(ti);
                    }
                    t.state = TrollState::Dead { t: dt0 + dt };
                }
            }

            // Loot pickup on touch. orig: Troll.OnTriggerStay2D → PickupLoot.
            if t.loot.is_none() && !t.is_dead() && !matches!(t.state, TrollState::Charge { .. }) {
                if let Some(l) = ctx
                    .loot
                    .iter()
                    .find(|l| !taken.contains(&l.index) && t.box_at().overlaps(&loot_rect(l)))
                {
                    t.loot = Some(l.loot);
                    taken.push(l.index);
                    events(GreedEvent::TookLoot {
                        by: t.id,
                        loot: l.loot,
                        index: l.index,
                    });
                    if l.loot == Loot::Crown {
                        events(GreedEvent::CrownTaken);
                    }
                }
            }

            // Physics: leaps are ballistic; on the ground the mover drives.
            let was = t.x;
            if t.airborne() {
                t.vy -= GRAVITY * dt;
                t.y += t.vy * dt;
                t.x += t.mover.vx * dt;
                if t.y <= 0.0 {
                    t.y = 0.0;
                    t.vy = 0.0;
                }
            } else {
                t.mover.update(&mut t.x, dt);
            }
            // orig: standing walls are solid to the greed's Rigidbody2D
            // (squids fly over them).
            if t.kind != EnemyKind::Squid {
                // A hair of overlap, so a body against the wall touches it
                // (its charge connects).
                let half = t.box_at().w / 2.0 - 0.05;
                for w in ctx.targets.iter().filter(|w| w.wall) {
                    let (left, right) = (w.rect.x - half, w.rect.x + w.rect.w + half);
                    let stop = if was <= left && t.x > left {
                        Some(left)
                    } else if was >= right && t.x < right {
                        Some(right)
                    } else {
                        None
                    };
                    if let Some(x) = stop {
                        t.x = x;
                        t.mover.vx = 0.0;
                    }
                }
            }
        }
        // orig: Boss.Puke: trolls tumble out in front of it.
        for (x, dir, wave) in pukes {
            for _ in 0..boss::PUKE_AMOUNT {
                let off = rng.range(boss::PUKE_OFFSET.0, boss::PUKE_OFFSET.1);
                let id = self.spawn_troll(EnemyKind::Troll, x + dir as f32 * off, wave, false, rng);
                events(GreedEvent::Spawned(id));
            }
        }
        gone.sort_unstable();
        gone.dedup();
        for &i in gone.iter().rev() {
            let t = self.trolls.remove(i);
            if !t.is_dead() {
                events(GreedEvent::Escaped {
                    by: t.id,
                    loot: t.loot,
                });
            }
        }
        self.squid_minds.retain(|m| {
            self.trolls
                .iter()
                .any(|t| t.id == m.enemy && t.kind == EnemyKind::Squid)
        });
    }

    fn update_arrows(&mut self, dt: f32, ctx: &GreedCtx, events: &mut impl FnMut(GreedEvent)) {
        let mut hits: Vec<(usize, i32)> = Vec::new();
        for a in &mut self.arrows {
            if a.stuck > 0.0 {
                a.stuck += dt;
                continue;
            }
            a.vy -= GRAVITY * dt;
            a.x += a.vx * dt;
            a.y += a.vy * dt;
            if a.y <= 0.0 || (a.missed && a.y < -1.0) {
                if !a.missed {
                    a.y = 0.0;
                }
                a.stuck = dt;
                continue;
            }
            if a.missed {
                continue;
            }
            // orig: Arrow.OnTriggerEnter2D: damage the first troll hit.
            if let Some(i) = self
                .trolls
                .iter()
                .position(|t| !t.is_dead() && point_in(&t.box_at(), a.x, a.y))
            {
                hits.push((i, a.damage));
                a.stuck = 2.0;
                continue;
            }
            // orig: the portal's trigger takes arrows too (Damageable, Arrow).
            if let Some(p) = ctx.portal_infos.iter().find(|p| {
                p.by_arrows
                    && a.x >= p.hit_box.0
                    && a.x <= p.hit_box.1
                    && a.y >= p.hit_box.2
                    && a.y <= p.hit_box.3
            }) {
                if self
                    .portals
                    .iter()
                    .any(|q| q.object == p.object && q.can_spawn())
                {
                    a.stuck = 2.0;
                    events(GreedEvent::PortalHit {
                        object: p.object,
                        damage: a.damage,
                    });
                }
            }
        }
        for (i, damage) in hits {
            self.hurt(i, damage, events);
        }
        self.arrows.retain(|a| a.stuck < 2.0);
    }

    fn update_crown(&mut self, dt: f32) {
        let Some(c) = &mut self.crown else { return };
        c.t += dt;
        c.vy -= CROWN_GRAVITY * dt;
        c.x += c.vx * dt;
        c.y += c.vy * dt;
        if c.y <= 0.0 {
            c.y = 0.0;
            c.vy = 0.0;
            c.vx *= 0.9;
        }
    }
}

fn season_index(season: Season) -> usize {
    match season {
        Season::Summer => 0,
        Season::Autumn => 1,
        Season::Winter => 2,
    }
}

/// orig: `Squid` FSM: fly at `baseAltitude` toward the campfire, swoop on
/// citizens in reach, carry off two, then head home to a portal.
/// Returns true when it vanishes.
fn update_squid(
    t: &mut Troll,
    mind: &mut SquidMind,
    dt: f32,
    ctx: &GreedCtx,
    grabbed_citizens: &mut Vec<u32>,
    events: &mut impl FnMut(GreedEvent),
) -> bool {
    if !dt.is_finite() || !(0.0..=60.0).contains(&dt) {
        return false;
    }
    let def = &ctx.world.squid;
    let first = !mind.runtime.initialized;
    if first {
        mind.runtime.initialized = true;
        mind.runtime.current_speed = def.move_speed;
        if mind.runtime.animation == SquidAnimation::Move {
            mind.runtime.migrate_grab(t, ctx.world);
        }
        if t.is_dead() {
            mind.runtime.chase_running = false;
        }
        if matches!(t.state, TrollState::Retreat { .. }) {
            mind.runtime.phase = SquidPhase::Retreat { wait: 0.0 };
        }
    }
    mind.scan_elapsed = (mind.scan_elapsed + dt).min(squid::SCAN_INTERVAL);
    mind.obstacle_elapsed = (mind.obstacle_elapsed + dt).min(squid::SCAN_INTERVAL);
    if let SquidPhase::Despawning { elapsed } = &mut mind.runtime.phase {
        *elapsed += dt;
        if *elapsed >= 1.0 {
            return true;
        }
    }
    let alive = !t.is_dead();
    if let TrollState::Dead { t: elapsed } = &mut t.state {
        *elapsed += dt;
        // Damageable.FadeAndDestroy: wait2s, fade1s, delayed pool removal2s.
        if *elapsed >= 4.0 {
            return true;
        }
    }
    if let TrollState::GrabCitizen { t: before } = t.state {
        let speed = SquidAnimation::Attack.speed(ctx.world);
        let now = before + dt * speed;
        t.anim_t = now;
        t.state = TrollState::GrabCitizen { t: now };
        if let Some(left) = &mut mind.runtime.grab_settle {
            *left = (*left - dt).max(0.0);
            if *left == 0.0 {
                mind.runtime.grab_settle = None;
                t.state = if matches!(mind.runtime.phase, SquidPhase::Chase) {
                    TrollState::Chase { t: 0.0 }
                } else {
                    TrollState::Retreat { t: 0.0 }
                };
            }
        }
    }
    let grabbing = matches!(t.state, TrollState::GrabCitizen { .. });
    let mut animation_reset = false;
    if alive && matches!(mind.runtime.phase, SquidPhase::Chase) {
        mind.runtime.transition_elapsed = if first {
            0.0
        } else {
            (mind.runtime.transition_elapsed + dt).min(2.0)
        };
        // StateMachine.Update does not check transitions on initial queued entry.
        // During a running coroutine it checks only after strictly >1 second;
        // a completed coroutine checks immediately before restarting Chase.
        if !first && (!mind.runtime.chase_running || mind.runtime.transition_elapsed > 1.0) {
            mind.runtime.transition_elapsed = 0.0;
            if t.should_retreat
                || t.victims >= def.carry_capacity
                || (ctx.campfire - t.x).abs() < 2.0
            {
                mind.runtime.phase = SquidPhase::Retreat { wait: 0.0 };
                if !grabbing {
                    t.state = TrollState::Retreat { t: 0.0 };
                }
            } else if !mind.runtime.chase_running {
                mind.runtime.chase_running = true;
                mind.runtime.chase_wait = 0.0;
            }
        }
    }
    // Squid.Update calls ShouldGrabCitizen independently of Chase/Retreat.
    if alive && !grabbing && t.victims < ctx.world.squid.carry_capacity {
        let dir = t.mover.facing as f32;
        let [gx, gy, gw, gh] = ctx.world.squid.grab_bounds;
        let (a, b) = (t.x + gx * dir, t.x + (gx + gw) * dir);
        let grab = Rect {
            x: a.min(b),
            y: t.y + gy,
            w: gw,
            h: gh,
        };
        if let Some(i) = ctx
            .citizens
            .iter()
            .filter(|i| i.rect.overlaps(&grab) && !grabbed_citizens.contains(&i.unit))
            // Physics2D.OverlapArea returns one collider, with lowest Z first.
            // Eligibility is checked afterward, without choosing a substitute.
            .min_by(|a, b| a.z.total_cmp(&b.z))
            .filter(|i| i.grabbable)
        {
            // Character.Grab changes its layer immediately; later squids in
            // this same tick cannot see or claim that collider again.
            grabbed_citizens.push(i.unit);
            t.victims += 1;
            t.state = TrollState::GrabCitizen { t: 0.0 };
            t.anim_t = 0.0;
            mind.runtime.grab_pending = true;
            mind.runtime.grab_settle = None;
            animation_reset = mind.runtime.attack();
            events(GreedEvent::Grabbed {
                by: t.id,
                unit: i.unit,
            });
        }
    }
    // Squid.Update evaluates flight before StepCoroutine changes _flyAltitude.
    let altitude = mind.runtime.altitude;
    step_squid_coroutine(t, mind, dt, ctx, grabbed_citizens);
    if let Some(id) = mind.runtime.target {
        if let Some(p) = ctx.citizens.iter().find(|c| c.unit == id) {
            t.mover.set_goal(p.x, def.move_speed);
        } else {
            mind.runtime.target = None;
            t.mover.goal = None;
            t.mover.moving_to_goal = false;
        }
    }
    // HandleOnDeath stops Mover once; a still-running Retreat coroutine may
    // subsequently assign another goal even while the body is dead.
    let move_velocity = t.mover.prepare_goal(t.x, dt);
    let mut motion = crate::flight::Motion {
        x: t.x,
        y: t.y,
        vx: t.mover.vx,
        vy: t.vy,
    };
    mind.runtime.flight.advance(
        dt,
        &mut motion,
        crate::flight::Input {
            altitude,
            move_velocity,
            alive,
            collide_with_obstacles: matches!(mind.runtime.phase, SquidPhase::Chase),
        },
        def,
        ctx.solid_obstacles,
    );
    t.x = motion.x;
    t.y = motion.y;
    t.vy = motion.vy;
    t.mover.vx = motion.vx;
    let completed = mind.runtime.animate(
        ctx.world,
        if animation_reset { 0.0 } else { dt },
        t.vy,
        match t.state {
            TrollState::Dead { t } => Some(t),
            _ => None,
        },
    );
    if completed {
        events(GreedEvent::GrabComplete(t.id));
    }
    if matches!(t.state, TrollState::GrabCitizen { .. }) && mind.runtime.grab_settle == Some(0.0) {
        mind.runtime.grab_settle = None;
        t.state = if matches!(mind.runtime.phase, SquidPhase::Chase) {
            TrollState::Chase { t: 0.0 }
        } else {
            TrollState::Retreat { t: 0.0 }
        };
    }

    false
}

fn step_squid_coroutine(
    t: &mut Troll,
    mind: &mut SquidMind,
    dt: f32,
    ctx: &GreedCtx,
    grabbed: &[u32],
) {
    let def = &ctx.world.squid;
    match mind.runtime.phase {
        SquidPhase::Chase if mind.runtime.chase_running => {
            if mind.runtime.chase_wait > 0.0 {
                mind.runtime.chase_wait = (mind.runtime.chase_wait - dt).max(0.0);
                if mind.runtime.chase_wait == 0.0 {
                    mind.runtime.chase_running = false;
                }
                return;
            }
            let prey = mind.closest(t, ctx.citizens, grabbed, def.scan_range);
            let pursue = prey.filter(|p| {
                let obstacle = mind.obstacle(t, ctx);
                mind.runtime.altitude = if obstacle.is_some_and(|x| (x - t.x) * (x - p.x) <= 0.0) {
                    def.base_altitude - crate::build::WALK_Y
                } else {
                    p.y
                };
                obstacle.is_none_or(|x| (x - p.x).abs() > 1.0)
            });
            if let Some(p) = pursue {
                mind.runtime.target = Some(p.unit);
                mind.runtime.chase_wait = 0.5;
                t.mover.set_goal(p.x, def.move_speed);
            } else {
                mind.runtime.altitude = def.base_altitude - crate::build::WALK_Y;
                mind.runtime.target = None;
                // Production squids belong to waves until Enemy.AttackDone.
                if t.attacking {
                    let side = if ctx.campfire > t.x { 1.0 } else { -1.0 };
                    mind.runtime.current_speed +=
                        if side * t.distance_from_wave_center > t.wave_target_offset {
                            -0.12
                        } else {
                            0.108
                        };
                    mind.runtime.current_speed = mind
                        .runtime
                        .current_speed
                        .clamp(def.move_speed - 0.4, def.move_speed);
                }
                t.mover.set_goal(ctx.campfire, mind.runtime.current_speed);
                mind.runtime.chase_wait = 5.0;
            }
        }
        SquidPhase::Retreat { mut wait } => {
            if wait > 0.0 {
                wait = (wait - dt).max(0.0);
                mind.runtime.phase = SquidPhase::Retreat { wait };
                if wait > 0.0 {
                    return;
                }
            }
            // AttackDone and layer change occur at retreat coroutine entry.
            t.attacking = false;
            let side = if t.x < ctx.campfire { -1 } else { 1 };
            if let Some(px) = retreat_portal(ctx, t.x, side) {
                mind.runtime.target = None;
                t.mover.set_goal(px, def.move_speed);
                if (px - t.x).abs() < 3.0 {
                    mind.runtime.altitude = 1.0 - crate::build::WALK_Y;
                    mind.runtime.phase = SquidPhase::Despawning { elapsed: 0.0 };
                } else {
                    mind.runtime.phase = SquidPhase::Retreat { wait: 1.0 };
                }
            } else {
                // No SetSpeed call here: keep the previous Mover goal/velocity.
                mind.runtime.altitude = 10.0 - crate::build::WALK_Y;
                mind.runtime.phase = SquidPhase::NoPortal { wait: 10.0 };
            }
        }
        SquidPhase::NoPortal { wait } => {
            let wait = (wait - dt).max(0.0);
            mind.runtime.phase = if wait == 0.0 {
                // Original CIL calls Despawn() but discards its IEnumerator;
                // unlike the near-portal path it never starts that coroutine.
                SquidPhase::Stranded
            } else {
                SquidPhase::NoPortal { wait }
            };
        }
        _ => {}
    }
}

struct BossProjectiles<'a> {
    boulders: &'a mut [crate::siege::Boulder],
    arrows: &'a [Arrow],
    bolts: &'a [Bolt],
}

impl BossProjectiles<'_> {
    fn held(&mut self, boss: u32) -> Option<&mut crate::siege::Boulder> {
        self.boulders
            .iter_mut()
            .find(|b| b.runtime.held_by == Some(boss))
    }

    fn catch(&mut self, boss: &mut Troll, world: &WorldData) -> bool {
        let sight = &mut boss.boss_sight;
        if sight.elapsed >= boss::SCAN_INTERVAL {
            sight.elapsed = 0.0;
            // Scanner.ComputeCorners: symmetric .6 X, world Y .5 to root+1.
            let low = 0.5 - crate::build::WALK_Y;
            let high = boss.y + 1.0;
            let scan = Rect {
                x: boss.x - 0.6,
                y: low.min(high),
                w: 1.2,
                h: (high - low).abs(),
            };
            let radius = world.boss_throw.boulder_radius;
            sight.projectiles.clear();
            // Unity's native traversal order is unavailable. Keep port order
            // deterministic, including non-boulders that can mask a catch.
            sight.projectiles.extend(
                self.arrows
                    .iter()
                    .filter(|a| {
                        !a.missed
                            && Rect::centered(0.0, 0.15, 0.0625)
                                .at(a.x, a.y, false)
                                .overlaps(&scan)
                    })
                    .map(|a| ProjectileRef::Arrow(a.scan_id))
                    .chain(
                        self.bolts
                            .iter()
                            .filter(|b| {
                                b.stuck <= 0.0
                                    && Rect::centered(0.0, 0.9, 0.09375)
                                        .at(b.x, b.y, false)
                                        .overlaps(&scan)
                            })
                            .map(|b| ProjectileRef::Bolt(b.scan_id)),
                    )
                    .chain(
                        self.boulders
                            .iter()
                            .filter(|b| {
                                let dx = b.x - b.x.clamp(scan.x, scan.x + scan.w);
                                let dy = b.y - b.y.clamp(scan.y, scan.y + scan.h);
                                b.scan_eligible() && dx * dx + dy * dy <= radius * radius
                            })
                            .map(|b| ProjectileRef::Boulder(b.runtime.id)),
                    )
                    .take(12),
            );
        }
        // GetAny selects the first still-active object, before checking its
        // component, stuck state, or hand alignment. Resolve cached identities
        // against the live objects even if they have since moved.
        let candidate = sight.projectiles.iter().copied().find(|p| match p {
            ProjectileRef::Arrow(id) => self.arrows.iter().any(|a| a.scan_id == *id),
            ProjectileRef::Bolt(id) => self.bolts.iter().any(|b| b.scan_id == *id),
            ProjectileRef::Boulder(id) => self.boulders.iter().any(|b| b.runtime.id == *id),
        });
        let Some(ProjectileRef::Boulder(id)) = candidate else {
            return false;
        };
        let hand = boss.boss_hand(world);
        let Some(boulder) = self.boulders.iter_mut().find(|b| b.runtime.id == id) else {
            return false;
        };
        if (boulder.x - hand[0]).abs() >= 1.0 / 32.0 || !boulder.hold_by(boss.id) {
            return false;
        }
        [boulder.x, boulder.y] = hand;
        true
    }
}

fn boss_throw_speed(world: &WorldData) -> f32 {
    world
        .animations
        .state_speed("boss", "Throw")
        .filter(|speed| speed.is_finite() && *speed > 0.0)
        .unwrap_or(1.0)
}

pub(crate) fn boss_boulder_release_time(world: &WorldData) -> f32 {
    world
        .animations
        .clip("boss_throw")
        .and_then(|clip| clip.first_event_time(crate::animation::EventKind::BoulderRelease))
        .unwrap_or(boss::RELEASE_AT)
        / boss_throw_speed(world)
}

/// orig: Boss.Update and the noninterruptible actions reached from Chase.
fn update_boss(
    t: &mut Troll,
    dt: f32,
    ctx: &GreedCtx,
    projectiles: &mut BossProjectiles<'_>,
    rng: &mut Rng,
    pukes: &mut Vec<(f32, i8, u32)>,
    events: &mut impl FnMut(GreedEvent),
) -> bool {
    if let TrollState::Dead { t: d } = t.state {
        t.state = TrollState::Dead { t: d + dt };
        if let Some(b) = projectiles.held(t.id) {
            [b.x, b.y] = t.boss_hand(ctx.world);
        }
        return d + dt >= 10.0;
    }
    t.since_puke += dt;
    t.boss_sight.elapsed = (t.boss_sight.elapsed + dt).min(boss::SCAN_INTERVAL);
    if let TrollState::ThrowBoulder { t: before } = t.state {
        // Boss.Update handles retreat before the Animator can emit the marker.
        if !t.should_retreat {
            let marker = boss_boulder_release_time(ctx.world);
            let now = before + dt;
            let until = (marker - before).clamp(0.0, dt);
            if before < marker && now >= marker {
                t.mover.update(&mut t.x, until);
                if let Some(b) = projectiles.held(t.id) {
                    let position = t.hand_at(ctx.world, Some(marker * boss_throw_speed(ctx.world)));
                    let force = ctx.world.boss_throw.force;
                    let speed = rng.range(force.0, force.1);
                    b.release_from_boss(
                        position,
                        t.mover.facing,
                        speed,
                        dt - until,
                        ctx.world,
                        rng,
                    );
                    events(GreedEvent::BoulderThrown { x: t.x });
                }
                t.mover.update(&mut t.x, dt - until);
            } else {
                // Pause keeps the old movement goal. At two seconds Mover
                // resumes it even though Throw still owns the FSM.
                t.mover.update(&mut t.x, dt);
            }
            t.state = if now >= boss::THROW_TIME {
                TrollState::Chase { t: -1.0 }
            } else {
                TrollState::ThrowBoulder { t: now }
            };
            if let Some(b) = projectiles.held(t.id) {
                [b.x, b.y] = t.boss_hand(ctx.world);
            }
        }
    } else if !t.should_retreat || (t.bash.is_none() && t.puke.is_none()) {
        update_boss_chase_or_attack(t, dt, ctx, projectiles, pukes, events);
    }
    if t.should_retreat {
        t.hp = 0;
        t.state = TrollState::Dead { t: 0.0 };
        t.attacking = false;
        t.mover.stop();
        if let Some(b) = projectiles.held(t.id) {
            [b.x, b.y] = t.boss_hand(ctx.world);
        }
        events(GreedEvent::Killed(t.id));
    }
    false
}

fn update_boss_chase_or_attack(
    t: &mut Troll,
    dt: f32,
    ctx: &GreedCtx,
    projectiles: &mut BossProjectiles<'_>,
    pukes: &mut Vec<(f32, i8, u32)>,
    events: &mut impl FnMut(GreedEvent),
) {
    let dir = t.mover.facing as f32;
    let boss_x = t.x;
    let in_front = |x: f32| (0.0..=boss::REACH).contains(&((x - boss_x) * dir));
    if let Some((bt, mut landed)) = t.bash {
        let bt = bt + dt;
        let impact_at = ctx
            .world
            .animations
            .clip("boss_bash_right")
            .and_then(|clip| clip.first_event_time(crate::animation::EventKind::BashImpact))
            .unwrap_or(boss::BASH_IMPACT);
        if !landed && bt >= impact_at {
            landed = true;
            events(GreedEvent::BossBashImpact { x: t.x, y: t.y });
            let mut hits = Vec::new();
            for i in ctx.targets {
                if in_front(i.x) {
                    hits.push(i.target);
                }
            }
            if ctx.monarch_has_crown && in_front(ctx.monarch_x) && !hits.contains(&Target::Monarch)
            {
                hits.push(Target::Monarch);
            }
            for target in hits {
                events(GreedEvent::Hit {
                    by: t.id,
                    target,
                    damage: boss::HIT_DAMAGE,
                });
            }
        }
        t.bash = (bt < boss::BASH_TIME).then_some((bt, landed));
        if t.bash.is_none() {
            t.state = TrollState::Chase { t: -1.0 };
        }
        return;
    }
    if let Some(before) = t.puke {
        let pt = before + dt;
        if pt >= 0.5 && before < 0.5 {
            pukes.push((t.x, t.mover.facing, t.wave));
        }
        t.puke = (pt < 1.5).then_some(pt);
        if t.puke.is_none() {
            t.state = TrollState::Chase { t: -1.0 };
        }
        return;
    }
    let elapsed = match t.state {
        TrollState::Chase { t } => t,
        _ => -1.0,
    };
    if elapsed < 0.0 {
        // The queued Chase entry executes without evaluating transitions.
        t.state = TrollState::Chase { t: 0.0 };
        move_boss(t, dt, ctx);
        return;
    }
    let elapsed = elapsed + dt;
    if !t.mover.moving_to_goal || elapsed > 1.0 {
        t.state = TrollState::Chase { t: 0.0 };
        // Awake adds Chase transitions in this order: Puke, Throw, Bash.
        let monarch_ahead =
            (ctx.monarch_x - t.x) * dir >= 0.0 && (ctx.monarch_x - t.x).abs() <= boss::PUKE_RANGE;
        if t.since_puke > boss::PUKE_INTERVAL && (t.puke_now || monarch_ahead) {
            t.puke_now = false;
            t.since_puke = 0.0;
            t.mover.stop();
            t.puke = Some(0.0);
            return;
        }
        if projectiles.catch(t, ctx.world) {
            t.mover.pause(boss::THROW_PAUSE);
            t.state = TrollState::ThrowBoulder { t: 0.0 };
            t.anim_t = 0.0;
            return;
        }
        if ctx.targets.iter().any(|i| in_front(i.x)) || in_front(ctx.monarch_x) {
            t.mover.stop();
            t.bash = Some((0.0, false));
            t.anim_t = 0.0;
            return;
        }
    } else {
        t.state = TrollState::Chase { t: elapsed };
    }
    move_boss(t, dt, ctx);
}

fn move_boss(t: &mut Troll, dt: f32, ctx: &GreedCtx) {
    let camp_goal = Some((ctx.campfire, boss::WALK));
    if t.mover.goal == camp_goal && !t.mover.moving_to_goal {
        t.reached_center = true;
    }
    let goal = if t.reached_center && ctx.monarch_has_crown {
        ctx.monarch_x
    } else {
        ctx.campfire
    };
    t.mover.set_goal(goal, boss::WALK);
    t.mover.update(&mut t.x, dt);
}

fn point_in(r: &Rect, x: f32, y: f32) -> bool {
    x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h
}

fn loot_rect(l: &LootInfo) -> Rect {
    Rect::centered(0.15, 0.25, 0.3).at(l.x, l.y, false)
}

/// orig: `Troll.ShouldChargePriorityTarget()` (walls ahead and archers) and
/// `ShouldChargeOtherTarget()` (everyone else), within `chargeRange`.
fn pick_target(t: &Troll, ctx: &GreedCtx, priority: bool) -> Option<TargetInfo> {
    if t.is_dead()
        || matches!(
            t.state,
            TrollState::Charge { .. } | TrollState::Retreat { .. }
        )
    {
        return None;
    }
    let dir = t.mover.facing as f32;
    let candidates = ctx.targets.iter().filter(|i| {
        if priority {
            // Walls: the wall scanner only looks ahead (rangeBehind = 0).
            (i.wall && (i.x - t.x) * dir >= 0.0 && (i.x - t.x).abs() <= troll::CHARGE_RANGE)
                || (i.priority && (i.x - t.x).abs() < troll::CHARGE_RANGE)
        } else {
            !i.wall && !i.priority && (i.x - t.x).abs() < troll::CHARGE_RANGE
        }
    });
    candidates
        .min_by(|a, b| (a.x - t.x).abs().total_cmp(&(b.x - t.x).abs()))
        .copied()
}

fn start_charge(t: &mut Troll, target: TargetInfo, events: &mut impl FnMut(GreedEvent)) {
    let dir: i8 = if t.x < target.x { 1 } else { -1 };
    t.angry = troll::ANGRY_COOLDOWN;
    t.target = Some(target.target);
    t.mover.set_speed(troll::CHARGE_SPEED, dir);
    // orig: Mover.Jump(chargeJumpForce).
    t.mover.vx = troll::CHARGE_JUMP.0 * dir as f32;
    t.vy = troll::CHARGE_JUMP.1;
    t.state = TrollState::Charge {
        t: 0.0,
        landed: false,
        hit: false,
        recover: 0.0,
    };
    events(GreedEvent::Charge(t.id));
}

/// orig: `Troll.ShouldGrabLoot()`: any droppable within `lootRange`.
fn nearest_loot<'a>(t: &Troll, ctx: &'a GreedCtx, taken: &[usize]) -> Option<&'a LootInfo> {
    ctx.loot
        .iter()
        .filter(|l| !taken.contains(&l.index) && (l.x - t.x).abs() <= troll::LOOT_RANGE)
        .min_by(|a, b| (a.x - t.x).abs().total_cmp(&(b.x - t.x).abs()))
}

/// orig: `Kingdom.ClosestSpawnerOnSameSideOfCampfire()`.
fn retreat_portal(ctx: &GreedCtx, x: f32, side: i8) -> Option<f32> {
    ctx.portals
        .iter()
        .map(|p| p.1)
        .filter(|&px| {
            if side < 0 {
                px < ctx.campfire
            } else {
                px > ctx.campfire
            }
        })
        .min_by(|a, b| (a - x).abs().total_cmp(&(b - x).abs()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{Curve, LevelConfigDef, WaveDef};
    use alloc::string::ToString;
    use alloc::vec;

    fn flat(v: f32) -> Curve {
        Curve {
            keys: vec![[0.0, v, 0.0, 0.0], [100.0, v, 0.0, 0.0]],
        }
    }

    fn citizen(unit: u32, x: f32) -> CitizenInfo {
        CitizenInfo {
            unit,
            x,
            y: 0.0,
            z: 0.0,
            rect: Rect::centered(0.32, 0.33, 0.64).at(x, 0.0, false),
            grabbable: true,
        }
    }

    fn squid_context<'a>(
        world: &'a WorldData,
        citizens: &'a [CitizenInfo],
        obstacles: &'a [ObstacleInfo],
        solid_obstacles: &'a [crate::data::ObstacleDef],
        portals: &'a [(u32, f32)],
    ) -> GreedCtx<'a> {
        GreedCtx {
            world,
            level_index: 0,
            campfire: 100.0,
            borders: Borders {
                left: 0.0,
                right: 100.0,
            },
            monarch_x: 100.0,
            monarch_has_crown: true,
            targets: &[],
            citizens,
            obstacles,
            solid_obstacles,
            loot: &[],
            portals,
            portal_infos: &[],
            threats: &[],
            season: Season::Summer,
            day: 0,
            boat: false,
        }
    }

    fn tick_squid(g: &mut Greed, ctx: &GreedCtx, dt: f32) -> Vec<GreedEvent> {
        let mut events = vec![];
        g.update_trolls(dt, ctx, &mut [], &mut Rng::new(31), &mut |e| events.push(e));
        events
    }

    #[test]
    fn boss_throw_checks_strict_chase_cadence_and_puke_throw_bash_order() {
        let world = WorldData::default();
        let wall = [TargetInfo {
            target: Target::Object(12),
            x: 1.0,
            rect: Rect::centered(0.5, 1.0, 1.0).at(1.0, 0.0, false),
            priority: false,
            wall: true,
        }];
        for (puke, aligned) in [(true, true), (false, true), (false, false)] {
            let mut ctx = squid_context(&world, &[], &[], &[], &[]);
            ctx.targets = &wall;
            ctx.monarch_x = if puke { 4.0 } else { 100.0 };
            let mut greed = Greed::default();
            let mut rng = Rng::new(29);
            greed.spawn_troll(EnemyKind::Boss, 0.0, 0, true, &mut rng);
            let t = &mut greed.trolls[0];
            t.state = TrollState::Chase { t: 0.75 };
            t.mover.set_goal(ctx.campfire, boss::WALK);
            t.mover.pause(1.0);
            let x = world.boss_throw.hand[0] + if aligned { 0.0 } else { 1.0 / 32.0 };
            let mut siege = crate::siege::Siege::default();
            siege.boulders.push(crate::siege::Boulder::test_lodged(x));
            siege.identify_boulders();
            greed.update_trolls(0.25, &ctx, &mut siege.boulders, &mut rng, &mut |_| {});
            assert_eq!(greed.trolls[0].state, TrollState::Chase { t: 1.0 });
            assert!(greed.trolls[0].puke.is_none() && greed.trolls[0].bash.is_none());
            greed.update_trolls(0.001, &ctx, &mut siege.boulders, &mut rng, &mut |_| {});
            let t = &greed.trolls[0];
            assert_eq!(t.puke.is_some(), puke);
            assert_eq!(t.bash.is_some(), !puke && !aligned);
            assert_eq!(
                siege.boulders[0].runtime.held_by,
                (!puke && aligned).then_some(t.id)
            );
        }
    }

    #[test]
    fn boss_scanner_uses_first_active_projectile_and_resolves_cached_boulder_position() {
        let world = WorldData::default();
        let mut greed = Greed::default();
        greed.spawn_troll(EnemyKind::Boss, 0.0, 0, true, &mut Rng::new(4));
        greed.shoot(0.0, 0.3, 0.0, 0.0, false, false);
        let boss = &mut greed.trolls[0];
        let mut siege = crate::siege::Siege::default();
        siege
            .boulders
            .push(crate::siege::Boulder::test_lodged(0.55));
        siege.boulders.push(crate::siege::Boulder::test_lodged(0.4));
        siege.identify_boulders();
        boss.boss_sight.elapsed = boss::SCAN_INTERVAL;
        let mut projectiles = BossProjectiles {
            boulders: &mut siege.boulders,
            arrows: &greed.arrows,
            bolts: &[],
        };
        assert!(
            !projectiles.catch(boss, &world),
            "a selected arrow masks both stones"
        );
        projectiles.arrows = &[];
        assert!(
            !projectiles.catch(boss, &world),
            "the first stone is outside hand reach"
        );
        projectiles.boulders[0].x = 0.4;
        assert!(
            projectiles.catch(boss, &world),
            "the cache resolves the moved stone's live X"
        );
        assert_eq!(projectiles.boulders[0].runtime.held_by, Some(boss.id));
        assert!(projectiles.boulders[1].stuck.is_some());
    }

    fn step_squid(g: &mut Greed, citizens: &[CitizenInfo], dt: f32) -> Vec<GreedEvent> {
        let world = WorldData::default();
        tick_squid(g, &squid_context(&world, citizens, &[], &[], &[]), dt)
    }

    fn squid_mind_fixture() -> Greed {
        let mut g = Greed::default();
        g.spawn_troll(EnemyKind::Squid, 0.0, 0, false, &mut Rng::new(31));
        g.trolls[0].y = 3.0;
        g
    }

    #[test]
    fn squid_obstacle_scanner_keeps_order_direction_and_exact_polygon_overlap() {
        let world = WorldData::default();
        let citizens = [citizen(1, 3.5)];
        let wall = |object, x| ObstacleInfo {
            object,
            x,
            rect: Rect {
                x: x - 0.1,
                y: -0.1,
                w: 0.2,
                h: 4.0,
            },
        };
        let obstacles = [wall(1, 1.0), wall(2, 3.0)];
        let ctx = squid_context(&world, &citizens, &obstacles, &[], &[]);
        let mut g = squid_mind_fixture();
        let t = &mut g.trolls[0];
        let m = &mut g.squid_minds[0];
        m.scan_elapsed = 0.5;
        m.obstacle_elapsed = 0.5;
        m.runtime.current_speed = world.squid.move_speed;
        step_squid_coroutine(t, m, 0.0, &ctx, &[]);
        assert_eq!(
            m.runtime.target,
            Some(1),
            "GetAny keeps first obstacle, not nearest to prey"
        );
        assert_eq!(
            m.runtime.altitude,
            world.squid.base_altitude - crate::build::WALK_Y
        );
        let close = [obstacles[1]];
        let ctx = squid_context(&world, &citizens, &close, &[], &[]);
        m.runtime.chase_wait = 0.0;
        step_squid_coroutine(t, m, 0.0, &ctx, &[]);
        assert_eq!(
            m.runtime.target, None,
            "within one unit of selected obstacle suppresses pursuit"
        );
        assert_eq!(m.runtime.chase_wait, 5.0);
        t.mover.facing = -1;
        m.obstacle_elapsed = 0.5;
        m.runtime.chase_wait = 0.0;
        step_squid_coroutine(t, m, 0.0, &ctx, &[]);
        assert_eq!(
            m.runtime.target,
            Some(1),
            "obstacle scanner sees four ahead but only one behind"
        );
        assert_eq!(m.runtime.altitude, citizens[0].y);
        let concave = [[0., 0.], [2., 0.], [2., 1.], [1., 1.], [1., 2.], [0., 2.]];
        assert!(!polygon_overlaps_rect(
            &concave,
            Rect {
                x: 1.2,
                y: 1.2,
                w: 0.2,
                h: 0.2
            }
        ));
        assert!(polygon_overlaps_rect(
            &concave,
            Rect {
                x: 0.9,
                y: 0.9,
                w: 0.2,
                h: 0.2
            }
        ));
    }

    #[test]
    fn squid_cohesion_speed_excludes_retreat_but_keeps_dead_wave_members() {
        let world = WorldData::default();
        let ctx = squid_context(&world, &[], &[], &[], &[]);
        let mut g = squid_mind_fixture();
        let t = &mut g.trolls[0];
        let m = &mut g.squid_minds[0];
        m.runtime.current_speed = world.squid.move_speed;
        t.wave_target_offset = 0.0;
        t.distance_from_wave_center = 10.0;
        for _ in 0..20 {
            m.runtime.chase_wait = 0.0;
            step_squid_coroutine(t, m, 0.0, &ctx, &[]);
        }
        assert!((m.runtime.current_speed - (world.squid.move_speed - 0.4)).abs() < 1e-6);
        t.distance_from_wave_center = -10.0;
        for _ in 0..20 {
            m.runtime.chase_wait = 0.0;
            step_squid_coroutine(t, m, 0.0, &ctx, &[]);
        }
        assert_eq!(m.runtime.current_speed, world.squid.move_speed);
        g.spawn_troll(EnemyKind::Squid, 10., 0, false, &mut Rng::new(7));
        g.spawn_troll(EnemyKind::Squid, 100., 0, false, &mut Rng::new(8));
        g.squid_minds[2].runtime.phase = SquidPhase::Stranded;
        g.trolls[2].attacking = false;
        g.trolls[1].state = TrollState::Dead { t: 0.0 };
        g.trolls[1].attacking = false;
        g.update_cohesion(3.0);
        assert_eq!(g.trolls[0].distance_from_wave_center, -5.0);
        assert_eq!(g.trolls[1].distance_from_wave_center, 5.0);
        assert_eq!(g.trolls[2].distance_from_wave_center, 0.0);
    }

    #[test]
    fn squid_dawn_and_strict_transition_cadence_match_state_machine() {
        let mut g = squid_mind_fixture();
        g.dawn();
        assert!(!g.trolls[0].should_retreat, "Squid.retreatAtDawn is false");
        step_squid(&mut g, &[], 0.0);
        g.retreat_all();
        step_squid(&mut g, &[], 0.5);
        assert_eq!(g.squid_minds[0].runtime.phase, SquidPhase::Chase);
        step_squid(&mut g, &[], 0.5);
        assert_eq!(
            g.squid_minds[0].runtime.phase,
            SquidPhase::Chase,
            "strictly more than one second"
        );
        step_squid(&mut g, &[], 0.001);
        assert!(matches!(
            g.squid_minds[0].runtime.phase,
            SquidPhase::NoPortal { .. }
        ));
        assert!(!g.trolls[0].attacking);
    }

    #[test]
    fn squid_retreat_requeries_each_second_then_fades_and_escapes_once() {
        let world = WorldData::default();
        let mut g = squid_mind_fixture();
        g.trolls[0].state = TrollState::Retreat { t: 0.0 };
        let portals = [(1, 8.0)];
        tick_squid(&mut g, &squid_context(&world, &[], &[], &[], &portals), 0.0);
        assert_eq!(g.trolls[0].mover.goal.unwrap().0, 8.0);
        let near = [(2, 2.0)];
        let ctx = squid_context(&world, &[], &[], &[], &near);
        tick_squid(&mut g, &ctx, 0.5);
        assert_eq!(g.trolls[0].mover.goal.unwrap().0, 8.0);
        g.trolls[0].x = 0.0;
        tick_squid(&mut g, &ctx, 0.5);
        assert!(matches!(
            g.squid_minds[0].runtime.phase,
            SquidPhase::Despawning { elapsed: 0.0 }
        ));
        assert_eq!(
            g.squid_minds[0].runtime.altitude,
            1.0 - crate::build::WALK_Y
        );
        assert!(tick_squid(&mut g, &ctx, 0.75).is_empty());
        assert_eq!(g.squid_visual(0).unwrap().overlay_elapsed, Some(0.75));
        #[cfg(feature = "save")]
        {
            let mut loaded: Greed =
                postcard::from_bytes(&postcard::to_allocvec(&g).unwrap()).unwrap();
            assert!(loaded.restore_squid_states(g.squid_states(), &world));
            let events = tick_squid(&mut loaded, &ctx, 0.25);
            assert_eq!(
                events
                    .iter()
                    .filter(|e| matches!(e, GreedEvent::Escaped { .. }))
                    .count(),
                1
            );
            assert!(loaded.trolls.is_empty());
        }
        let events = tick_squid(&mut g, &ctx, 0.25);
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, GreedEvent::Escaped { .. }))
                .count(),
            1
        );
        assert!(g.trolls.is_empty() && g.squid_minds.is_empty());
        assert!(tick_squid(&mut g, &ctx, 1.0).is_empty());
    }

    #[test]
    fn squid_without_portal_retains_live_goal_and_discards_despawn_enumerator() {
        let world = WorldData::default();
        let mut citizens = [citizen(1, 4.0)];
        let mut g = squid_mind_fixture();
        g.squid_minds[0].scan_elapsed = 0.5;
        step_squid(&mut g, &citizens, 0.0);
        assert_eq!(g.squid_minds[0].runtime.target, Some(1));
        g.retreat_all();
        g.squid_minds[0].runtime.transition_elapsed = 1.1;
        step_squid(&mut g, &citizens, 0.0);
        assert_eq!(
            g.squid_minds[0].runtime.altitude,
            10.0 - crate::build::WALK_Y
        );
        citizens[0] = citizen(1, 8.0);
        step_squid(&mut g, &citizens, 0.1);
        assert_eq!(g.trolls[0].mover.goal.unwrap().0, 8.0);
        step_squid(&mut g, &citizens, 10.0);
        assert_eq!(g.squid_minds[0].runtime.phase, SquidPhase::Stranded);
        assert!(step_squid(&mut g, &citizens, 20.0).is_empty());
        assert_eq!(g.trolls.len(), 1);
        assert_eq!(g.squid_visual(0).unwrap().overlay_elapsed, None);
        assert!(g.squid_minds[0].runtime.valid(&world.squid));
    }

    #[test]
    fn squid_dead_retreat_continues_coroutine_and_four_second_lifetime() {
        let world = WorldData::default();
        let portals = [(1, 20.0)];
        let ctx = squid_context(&world, &[], &[], &[], &portals);
        let mut g = squid_mind_fixture();
        g.trolls[0].state = TrollState::Retreat { t: 0.0 };
        tick_squid(&mut g, &ctx, 0.0);
        g.hurt(0, 100, &mut |_| {});
        assert_eq!(
            g.squid_visual(0).unwrap().clip,
            "squid_death",
            "death is visible before another physics tick"
        );
        assert!(g.trolls[0].mover.goal.is_none());
        tick_squid(&mut g, &ctx, 0.5);
        assert!(g.trolls[0].mover.goal.is_none());
        tick_squid(&mut g, &ctx, 0.5);
        assert_eq!(
            g.trolls[0].mover.goal.unwrap().0,
            20.0,
            "running Retreat can assign a corpse goal"
        );
        tick_squid(&mut g, &ctx, 1.5);
        assert!((g.squid_visual(0).unwrap().death_alpha - 0.5).abs() < 1e-6);
        tick_squid(&mut g, &ctx, 1.0);
        assert_eq!(g.trolls.len(), 1);
        assert_eq!(g.squid_visual(0).unwrap().death_alpha, 0.0);
        assert!(tick_squid(&mut g, &ctx, 0.5).is_empty());
        assert!(g.trolls.is_empty());
    }

    #[test]
    fn squid_motion_animation_obeys_exit_gates_blend_and_attack_restart() {
        let mut world = WorldData::default();
        let mut state = SquidRuntime::new(1);
        state.animate(&world, 0.74, -1.0, None);
        assert_eq!(state.animation, SquidAnimation::Move);
        state.animate(&world, 0.02, -1.0, None);
        assert!(state.dive_blend.is_some());
        assert_eq!(state.animation, SquidAnimation::Move);
        state.animate(&world, 0.24, -1.0, None);
        assert_eq!(state.animation, SquidAnimation::DiveIn);
        assert!((state.animation_elapsed - 0.25).abs() < 1e-5);
        state.animate(&world, 1.0, -0.3, None);
        assert_eq!(
            state.animation,
            SquidAnimation::DiveIn,
            "strict greater-than exit threshold"
        );
        state.animate(&world, 0.0, -0.29, None);
        assert_eq!(state.animation, SquidAnimation::DiveOut);
        state.animate(&world, 0.375, 0.0, None);
        assert_eq!(state.animation, SquidAnimation::Move);
        state.animate(&world, 0.8, 0.0, None);
        state.animate(&world, 0.1, -1.0, None);
        assert!(
            state.dive_blend.is_none(),
            "missed loop exit waits until next .75 crossing"
        );
        state.attack();
        state.animate(&world, 0.6, 0.0, None);
        assert_eq!(
            state.animation,
            SquidAnimation::Attack,
            "hand settle does not end Attack clip"
        );
        state.attack();
        state.animate(&world, 0.15, 0.0, None);
        assert_eq!(state.animation, SquidAnimation::Attack);
        assert!(
            state.animation_elapsed.abs() < 1e-5,
            "queued Attack starts after non-self exit"
        );
        world
            .animations
            .states
            .push(crate::animation::AnimatorState {
                controller: "squid".into(),
                name: "Attack".into(),
                speed: 2.0,
                ..Default::default()
            });
        state.animate(&world, 0.375, 0.0, None);
        assert_eq!(state.animation, SquidAnimation::Move);
        state.animate(&world, 0.0, 0.0, Some(2.5));
        assert_eq!(state.animation, SquidAnimation::Die);
        assert_eq!(state.animation_elapsed, 2.5);
    }

    #[test]
    fn squid_queued_attack_completes_grab_at_actual_animation_marker() {
        let world = WorldData::default();
        let mut g = squid_mind_fixture();
        g.trolls[0].y = 0.0;
        step_squid(&mut g, &[citizen(1, 0.0)], 0.0);
        assert_eq!(g.trolls[0].victims, 1);
        let first = step_squid(&mut g, &[], 0.5);
        assert_eq!(
            first
                .iter()
                .filter(|e| matches!(e, GreedEvent::GrabComplete(_)))
                .count(),
            1
        );
        g.trolls[0].x = 0.0;
        g.trolls[0].y = 0.0;
        let second = step_squid(&mut g, &[citizen(2, 0.0)], 0.1);
        assert_eq!(g.trolls[0].victims, 2);
        assert!(second
            .iter()
            .all(|e| !matches!(e, GreedEvent::GrabComplete(_))));
        assert!(g.squid_minds[0].runtime.attack_pending);
        #[cfg(feature = "save")]
        let mut loaded = {
            let mut loaded: Greed =
                postcard::from_bytes(&postcard::to_allocvec(&g).unwrap()).unwrap();
            assert!(loaded.restore_squid_states(g.squid_states(), &world));
            loaded
        };
        for dt in [0.4, 0.1] {
            assert!(
                !step_squid(&mut g, &[], dt)
                    .iter()
                    .any(|e| matches!(e, GreedEvent::GrabComplete(_))),
                "time since capture cannot complete a queued Attack before its marker"
            );
            #[cfg(feature = "save")]
            assert!(!step_squid(&mut loaded, &[], dt)
                .iter()
                .any(|e| matches!(e, GreedEvent::GrabComplete(_))));
        }
        let complete = step_squid(&mut g, &[], 0.151);
        assert_eq!(
            complete
                .iter()
                .filter(|e| matches!(e, GreedEvent::GrabComplete(_)))
                .count(),
            1
        );
        assert!(g.trolls[0].grab_animation_time().is_some());
        #[cfg(feature = "save")]
        {
            assert_eq!(complete, step_squid(&mut loaded, &[], 0.151));
            assert_eq!(g.squid_states(), loaded.squid_states());
        }
        step_squid(&mut g, &[], 0.1);
        assert!(g.trolls[0].grab_animation_time().is_none());
        assert!(g.squid_minds[0].runtime.valid(&world.squid));
    }

    #[test]
    fn squid_scanner_caps_before_selection_and_refreshes_cached_live_ids() {
        let mut g = squid_mind_fixture();
        let t = &g.trolls[0];
        let mind = &mut g.squid_minds[0];
        let mut citizens: Vec<_> = (0..12)
            .map(|id| citizen(id, 3.0 + id as f32 * 0.1))
            .collect();
        citizens.push(citizen(12, 0.5));
        assert!(
            mind.closest(t, &citizens, &[], 5.0).is_none(),
            "new scanner waits for its initial phase"
        );
        mind.scan_elapsed = 0.5;
        assert_eq!(mind.closest(t, &citizens, &[], 5.0).unwrap().unit, 0);
        assert_eq!(mind.citizens.len(), 12);
        // A cached object's current position is used outside the scan bounds;
        // a new nearer collider does not enter until the next refresh.
        for c in &mut citizens[..12] {
            c.x += 20.0;
            c.rect.x += 20.0;
        }
        mind.scan_elapsed = 0.49;
        assert_eq!(mind.closest(t, &citizens, &[], 5.0).unwrap().x, 23.0);
        citizens.remove(0);
        assert_eq!(mind.closest(t, &citizens, &[], 5.0).unwrap().unit, 1);
        assert!(
            !mind.citizens.contains(&0),
            "inactive IDs are compacted before selection"
        );
        mind.scan_elapsed = 0.5;
        assert_eq!(mind.closest(t, &citizens, &[], 5.0).unwrap().unit, 12);
        // A cached citizen claimed earlier this tick blocks selection rather
        // than substituting another citizen before the next scanner refresh.
        assert!(mind.closest(t, &citizens, &[12], 5.0).is_none());
    }

    #[test]
    fn squid_chase_waits_follow_live_goals_and_do_not_retarget_each_tick() {
        let mut g = squid_mind_fixture();
        let mut citizens = vec![citizen(1, 4.0)];
        step_squid(&mut g, &citizens, 0.0);
        assert_eq!(g.trolls[0].mover.goal.unwrap().0, 100.0);
        // An initially empty scanner does not cause a new chase decision as
        // soon as its 0.5s interval passes: the fallback coroutine waits 5s.
        for _ in 0..10 {
            g.trolls[0].x = 0.0;
            step_squid(&mut g, &citizens, 0.5);
            assert_eq!(g.trolls[0].mover.goal.unwrap().0, 100.0);
        }
        g.trolls[0].x = 0.0;
        step_squid(&mut g, &citizens, 0.0);
        assert_eq!(g.trolls[0].mover.goal.unwrap().0, 4.0);
        let wait = g.squid_minds[0].runtime.chase_wait;
        let scan = g.squid_minds[0].scan_elapsed;
        step_squid(&mut g, &citizens, 0.0);
        assert_eq!(g.squid_minds[0].runtime.chase_wait, wait);
        assert_eq!(g.squid_minds[0].scan_elapsed, scan);

        citizens[0] = citizen(1, 8.0);
        citizens.push(citizen(2, 2.0));
        g.trolls[0].x = 0.0;
        step_squid(&mut g, &citizens, 0.25);
        assert_eq!(
            g.trolls[0].mover.goal.unwrap().0,
            8.0,
            "object goal follows live position while waiting"
        );
        g.trolls[0].x = 0.0;
        step_squid(&mut g, &citizens, 0.25);
        assert_eq!(g.trolls[0].mover.goal.unwrap().0, 8.0);
        step_squid(&mut g, &citizens, 0.0);
        assert_eq!(g.trolls[0].mover.goal.unwrap().0, 2.0);
        let before = g.trolls[0].x;
        step_squid(&mut g, &[], 0.1);
        assert!(
            g.trolls[0].mover.goal.is_none(),
            "inactive object goal is released until the next decision"
        );
        assert!(
            g.trolls[0].x > before,
            "turning the goal off preserves Mover's last speed"
        );
    }

    #[test]
    fn retreating_squid_can_grab_until_full_without_losing_retreat() {
        let mut g = squid_mind_fixture();
        // Already retreating because it reached the campfire, without the
        // separate Enemy.shouldRetreat order. Grabbing must not lose the FSM.
        g.trolls[0].state = TrollState::Retreat { t: 0.0 };
        assert!(!g.trolls[0].should_retreat);
        g.trolls[0].y = 0.0;
        let events = step_squid(&mut g, &[citizen(1, 0.0)], 0.0);
        assert!(events
            .iter()
            .any(|e| matches!(e, GreedEvent::Grabbed { unit: 1, .. })));
        assert!(g.trolls[0].grab_animation_time().is_some());
        assert!(
            !g.trolls[0].should_retreat,
            "FSM retreat does not invent an Enemy retreat order"
        );
        assert!(matches!(
            g.squid_minds[0].runtime.phase,
            SquidPhase::NoPortal { .. }
        ));
        #[cfg(feature = "save")]
        {
            let mut loaded: Greed =
                postcard::from_bytes(&postcard::to_allocvec(&g).unwrap()).unwrap();
            assert!(loaded.restore_squid_states(g.squid_states(), &WorldData::default()));
            step_squid(&mut loaded, &[], 0.7);
            assert!(matches!(loaded.trolls[0].state, TrollState::Retreat { .. }));
        }
        step_squid(&mut g, &[], 0.7);
        assert!(matches!(g.trolls[0].state, TrollState::Retreat { .. }));
        g.trolls[0].victims = WorldData::default().squid.carry_capacity;
        g.trolls[0].x = 0.0;
        g.trolls[0].y = 0.0;
        assert!(!step_squid(&mut g, &[citizen(2, 0.0)], 0.0)
            .iter()
            .any(|e| matches!(e, GreedEvent::Grabbed { .. })));
        // Dead/despawned actors cannot leave cognition entries indefinitely.
        g.trolls[0].state = TrollState::Dead { t: 4.0 };
        step_squid(&mut g, &[], 0.0);
        assert!(g.squid_minds.is_empty());
    }

    #[cfg(feature = "save")]
    #[test]
    fn squid_scanners_are_seeded_runtime_state_and_reinitialize_after_load() {
        let mut a = squid_mind_fixture();
        let b = squid_mind_fixture();
        assert_eq!(a.squid_minds[0].scan_elapsed, b.squid_minds[0].scan_elapsed);
        let bytes = postcard::to_allocvec(&a).unwrap();
        a.squid_minds[0].scan_elapsed = 0.1;
        a.squid_minds[0].citizens = vec![42];
        a.squid_minds[0].runtime.target = Some(42);
        a.squid_minds[0].runtime.chase_wait = 4.9;
        assert_eq!(
            postcard::to_allocvec(&a).unwrap(),
            bytes,
            "transient scanner does not alter historical save prefix"
        );
        let mut loaded: Greed = postcard::from_bytes(&bytes).unwrap();
        assert!(loaded.squid_minds.is_empty());
        step_squid(&mut loaded, &[], 0.0);
        assert_eq!(loaded.squid_minds.len(), 1);
        let mind = &loaded.squid_minds[0];
        assert!((0.0..0.5).contains(&mind.scan_elapsed));
        assert!(mind.citizens.is_empty());
        assert_eq!(mind.runtime.target, None);
        assert_eq!(mind.runtime.chase_wait, 5.0);
    }

    fn portal_fixture() -> (WorldData, Level, u32) {
        use crate::animation::{AnimatorState, AnimatorTransition, Clip};
        use crate::data::{Layer, PortalDef, Proto};
        let mut world = WorldData {
            protos: vec![Proto {
                portal: Some(PortalDef {
                    cliff: false,
                    hit_points: 100,
                    by_arrows: true,
                    by_knights: true,
                    hit_box: (-1.0, 1.0, 0.0, 3.0),
                    defense_range: 3.0,
                    defense_delay: (2.0, 16.0),
                    defense_cooldown: 6.0,
                    rebuild_days: 3.0,
                    rift: None,
                }),
                ..Default::default()
            }],
            levels: vec![LevelConfigDef {
                name: "portal fixture".into(),
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
            ..Default::default()
        };
        // Numeric facts from the retained portal.controller / portal_*.anim.
        for (name, length, looping) in [
            ("Close", 1.2, false),
            ("Open", 1.1000001, false),
            ("Idle", 2.0, true),
            ("Destroy", 1.6875, false),
        ] {
            world.animations.clips.push((
                name.into(),
                Clip {
                    length,
                    looping,
                    ..Default::default()
                },
            ));
            world.animations.states.push(AnimatorState {
                controller: "portal".into(),
                name: name.into(),
                clip: Some(name.into()),
                speed: 1.0,
                ..Default::default()
            });
        }
        world.animations.transitions.push(AnimatorTransition {
            controller: "portal".into(),
            from: "Open".into(),
            to: Some("Idle".into()),
            conditions: vec![],
            exit_time: Some(1.0),
            duration: 0.0,
            fixed_duration: true,
            offset: 0.0,
        });
        let mut level = Level::empty(-20.0, 200.0);
        let id = level.add(crate::level::instantiate(
            &world,
            0,
            Layer::Game,
            0.0,
            1.0,
            0.0,
            1.0,
        ));
        (world, level, id)
    }

    fn advance_portal(
        g: &mut Greed,
        w: &WorldData,
        level: &Level,
        id: u32,
        dt: f32,
        cliff: bool,
    ) -> Vec<GreedEvent> {
        let infos = [PortalInfo {
            object: id,
            x: 0.0,
            hit_box: (-1.0, 1.0, 0.0, 3.0),
            by_arrows: true,
            defense_range: 3.0,
            defense_delay: (2.0, 16.0),
            defense_cooldown: 6.0,
            cliff,
        }];
        let ctx = GreedCtx {
            world: w,
            level_index: 0,
            campfire: 100.0,
            borders: Borders {
                left: 10.0,
                right: 120.0,
            },
            monarch_x: 100.0,
            monarch_has_crown: true,
            targets: &[],
            citizens: &[],
            obstacles: &[],
            solid_obstacles: &[],
            loot: &[],
            portals: &[],
            portal_infos: &infos,
            threats: &[],
            season: Season::Summer,
            day: 0,
            boat: false,
        };
        let mut events = vec![];
        g.update(dt, 0.0, &ctx, level, &mut [], &mut Rng::new(3), |e| {
            events.push(e)
        });
        events
    }

    #[test]
    fn portal_wave_open_close_waits_for_authored_exit_and_cliff_stays_open() {
        let (world, level, id) = portal_fixture();
        let mut g = Greed::default();
        g.queue_wave(id, vec![EnemyKind::Troll], false, true);
        assert!(g.portal_open(id), "gameplay opens immediately");
        assert_eq!(g.portal_animation(id), Some(("Open", 0.0)));
        advance_portal(&mut g, &world, &level, id, 0.6, false);
        let before = g.portal_animation(id);
        g.portal_hit(id, &mut |_| {});
        assert_eq!(
            g.portal_animation(id),
            before,
            "repeated Open must not restart"
        );
        advance_portal(&mut g, &world, &level, id, 0.1, false);
        assert_eq!(g.portal_phase(id), PortalPhase::Closed);
        assert_eq!(
            g.portal_animation(id).unwrap().0,
            "Open",
            "Close trigger waits for Idle"
        );
        advance_portal(&mut g, &world, &level, id, 0.5, false);
        let (state, time) = g.portal_animation(id).unwrap();
        assert_eq!(state, "Close");
        assert!(
            (time - 0.1).abs() < 1e-6,
            "catch-up carries time into closing: {time}"
        );
        advance_portal(&mut g, &world, &level, id, 10.0, false);
        assert_eq!(g.portal_animation(id), Some(("Close", 1.2)));

        let mut cliff = Greed::default();
        cliff.queue_wave(id, vec![EnemyKind::Troll], false, true);
        advance_portal(&mut cliff, &world, &level, id, 0.6, true);
        advance_portal(&mut cliff, &world, &level, id, 0.6, true);
        assert!(cliff.portal_open(id));
        assert_eq!(cliff.portal_animation(id).unwrap().0, "Idle");
    }

    #[test]
    fn portal_destroy_trigger_waits_during_open_then_rebuild_restarts_open() {
        let (world, mut level, id) = portal_fixture();
        let mut g = Greed::default();
        g.portal_hit(id, &mut |_| {});
        advance_portal(&mut g, &world, &level, id, 0.25, true);
        g.portal_destroyed(id, true, 3.0, &mut |_| {});
        assert_eq!(g.portal_animation(id), Some(("Open", 0.25)));
        advance_portal(&mut g, &world, &level, id, 1.1, true);
        let (state, time) = g.portal_animation(id).unwrap();
        assert_eq!(state, "Destroy");
        assert!((time - 0.25).abs() < 1e-6);
        advance_portal(&mut g, &world, &level, id, 2.0, true);
        assert_eq!(g.portal_animation(id), Some(("Destroy", 1.6875)));
        g.portal_mut(id).phase = PortalPhase::Rebuilding { left: 0.01 };
        let events = advance_portal(&mut g, &world, &level, id, 0.02, true);
        assert_eq!(g.portal_animation(id), Some(("Open", 0.02)));
        assert!(events.contains(&GreedEvent::PortalRebuilt(id)));
        level.objects.clear();
        g.advance_portal_visuals(&world, &level, 0.1);
        assert!(
            g.portal_visuals().is_empty(),
            "removed objects cannot leave clocks behind"
        );

        let mut closed = Greed::default();
        closed.portal_destroyed(id, false, 0.0, &mut |_| {});
        assert_eq!(
            closed.portal_animation(id),
            Some(("Close", 0.0)),
            "the PC graph has no Destroy edge from Close"
        );
    }

    #[test]
    fn portal_controller_speed_and_saved_clock_validation_preserve_phase() {
        let (mut world, level, id) = portal_fixture();
        world
            .animations
            .states
            .iter_mut()
            .find(|s| s.name == "Open")
            .unwrap()
            .speed = 2.0;
        let mut g = Greed::default();
        g.portal_hit(id, &mut |_| {});
        advance_portal(&mut g, &world, &level, id, 0.4, false);
        assert_eq!(g.portal_animation(id), Some(("Open", 0.8)));
        let saved = g.portal_visuals().to_vec();
        let mut resumed = g.clone();
        resumed.portal_visuals.clear();
        assert!(resumed.restore_portal_visuals(saved.clone(), &world, &level));
        advance_portal(&mut g, &world, &level, id, 0.2, false);
        advance_portal(&mut resumed, &world, &level, id, 0.2, false);
        assert_eq!(g.portal_visuals(), resumed.portal_visuals());
        let (state, time) = g.portal_animation(id).unwrap();
        assert_eq!(state, "Idle");
        assert!(
            (time - 0.05).abs() < 1e-6,
            "destination uses its own speed: {time}"
        );
        let preserved = resumed.portal_visuals().to_vec();
        let mut bad = saved.clone();
        bad[0].elapsed = f32::NAN;
        assert!(!resumed.restore_portal_visuals(bad, &world, &level));
        assert!(!resumed.restore_portal_visuals(vec![saved[0], saved[0]], &world, &level));
        assert_eq!(resumed.portal_visuals(), preserved);
        resumed.migrate_portal_visuals(&world, &level);
        assert_eq!(resumed.portal_animation(id), Some(("Idle", 0.0)));
    }

    #[test]
    fn curve_eval_matches_unity_shapes() {
        let c = Curve {
            keys: vec![
                [0.0, 0.0, 0.0, 1.0],
                [2.0, 2.0, 1.0, f32::INFINITY],
                [3.0, 5.0, 0.0, 0.0],
            ],
        };
        assert!((c.eval(1.0) - 1.0).abs() < 1e-5, "{}", c.eval(1.0));
        // Stepped tangent: holds the left value until the next key.
        assert_eq!(c.eval(2.5), 2.0);
        assert_eq!(c.eval(10.0), 5.0);
        assert_eq!(c.eval(-1.0), 0.0);
    }

    #[test]
    fn wave_size_uses_the_day_and_early_multiplier() {
        let w = WorldData {
            waves: vec![WaveDef {
                name: "r".to_string(),
                trolls: flat(4.0),
                tough_trolls: flat(1.0),
                ..Default::default()
            }],
            levels: vec![LevelConfigDef {
                name: "t".to_string(),
                autumn_start: (0, 0),
                winter_start: (0, 0),
                days_before_game_over: -1,
                starting_coins: 0,
                min_level_width: 0,
                group_counts: vec![],
                waves: Default::default(),
                wave_timed_multiplier: 0.5,
                wave_multiplier_timeframe: (0, 5),
                season_change_days: 3.0,
                income_multiplier: 1.0,
                free_boat_parts: 0,
                wave_boat_multiplier: 1.0,
                force_all_unlocks: false,
                defence_waves: [None; 3],
                retaliation_waves: [None; 3],
            }],
            ..Default::default()
        };
        let early = Greed::enemies(&w, 0, 0, 2, false);
        assert_eq!(early.iter().filter(|&&e| e == EnemyKind::Troll).count(), 2);
        assert_eq!(
            early
                .iter()
                .filter(|&&e| e == EnemyKind::ToughTroll)
                .count(),
            0
        );
        let later = Greed::enemies(&w, 0, 0, 9, false);
        assert_eq!(later.len(), 5);
    }

    #[test]
    fn launch_reaches_the_target() {
        let (vx, vy) = crate::units::tests_support::launch(4.0, -0.05, ARROW_SPEED, true);
        // Max range is v²/g ≈ 4.8, so aim at 4 and check it passes near the target.
        let (mut x, mut y, mut vy2) = (0.0f32, 0.0f32, vy);
        while x < 4.0 && y > -3.0 {
            vy2 -= GRAVITY / 600.0;
            x += vx / 600.0;
            y += vy2 / 600.0;
        }
        assert!((y + 0.05).abs() < 0.15, "y at target {y}");
    }
}
