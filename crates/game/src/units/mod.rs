//! Citizens: vagrants (beggars), villagers (peasants) and the roles their
//! tools give them, plus the tools themselves and beggar camps.
//!
//! orig: `Character`, `Mover`, `Beggar`, `BeggarCamp`, `Peasant`, `Worker`,
//! `Archer` (movement only so far), `PayableShop` items, `DroppableTool`,
//! `Wallet` (taxes). The original's coroutine states are explicit enums with
//! timers here.

use alloc::vec::Vec;

pub mod knight;
pub use knight::{KnightCtx, KnightData, ShieldShop, ShieldStatus};

use crate::build::{self, under_construction};
use crate::coins::{Coins, Grabber, Policy, Rect, Who};
use crate::data::{Role, ToolKind, WorldData};
use crate::kingdom::{assign, Borders};
use crate::level::Level;
use crate::rng::Rng;

/// Ground deceleration when a character slows down: default physics
/// material friction √(0.4 × 0.4) × gravity 15.
const FRICTION_DECEL: f32 = 6.0;
/// orig: `Mover.PositionEpsilon` / `RubberBandDist`.
const POSITION_EPSILON: f32 = 0.0625;
const RUBBER_BAND: f32 = 0.75;
/// How often FSM transitions are re-checked while a state's coroutine runs.
/// orig: `StateMachine.Update()` (once per second).
const TRANSITION_CHECK: f32 = 1.0;
/// Citizen pickup trigger. orig: character prefabs' BoxCollider2D trigger (0.33 × ~0.64).
const UNIT_BOX: Rect = Rect::centered(0.32, 0.33, 0.64);
/// Tool trigger, roughly the bow's and hammer's. orig: Tool*.prefab BoxCollider2D.
const TOOL_BOX: Rect = Rect::centered(0.22, 0.2, 0.45);
/// The shield's trigger, centred on it. orig: ToolShield.prefab BoxCollider2D (0.35 × 0.83).
const SHIELD_BOX: Rect = Rect::centered(0.0, 0.35, 0.83);

fn tool_box(kind: ToolKind) -> Rect {
    if kind == ToolKind::Shield {
        SHIELD_BOX
    } else {
        TOOL_BOX
    }
}
/// orig: `Wallet.position` on citizens.
const PURSE_Y: f32 = 0.2;

/// Tuning per role. orig: character prefab fields.
struct Tuning {
    walk: f32,
    run: f32,
    /// FixedTransform z.
    z: f32,
    /// orig: `Wallet.maxCoins`.
    max_coins: u32,
    /// Animator thresholds on `Speed` (|vx| × 1.1) for walk and run clips.
    walk_anim: f32,
    run_anim: f32,
}

fn tuning(role: Role) -> Tuning {
    match role {
        Role::Beggar => Tuning {
            walk: 0.4,
            run: 1.4,
            z: 0.75,
            max_coins: 1,
            walk_anim: 0.01,
            run_anim: 0.5,
        },
        Role::Peasant => Tuning {
            walk: 0.6,
            run: 1.4,
            z: 0.75,
            max_coins: 3,
            walk_anim: 0.01,
            run_anim: 0.7,
        },
        Role::Worker => Tuning {
            walk: 0.65,
            run: 1.4,
            z: 0.5,
            max_coins: 3,
            walk_anim: 0.01,
            run_anim: 0.7,
        },
        Role::Archer => Tuning {
            walk: 0.65,
            run: 1.6,
            z: 0.75,
            max_coins: 12,
            walk_anim: 0.005,
            run_anim: 1.0,
        },
        Role::Farmer => Tuning {
            walk: 0.65,
            run: 1.0,
            z: 0.8,
            max_coins: 15,
            walk_anim: 0.01,
            run_anim: 0.7,
        },
        Role::Knight => Tuning {
            walk: knight::WALK,
            run: knight::RUN,
            z: 0.75,
            max_coins: 8,
            walk_anim: 0.01,
            run_anim: 0.7,
        },
    }
}

/// Coins a citizen keeps when paying taxes. orig: `Wallet.payTaxesAbove`.
fn tax_above(role: Role) -> u32 {
    if role == Role::Knight {
        8
    } else {
        1
    }
}

/// orig: `Beggar` prefab: coin spot range, time before joining a camp.
const BEGGAR_COIN_RANGE: f32 = 4.0;
const BEGGAR_SCAN_INTERVAL: f32 = 1.0 / 1.5;
const BEGGAR_TIME_TO_CAMP: f32 = 50.0;
/// orig: `Peasant` prefab: tool spot range.
const PEASANT_TOOL_RANGE: f32 = 10.0;
/// How far inside the shelter night work must be (beyond a work position's spread).
const NIGHT_WORK_MARGIN: f32 = 2.0;

/// orig: `Worker.workTime` (prefab).
const WORK_TIME: f32 = 0.7;
/// orig: `Farmer` prefab: `farmTime`, `fleeRange`, `dumpToolDays`.
const FARM_TIME: f32 = 6.0;
const FARMER_FLEE_RANGE: f32 = 4.0;
const FARMER_DUMP_TOOL_DAYS: f32 = 4.0;
/// orig: `Director.secondsPerInGameHour` × 24.
const SECONDS_PER_DAY: f32 = 240.0;
/// orig: `Kingdom.workPortalDistance` (scene).
const WORK_PORTAL_DISTANCE: f32 = 12.0;
/// orig: `Kingdom.minFreeArchers` (scene).
const MIN_FREE_ARCHERS: usize = 1;
/// orig: `Archer.borderHuntRange` and `distanceFromWall` (prefab).
const ARCHER_HUNT_RANGE: f32 = 16.0;
/// orig: Archer prefab `maxPursueDistance`, `coinPickupRange`, `fleeRange`.
const ARCHER_MAX_PURSUE: f32 = 12.0;
const ARCHER_COIN_RANGE: f32 = 6.0;
const ARCHER_FLEE_RANGE: f32 = 4.0;
const ARCHER_WALL_DISTANCE: (f32, f32) = (1.0, 2.0);

/// Simple HSV → RGB. orig: `ColorFunctions.HSBToColor()`.
pub fn hsb(h: f32, s: f32, b: f32) -> [f32; 3] {
    if s <= 0.0 {
        return [b; 3];
    }
    let c = b * s;
    let m = b - c;
    let deg = (h % 1.0) * 360.0;
    let x = |d: f32| d * c / 60.0 + m;
    match deg {
        d if d < 60.0 => [b, x(d), m],
        d if d < 120.0 => [x(120.0 - d), b, m],
        d if d < 180.0 => [m, b, x(d - 120.0)],
        d if d < 240.0 => [m, x(240.0 - d), b],
        d if d < 300.0 => [x(d - 240.0), m, b],
        d => [b, m, x(360.0 - d)],
    }
}

/// orig: `ColorFunctions.RandomSkinColor()`.
pub fn random_skin(rng: &mut Rng) -> [f32; 3] {
    let v = rng.value();
    hsb(v * 20.0 / 360.0, 0.19 + v / 5.0, 0.97 - v * 0.65)
}

/// orig: `Character.PickOutfitColor()`.
pub fn random_outfit(role: Role, rng: &mut Rng) -> [f32; 3] {
    if role == Role::Beggar {
        let h = rng.range(0.08, 0.12);
        let b = rng.range(0.15, 0.25);
        hsb(h, 0.24, b)
    } else {
        let h = rng.value();
        let b = rng.range(0.4, 0.5);
        hsb(h, 0.25, b)
    }
}

/// orig: `Mover`. Goal seeking slows near the goal; speeding up is instant,
/// slowing down is friction.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mover {
    pub vx: f32,
    pub facing: i8,
    move_speed: f32,
    pub(crate) goal: Option<(f32, f32)>,
    pub moving_to_goal: bool,
    pub(crate) pause: f32,
}

impl Mover {
    /// A stopped mover facing right (prefabs face right).
    pub fn new() -> Self {
        Mover {
            facing: 1,
            ..Default::default()
        }
    }

    /// orig: `Mover.SetGoal(float, speed)`.
    pub fn set_goal(&mut self, x: f32, speed: f32) {
        // Re-setting the same goal every tick (following a target) must not
        // restart the approach.
        if self.goal == Some((x, speed)) {
            return;
        }
        self.goal = Some((x, speed));
        self.moving_to_goal = true;
    }

    /// orig: `Mover.SetSpeed(speed, direction)`.
    pub fn set_speed(&mut self, speed: f32, dir: i8) {
        self.goal = None;
        self.moving_to_goal = false;
        self.move_speed = speed;
        if dir != 0 {
            self.facing = dir;
        }
    }

    pub fn stop(&mut self) {
        self.set_speed(0.0, 0);
    }

    /// orig: `Mover.Pause()`.
    pub fn pause(&mut self, t: f32) {
        self.pause = self.pause.max(t);
    }

    /// Resolve the movement command without applying ground friction or moving
    /// the root. Flying actors pass this command to their rigidbody integrator.
    pub fn prepare_goal(&mut self, x: f32, dt: f32) -> Option<f32> {
        let mut target = None;
        if self.pause > 0.0 {
            self.pause -= dt;
        }
        if self.pause <= 0.0 {
            if let Some((g, speed)) = self.goal {
                let d = (x - g).abs();
                if d > POSITION_EPSILON {
                    self.facing = if g > x { 1 } else { -1 };
                    let t = (d / RUBBER_BAND).min(1.0);
                    self.move_speed = 0.4 * speed + (speed - 0.4 * speed) * t;
                } else {
                    self.move_speed = 0.0;
                    self.moving_to_goal = false;
                }
            }
            target = Some(self.move_speed * self.facing as f32);
        }
        target
    }

    pub fn update(&mut self, x: &mut f32, dt: f32) {
        let target = self.prepare_goal(*x, dt);
        match target {
            Some(t) if (t > 0.0 && t > self.vx) || (t < 0.0 && t < self.vx) => self.vx = t,
            _ => {
                // Slowing to a lower speed the same way (the rubber band near
                // a goal) stops at it: braking past it and snapping back up
                // each tick saw-toothed across the walk/run threshold, and
                // every switch restarted the clip (builders "stutter-stepped").
                let floor = target.filter(|t| t * self.vx > 0.0).unwrap_or(0.0);
                let step = FRICTION_DECEL * dt;
                self.vx = if self.vx > 0.0 {
                    (self.vx - step).max(floor)
                } else {
                    (self.vx + step).min(floor)
                };
            }
        }
        *x += self.vx * dt;
    }
}

/// What a unit is doing; each variant is one FSM state of its role.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ai {
    /// Waiting `t` seconds, then `then`.
    Idle { phase: u8, t: f32 },
    /// Beggar running for a claimed coin. orig: `Beggar.GrabCoin()`.
    GrabCoin,
    /// Peasant running for a tool. orig: `Peasant.GrabTool()`.
    GrabTool { tool: u32 },
    /// Worker sheltering. orig: `Worker.Shelter()`.
    Shelter { phase: u8, t: f32 },
    /// Worker going to and working on an object. orig: `Worker.Work()`.
    Work { target: u32, working: bool, t: f32 },
    /// Archer roaming near the border by day. orig: `Archer.Hunt()`.
    Hunt { phase: u8, t: f32 },
    /// Archer standing behind the outer wall at night. orig: `Archer.GoToWall()`.
    GoToWall { phase: u8, t: f32 },
    /// Between states. orig: archer `Stand`.
    Stand,
    /// Farmer walking to and working a field. orig: `Farmer.Farm()`.
    Farm { target: u32, phase: u8, t: f32 },
    /// Running for the campfire. orig: `Farmer.Flee()`.
    Flee { t: f32 },
    /// Knight waiting by the border banner. orig: `Knight.Assemble()`.
    Assemble { t: f32 },
    /// Knight taking post at the wall. orig: `Knight.GoToWall()`.
    KnightWall,
    /// Knight attacking a portal. orig: `Knight.Charge()`.
    Charge { portal: u32, target: f32, t: f32 },
    /// Knight heading for the boat (no portal left on its side): escorting
    /// it while it's pushed, then boarding. orig: `Knight.Charge()`'s boat branch.
    Board { waiting: bool, t: f32 },
    /// Archer behind a knight. orig: `Archer.FollowKnight()`.
    Follow,
    /// Beggar called to the bakery. orig: `Beggar.GoToBaker()`.
    GoToBaker { phase: u8, t: f32 },
}

/// Animation the renderer should show.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anim {
    Idle,
    Walk,
    Run,
    Hammer,
    /// Archer drawing or loosing. orig: `archer_shoot_prep` / `archer_shoot`.
    Shoot,
    /// Knocked down. orig: `peasant_stunned` / `worker_stunned`.
    Stunned,
    /// Working a field. orig: `farmer_work`.
    Farm,
    /// orig: `knight_slash`.
    Slash,
    /// orig: `worker_push`.
    Push,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug)]
pub struct Unit {
    pub id: u32,
    pub role: Role,
    pub x: f32,
    pub z: f32,
    pub mover: Mover,
    pub coins: u32,
    pub skin: [f32; 3],
    pub outfit: [f32; 3],
    pub ai: Ai,
    check_timer: f32,
    scan_timer: f32,
    /// orig: `Beggar.camp` (camp object id).
    camp: Option<u32>,
    idle_for: f32,
    /// Worker: -1..1, spreads workers along a building. orig: `workPositionOffset`.
    work_offset: f32,
    works_before_moving: i32,
    queued_work: Option<u32>,
    /// Archer: which border they guard. orig: `_guardSide`.
    guard_side: Option<i8>,
    /// Paying taxes: (time into it, coins left to drop). orig: `Wallet.PayTaxes()`.
    taxes: Option<(f32, u32)>,
    /// Seconds into the current animation clip.
    pub anim_t: f32,
    anim: Anim,
    /// Archer volley in progress: (target troll, shots left, timer, aiming).
    shoot: Option<(Quarry, u8, f32, bool)>,
    /// orig: `Archer._huntingTarget` (animal id).
    hunting: Option<u32>,
    /// This draw is a perfect shot. orig: `Archer.Shoot()` `perfectShot`.
    perfect_shot: bool,
    /// Archer told to hold still by its knight. orig: `Archer.HoldPosition()`.
    hold: f32,
    /// orig: `Archer._cooldown`.
    shoot_cooldown: f32,
    /// Knocked down: no AI, can't be hurt. orig: `Peasant.Stun()`.
    pub stunned: f32,
    /// Farmer's assigned field. orig: `Farmer._farmland`.
    pub farm: Option<u32>,
    /// Ground-relative height (archers up on a tower).
    pub y: f32,
    /// Archer's tower guard slot: (object id, slot index). orig: `Archer._guardSlot`.
    pub slot: Option<(u32, u8)>,
    /// Standing in it. orig: `Archer._inGuardSlot`.
    pub in_slot: bool,
    /// Wearing the banner colors. orig: `Archer.ConvertToSoldier()`.
    pub soldier: bool,
    pub knight: Option<KnightData>,
    /// Seconds of the warhorse's blessing left: hits don't cost coins or tools.
    /// orig: `Character._inspiredUntil`.
    pub inspired: f32,
    /// Archer: the knight it follows. orig: `Archer._knight`.
    pub follows: Option<u32>,
    /// Beggar: the bakery that called it. orig: `Beggar._baker`.
    pub baker: Option<u32>,
    /// A loaf this beggar just ate (removed by the population).
    eaten: Option<u32>,
    /// On a job with nothing to do right now (a loaded ballista).
    resting_on_job: bool,
    /// Pushing a catapult or the boat. orig: `Worker.WorkAnimation.Push`.
    pub pushing: bool,
    /// Archer: how far behind its knight. orig: `distanceFromKnight.Random()`.
    formation: f32,
    /// orig: `Time.time - Farmer._lastFarmed`.
    since_farmed: f32,
}

impl Unit {
    pub fn anim(&self) -> Anim {
        self.anim
    }

    fn new(id: u32, role: Role, x: f32, rng: &mut Rng) -> Self {
        let t = tuning(role);
        Unit {
            id,
            role,
            x,
            z: t.z + rng.value() * 0.01,
            mover: Mover {
                facing: 1,
                ..Default::default()
            },
            coins: 0,
            skin: random_skin(rng),
            outfit: random_outfit(role, rng),
            ai: Ai::Idle { phase: 0, t: 0.0 },
            check_timer: 0.0,
            scan_timer: rng.value() * BEGGAR_SCAN_INTERVAL,
            camp: None,
            idle_for: 0.0,
            work_offset: rng.value() * 2.0 - 1.0,
            works_before_moving: 0,
            queued_work: None,
            guard_side: None,
            taxes: None,
            anim_t: 0.0,
            anim: Anim::Idle,
            shoot: None,
            hunting: None,
            perfect_shot: false,
            hold: 0.0,
            shoot_cooldown: 0.0,
            stunned: 0.0,
            farm: None,
            since_farmed: 0.0,
            y: 0.0,
            slot: None,
            in_slot: false,
            soldier: false,
            knight: None,
            inspired: 0.0,
            follows: None,
            baker: None,
            eaten: None,
            resting_on_job: false,
            pushing: false,
            formation: 1.0,
        }
    }

    fn start_state(&self) -> Ai {
        match self.role {
            Role::Archer => Ai::Stand,
            _ => Ai::Idle { phase: 0, t: 0.0 },
        }
    }

    /// orig: `Character.Promote()` / `ReplaceBy()`: same position, colors and coins.
    fn promote(&mut self, role: Role, rng: &mut Rng) {
        if self.role == Role::Beggar {
            self.outfit = random_outfit(role, rng);
        }
        self.role = role;
        self.z = tuning(role).z + rng.value() * 0.01;
        self.ai = self.start_state();
        self.queued_work = None;
        self.camp = None;
        self.farm = None;
        self.since_farmed = 0.0;
        self.slot = None;
        self.in_slot = false;
        self.soldier = false;
        self.y = 0.0;
        self.follows = None;
        if role != Role::Knight {
            self.knight = None;
        }
        self.mover.stop();
    }

    fn box_at(&self) -> Rect {
        UNIT_BOX.at(self.x, 0.0, false)
    }
}

/// A tool on a shop rack or on the ground. orig: `DroppableTool`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct Tool {
    pub id: u32,
    pub kind: ToolKind,
    pub x: f32,
    pub y: f32,
    vy: f32,
    /// Shop object id while on its rack, or a dropped shield's banner owner.
    /// orig: `PayableShop._items` / `DroppableShield`.
    pub shop: Option<u32>,
    /// Rack index; `u8::MAX` denotes a dropped tool, including a shield that
    /// retains its banner owner. This preserves the serialized Tool layout.
    pub slot: u8,
}

impl Tool {
    /// Whether the tool occupies a shop rack, rather than lying loose with
    /// a shield's banner association retained in `shop`.
    pub fn on_rack(&self) -> bool {
        self.shop.is_some() && self.slot != u8::MAX
    }
}

/// orig: `BeggarCamp.SlowUpdate()` progress.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug)]
struct Camp {
    object: u32,
    t: f32,
    waiting_spawn: bool,
}

/// What happened, for sounds and effects.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UnitEvent {
    Promoted {
        unit: u32,
        role: Role,
    },
    Spawned(u32),
    BuildPoint(u32),
    /// A completed worker stroke, positioned before its target can disappear.
    WorkSound {
        x: f32,
        tree: bool,
    },
    TreeChopped(u32),
    PaidTaxes(u32),
    /// A farmer worked a field. orig: `Farmland.ReceiveFarming()`.
    Farmed(u32),
    /// A knight's slash connected. orig: `Knight.Slash()` → `ReceiveDamage`.
    Strike {
        troll: u32,
        damage: i32,
        from_x: f32,
    },
    StrikePortal {
        portal: u32,
        damage: i32,
    },
    /// A builder repaired a wall a little. orig: `Wall.OnWorkFinish()`.
    Repaired(u32),
    /// A builder wound a ballista one notch, cranked a catapult or fitted at
    /// the wharf. orig: `Ballista/Catapult/Wharf.OnWorkFinish()`.
    Wound(u32),
    /// A worker or knight stepped onto the boat. orig: `Embark()`.
    Embark(u32),
    /// A knight fell back to peasant; its shield shop's banner is torn.
    KnightLost {
        shop: Option<u32>,
    },
    /// An archer loosed an arrow. orig: `Archer.Shoot()`.
    Arrow {
        x: f32,
        y: f32,
        vx: f32,
        vy: f32,
        missed: bool,
        /// orig: `Arrow.PerfectShot()` (archer statue).
        perfect: bool,
    },
}

/// A living enemy as citizens see it this tick. The transform position is
/// separate from the collider bounds: orig: `Archer.TargetInRange` uses the
/// former, while `Archer.BestShot` aims at `Collider2D.bounds.center`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnemyTarget {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub hp: i32,
    pub bounds: Rect,
}

impl From<&crate::greed::Troll> for EnemyTarget {
    fn from(troll: &crate::greed::Troll) -> Self {
        Self {
            id: troll.id,
            x: troll.x,
            y: troll.y,
            vx: troll.mover.vx,
            hp: troll.hp,
            bounds: troll.box_at(),
        }
    }
}

/// Everything the units need to know about the rest of the game this tick.
pub struct Ctx<'a> {
    pub world: &'a WorldData,
    pub borders: Borders,
    /// Where citizens spend the night: just inside the outermost built,
    /// intact wall on each side (the border where there's none).
    pub shelter: Borders,
    pub campfire: f32,
    pub daytime: bool,
    pub safe: bool,
    pub monarch_x: f32,
    pub monarch_running: bool,
    pub forests: &'a [(f32, f32)],
    pub enemies: &'a [EnemyTarget],
    /// x of every wall, to decide between low and high arrow arcs.
    pub walls: &'a [f32],
    pub farmlands: &'a [FarmInfo],
    /// Built towers' guard slots.
    pub slots: &'a [SlotInfo],
    /// orig: `Kingdom.archerStatueCharged` / `knightStatueCharged` / `workerStatueCharged`.
    pub archer_statue: bool,
    pub knight_statue: bool,
    pub worker_statue: bool,
    pub portals: &'a [PortalTarget],
    pub knights: KnightCtx,
    pub coat: crate::kingdom::CoatOfArms,
    /// Living animals: (id, x, vx, is deer, collider center y).
    pub wildlife: &'a [(u32, f32, f32, bool, f32)],
    /// orig: `Kingdom.closestForest`: the forest edges nearest the campfire.
    pub closest_forest: (f32, f32),
    /// orig: `Kingdom.boat`.
    pub boat: Option<BoatTarget>,
}

/// The boat as knights see it.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoatTarget {
    pub x: f32,
    /// Toward the beach. orig: `Boat.side`.
    pub dir: i8,
    /// `boat::mode`.
    pub mode: u8,
    /// orig: `numKnights < maxKnights`.
    pub knight_room: bool,
}

/// A portal as archers and knights see it.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PortalTarget {
    pub id: u32,
    pub x: f32,
    /// Its trigger: x range and centre height (ground-relative).
    pub x0: f32,
    pub x1: f32,
    pub cy: f32,
    pub hp: i32,
    /// Standing (not crumbled or rebuilding).
    pub alive: bool,
    pub by_arrows: bool,
    pub by_knights: bool,
}

/// A tower guard slot. orig: `GuardSlot`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SlotInfo {
    pub id: (u32, u8),
    pub x: f32,
    /// Ground-relative.
    pub y: f32,
}

/// A field as farmers see it.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FarmInfo {
    pub id: u32,
    pub x: f32,
    pub farmable: bool,
    /// Its farmhouse: (x, level), while built.
    pub house: Option<(f32, u8)>,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default)]
pub struct Population {
    pub units: Vec<Unit>,
    pub tools: Vec<Tool>,
    camps: Vec<Camp>,
    next_id: u32,
    reassign_timer: f32,
    /// orig: `PayableShield` state per shop object.
    pub shields: Vec<ShieldShop>,
    /// Bakeries: (object, seconds to the next call, beggars called).
    /// orig: `Baker._beggars`, `CallBeggars()`.
    bakers: Vec<(u32, f32, Vec<u32>)>,
}

impl Population {
    /// Takes the citizens placed in the level (e.g. the starting beggars)
    /// out of it as units, and registers beggar camps.
    pub fn from_level(world: &WorldData, level: &mut Level, rng: &mut Rng) -> Self {
        let mut p = Population::default();
        let mut i = 0;
        while i < level.objects.len() {
            let proto = &world.protos[level.objects[i].proto as usize];
            if let Some(role) = proto.character {
                let o = level.objects.remove(i);
                p.spawn(role, o.x, rng);
                continue;
            }
            if proto.camp.is_some() {
                p.camps.push(Camp {
                    object: level.objects[i].id,
                    t: 0.0,
                    waiting_spawn: false,
                });
            }
            i += 1;
        }
        p
    }

    pub fn spawn(&mut self, role: Role, x: f32, rng: &mut Rng) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        let mut u = Unit::new(id, role, x, rng);
        u.ai = u.start_state();
        self.units.push(u);
        id
    }

    pub fn count(&self, role: Role) -> usize {
        self.units.iter().filter(|u| u.role == role).count()
    }

    /// Coin grabbers for every citizen with room in their purse.
    pub fn grabbers(&self) -> impl Iterator<Item = Grabber> + '_ {
        self.units.iter().map(|u| Grabber {
            who: Who::Unit(u.id),
            x: u.x,
            y: 0.0,
            flip: false,
            rect: UNIT_BOX,
            purse: (u.x, PURSE_Y),
            only_monarch_coins: u.role == Role::Beggar,
            // orig: EnterGuardSlot sets Wallet.canGrabCoins = false.
            room: if u.in_slot {
                0
            } else {
                tuning(u.role).max_coins.saturating_sub(u.coins)
            },
        })
    }

    /// A citizen picked up a coin. Beggars become peasants.
    /// orig: `Wallet.SuckCoin()`, `Beggar.HandleOnPickupPlayerCoin()`.
    pub fn credit(&mut self, id: u32, rng: &mut Rng, events: &mut impl FnMut(UnitEvent)) {
        if let Some(u) = self.units.iter_mut().find(|u| u.id == id) {
            u.coins += 1;
            if u.role == Role::Beggar {
                u.promote(Role::Peasant, rng);
                events(UnitEvent::Promoted {
                    unit: id,
                    role: Role::Peasant,
                });
            }
        }
    }

    /// Tools on a shop's rack. orig: `PayableShop.GetItemCount()`.
    pub fn rack_count(&self, shop: u32) -> u8 {
        self.tools
            .iter()
            .filter(|t| t.shop == Some(shop) && t.on_rack())
            .count() as u8
    }

    /// Puts a bought tool in the first free rack slot. orig: `PayableShop.Pay()` / `AddItem()`.
    pub fn stock_shop(&mut self, world: &WorldData, level: &Level, shop: u32) {
        let Some(o) = level.index_of(shop).map(|i| &level.objects[i]) else {
            return;
        };
        let Some(def) = world.protos[o.proto as usize].shop else {
            return;
        };
        let used: Vec<u8> = self
            .tools
            .iter()
            .filter(|t| t.shop == Some(shop) && t.on_rack())
            .map(|t| t.slot)
            .collect();
        let Some(slot) = (0..def.max_items).find(|s| !used.contains(s)) else {
            return;
        };
        let dir = if o.flip { -1.0 } else { 1.0 };
        let id = self.next_id;
        self.next_id += 1;
        self.tools.push(Tool {
            id,
            kind: def.tool,
            x: o.x + (def.item_placement.0 + slot as f32 * def.item_spacing) * dir,
            y: o.y + def.item_placement.1 - build::WALK_Y,
            vy: 0.0,
            shop: Some(shop),
            slot,
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        dt: f32,
        ctx: &Ctx,
        level: &mut Level,
        coins: &mut Coins,
        rng: &mut Rng,
        mut events: impl FnMut(UnitEvent),
    ) {
        self.update_camps(dt, ctx.world, level, rng, &mut events);
        self.reassign_timer -= dt;
        if self.reassign_timer <= 0.0 {
            self.reassign_timer = 1.0;
            self.reassign_work(ctx, level);
            self.distribute_free_archers();
        }
        // Tools fall to the ground when not on a rack.
        for t in &mut self.tools {
            if !t.on_rack() && t.y > 0.0 {
                t.vy -= crate::coins::GRAVITY * dt;
                t.y = (t.y + t.vy * dt).max(0.0);
            }
        }

        for s in &mut self.shields {
            s.constructing -= dt;
        }
        self.update_bakers(dt, ctx.world, level);
        self.update_knight_bands(ctx);

        // Workers at each open job, kept current as units start and stop
        // working (built once per tick: it means a pass over every object).
        let working = |u: &Unit| match u.ai {
            Ai::Work {
                target,
                working: true,
                ..
            } => Some(target),
            _ => None,
        };
        let mut busy_workers: Vec<(u32, usize)> = level
            .objects
            .iter()
            .filter(|o| work_available(ctx.world, o, ctx.worker_statue))
            .map(|o| (o.id, 0))
            .collect();
        for u in &self.units {
            if let Some(t) = working(u) {
                if let Ok(i) = busy_workers.binary_search_by_key(&t, |b| b.0) {
                    busy_workers[i].1 += 1;
                }
            }
        }
        let mut promotions: Vec<(usize, Role, u32)> = Vec::new();
        for ui in 0..self.units.len() {
            let before = working(&self.units[ui]);
            let tool_hit = self.step_unit(
                ui,
                dt,
                ctx,
                level,
                coins,
                rng,
                &mut events,
                &mut busy_workers,
            );
            let after = self.units.get(ui).and_then(working);
            if before != after {
                for (t, d) in [(before, -1i32), (after, 1)] {
                    if let Some(i) =
                        t.and_then(|t| busy_workers.binary_search_by_key(&t, |b| b.0).ok())
                    {
                        busy_workers[i].1 = (busy_workers[i].1 as i32 + d).max(0) as usize;
                    }
                }
            }
            if let Some((tool_idx, role)) = tool_hit {
                let tool_id = self.tools[tool_idx].id;
                promotions.push((ui, role, tool_id));
            }
        }
        // Loaves eaten this tick.
        for u in &mut self.units {
            if let Some(b) = u.eaten.take() {
                self.tools.retain(|t| t.id != b);
            }
        }
        // orig: PowerSlash → every follower's Archer.Hold().
        for ki in 0..self.units.len() {
            let Some(k) = self.units[ki].knight.as_mut() else {
                continue;
            };
            if !core::mem::take(&mut k.hold_archers) {
                continue;
            }
            let kid = self.units[ki].id;
            for u in &mut self.units {
                if u.follows == Some(kid) {
                    u.hold = 4.5;
                }
            }
        }
        for (ui, role, tool_id) in promotions {
            if let Some(ti) = self.tools.iter().position(|t| t.id == tool_id) {
                let tool = self.tools.remove(ti);
                self.units[ui].promote(role, rng);
                if role == Role::Knight {
                    self.knight_takes_shield(ui, tool.shop, rng);
                }
                events(UnitEvent::Promoted {
                    unit: self.units[ui].id,
                    role,
                });
            }
        }
    }

    /// orig: `Knight.GrabShield()`: the shop's side and colours; its banner flies.
    fn knight_takes_shield(&mut self, ui: usize, shop: Option<u32>, rng: &mut Rng) {
        let info = shop.and_then(|id| self.shields.iter_mut().find(|s| s.object == id));
        let (side, colors) = match info {
            Some(s) => {
                s.status = ShieldStatus::Taken;
                (s.side, Some(s.colors))
            }
            // orig: Knight.Awake picks a random side.
            None => (if rng.value() < 0.5 { 1 } else { -1 }, None),
        };
        let rank = self
            .units
            .iter()
            .filter(|u| u.knight.as_ref().is_some_and(|k| k.side == side))
            .count() as u8;
        let u = &mut self.units[ui];
        if let Some((a, b)) = colors {
            // orig: SpriteFX recolorA / recolorB (no skin colour on knights).
            u.outfit = a;
            u.skin = b;
        }
        u.knight = Some(KnightData::new(side, rank, shop));
    }

    /// Knights collect archers, archers drop knights that are gone.
    /// orig: `Knight.Update()` → `Kingdom.FetchArchersForJob(knight, n, 10)`,
    /// `Knight.OnDisable()` → `Archer.RemoveFromKnight()`.
    fn update_knight_bands(&mut self, ctx: &Ctx) {
        let knights: Vec<(u32, f32)> = self
            .units
            .iter()
            .filter(|u| u.knight.is_some())
            .map(|u| (u.id, u.x))
            .collect();
        for u in &mut self.units {
            if let Some(k) = u.follows {
                if !knights.iter().any(|&(id, _)| id == k) {
                    u.follows = None;
                    u.ai = Ai::Stand;
                }
            }
        }
        let _ = ctx;
        for &(kid, kx) in &knights {
            let followers = self.units.iter().filter(|u| u.follows == Some(kid)).count();
            let Some(ki) = self.units.iter().position(|u| u.id == kid) else {
                continue;
            };
            let Some(n) = knight::wants_archers(&mut self.units[ki], followers) else {
                continue;
            };
            let mut free: Vec<usize> = (0..self.units.len())
                .filter(|&i| {
                    let u = &self.units[i];
                    u.role == Role::Archer && u.slot.is_none() && u.follows.is_none()
                })
                .collect();
            free.sort_by(|&a, &b| {
                (self.units[a].x - kx)
                    .abs()
                    .total_cmp(&(self.units[b].x - kx).abs())
            });
            let can = free.len().saturating_sub(MIN_FREE_ARCHERS);
            for &i in free.iter().take(n.min(can)) {
                if (self.units[i].x - kx).abs() < knight::FETCH_RANGE {
                    self.units[i].follows = Some(kid);
                }
            }
        }
    }

    /// orig: `Baker.CallBeggars()`: every `callBeggarInterval`, the closest
    /// beggar not yet called comes for a loaf, while there are loaves and room.
    fn update_bakers(&mut self, dt: f32, world: &WorldData, level: &Level) {
        self.bakers.retain(|b| level.index_of(b.0).is_some());
        for o in &level.objects {
            if world.protos[o.proto as usize].baker.is_some()
                && !under_construction(world, o)
                && !self.bakers.iter().any(|b| b.0 == o.id)
            {
                self.bakers.push((o.id, 0.0, Vec::new()));
            }
        }
        for bi in 0..self.bakers.len() {
            let (b, _, _) = self.bakers[bi];
            let Some(o) = level.index_of(b).map(|i| &level.objects[i]) else {
                continue;
            };
            let Some((max, interval)) = world.protos[o.proto as usize].baker else {
                continue;
            };
            let bx = o.x;
            self.bakers[bi].1 += dt;
            if self.bakers[bi].1 < interval {
                continue;
            }
            self.bakers[bi].1 = 0.0;
            // Called beggars who became something else don't count.
            let units = &self.units;
            self.bakers[bi]
                .2
                .retain(|id| units.iter().any(|u| u.id == *id && u.role == Role::Beggar));
            let loaves = self.tools.iter().filter(|t| t.shop == Some(b)).count();
            let called = self.bakers[bi].2.len();
            if called >= max as usize || called >= loaves {
                continue;
            }
            // orig: Kingdom.GetClosestNonBakerBeggar.
            let pick = self
                .units
                .iter_mut()
                .filter(|u| u.role == Role::Beggar && u.baker.is_none())
                .min_by(|a, c| (a.x - bx).abs().total_cmp(&(c.x - bx).abs()));
            if let Some(u) = pick {
                // orig: Beggar.ComeToBaker leaves its camp.
                u.baker = Some(b);
                u.camp = None;
                self.bakers[bi].2.push(u.id);
            }
        }
    }

    /// Shield shops appearing in the level. `constructing` is how long until
    /// the castle keep is done (castle shops) or 0.
    pub fn register_shield_shops(
        &mut self,
        world: &WorldData,
        level: &Level,
        campfire: f32,
        coat: crate::kingdom::CoatOfArms,
        constructing: f32,
        rng: &mut Rng,
    ) {
        for o in &level.objects {
            if world.protos[o.proto as usize].shield_shop.is_some()
                && !self.shields.iter().any(|s| s.object == o.id)
            {
                self.shields.push(ShieldShop::new(
                    o.id,
                    o.x,
                    campfire,
                    coat,
                    constructing,
                    rng,
                ));
            }
        }
        self.shields.retain(|s| level.index_of(s.object).is_some());
    }

    /// orig: `PayableShield.CanPay()`.
    pub fn shield_can_pay(&self, shop: u32) -> bool {
        self.shields
            .iter()
            .find(|s| s.object == shop)
            .is_some_and(|s| s.can_pay())
    }

    /// orig: `PayableShield.Pay()` → `MakeAvailable()`: a shield on its stand.
    pub fn stock_shield(&mut self, world: &WorldData, level: &Level, shop: u32) {
        let Some(o) = level.index_of(shop).map(|i| &level.objects[i]) else {
            return;
        };
        let Some(def) = world.protos[o.proto as usize].shield_shop else {
            return;
        };
        let pos = def.shield_pos;
        let Some(s) = self.shields.iter_mut().find(|s| s.object == shop) else {
            return;
        };
        s.status = ShieldStatus::Available;
        let dir = if o.flip { -1.0 } else { 1.0 };
        let id = self.next_id;
        self.next_id += 1;
        self.tools.push(Tool {
            id,
            kind: ToolKind::Shield,
            x: o.x + pos.0 * dir,
            y: o.y + pos.1 - build::WALK_Y,
            vy: 0.0,
            shop: Some(shop),
            slot: 0,
        });
    }

    /// Restore a thief's carried tool on death. A modern payload keeps its
    /// identity; legacy saves only retained its kind and need a fresh id.
    /// orig: `Troll.DropLoot` releases the existing droppable, with upward
    /// impulse 1, while a shield keeps its `PayableShield` association.
    pub(crate) fn release_stolen_tool(
        &mut self,
        kind: ToolKind,
        id: Option<u32>,
        shield_shop: Option<u32>,
        x: f32,
        y: f32,
    ) {
        let id = match id {
            Some(id) => {
                // Death effects are normally consumed once; don't duplicate
                // an already restored payload if a caller repeats one.
                if self.tools.iter().any(|t| t.id == id) {
                    return;
                }
                self.next_id = self.next_id.max(id.saturating_add(1));
                id
            }
            None => {
                let id = self.next_id;
                self.next_id += 1;
                id
            }
        };
        let shop = (kind == ToolKind::Shield).then_some(shield_shop).flatten();
        if let Some(s) = shop.and_then(|id| self.shields.iter_mut().find(|s| s.object == id)) {
            if matches!(s.status, ShieldStatus::Empty | ShieldStatus::Broken) {
                s.status = ShieldStatus::Available;
            }
        }
        self.tools.push(Tool {
            id,
            kind,
            x,
            y,
            vy: 1.0,
            shop,
            slot: u8::MAX,
        });
    }

    /// Carried tools remain in the population's identity space while absent
    /// from `tools`; save validation must still reject unallocated ids.
    pub(crate) fn tool_id_allocated(&self, id: u32) -> bool {
        id < self.next_id
    }

    /// Archers following each knight.
    pub fn followers(&self, knight: u32) -> usize {
        self.units
            .iter()
            .filter(|u| u.follows == Some(knight))
            .count()
    }

    /// The knights within `range` of `x` that could charge, with their archer counts.
    pub fn knights_near(&self, x: f32, range: f32) -> Vec<(u32, usize)> {
        self.units
            .iter()
            .filter(|u| {
                u.knight.is_some() && !KnightData::charging(&u.ai) && (u.x - x).abs() <= range
            })
            .map(|u| (u.id, self.followers(u.id)))
            .collect()
    }

    /// orig: `PayableBorder.Pay()` → `Knight.DoCharge()`.
    /// Takes a unit out (it boarded the boat).
    pub fn take(&mut self, id: u32) -> Option<Unit> {
        let i = self.units.iter().position(|u| u.id == id)?;
        Some(self.units.remove(i))
    }

    /// The archers following a knight. orig: `Knight._archers`.
    pub fn followers_of(&self, knight: u32) -> Vec<u32> {
        self.units
            .iter()
            .filter(|u| u.follows == Some(knight))
            .map(|u| u.id)
            .collect()
    }

    /// A knight that sailed over from the last island. orig: `Knight.Awake()`
    /// (random side, no shield shop).
    pub fn spawn_knight(&mut self, x: f32, rng: &mut Rng) -> u32 {
        let id = self.spawn(Role::Knight, x, rng);
        let side = if rng.value() < 0.5 { 1 } else { -1 };
        let rank = self
            .units
            .iter()
            .filter(|u| u.knight.as_ref().is_some_and(|k| k.side == side))
            .count() as u8;
        if let Some(u) = self.units.iter_mut().find(|u| u.id == id) {
            u.knight = Some(KnightData::new(side, rank, None));
        }
        id
    }

    pub fn order_charge(&mut self, knight: u32) {
        let n = self.followers(knight);
        if let Some(k) = self
            .units
            .iter_mut()
            .find(|u| u.id == knight)
            .and_then(|u| u.knight.as_mut())
        {
            k.charge(n);
        }
    }

    /// Runs one unit's FSM, movement and taxes. Returns a tool it touched,
    /// if it's a peasant that can take it.
    #[allow(clippy::too_many_arguments)]
    fn step_unit(
        &mut self,
        ui: usize,
        dt: f32,
        ctx: &Ctx,
        level: &mut Level,
        coins: &mut Coins,
        rng: &mut Rng,
        events: &mut impl FnMut(UnitEvent),
        busy_workers: &mut [(u32, usize)],
    ) -> Option<(usize, Role)> {
        let uid = self.units[ui].id;
        let followers = self.units.iter().filter(|u| u.follows == Some(uid)).count();
        // Archers behind a knight: where it is and whether it holds fire.
        let lead = self.units[ui].follows.and_then(|k| {
            self.units.iter().find(|u| u.id == k).map(|u| {
                (
                    u.x,
                    u.mover.facing,
                    u.knight.as_ref().is_some_and(|k| k.hold_fire),
                )
            })
        });
        let u = &mut self.units[ui];
        let tune = tuning(u.role);
        u.inspired -= dt;
        if u.stunned > 0.0 {
            u.stunned -= dt;
            u.mover.update(&mut u.x, dt);
            u.anim = Anim::Stunned;
            u.anim_t += dt;
            return None;
        }
        if u.role == Role::Archer && u.slot.is_some() && archer_tower(u, ctx, rng) {
            archer_shoot(u, dt, ctx, false, rng, events);
            u.anim = if u.shoot.is_some() {
                Anim::Shoot
            } else {
                Anim::Idle
            };
            u.anim_t += dt;
            return None;
        }
        if u.role == Role::Archer {
            if let Some((kx, kf, _)) = lead {
                // orig: Archer.FollowKnight: soldier colours, in formation behind.
                if u.ai != Ai::Follow {
                    u.ai = Ai::Follow;
                    u.formation = rng.range(knight::ARCHER_DISTANCE.0, knight::ARCHER_DISTANCE.1);
                    if !u.soldier {
                        u.soldier = true;
                        u.outfit = if rng.value() < 0.5 {
                            ctx.coat.primary
                        } else {
                            ctx.coat.secondary
                        };
                    }
                }
                if u.hold > 0.0 {
                    // orig: Archer.HoldPosition: stand still 4.5 s.
                    u.hold -= dt;
                    u.mover.stop();
                } else {
                    u.mover.set_goal(kx - kf as f32 * u.formation, tune.run);
                }
            } else if u.ai == Ai::Follow {
                // orig: Archer.RemoveFromKnight → ConvertToHunter.
                u.ai = Ai::Stand;
                if u.soldier {
                    u.soldier = false;
                    u.outfit = random_outfit(Role::Archer, rng);
                }
            }
        }
        if u.role == Role::Archer && !lead.is_some_and(|l| l.2) {
            archer_shoot(u, dt, ctx, lead.is_some(), rng, events);
            if u.shoot.is_some() {
                u.mover.update(&mut u.x, dt);
                if u.anim != Anim::Shoot {
                    u.anim = Anim::Shoot;
                    u.anim_t = 0.0;
                } else {
                    u.anim_t += dt;
                }
                return None;
            }
        }
        u.idle_for += dt;
        u.check_timer -= dt;
        let check = u.check_timer <= 0.0;
        if check {
            u.check_timer = TRANSITION_CHECK;
        }
        let mut dumped_tool = false;
        if u.role == Role::Farmer {
            dumped_tool = farmer_ai(u, dt, check, ctx, rng, events);
        }
        if u.role == Role::Knight {
            knight::knight_ai(u, dt, check, ctx, followers, coins, rng, events);
        }

        // Transitions that interrupt running states.
        match u.role {
            Role::Beggar => {
                u.scan_timer -= dt;
                if u.scan_timer <= 0.0 && matches!(u.ai, Ai::Idle { .. } | Ai::GoToBaker { .. }) {
                    u.scan_timer = BEGGAR_SCAN_INTERVAL;
                    // orig: Beggar.ShouldGrabCoin: a coin the monarch dropped, claimed for 5 s.
                    let x = u.x;
                    let id = u.id;
                    if let Some(c) = coins.list.iter_mut().find(|c| {
                        c.is_free()
                            && c.dropped_by_monarch()
                            && (c.x - x).abs() <= BEGGAR_COIN_RANGE
                    }) {
                        if c.try_claim(Who::Unit(id), 5.0) {
                            u.mover.set_goal(c.x, tune.run);
                            u.ai = Ai::GrabCoin;
                        }
                    }
                }
            }
            Role::Peasant => {
                if check && matches!(u.ai, Ai::Idle { .. }) {
                    // orig: Peasant.ShouldGrabTool (scanner range 10).
                    let x = u.x;
                    // At night, only tools behind the walls (else they'd turn
                    // back to shelter every tick).
                    let reachable = |tx: f32| {
                        ctx.daytime || (ctx.shelter.left <= tx && tx <= ctx.shelter.right)
                    };
                    if let Some(t) = self
                        .tools
                        .iter()
                        .filter(|t| {
                            t.kind != ToolKind::Bread
                                && (t.x - x).abs() <= PEASANT_TOOL_RANGE
                                && reachable(t.x)
                        })
                        .min_by(|a, b| (a.x - x).abs().total_cmp(&(b.x - x).abs()))
                    {
                        u.ai = Ai::GrabTool { tool: t.id };
                    }
                }
            }
            Role::Worker => {
                if let Some(w) = u.queued_work.take() {
                    if matches!(u.ai, Ai::Idle { .. } | Ai::Shelter { .. }) {
                        u.ai = Ai::Work {
                            target: w,
                            working: false,
                            t: 0.0,
                        };
                    }
                }
            }
            Role::Archer => {
                // orig: ShouldGoToWall (any state, interrupts), ShouldFlee (any state,
                // interrupts; fleeing itself can't be interrupted), ShouldHunt (from Stand).
                // On the way to a tower (ShouldGoToTower comes first), none apply.
                if u.slot.is_some() || u.follows.is_some() {
                } else if let Ai::Flee { t } = u.ai {
                    u.ai = if t + dt >= 0.0 {
                        Ai::Stand
                    } else {
                        Ai::Flee { t: t + dt }
                    };
                } else if !ctx.safe && !matches!(u.ai, Ai::GoToWall { .. }) {
                    u.ai = Ai::GoToWall { phase: 0, t: 0.0 };
                } else if check && archer_should_flee(u, ctx) {
                    u.mover
                        .set_speed(tune.run, if u.x < ctx.campfire { 1 } else { -1 });
                    u.ai = Ai::Flee { t: -2.0 };
                } else if ctx.safe && matches!(u.ai, Ai::Stand) {
                    u.ai = Ai::Hunt { phase: 0, t: 0.0 };
                }
            }
            Role::Farmer | Role::Knight => {}
        }
        // At night everyone but the beggars, knights and posted archers stays
        // behind the walls: anyone outside, or headed out, shelters (the
        // original lets peasants stroll to outlying shops after dark).
        if !ctx.daytime
            && matches!(u.role, Role::Peasant | Role::Worker)
            && !matches!(u.ai, Ai::Shelter { .. } | Ai::Flee { .. })
        {
            let (l, r) = (ctx.shelter.left, ctx.shelter.right);
            let out = |x: f32| x < l || x > r;
            let posted = match u.ai {
                Ai::Work { target, .. } => level
                    .index_of(target)
                    .is_some_and(|i| night_post(ctx.world, &level.objects[i])),
                _ => false,
            };
            if !posted && (out(u.x) || u.mover.goal.is_some_and(|(g, _)| out(g))) {
                u.ai = Ai::Shelter { phase: 0, t: 0.0 };
            }
        }
        // Run the current state (farmer_ai already ran the farmer's).
        let mut tool_hit = None;
        if u.role == Role::Worker && !matches!(u.ai, Ai::Work { working: true, .. }) {
            u.y = 0.0;
            u.resting_on_job = false;
            u.pushing = false;
        }
        if u.role != Role::Farmer && u.role != Role::Knight {
            match u.ai {
                Ai::Idle { phase, t } => match u.role {
                    Role::Beggar => {
                        u.ai = beggar_idle(u, phase, t + dt, ctx, level, &self.camps, rng)
                    }
                    Role::Worker => u.ai = worker_idle(u, phase, t + dt, rng),
                    _ => u.ai = peasant_idle(u, phase, t + dt, ctx, level, rng),
                },
                Ai::GrabCoin => {
                    if !u.mover.moving_to_goal {
                        u.ai = u.start_state();
                    }
                }
                Ai::GoToBaker { phase, t } => {
                    u.ai = beggar_to_baker(
                        u,
                        phase,
                        t + dt,
                        level,
                        &self.tools,
                        &mut self.bakers,
                        rng,
                    );
                }
                Ai::GrabTool { tool } => match self.tools.iter().position(|t| t.id == tool) {
                    None => u.ai = Ai::Idle { phase: 0, t: 0.0 },
                    Some(ti) => {
                        let t = &self.tools[ti];
                        u.mover.set_goal(t.x, tune.run);
                        // orig: Peasant.OnTriggerStay2D: touching the tool promotes.
                        if u.box_at().overlaps(&tool_box(t.kind).at(t.x, t.y, false)) {
                            tool_hit = Some((ti, Role::for_tool(t.kind)));
                        } else if !u.mover.moving_to_goal {
                            u.ai = Ai::Idle { phase: 0, t: 0.0 };
                        }
                    }
                },
                Ai::Shelter { phase, t } => u.ai = worker_shelter(u, phase, t + dt, ctx, rng),
                Ai::Work { target, working, t } => {
                    u.ai = worker_work(
                        u,
                        target,
                        working,
                        t + dt,
                        ctx,
                        level,
                        coins,
                        rng,
                        busy_workers,
                        events,
                    )
                }
                Ai::Hunt { phase, t } => {
                    u.ai = archer_hunt(u, phase, t + dt, ctx, level, coins, rng)
                }
                Ai::GoToWall { phase, t } => u.ai = archer_wall(u, phase, t + dt, ctx, rng),
                Ai::Stand
                | Ai::Farm { .. }
                | Ai::Flee { .. }
                | Ai::Assemble { .. }
                | Ai::KnightWall
                | Ai::Charge { .. }
                | Ai::Board { .. }
                | Ai::Follow => {}
            }
        }
        if dumped_tool {
            // orig: Farmer.Idle: DropItem().DropInWater(), Demote().
            u.promote(Role::Peasant, rng);
            u.outfit = random_outfit(Role::Peasant, rng);
            events(UnitEvent::Promoted {
                unit: u.id,
                role: Role::Peasant,
            });
        }

        // orig: Wallet.TryToPayTaxes / PayTaxes: touching the monarch with spare coins.
        let monarch_box = crate::coins::MONARCH_BOX.at(ctx.monarch_x, 0.0, false);
        match u.taxes {
            None => {
                if u.coins > tax_above(u.role)
                    && !ctx.monarch_running
                    && u.box_at().overlaps(&monarch_box)
                {
                    u.taxes = Some((0.0, 0));
                    u.mover.pause(1.0);
                    events(UnitEvent::PaidTaxes(u.id));
                }
            }
            Some((t, dropped)) => {
                let t = t + dt;
                // First coin after 0.4 s, then one every 0.1 s, keeping one.
                let due = if t >= 0.4 {
                    1 + ((t - 0.4) / 0.1) as u32
                } else {
                    0
                };
                let mut dropped = dropped;
                while dropped < due && u.coins > tax_above(u.role) {
                    let dir = if ctx.monarch_x >= u.x { 1.0 } else { -1.0 };
                    coins.drop(
                        Who::Unit(u.id),
                        (u.x, PURSE_Y),
                        (rng.range(1.1, 2.1) * dir, 2.5),
                        Policy::Nobody,
                        Some(Who::Monarch),
                        Some(0.6),
                    );
                    u.coins -= 1;
                    dropped += 1;
                }
                u.taxes = if u.coins <= tax_above(u.role) {
                    None
                } else {
                    Some((t, dropped))
                };
            }
        }

        // Movement, staying on the island.
        u.mover.update(&mut u.x, dt);
        if level.width > 1.0 {
            u.x = u.x.clamp(level.left + 0.5, level.left + level.width - 0.5);
        }

        // Animation. orig: Mover sets Animator "Speed" = |vx| × 1.1.
        let speed = u.mover.vx.abs() * 1.1;
        let anim = if matches!(u.ai, Ai::Work { working: true, .. }) && u.pushing {
            Anim::Push
        } else if matches!(u.ai, Ai::Work { working: true, .. }) && !u.resting_on_job {
            Anim::Hammer
        } else if matches!(u.ai, Ai::Farm { phase: 2..=3, .. }) {
            Anim::Farm
        } else if u.knight.as_ref().is_some_and(|k| k.is_slashing()) {
            Anim::Slash
        } else if speed > tune.run_anim {
            Anim::Run
        } else if speed > tune.walk_anim {
            Anim::Walk
        } else {
            Anim::Idle
        };
        let entered = anim != u.anim;
        let before = if entered { 0.0 } else { u.anim_t };
        if entered {
            u.anim = anim;
            u.anim_t = 0.0;
        } else {
            let speed = if anim == Anim::Hammer {
                ctx.world
                    .animations
                    .state_speed("worker", "Hammer")
                    .unwrap_or(1.0)
            } else {
                1.0
            };
            u.anim_t += dt * speed;
        }
        if anim == Anim::Hammer {
            use crate::animation::EventKind;
            // Worker.OnAnimHammerHit is sound only. Workable.FinishWork keeps
            // its separate workTime timer, even when the hammer hits earlier.
            let hits = ctx.world.animations.clip("worker_hammer").map_or_else(
                || {
                    if ctx.world.animations.clips.is_empty() {
                        crate::animation::event_occurrences(
                            0.41666666, 1.0, true, before, u.anim_t, entered,
                        )
                    } else {
                        0
                    }
                },
                |clip| clip.event_count(EventKind::HammerHit, before, u.anim_t, entered),
            );
            for _ in 0..hits {
                events(UnitEvent::WorkSound {
                    x: u.x,
                    tree: false,
                });
            }
        }
        tool_hit
    }

    /// orig: `BeggarCamp.SlowUpdate()`: refill to `maxBeggars`, one per `spawnInterval`.
    fn update_camps(
        &mut self,
        dt: f32,
        world: &WorldData,
        level: &Level,
        rng: &mut Rng,
        events: &mut impl FnMut(UnitEvent),
    ) {
        for ci in 0..self.camps.len() {
            let camp = self.camps[ci];
            let Some(o) = level.index_of(camp.object).map(|i| &level.objects[i]) else {
                continue;
            };
            let Some(def) = world.protos[o.proto as usize].camp else {
                continue;
            };
            let members = self
                .units
                .iter()
                .filter(|u| u.camp == Some(camp.object))
                .count();
            let c = &mut self.camps[ci];
            c.t += dt;
            if !c.waiting_spawn {
                if c.t >= 5.0 {
                    c.t = 0.0;
                    if members < def.max_beggars as usize {
                        c.waiting_spawn = true;
                    }
                }
            } else if c.t >= def.spawn_interval {
                c.t = 0.0;
                c.waiting_spawn = false;
                if members < def.max_beggars as usize {
                    let x = o.x + if o.flip { -1.0 } else { 1.0 };
                    let id = self.spawn(Role::Beggar, x, rng);
                    if let Some(u) = self.units.iter_mut().find(|u| u.id == id) {
                        u.camp = Some(camp.object);
                    }
                    events(UnitEvent::Spawned(id));
                }
            }
        }
    }

    /// A troll hit a citizen. Armed citizens drop their tool and become
    /// peasants; peasants drop coins and, broke, become beggars; the hit
    /// one is knocked down. orig: `Character.HandleOnReceiveDamage()`, `Peasant.Stun()`.
    pub fn hurt(
        &mut self,
        id: u32,
        damage: i32,
        from_dir: f32,
        coins: &mut Coins,
        rng: &mut Rng,
        events: &mut impl FnMut(UnitEvent),
    ) {
        let Some(ui) = self.units.iter().position(|u| u.id == id) else {
            return;
        };
        let u = &self.units[ui];
        if u.stunned > 0.0 || u.role == Role::Beggar {
            return;
        }
        let x = u.x;
        // orig: Character.HandleOnReceiveDamage: inspired citizens only get bumped.
        if u.inspired > 0.0 {
            let u = &mut self.units[ui];
            if u.role == Role::Peasant {
                u.stunned = damage as f32 + rng.range(0.5, 2.0);
            }
            u.mover.vx = from_dir * rng.range(4.5, 5.5) * 0.5;
            return;
        }
        if u.role == Role::Knight {
            // orig: Character (knight): drop a coin per damage at 40 % (half
            // lost in the water), demote at 1 coin or less; no shield drops.
            for _ in 0..damage {
                let u = &mut self.units[ui];
                if u.coins == 0 {
                    break;
                }
                if rng.value() < 0.4 {
                    u.coins -= 1;
                    if rng.value() >= 0.5 {
                        let v = (
                            rng.range(-1.0, 1.0) * rng.range(2.0, 3.0),
                            rng.range(2.0, 3.0),
                        );
                        coins.drop(Who::Unit(id), (x, PURSE_Y), v, Policy::Nobody, None, None);
                    }
                }
            }
            let u = &mut self.units[ui];
            // orig: Knight.HandleOnReceiveDamage: pause and block, facing the hitter.
            u.mover.pause(1.0);
            u.mover.facing = if from_dir > 0.0 { -1 } else { 1 };
            if u.coins <= 1 {
                let shop = u.knight.as_ref().and_then(|k| k.shop);
                if let Some(s) = shop.and_then(|s| self.shields.iter_mut().find(|x| x.object == s))
                {
                    s.status = ShieldStatus::Broken;
                }
                let u = &mut self.units[ui];
                u.promote(Role::Peasant, rng);
                u.outfit = random_outfit(Role::Peasant, rng);
                u.skin = random_skin(rng);
                events(UnitEvent::KnightLost { shop });
                events(UnitEvent::Promoted {
                    unit: id,
                    role: Role::Peasant,
                });
            }
            return;
        }
        if let Some(tool) = u.role.tool() {
            // orig: DropItem, then Demote to peasant.
            let tid = self.next_id;
            self.next_id += 1;
            self.tools.push(Tool {
                id: tid,
                kind: tool,
                x: x - 0.1 * from_dir,
                y: 0.3,
                vy: rng.range(2.0, 4.0),
                shop: None,
                slot: 0,
            });
            let u = &mut self.units[ui];
            u.promote(Role::Peasant, rng);
            u.outfit = random_outfit(Role::Peasant, rng);
            events(UnitEvent::Promoted {
                unit: id,
                role: Role::Peasant,
            });
        } else if u.role == Role::Peasant {
            // orig: dropCoinsBeforeDemote: one coin per damage, half lost in the water.
            for _ in 0..damage {
                let u = &mut self.units[ui];
                if u.coins == 0 {
                    break;
                }
                u.coins -= 1;
                if rng.value() >= 0.5 {
                    let v = (
                        rng.range(-1.0, 1.0) * rng.range(2.0, 4.0),
                        rng.range(2.0, 4.0),
                    );
                    coins.drop(Who::Unit(id), (x, PURSE_Y), v, Policy::Nobody, None, None);
                }
            }
            if self.units[ui].coins == 0 {
                let u = &mut self.units[ui];
                u.promote(Role::Beggar, rng);
                u.outfit = random_outfit(Role::Beggar, rng);
                events(UnitEvent::Promoted {
                    unit: id,
                    role: Role::Beggar,
                });
            }
        }
        // orig: bump away and stun (peasants; demoted ones are peasants now).
        let u = &mut self.units[ui];
        if u.role == Role::Peasant {
            u.stunned = damage as f32 + rng.range(0.5, 2.0);
            u.mover.stop();
            u.mover.vx = from_dir * rng.range(4.5, 5.5) * 0.5;
            u.mover.facing = if from_dir > 0.0 { -1 } else { 1 };
        }
    }

    /// Sends the closest available archer to a free guard slot, keeping one
    /// archer free. orig: `Kingdom.FetchArchersForJob(slot, 1)` (`minFreeArchers` 1).
    pub fn fetch_archer_for_slot(&mut self, slot: (u32, u8), x: f32) {
        let mut free: Vec<usize> = (0..self.units.len())
            .filter(|&i| {
                self.units[i].role == Role::Archer
                    && self.units[i].slot.is_none()
                    && self.units[i].follows.is_none()
            })
            .collect();
        if free.len() <= MIN_FREE_ARCHERS {
            return;
        }
        free.sort_by(|&a, &b| {
            (self.units[a].x - x)
                .abs()
                .total_cmp(&(self.units[b].x - x).abs())
        });
        self.units[free[0]].slot = Some(slot);
    }

    /// Splits unposted hunters between the two walls by position.
    /// Refreshed at dusk and on the one-second reassignment tick.
    /// orig: `Kingdom.DistributeFreeArchers()`.
    pub fn distribute_free_archers(&mut self) {
        let mut free: Vec<usize> = (0..self.units.len())
            .filter(|&i| {
                self.units[i].role == Role::Archer
                    && self.units[i].slot.is_none()
                    && self.units[i].follows.is_none()
            })
            .collect();
        free.sort_by(|&a, &b| self.units[a].x.total_cmp(&self.units[b].x));
        let half = free.len() / 2;
        for (k, &i) in free.iter().enumerate() {
            self.units[i].guard_side = Some(if k >= half { 1 } else { -1 });
        }
    }

    /// Guard slots with an archer assigned.
    pub fn taken_slots(&self) -> impl Iterator<Item = (u32, u8)> + '_ {
        self.units.iter().filter_map(|u| u.slot)
    }

    /// Matches farmers to farmable fields by distance. orig: `Kingdom.ReassignFarms()`.
    pub fn assign_farms(&mut self, lands: &[(u32, f32)]) {
        let farmers: Vec<usize> = (0..self.units.len())
            .filter(|&i| self.units[i].role == Role::Farmer)
            .collect();
        if farmers.is_empty() || lands.is_empty() {
            return;
        }
        // orig: Farmer.GetTaskCost: rounded distance.
        let a = assign(farmers.len(), lands.len(), |i, j| {
            libm::roundf((self.units[farmers[i]].x - lands[j].1).abs()) as i32
        });
        for (k, &j) in a.iter().enumerate() {
            self.units[farmers[k]].farm = lands.get(j).map(|l| l.0);
        }
    }

    /// A squid took this citizen: armed ones drop their tool and coins
    /// first. orig: `Character.Grab()`. Knights can't be grabbed.
    pub fn snatch(&mut self, id: u32, coins: &mut Coins, rng: &mut Rng) -> bool {
        let Some(ui) = self.units.iter().position(|u| u.id == id) else {
            return false;
        };
        let u = &self.units[ui];
        if u.role == Role::Knight || u.role == Role::Beggar {
            return false;
        }
        let (x, role, n) = (u.x, u.role, u.coins);
        if role != Role::Peasant {
            for _ in 0..n {
                let v = (
                    rng.range(-1.0, 1.0) * rng.range(2.0, 4.0),
                    rng.range(2.0, 4.0),
                );
                coins.drop(Who::Unit(id), (x, PURSE_Y), v, Policy::Nobody, None, None);
            }
            if let Some(tool) = role.tool() {
                let tid = self.next_id;
                self.next_id += 1;
                self.tools.push(Tool {
                    id: tid,
                    kind: tool,
                    x,
                    y: 0.3,
                    vy: rng.range(2.0, 4.0),
                    shop: None,
                    slot: 0,
                });
            }
        }
        self.units.remove(ui);
        true
    }

    /// Gives free workers the cheapest open jobs. orig: `Kingdom.ReassignWork()`.
    fn reassign_work(&mut self, ctx: &Ctx, level: &Level) {
        let world = ctx.world;
        // orig: only work more than workPortalDistance inside the closest portals.
        let (mut lo, mut hi) = (f32::MIN, f32::MAX);
        for p in ctx.portals {
            if p.x < ctx.campfire {
                lo = lo.max(p.x + WORK_PORTAL_DISTANCE);
            } else {
                hi = hi.min(p.x - WORK_PORTAL_DISTANCE);
            }
        }
        let (portal_lo, portal_hi) = (lo, hi);
        // At night only work well inside the walls (workers sent further
        // would turn back to shelter every tick).
        if !ctx.daytime {
            lo = lo.max(ctx.shelter.left + NIGHT_WORK_MARGIN);
            hi = hi.min(ctx.shelter.right - NIGHT_WORK_MARGIN);
        }
        // (target id, x, workersNeeded rank, priority)
        let mut jobs: Vec<(u32, f32, i32, i32)> = Vec::new();
        // Workers at each target, in one pass: a per-site scan over the units
        // made this O(sites × units) once a second.
        let mut busy: Vec<(u32, i32)> = Vec::new();
        for u in &self.units {
            let t = match u.ai {
                Ai::Work { target, .. } => target,
                _ => match u.queued_work {
                    Some(t) => t,
                    None => continue,
                },
            };
            match busy.binary_search_by_key(&t, |b| b.0) {
                Ok(i) => busy[i].1 += 1,
                Err(i) => busy.insert(i, (t, 1)),
            }
        }
        for o in &level.objects {
            if !work_available(world, o, ctx.worker_statue) {
                continue;
            }
            // orig: ballista crews never flee, so they wind it at night too.
            let (lo, hi) = if night_post(world, o) {
                (portal_lo, portal_hi)
            } else {
                (lo, hi)
            };
            if o.x <= lo || o.x >= hi {
                continue;
            }
            // orig: Wall.GetWorkPriority −20 (repairs come after building),
            // Ballista.GetWorkPriority 1.
            let p = &world.protos[o.proto as usize];
            let priority = if under_construction(world, o) {
                0
            } else if p.wall_def.is_some() {
                -20
            } else if p.ballista.is_some() {
                1
            } else if p.catapult.is_some() {
                // orig: Catapult.GetWorkPriority: 0 while moving or reloading, else −200.
                if matches!(
                    o.mode,
                    crate::siege::mode::MOVING | crate::siege::mode::RELOADING
                ) {
                    0
                } else {
                    -200
                }
            } else {
                0
            };
            let max = max_workers(world, o) as i32;
            let busy = busy
                .binary_search_by_key(&o.id, |b| b.0)
                .map(|i| busy[i].1)
                .unwrap_or(0);
            let needed = max - busy;
            for i in 0..needed.max(0) {
                jobs.push((o.id, o.x, needed - i, priority));
            }
        }
        let free: Vec<usize> = (0..self.units.len())
            .filter(|&i| {
                let u = &self.units[i];
                u.role == Role::Worker
                    && u.queued_work.is_none()
                    && !matches!(u.ai, Ai::Work { .. })
            })
            .collect();
        if jobs.is_empty() || free.is_empty() {
            return;
        }
        // orig: Worker.GetTaskCost: distance, minus 10 per extra worker
        // still needed, minus the job's priority.
        let a = assign(free.len(), jobs.len(), |i, j| {
            let u = &self.units[free[i]];
            (jobs[j].1 - u.x).abs() as i32 - (jobs[j].2 - 1) * 10 - jobs[j].3
        });
        for (k, &j) in a.iter().enumerate() {
            if j < jobs.len() {
                self.units[free[k]].queued_work = Some(jobs[j].0);
            }
        }
    }
}

/// Work left on an object. orig: `WorkableBuilding.WorkAvailable()` (only
/// buildings without auto-build need workers), `WorkableTree.WorkAvailable()`.
fn work_available(world: &WorldData, o: &crate::level::Object, worker_statue: bool) -> bool {
    let p = &world.protos[o.proto as usize];
    if let Some(b) = &p.building {
        if b.auto_build_rate <= 0.0 && under_construction(world, o) {
            return true;
        }
    }
    if o.repair && wall_needs_repair(world, o, worker_statue) {
        return true;
    }
    // orig: Ballista / Catapult.WorkAvailable: always (someone has to wind it).
    if (p.ballista.is_some() || p.catapult.is_some()) && !under_construction(world, o) {
        return true;
    }
    // orig: Wharf.WorkAvailable: parts stacked (`reload` counts them).
    if p.wharf.is_some() {
        return o.reload > 0 && o.mode == 0;
    }
    // orig: BoatPushPosition.WorkAvailable while pushing; Boat.WorkAvailable
    // (room on deck) while waiting. `reload` counts the workers aboard.
    if let Some(b) = &p.boat {
        return o.mode == crate::boat::mode::PUSHING
            || (o.mode == crate::boat::mode::WAITING && o.reload < b.max_workers as i32);
    }
    p.tree_work.is_some() && o.marked && o.chops_left > 0
}

/// orig: `Wall.GetTotalMaxHitPoints()`: ×`bonusHitPointsMultiplier` with the worker statue.
pub fn wall_max_hp(world: &WorldData, o: &crate::level::Object, worker_statue: bool) -> i32 {
    world.protos[o.proto as usize].wall_def.map_or(0, |w| {
        if worker_statue {
            libm::floorf(w.hit_points as f32 * w.bonus_hit_points_multiplier) as i32
        } else {
            w.hit_points
        }
    })
}

/// orig: `Wall.WorkAvailable()`.
pub fn wall_needs_repair(world: &WorldData, o: &crate::level::Object, worker_statue: bool) -> bool {
    world.protos[o.proto as usize].wall_def.is_some()
        && !under_construction(world, o)
        && o.hp < wall_max_hp(world, o, worker_statue)
}

/// A built ballista: its crew stays at it through the night.
fn night_post(world: &WorldData, o: &crate::level::Object) -> bool {
    world.protos[o.proto as usize].ballista.is_some() && !under_construction(world, o)
}

/// orig: `Workable.GetMaxWorkers()` (2), `WorkableTree` (1).
fn max_workers(world: &WorldData, o: &crate::level::Object) -> usize {
    let p = &world.protos[o.proto as usize];
    // orig: WorkableWharf.GetMaxWorkers 3; one per BoatPushPosition; Boat.maxWorkers.
    if p.wharf.is_some() {
        return 3;
    }
    if let Some(b) = &p.boat {
        return if o.mode == crate::boat::mode::PUSHING {
            b.push_positions.len()
        } else {
            (b.max_workers as i32 - o.reload).max(0) as usize
        };
    }
    if p.tree_work.is_some() || (p.ballista.is_some() && !under_construction(world, o)) {
        1
    } else {
        2
    }
}

/// orig: `Beggar.Idle()`: wander, or hang around the camp once joined.
fn beggar_idle(
    u: &mut Unit,
    phase: u8,
    t: f32,
    ctx: &Ctx,
    level: &Level,
    camps: &[Camp],
    rng: &mut Rng,
) -> Ai {
    let walk = tuning(u.role).walk;
    match phase {
        0 if t >= 0.3 => {
            if u.camp.is_none() && u.idle_for > BEGGAR_TIME_TO_CAMP {
                // orig: Kingdom.GetClosestBeggarCamp
                u.camp = camps
                    .iter()
                    .filter_map(|c| {
                        level
                            .index_of(c.object)
                            .map(|i| (c.object, level.objects[i].x))
                    })
                    .min_by(|a, b| (a.1 - u.x).abs().total_cmp(&(b.1 - u.x).abs()))
                    .map(|c| c.0);
            }
            let camp_x = u
                .camp
                .and_then(|c| level.index_of(c))
                .map(|i| level.objects[i].x);
            let goal = match camp_x {
                Some(cx) => cx + rng.range(-4.0, 4.0),
                None => u.x + rng.range(-0.5, 0.5),
            };
            u.mover.set_goal(goal, walk);
            let _ = ctx;
            Ai::Idle { phase: 1, t: 0.0 }
        }
        1 if !u.mover.moving_to_goal => Ai::Idle { phase: 2, t: 0.0 },
        // orig: ShouldGoToBaker when Idle ends.
        2 if t >= 8.0 && u.baker.is_some() => Ai::GoToBaker { phase: 0, t: 0.0 },
        2 if t >= 8.0 => Ai::Idle { phase: 0, t: 0.0 },
        _ => Ai::Idle { phase, t },
    }
}

/// orig: `Beggar.GoToBaker()`: within 7 of the bakery, to its loaf, eat it,
/// then wander off. Eating the bread is what brings beggars in from camp.
fn beggar_to_baker(
    u: &mut Unit,
    phase: u8,
    t: f32,
    level: &Level,
    tools: &[Tool],
    bakers: &mut [(u32, f32, Vec<u32>)],
    rng: &mut Rng,
) -> Ai {
    let tune = tuning(u.role);
    let Some(b) = u.baker else {
        return Ai::Idle { phase: 0, t: 0.0 };
    };
    let Some(bx) = level.index_of(b).map(|i| level.objects[i].x) else {
        u.baker = None;
        return Ai::Idle { phase: 0, t: 0.0 };
    };
    let breads: Vec<&Tool> = tools.iter().filter(|t| t.shop == Some(b)).collect();
    let entry = bakers.iter_mut().find(|e| e.0 == b);
    match phase {
        0 => {
            u.mover.set_goal(u.x.clamp(bx - 7.0, bx + 7.0), tune.run);
            Ai::GoToBaker { phase: 1, t: 0.0 }
        }
        1 if !u.mover.moving_to_goal => {
            // orig: Baker.GetBreadPosition: this beggar's loaf.
            let idx = entry
                .and_then(|e| e.2.iter().position(|&id| id == u.id))
                .unwrap_or(0);
            let goal = if breads.is_empty() {
                bx
            } else {
                breads[idx % breads.len()].x
            };
            u.mover.set_goal(goal, tune.run);
            Ai::GoToBaker { phase: 2, t: 0.0 }
        }
        2 if !u.mover.moving_to_goal => {
            // orig: Baker.TryEatBread → the loaf is eaten (4 s).
            let mut ate = false;
            if let Some(e) = entry {
                if let Some(idx) = e.2.iter().position(|&id| id == u.id) {
                    if let Some(bread) = breads.get(idx) {
                        u.eaten = Some(bread.id);
                        ate = true;
                    }
                    e.2.remove(idx);
                }
            }
            if ate {
                Ai::GoToBaker { phase: 3, t: -4.0 }
            } else {
                u.baker = None;
                u.mover.set_goal(u.x + rng.range(-4.0, 4.0), tune.walk);
                Ai::Idle { phase: 1, t: 0.0 }
            }
        }
        3 if t >= 0.0 => {
            u.baker = None;
            u.mover.set_goal(u.x + rng.range(-4.0, 4.0), tune.walk);
            Ai::Idle { phase: 1, t: 0.0 }
        }
        p => Ai::GoToBaker { phase: p, t },
    }
}

/// orig: `Peasant.Idle()`: walk to a random shop (or anywhere inside the
/// borders), then wait 8–19 s.
fn peasant_idle(u: &mut Unit, phase: u8, t: f32, ctx: &Ctx, level: &Level, rng: &mut Rng) -> Ai {
    let walk = tuning(u.role).walk;
    match phase {
        0 => {
            let shops: Vec<f32> = level
                .objects
                .iter()
                .filter(|o| ctx.world.protos[o.proto as usize].shop.is_some())
                .map(|o| o.x)
                .collect();
            let goal = if shops.is_empty() {
                rng.range(ctx.borders.left, ctx.borders.right)
            } else {
                shops[rng.range_i32(0, shops.len() as i32) as usize]
            };
            u.mover.set_goal(goal, walk);
            Ai::Idle { phase: 1, t: 0.0 }
        }
        1 if !u.mover.moving_to_goal => Ai::Idle {
            phase: 2,
            t: -(rng.range_i32(8, 20) as f32),
        },
        2 if t >= 0.0 => Ai::Idle { phase: 0, t: 0.0 },
        _ => Ai::Idle { phase, t },
    }
}

/// orig: `Worker.Idle()`: shuffle 1–3 times, then shelter.
fn worker_idle(u: &mut Unit, phase: u8, t: f32, rng: &mut Rng) -> Ai {
    let walk = tuning(u.role).walk;
    // phase encodes moves left × 2 + (0 = start move, 1 = moving/waiting).
    match phase {
        0 => {
            let n = rng.range_i32(1, 4) as u8;
            u.mover.set_goal(u.x + rng.range(-0.3, 0.3), walk);
            Ai::Idle {
                phase: n * 2 + 1,
                t: 0.0,
            }
        }
        p if p % 2 == 1 => {
            if u.mover.moving_to_goal {
                Ai::Idle { phase: p, t: 0.0 }
            } else if t >= 2.0 {
                let left = p / 2 - 1;
                if left == 0 {
                    Ai::Shelter { phase: 0, t: 0.0 }
                } else {
                    u.mover.set_goal(u.x + rng.range(-0.3, 0.3), walk);
                    Ai::Idle {
                        phase: left * 2 + 1,
                        t: 0.0,
                    }
                }
            } else {
                Ai::Idle { phase: p, t }
            }
        }
        p => Ai::Idle { phase: p, t },
    }
}

/// orig: `Worker.Shelter()`: get inside the borders (running at night if
/// far), then wait 5–8 s.
fn worker_shelter(u: &mut Unit, phase: u8, t: f32, ctx: &Ctx, rng: &mut Rng) -> Ai {
    let tune = tuning(u.role);
    match phase {
        0 => {
            let pos = u.x;
            let (l, r) = (ctx.shelter.left + 0.5, ctx.shelter.right - 0.5);
            let shelter = pos.clamp(l, r);
            let run = !ctx.daytime && (shelter - pos).abs() > 4.0;
            u.mover.set_goal(
                (shelter + rng.range(-3.0, 3.0)).clamp(l, r.max(l)),
                if run { tune.run } else { tune.walk },
            );
            Ai::Shelter { phase: 1, t: 0.0 }
        }
        1 => {
            if !u.mover.moving_to_goal {
                Ai::Shelter {
                    phase: 2,
                    t: -rng.range(5.0, 8.0),
                }
            } else if t > 10.0 {
                // orig: after 10 s on the way, slow to a walk.
                if let Some((g, _)) = u.mover.goal {
                    u.mover.set_goal(g, tune.walk);
                }
                Ai::Shelter { phase: 1, t }
            } else {
                Ai::Shelter { phase: 1, t }
            }
        }
        2 if t >= 0.0 => Ai::Idle { phase: 0, t: 0.0 },
        p => Ai::Shelter { phase: p, t },
    }
}

/// orig: `Worker.Work()`: go to the job, then work in `workTime` strokes,
/// each adding a build point or a chop.
#[allow(clippy::too_many_arguments)]
fn worker_work(
    u: &mut Unit,
    target: u32,
    working: bool,
    t: f32,
    ctx: &Ctx,
    level: &mut Level,
    coins: &mut Coins,
    rng: &mut Rng,
    busy: &[(u32, usize)],
    events: &mut impl FnMut(UnitEvent),
) -> Ai {
    let tune = tuning(u.role);
    let Some(i) = level.index_of(target) else {
        return Ai::Idle { phase: 0, t: 0.0 };
    };
    let o = &level.objects[i];
    if !work_available(ctx.world, o, ctx.worker_statue) {
        return Ai::Idle { phase: 0, t: 0.0 };
    }
    let is_tree = ctx.world.protos[o.proto as usize].tree_work.is_some();
    let is_repair =
        !under_construction(ctx.world, o) && ctx.world.protos[o.proto as usize].wall_def.is_some();
    let ballista = ctx.world.protos[o.proto as usize]
        .ballista
        .as_ref()
        .filter(|_| !under_construction(ctx.world, o));
    let catapult = ctx.world.protos[o.proto as usize]
        .catapult
        .filter(|_| !under_construction(ctx.world, o));
    if catapult.is_some() {
        use crate::siege::mode;
        // orig: Catapult.GetWorkOffset: pushers behind it in a row, the cranker
        // at 0.875 behind, otherwise anywhere near.
        let dir = if o.flip { -1.0 } else { 1.0 };
        let rank = busy.iter().find(|b| b.0 == target).map_or(0, |b| b.1) as f32;
        let spot = match o.mode {
            mode::MOVING => o.x - dir * (0.125 + 0.75 * if working { 0.0 } else { rank }),
            mode::RELOADING => o.x - dir * 0.875,
            _ => o.x + u.work_offset,
        };
        let others = busy.iter().find(|b| b.0 == target).map_or(0, |b| b.1);
        if !working {
            u.mover.set_goal(spot, tune.run);
            if others >= 2 {
                return Ai::Idle { phase: 0, t: 0.0 };
            }
            if !u.mover.moving_to_goal {
                return Ai::Work {
                    target,
                    working: true,
                    t: 0.0,
                };
            }
            return Ai::Work { target, working, t };
        }
        // Keep to the catapult as it rolls.
        if o.mode == mode::MOVING {
            u.mover.set_goal(
                o.x - dir * 0.125 - dir * 0.75 * (u.work_offset.abs()),
                tune.run,
            );
        } else {
            u.mover.set_goal(spot, tune.walk);
        }
        u.mover.facing = dir as i8;
        u.pushing = o.mode == mode::MOVING;
        let cranking = o.mode == mode::RELOADING;
        u.resting_on_job = !cranking && !u.pushing;
        if !cranking || t < WORK_TIME {
            return Ai::Work {
                target,
                working: true,
                t: if cranking { t } else { 0.0 },
            };
        }
        events(UnitEvent::Wound(target));
        return Ai::Work {
            target,
            working: true,
            t: 0.0,
        };
    }
    if let Some(b) = ctx.world.protos[o.proto as usize].boat.as_ref() {
        use crate::boat::mode;
        let dir = if o.flip { -1.0 } else { 1.0 };
        let others = busy.iter().find(|b| b.0 == target).map_or(0, |b| b.1);
        if o.mode == mode::PUSHING {
            // orig: BoatPushPosition: a fixed spot just behind each push position
            // (which one is kept in `works_before_moving` while pushing).
            let slot = if working {
                u.works_before_moving.max(0) as usize
            } else {
                others
            };
            let Some(&px) = b.push_positions.get(slot) else {
                return Ai::Idle { phase: 0, t: 0.0 };
            };
            let spot = o.x + (px + crate::boat::PUSH_OFFSET) * dir;
            if !working {
                u.mover.set_goal(spot, tune.run);
                if !u.mover.moving_to_goal {
                    u.works_before_moving = slot as i32;
                    return Ai::Work {
                        target,
                        working: true,
                        t: 0.0,
                    };
                }
                return Ai::Work { target, working, t };
            }
            // Keep to the boat as it moves.
            u.mover.set_goal(spot, tune.run);
            u.mover.facing = dir as i8;
            u.pushing = true;
            u.resting_on_job = false;
            return Ai::Work {
                target,
                working: true,
                t: 0.0,
            };
        }
        if u.pushing {
            // orig: BoatPushPosition.OnWorkFinish once it waits: the job is done.
            u.pushing = false;
            return Ai::Idle { phase: 0, t: 0.0 };
        }
        // orig: Boat.GetWorkOffset: DeckLeft; GetWorkAnimation: none.
        let spot = o.x + crate::boat::DECK_LEFT * dir;
        if !working {
            u.mover.set_goal(spot, tune.run);
            if !u.mover.moving_to_goal {
                return Ai::Work {
                    target,
                    working: true,
                    t: 0.0,
                };
            }
            return Ai::Work { target, working, t };
        }
        u.resting_on_job = true;
        if t < WORK_TIME {
            return Ai::Work { target, working, t };
        }
        events(UnitEvent::Embark(u.id));
        return Ai::Idle { phase: 0, t: 0.0 };
    }
    if ctx.world.protos[o.proto as usize].wharf.is_some() {
        // orig: WorkableWharf.GetWorkOffset: 2 × workPositionOffset.
        if !working {
            u.mover.set_goal(o.x + 2.0 * u.work_offset, tune.run);
            let others = busy.iter().find(|b| b.0 == target).map_or(0, |b| b.1);
            if others >= max_workers(ctx.world, o) {
                return Ai::Idle { phase: 0, t: 0.0 };
            }
            if !u.mover.moving_to_goal {
                u.mover.set_speed(0.0, if u.x < o.x { 1 } else { -1 });
                return Ai::Work {
                    target,
                    working: true,
                    t: 0.0,
                };
            }
            return Ai::Work { target, working, t };
        }
        if t < WORK_TIME {
            return Ai::Work { target, working, t };
        }
        events(UnitEvent::Wound(target));
        return Ai::Work {
            target,
            working: true,
            t: 0.0,
        };
    }
    if let Some(b) = ballista {
        // orig: Ballista: a fixed spot up on the tower (GetWorkOffset −0.5,
        // GetWorkHeight 2.0625), never fleeing.
        let dir = if o.flip { -1.0 } else { 1.0 };
        let spot = o.x + b.work_pos.0 * dir;
        if !working {
            u.y = 0.0;
            u.mover.set_goal(spot, tune.run);
            let others = busy.iter().find(|b| b.0 == target).map_or(0, |b| b.1);
            if others >= 1 {
                return Ai::Idle { phase: 0, t: 0.0 };
            }
            if !u.mover.moving_to_goal {
                return Ai::Work {
                    target,
                    working: true,
                    t: 0.0,
                };
            }
            return Ai::Work { target, working, t };
        }
        u.x = spot;
        u.y = b.work_pos.1 - build::WALK_Y;
        u.mover.stop();
        u.mover.facing = dir as i8;
        // orig: Ballista.GetWorkAnimation: hammer only while winding.
        let winding = o.reload < b.reload_work;
        u.resting_on_job = !winding;
        if !winding || t < WORK_TIME {
            return Ai::Work {
                target,
                working: true,
                t: if winding { t } else { 0.0 },
            };
        }
        // orig: Ballista.OnWorkFinish.
        level.objects[i].reload += 1;
        events(UnitEvent::Wound(target));
        return Ai::Work {
            target,
            working: true,
            t: 0.0,
        };
    }
    if !working {
        // orig: GetWorkOffset: buildings spread workers over ±0.5, trees ±0.35,
        // walls −0.5 (the inner side).
        let dir = if o.flip { -1.0 } else { 1.0 };
        let offset = if is_tree {
            if u.work_offset < 0.0 {
                -0.35
            } else {
                0.35
            }
        } else if is_repair {
            -0.5 * dir
        } else {
            0.5 * u.work_offset * dir
        };
        u.mover.set_goal(o.x + offset, tune.run);
        let others = busy.iter().find(|b| b.0 == target).map_or(0, |b| b.1);
        if others >= max_workers(ctx.world, o) {
            return Ai::Idle { phase: 0, t: 0.0 };
        }
        if !u.mover.moving_to_goal {
            u.mover.set_speed(0.0, if u.x < o.x { 1 } else { -1 });
            return Ai::Work {
                target,
                working: true,
                t: 0.0,
            };
        }
        return Ai::Work { target, working, t };
    }
    if t < WORK_TIME {
        return Ai::Work { target, working, t };
    }
    // A stroke finished. orig: Workable.FinishWork → OnWorkFinish.
    u.works_before_moving -= 1;
    if is_tree {
        // WorkableTree.OnWorkFinish plays the chop separately from the
        // worker's animation-event hammer sound.
        events(UnitEvent::WorkSound { x: u.x, tree: true });
        let o = &mut level.objects[i];
        o.chops_left -= 1;
        if o.chops_left <= 0 {
            // orig: WorkableTree.OnWorkFinish: coins fall, the tree is felled.
            let def =
                ctx.world.protos[o.proto as usize]
                    .tree_work
                    .unwrap_or(crate::data::TreeWorkDef {
                        strength: 0,
                        coin_yield: (0, 0),
                        chop_mark: None,
                    });
            let n = rng.range_i32(def.coin_yield.0, def.coin_yield.1 + 1);
            for _ in 0..n {
                coins.drop(
                    Who::Thing,
                    (o.x, 1.0 - build::WALK_Y),
                    (0.0, 0.0),
                    Policy::Nobody,
                    None,
                    Some(0.8),
                );
            }
            events(UnitEvent::TreeChopped(target));
            level.objects.remove(i);
            return Ai::Idle { phase: 0, t: 0.0 };
        }
        Ai::Work {
            target,
            working: true,
            t: 0.0,
        }
    } else if is_repair {
        // orig: Wall.OnWorkFinish: repairRate below full, repairRateAboveMax over.
        let o = &mut level.objects[i];
        let Some(w) = ctx.world.protos[o.proto as usize].wall_def else {
            return Ai::Idle { phase: 0, t: 0.0 };
        };
        o.hp += if o.hp < w.hit_points {
            w.repair_rate
        } else {
            w.repair_rate_above_max
        };
        events(UnitEvent::Repaired(target));
        if o.hp >= wall_max_hp(ctx.world, o, ctx.worker_statue) {
            o.repair = false;
            return Ai::Idle { phase: 0, t: 0.0 };
        }
        Ai::Work {
            target,
            working: true,
            t: 0.0,
        }
    } else {
        events(UnitEvent::BuildPoint(target));
        if build::add_build_point(ctx.world, level, target) {
            return Ai::Idle { phase: 0, t: 0.0 };
        }
        if u.works_before_moving <= 0 {
            // orig: every 3–4 strokes, shift position along the building.
            u.work_offset = rng.value() * 2.0 - 1.0;
            u.works_before_moving = rng.range_i32(3, 5);
            return Ai::Work {
                target,
                working: false,
                t: 0.0,
            };
        }
        Ai::Work {
            target,
            working: true,
            t: 0.0,
        }
    }
}

/// orig: `Farmer.ShouldFlee()`: outside the walls with greed within `fleeRange`.
fn farmer_should_flee(u: &Unit, ctx: &Ctx) -> bool {
    !ctx.borders.contains(u.x)
        && ctx
            .enemies
            .iter()
            .any(|e| (e.x - u.x).abs() <= FARMER_FLEE_RANGE)
}

/// The farmer's FSM: Idle → Farm while safe, Shelter when not, Flee from
/// greed outside the walls; every state falls back to Idle.
/// orig: `Farmer` (states and transitions from `Awake`). Returns true when
/// the farmer gives up and dumps the scythe.
fn farmer_ai(
    u: &mut Unit,
    dt: f32,
    check: bool,
    ctx: &Ctx,
    rng: &mut Rng,
    events: &mut impl FnMut(UnitEvent),
) -> bool {
    let tune = tuning(Role::Farmer);
    u.since_farmed += dt;
    let flee = |u: &mut Unit, rng: &mut Rng| {
        u.mover
            .set_speed(tune.run, if u.x < ctx.campfire { 1 } else { -1 });
        Ai::Flee {
            t: -rng.range(3.0, 5.0),
        }
    };
    // Interrupting transitions, checked once a second.
    if check {
        if !matches!(u.ai, Ai::Flee { .. }) && farmer_should_flee(u, ctx) {
            u.ai = flee(u, rng);
            return false;
        }
        if matches!(u.ai, Ai::Idle { .. } | Ai::Farm { .. }) && !ctx.safe {
            u.ai = Ai::Shelter { phase: 0, t: 0.0 };
        }
    }
    let land = |id: u32| ctx.farmlands.iter().find(|f| f.id == id);
    let mut dumped = false;
    let done = match u.ai {
        Ai::Idle { phase, t } => {
            let t = t + dt;
            match phase {
                0 => {
                    u.mover.set_goal(u.x + rng.range(-1.0, 1.0), tune.walk);
                    u.ai = Ai::Idle { phase: 1, t: 0.0 };
                    false
                }
                1 if !u.mover.moving_to_goal => {
                    u.ai = Ai::Idle {
                        phase: 2,
                        t: -rng.range(4.0, 5.0),
                    };
                    false
                }
                2 if t >= 0.0 => {
                    dumped = u.since_farmed > SECONDS_PER_DAY * FARMER_DUMP_TOOL_DAYS;
                    true
                }
                _ => {
                    u.ai = Ai::Idle { phase, t };
                    false
                }
            }
        }
        Ai::Farm { target, phase, t } => {
            let t = t + dt;
            match phase {
                0 => match land(target) {
                    Some(f) => {
                        u.mover.set_goal(f.x, tune.walk);
                        u.ai = Ai::Farm {
                            target,
                            phase: 1,
                            t: 0.0,
                        };
                        false
                    }
                    None => true,
                },
                1 => {
                    if u.farm.is_none() || land(target).is_none() {
                        true
                    } else if u.mover.moving_to_goal {
                        false
                    } else if u.farm == Some(target) {
                        u.since_farmed = 0.0;
                        u.ai = Ai::Farm {
                            target,
                            phase: 2,
                            t: -FARM_TIME / 2.0,
                        };
                        false
                    } else {
                        true
                    }
                }
                2 if t >= 0.0 => {
                    if land(target).is_some() {
                        events(UnitEvent::Farmed(target));
                        u.ai = Ai::Farm {
                            target,
                            phase: 3,
                            t: -FARM_TIME / 2.0,
                        };
                        false
                    } else {
                        true
                    }
                }
                3 if t >= 0.0 => true,
                _ => {
                    u.ai = Ai::Farm { target, phase, t };
                    false
                }
            }
        }
        Ai::Shelter { phase, t } => {
            let t = t + dt;
            match phase {
                0 => {
                    // orig: shelter at a farmhouse past level 1, else run to the campfire.
                    let house = u.farm.and_then(land).and_then(|f| f.house);
                    let inside = |x: f32| x > ctx.shelter.left && x < ctx.shelter.right;
                    match house {
                        // Only a farmhouse behind the walls (a deviation:
                        // the original's farmers also shelter outside).
                        Some((hx, level)) if level > 1 && inside(hx) => {
                            u.mover.set_goal(hx + rng.range(-0.5, 0.5), tune.walk)
                        }
                        _ => u
                            .mover
                            .set_goal(ctx.campfire + rng.range(-2.5, 2.5), tune.run),
                    }
                    u.ai = Ai::Shelter { phase: 1, t: 0.0 };
                    false
                }
                1 if !u.mover.moving_to_goal => {
                    u.ai = Ai::Shelter {
                        phase: 2,
                        t: -rng.range(3.0, 5.0),
                    };
                    false
                }
                2 if t >= 0.0 => true,
                _ => {
                    u.ai = Ai::Shelter { phase, t };
                    false
                }
            }
        }
        Ai::Flee { t } => {
            let t = t + dt;
            u.ai = Ai::Flee { t };
            t >= 0.0
        }
        _ => true,
    };
    if done {
        // The state's coroutine ended: every transition may fire, else back to Idle.
        let should_farm = ctx.safe && u.farm.and_then(land).is_some_and(|f| f.farmable);
        u.ai = if farmer_should_flee(u, ctx) {
            flee(u, rng)
        } else {
            match u.ai {
                Ai::Idle { .. } if should_farm => Ai::Farm {
                    target: u.farm.unwrap_or(0),
                    phase: 0,
                    t: 0.0,
                },
                Ai::Idle { .. } | Ai::Farm { .. } if !ctx.safe => Ai::Shelter { phase: 0, t: 0.0 },
                _ => Ai::Idle { phase: 0, t: 0.0 },
            }
        };
    }
    dumped
}

/// What an archer is shooting at.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quarry {
    Troll(u32),
    Animal(u32),
    Portal(u32),
}

/// orig: `Archer.TargetInRange()`: reachable at `maxForce` at all.
fn target_in_range(dx: f32, dy: f32) -> bool {
    let v2 = crate::greed::ARROW_SPEED * crate::greed::ARROW_SPEED;
    let g = crate::coins::GRAVITY;
    v2 * v2 - g * (g * dx * dx + 2.0 * dy * v2) >= 0.0
}

/// orig: `Archer.Update` → `ShouldShootEnemy()` / `ShouldShootWildlife()` /
/// `Shoot()`: aim at the toughest troll in range, else the closest animal,
/// and loose 2 arrows 1.3 s apart (0.4 s to draw).
fn archer_shoot(
    u: &mut Unit,
    dt: f32,
    ctx: &Ctx,
    with_knight: bool,
    rng: &mut Rng,
    events: &mut impl FnMut(UnitEvent),
) {
    /// orig: Archer prefab `shootRange`, `shootPrepTime`, `shootIntervalTime`, `shootCooldownTime`.
    const RANGE: f32 = 5.5;
    const PREP: f32 = 0.4;
    const INTERVAL: f32 = 1.3;
    const COOLDOWN: f32 = 0.3;
    /// orig: `maxError`, `dropInWaterProbability`.
    const MAX_ERROR: f32 = 0.2;
    const WATER: f32 = 0.1;
    u.shoot_cooldown -= dt;
    let Some((target, left, t, aiming)) = u.shoot else {
        if u.shoot_cooldown > 0.0 || matches!(u.ai, Ai::Flee { .. }) {
            return;
        }
        // orig: ShouldShootEnemy: the most hit points among those in range.
        // Portals are on the Enemies layer too, with far more hit points.
        let trolls = ctx
            .enemies
            .iter()
            .filter(|e| (e.x - u.x).abs() <= RANGE && target_in_range(e.x - u.x, e.y - u.y))
            .map(|e| (Quarry::Troll(e.id), e.hp));
        let portals = ctx
            .portals
            .iter()
            .filter(|p| {
                p.alive
                    && p.by_arrows
                    && (p.x - u.x).abs() <= RANGE
                    && target_in_range(p.x - u.x, -u.y)
            })
            .map(|p| (Quarry::Portal(p.id), p.hp));
        let enemy = trolls
            .chain(portals)
            .fold(None::<(Quarry, i32)>, |best, c| match best {
                Some(b) if b.1 >= c.1 => Some(b),
                _ => Some(c),
            })
            .map(|b| b.0);
        // orig: ShouldShootWildlife: the closest; rabbits only by day.
        let prey = || {
            let a = ctx
                .wildlife
                .iter()
                .filter(|a| (a.1 - u.x).abs() <= RANGE)
                .min_by(|a, b| (a.1 - u.x).abs().total_cmp(&(b.1 - u.x).abs()))?;
            if !((ctx.daytime && u.slot.is_none()) || a.3) {
                return None;
            }
            Some(a)
        };
        let quarry = enemy.or_else(|| {
            // orig: ShouldShootWildlife: not while following a knight.
            if with_knight {
                return None;
            }
            let a = prey()?;
            u.hunting = Some(a.0);
            target_in_range(a.1 - u.x, -u.y).then_some(Quarry::Animal(a.0))
        });
        if let Some(q) = quarry {
            u.shoot_cooldown = 5.0;
            u.mover.pause(5.0);
            // orig: Random.Range(minAttempts 2, maxAttempts 3) → always 2.
            u.shoot = Some((q, 2, 0.0, true));
            u.perfect_shot = ctx.archer_statue && rng.value() < PERFECT_PROBABILITY;
        }
        return;
    };
    let pos = match target {
        Quarry::Troll(id) => ctx.enemies.iter().find(|e| e.id == id).map(|e| {
            (
                e.bounds.x + e.bounds.w * 0.5,
                e.vx,
                e.bounds.y + e.bounds.h * 0.5,
            )
        }),
        Quarry::Animal(id) => ctx
            .wildlife
            .iter()
            .find(|a| a.0 == id)
            .map(|a| (a.1, a.2, a.4)),
        Quarry::Portal(id) => ctx
            .portals
            .iter()
            .find(|p| p.id == id && p.alive)
            .map(|p| ((p.x0 + p.x1) * 0.5, 0.0, p.cy)),
    };
    let Some((ex, evx, ey)) = pos else {
        u.shoot = None;
        u.mover.pause = 0.0;
        u.shoot_cooldown = COOLDOWN;
        return;
    };
    let t = t + dt;
    let dir: i8 = if ex > u.x { 1 } else { -1 };
    u.mover.facing = dir;
    // orig: a perfect shot draws in a third of the time.
    let prep = if u.perfect_shot { PREP / 3.0 } else { PREP };
    if aiming && t >= prep {
        // orig: Archer.BestShot from (x + 0.15·dir, 0.4) at the target's collider center.
        let (ox, oy) = (u.x + 0.15 * dir as f32, u.y + 0.4);
        let mut dx = ex - ox;
        let dy = ey - oy;
        if dx.abs() > 4.0 {
            dx += (0.5 * evx).min(0.75);
        }
        let blocked = ctx.walls.iter().any(|&w| (w - ox) * (w - ex) < 0.0);
        let (vx, vy) = launch(dx, dy, crate::greed::ARROW_SPEED, !blocked);
        // orig: perfect shots fly true; others scatter and some fall in the water.
        let (err, missed) = if u.perfect_shot {
            (1.0, false)
        } else {
            (
                rng.range(1.0 - MAX_ERROR, 1.0 + MAX_ERROR),
                rng.value() < WATER,
            )
        };
        events(UnitEvent::Arrow {
            x: ox,
            y: oy,
            vx: vx * err,
            vy: vy * err,
            missed,
            perfect: u.perfect_shot,
        });
        u.shoot = Some((target, left - 1, 0.0, false));
    } else if !aiming && t >= INTERVAL {
        if left == 0 {
            u.shoot = None;
            u.mover.pause = 0.0;
            // orig: shootCooldownWithKnightTime 1.3.
            u.shoot_cooldown = if with_knight { 1.3 } else { COOLDOWN };
        } else {
            u.shoot = Some((target, left, 0.0, true));
            u.perfect_shot = ctx.archer_statue && rng.value() < PERFECT_PROBABILITY;
        }
    } else {
        u.shoot = Some((target, left, t, aiming));
    }
}

/// orig: Archer prefab `perfectArrowProbability`.
const PERFECT_PROBABILITY: f32 = 0.99;

/// Launch velocity reaching (dx, dy) at `speed`: the low arc unless something
/// is in the way. orig: `Util.ComputeTrajectoryAngle()`.
fn launch(dx: f32, dy: f32, speed: f32, low: bool) -> (f32, f32) {
    let g = crate::coins::GRAVITY;
    let v2 = speed * speed;
    let disc = v2 * v2 - g * (g * dx * dx + 2.0 * dy * v2);
    if disc < 0.0 {
        let s = core::f32::consts::FRAC_1_SQRT_2 * speed;
        return (if dx > 0.0 { s } else { -s }, s);
    }
    let root = libm::sqrtf(disc);
    let (x, y) = (g * dx, if low { v2 - root } else { v2 + root });
    let len = libm::hypotf(x, y);
    (x / len * speed, y / len * speed)
}

/// orig: `Archer.GuessSideIfUnknown()`.
fn guard_side(u: &mut Unit, ctx: &Ctx, rng: &mut Rng) -> i8 {
    *u.guard_side.get_or_insert_with(|| {
        if (ctx.campfire - u.x).abs() < 10.0 {
            if rng.value() < 0.5 {
                1
            } else {
                -1
            }
        } else if u.x > ctx.campfire {
            1
        } else {
            -1
        }
    })
}

/// orig: `Archer.Hunt()`: chase the prey last aimed at if it's near the
/// forests, else roam around the border; then wait 4–10 s. Afterwards, go
/// for a coin an animal dropped (`ShouldGrabCoin`).
#[allow(clippy::too_many_arguments)]
fn archer_hunt(
    u: &mut Unit,
    phase: u8,
    t: f32,
    ctx: &Ctx,
    level: &Level,
    coins: &mut Coins,
    rng: &mut Rng,
) -> Ai {
    let tune = tuning(u.role);
    match phase {
        0 => {
            let side = guard_side(u, ctx, rng);
            let (fl, fr) = ctx.closest_forest;
            let prey = u
                .hunting
                .and_then(|id| ctx.wildlife.iter().find(|a| a.0 == id))
                .filter(|a| a.1 > fl - ARCHER_MAX_PURSUE && a.1 < fr + ARCHER_MAX_PURSUE);
            match prey {
                Some(a) => u.mover.set_goal(a.1, tune.run),
                None => {
                    u.hunting = None;
                    let r = ARCHER_HUNT_RANGE;
                    let goal = if side > 0 {
                        ctx.borders.right + rng.range(-0.5 * r, r)
                    } else {
                        ctx.borders.left + rng.range(-r, 0.5 * r)
                    };
                    u.mover
                        .set_goal(goal.clamp(level.left, level.left + level.width), tune.walk);
                }
            }
            Ai::Hunt { phase: 1, t: 0.0 }
        }
        1 if !u.mover.moving_to_goal => Ai::Hunt {
            phase: 2,
            t: -rng.range(4.0, 10.0),
        },
        2 if t >= 0.0 => {
            // orig: ShouldGrabCoin: the closest coin within range, if hunted.
            if u.coins < tune.max_coins {
                let x = u.x;
                let id = u.id;
                let closest = coins
                    .list
                    .iter_mut()
                    .filter(|c| c.is_free() && (c.x - x).abs() <= ARCHER_COIN_RANGE)
                    .min_by(|a, b| (a.x - x).abs().total_cmp(&(b.x - x).abs()));
                if let Some(c) = closest {
                    if c.dropper == Who::Wildlife && c.try_claim(Who::Unit(id), 5.0) {
                        u.mover.set_goal(c.x, tune.run);
                        return Ai::GrabCoin;
                    }
                }
            }
            Ai::Stand
        }
        p => Ai::Hunt { phase: p, t },
    }
}

/// orig: `Archer.GoToTower()` / `EnterGuardSlot()` / `ExitGuardSlot()`.
/// Returns true while the archer stands in the slot.
fn archer_tower(u: &mut Unit, ctx: &Ctx, rng: &mut Rng) -> bool {
    let Some(slot) = u.slot.and_then(|s| ctx.slots.iter().find(|i| i.id == s)) else {
        // The tower is gone or was upgraded.
        exit_slot(u, rng);
        return false;
    };
    if u.in_slot {
        u.x = slot.x;
        u.y = slot.y;
        u.mover.vx = 0.0;
        return true;
    }
    // orig: ShouldGoToTower interrupts everything; run there, then step in.
    u.ai = Ai::Stand;
    u.mover.set_goal(slot.x, tuning(u.role).run);
    if !u.mover.moving_to_goal {
        u.in_slot = true;
        u.mover.stop();
        u.mover.facing = if u.x > ctx.campfire { 1 } else { -1 };
        u.x = slot.x;
        u.y = slot.y;
        if !u.soldier {
            u.soldier = true;
            u.outfit = if rng.value() < 0.5 {
                ctx.coat.primary
            } else {
                ctx.coat.secondary
            };
        }
        u.taxes = None;
        return true;
    }
    false
}

/// orig: `Archer.ExitGuardSlot()` → `ConvertToHunter()`.
pub(crate) fn exit_slot(u: &mut Unit, rng: &mut Rng) {
    u.slot = None;
    u.in_slot = false;
    u.y = 0.0;
    if u.soldier {
        u.soldier = false;
        u.outfit = random_outfit(u.role, rng);
    }
    u.ai = Ai::Stand;
}

/// orig: `Archer.Flee()`: run for the campfire for 2 s (uninterruptible).
fn archer_should_flee(u: &Unit, ctx: &Ctx) -> bool {
    !ctx.borders.contains(u.x)
        && ctx
            .enemies
            .iter()
            .map(|e| (e.x - u.x).abs())
            .min_by(f32::total_cmp)
            .is_some_and(|d| d < ARCHER_FLEE_RANGE)
}

/// orig: `Archer.GoToWall()`: stand 1–2 units inside the outer wall.
fn archer_wall(u: &mut Unit, phase: u8, t: f32, ctx: &Ctx, rng: &mut Rng) -> Ai {
    let run = tuning(u.role).run;
    match phase {
        0 => {
            u.mover.stop();
            Ai::GoToWall {
                phase: 1,
                t: -rng.range(0.2, 1.5),
            }
        }
        1 if t >= 0.0 => {
            let side = guard_side(u, ctx, rng);
            let d = rng.range(ARCHER_WALL_DISTANCE.0, ARCHER_WALL_DISTANCE.1);
            u.mover
                .set_goal(ctx.shelter.side(side) - side as f32 * d, run);
            Ai::GoToWall { phase: 2, t: 0.0 }
        }
        2 if !u.mover.moving_to_goal => Ai::GoToWall {
            phase: 3,
            t: -rng.range(2.0, 8.0),
        },
        3 if t >= 0.0 => Ai::GoToWall { phase: 0, t: 0.0 },
        p => Ai::GoToWall { phase: p, t },
    }
}

#[cfg(test)]
pub(crate) mod tests_support {
    pub fn launch(dx: f32, dy: f32, speed: f32, low: bool) -> (f32, f32) {
        super::launch(dx, dy, speed, low)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn combat_world() -> WorldData {
        WorldData {
            levels: alloc::vec![crate::data::LevelConfigDef {
                name: "archer-fixture".into(),
                autumn_start: (40, 40),
                winter_start: (60, 60),
                days_before_game_over: -1,
                starting_coins: 0,
                min_level_width: 0,
                group_counts: Vec::new(),
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
        }
    }

    fn unit_ctx<'a>(world: &'a WorldData, enemies: &'a [EnemyTarget]) -> Ctx<'a> {
        Ctx {
            world,
            borders: Borders {
                left: -10.0,
                right: 10.0,
            },
            shelter: Borders {
                left: -10.0,
                right: 10.0,
            },
            campfire: 0.0,
            daytime: true,
            safe: true,
            monarch_x: 0.0,
            monarch_running: false,
            forests: &[],
            enemies,
            walls: &[],
            farmlands: &[],
            slots: &[],
            archer_statue: false,
            knight_statue: false,
            worker_statue: false,
            portals: &[],
            knights: KnightCtx::default(),
            coat: crate::kingdom::CoatOfArms {
                primary: [1.0; 3],
                secondary: [0.5; 3],
                emblem: [0.0; 3],
            },
            wildlife: &[],
            closest_forest: (-20.0, 20.0),
            boat: None,
        }
    }

    #[test]
    fn archer_range_uses_enemy_root_height_not_ground_or_collider_center() {
        use crate::greed::EnemyKind;
        let world = WorldData::default();
        let mut rng = Rng::new(31);
        let mut greed = crate::greed::Greed::default();
        greed.spawn_troll(EnemyKind::Troll, 2.0, 0, false, &mut rng);
        greed.spawn_troll(EnemyKind::Squid, 1.0, 0, false, &mut rng);
        // The healthier flying target is unreachable, despite its close x.
        greed.trolls[1].y = 3.0;
        let targets: Vec<_> = greed.trolls.iter().map(EnemyTarget::from).collect();
        let mut archer = Unit::new(10, Role::Archer, 0.0, &mut rng);
        archer_shoot(
            &mut archer,
            1.0 / 60.0,
            &unit_ctx(&world, &targets),
            false,
            &mut rng,
            &mut |_| {},
        );
        assert_eq!(archer.shoot.unwrap().0, Quarry::Troll(greed.trolls[0].id));

        // PC TargetInRange uses the root, even when the bounds centre would
        // fail its discriminant. BestShot has its own aim point/fallback.
        greed.trolls[1].y = 2.2;
        let targets: Vec<_> = greed.trolls.iter().map(EnemyTarget::from).collect();
        assert!(target_in_range(1.0, 2.2));
        assert!(!target_in_range(1.0, 2.5));
        let mut archer = Unit::new(11, Role::Archer, 0.0, &mut rng);
        archer_shoot(
            &mut archer,
            1.0 / 60.0,
            &unit_ctx(&world, &targets),
            false,
            &mut rng,
            &mut |_| {},
        );
        assert_eq!(archer.shoot.unwrap().0, Quarry::Troll(greed.trolls[1].id));
    }

    #[test]
    fn ground_and_tower_archers_hit_ground_and_airborne_colliders() {
        use crate::director::Season;
        use crate::greed::{EnemyKind, Greed, GreedCtx};
        // Exercise actual slot placement, acquisition, draw, arrow spawn and
        // collision. Reset target positions each tick to isolate aim geometry
        // from accumulated squid motion; a blessed shot removes scatter.
        for (height, kind, target_x, target_y) in [
            (0.0, EnemyKind::Troll, 3.5, 0.0),
            (0.0, EnemyKind::Squid, 2.0, 1.5),
            (2.0, EnemyKind::Squid, 3.5, 2.2),
        ] {
            let world = combat_world();
            let mut rng = Rng::new(73);
            let mut greed = Greed::default();
            greed.spawn_troll(kind, target_x, 0, false, &mut rng);
            greed.trolls[0].y = target_y;
            let hp = greed.trolls[0].hp;
            let target = EnemyTarget::from(&greed.trolls[0]);
            assert_eq!(target.y, target_y);
            assert_eq!(target.bounds, greed.trolls[0].box_at());
            let targets = [target];
            let slots = [SlotInfo {
                id: (40, 0),
                x: 0.0,
                y: height,
            }];
            let mut ctx = unit_ctx(&world, &targets);
            ctx.archer_statue = true;
            let mut population = Population::default();
            population.spawn(Role::Archer, 0.0, &mut rng);
            if height > 0.0 {
                population.units[0].slot = Some(slots[0].id);
                population.units[0].in_slot = true;
                ctx.slots = &slots;
            }
            let mut level = Level::empty(-20.0, 40.0);
            let mut coins = Coins::default();
            let mut shot = None;
            for _ in 0..60 {
                population.update(
                    1.0 / 60.0,
                    &ctx,
                    &mut level,
                    &mut coins,
                    &mut rng,
                    |event| {
                        if let UnitEvent::Arrow {
                            x,
                            y,
                            vx,
                            vy,
                            missed,
                            perfect,
                        } = event
                        {
                            shot = Some((x, y, vx, vy, missed, perfect));
                        }
                    },
                );
                if shot.is_some() {
                    break;
                }
            }
            let (x, y, vx, vy, missed, perfect) = shot.expect("archer must release its shot");
            assert!((y - (height + 0.4)).abs() < 1e-5);
            assert!(perfect && !missed);
            greed.shoot(x, y, vx, vy, missed, perfect);
            let greed_ctx = GreedCtx {
                obstacles: &[],
                solid_obstacles: &[],
                world: &world,
                level_index: 0,
                campfire: 0.0,
                borders: ctx.borders,
                monarch_x: 0.0,
                monarch_has_crown: true,
                targets: &[],
                citizens: &[],
                loot: &[],
                portals: &[],
                portal_infos: &[],
                threats: &[],
                season: Season::Summer,
                day: 1,
                boat: false,
            };
            let mut hit = false;
            for _ in 0..120 {
                greed.trolls[0].x = target_x;
                greed.trolls[0].y = target_y;
                greed.trolls[0].mover.vx = 0.0;
                greed.update(
                    1.0 / 60.0,
                    12.0,
                    &greed_ctx,
                    &level,
                    &mut [],
                    &mut rng,
                    |_| {},
                );
                if greed.trolls[0].hp < hp {
                    hit = true;
                    break;
                }
            }
            assert!(
                hit,
                "{kind:?} at ({target_x}, {target_y}) missed from height {height}"
            );
        }
    }

    #[test]
    fn recovered_tools_keep_identity_and_dropped_shields_keep_their_owner() {
        let world = WorldData::default();
        let mut rng = Rng::new(7);
        let ctx = unit_ctx(&world, &[]);
        let mut p = Population::default();
        p.shields
            .push(ShieldShop::new(20, 2.0, 0.0, ctx.coat, 0.0, &mut rng));
        p.release_stolen_tool(ToolKind::Shield, Some(8), Some(20), 3.0, 1.0);
        p.release_stolen_tool(ToolKind::Shield, Some(8), Some(20), 3.0, 1.0);
        assert_eq!(p.tools.len(), 1);
        assert_eq!(p.tools[0].id, 8);
        assert_eq!(p.tools[0].shop, Some(20));
        assert_eq!(p.tools[0].vy, 1.0);
        assert_eq!(p.shields[0].status, ShieldStatus::Available);
        assert_eq!(p.rack_count(20), 0);
        assert!(p.tool_id_allocated(8));
        p.update(
            0.1,
            &ctx,
            &mut Level::empty(-20.0, 40.0),
            &mut Coins::default(),
            &mut rng,
            |_| {},
        );
        assert!(
            p.tools[0].y < 1.0,
            "a dropped shield must fall despite retaining its owner"
        );
        assert_eq!(p.tools[0].shop, Some(20));
        p.release_stolen_tool(ToolKind::Bow, None, Some(20), 0.0, 0.5);
        assert_eq!(p.tools[1].id, 9);
        assert_eq!(p.tools[1].shop, None);
        assert!(!p.tool_id_allocated(10));
    }

    #[test]
    fn recovering_an_old_shield_does_not_overwrite_a_taken_banner() {
        let mut p = Population::default();
        let mut rng = Rng::new(8);
        let world = WorldData::default();
        let ctx = unit_ctx(&world, &[]);
        let mut shop = ShieldShop::new(20, 2.0, 0.0, ctx.coat, 0.0, &mut rng);
        shop.status = ShieldStatus::Taken;
        p.shields.push(shop);
        p.release_stolen_tool(ToolKind::Shield, Some(3), Some(20), 1.0, 0.5);
        assert_eq!(p.shields[0].status, ShieldStatus::Taken);
    }

    #[test]
    fn hammer_impact_sound_uses_clip_marker_without_finishing_work_early() {
        use crate::animation::{Clip, Event, EventKind};
        let mut world = WorldData::default();
        world.protos.push(crate::data::Proto {
            building: Some(crate::data::BuildingDef {
                build_points: 20,
                auto_build_rate: 0.0,
                scaffold: vec![],
            }),
            ..Default::default()
        });
        world.animations.clips.push((
            "worker_hammer".into(),
            Clip {
                length: 1.0,
                looping: true,
                events: vec![Event {
                    time: 0.41666666,
                    kind: EventKind::HammerHit,
                    float_parameter: 0.0,
                    int_parameter: 0,
                    string_parameter: Default::default(),
                }],
                ..Default::default()
            },
        ));
        let ctx = unit_ctx(&world, &[]);
        let mut level = Level::empty(-20.0, 40.0);
        let target = level.add(crate::level::instantiate(
            &world,
            0,
            crate::data::Layer::Game,
            0.0,
            0.0,
            0.0,
            1.0,
        ));
        level.objects[0].build = Some(0.0);
        let mut p = Population::default();
        let mut rng = Rng::new(73);
        p.spawn(Role::Worker, 0.0, &mut rng);
        p.units[0].ai = Ai::Work {
            target,
            working: true,
            t: 0.0,
        };
        p.units[0].anim = Anim::Hammer;
        p.units[0].works_before_moving = 10;
        let mut coins = Coins::default();
        let mut ev = Vec::new();
        for _ in 0..24 {
            p.update(1.0 / 60.0, &ctx, &mut level, &mut coins, &mut rng, |e| {
                ev.push(e)
            });
        }
        assert!(!ev.iter().any(|e| matches!(e, UnitEvent::WorkSound { .. })));
        assert_eq!(level.objects[0].build, Some(0.0));
        for _ in 0..3 {
            p.update(1.0 / 60.0, &ctx, &mut level, &mut coins, &mut rng, |e| {
                ev.push(e)
            });
        }
        assert_eq!(
            ev.iter()
                .filter(|e| matches!(e, UnitEvent::WorkSound { tree: false, .. }))
                .count(),
            1
        );
        assert_eq!(
            level.objects[0].build,
            Some(0.0),
            "hammer sound is not FinishWork"
        );
        for _ in 0..20 {
            p.update(1.0 / 60.0, &ctx, &mut level, &mut coins, &mut rng, |e| {
                ev.push(e)
            });
        }
        assert_eq!(
            ev.iter()
                .filter(|e| matches!(e, UnitEvent::BuildPoint(_)))
                .count(),
            1
        );
        assert!(level.objects[0].build.unwrap() > 0.0);
        assert_eq!(
            ev.iter()
                .filter(|e| matches!(e, UnitEvent::WorkSound { .. }))
                .count(),
            1
        );
        // Inert/stunned interrupts animation and cannot emit a pending hit.
        p.units[0].anim_t = 0.4;
        p.units[0].stunned = 1.0;
        ev.clear();
        p.update(0.1, &ctx, &mut level, &mut coins, &mut rng, |e| ev.push(e));
        assert!(!ev.iter().any(|e| matches!(e, UnitEvent::WorkSound { .. })));
    }

    #[test]
    fn free_archers_balance_after_recruitment_and_losses_without_taking_specialists() {
        let mut rng = Rng::new(42);
        let mut p = Population::default();
        p.spawn(Role::Archer, -100.0, &mut rng);
        p.units[0].follows = Some(99);
        p.spawn(Role::Archer, 100.0, &mut rng);
        p.units[1].slot = Some((99, 0));
        for n in 0..33 {
            p.distribute_free_archers();
            let free = &p.units[2..];
            let left = free.iter().filter(|u| u.guard_side == Some(-1)).count();
            let right = free.iter().filter(|u| u.guard_side == Some(1)).count();
            assert_eq!(left + right, n);
            assert!(left.abs_diff(right) <= 1);
            assert!(p.units[..2].iter().all(|u| u.guard_side.is_none()));
            p.spawn(Role::Archer, 20.0 - n as f32, &mut rng);
        }
        p.units.drain(2..12);
        p.distribute_free_archers();
        let free = &p.units[2..];
        let left = free.iter().filter(|u| u.guard_side == Some(-1)).count();
        assert!(left.abs_diff(free.len() - left) <= 1);
        p.fetch_archer_for_slot((88, 0), -100.0);
        assert_eq!(p.units[0].slot, None);
        assert_eq!(p.units[0].follows, Some(99));
    }

    #[test]
    fn flying_goal_resumes_on_pause_boundary_without_integrating_or_braking_body() {
        let mut m = Mover::default();
        m.set_goal(10., 1.3);
        m.vx = 2.;
        m.pause(0.125);
        assert_eq!(m.prepare_goal(0., 0.0625), None);
        assert_eq!(m.vx, 2.);
        assert_eq!(m.prepare_goal(0., 0.0625), Some(1.3));
        assert_eq!(
            m.vx, 2.,
            "flight integration applies drag; command preparation must not brake"
        );
        assert_eq!(m.prepare_goal(10., 0.01), Some(0.));
        assert!(!m.moving_to_goal);
        assert_eq!(m.vx, 2.);
    }

    #[test]
    fn mover_slows_near_goal_and_stops() {
        let mut m = Mover::default();
        let mut x = 0.0;
        m.set_goal(3.0, 1.4);
        for _ in 0..600 {
            m.update(&mut x, 1.0 / 60.0);
        }
        assert!((x - 3.0).abs() < 0.1, "{x}");
        assert!(!m.moving_to_goal);
    }

    #[test]
    fn hsb_matches_unity_primaries() {
        assert_eq!(hsb(0.0, 1.0, 1.0), [1.0, 0.0, 0.0]);
        let g = hsb(1.0 / 3.0, 1.0, 1.0);
        assert!((g[1] - 1.0).abs() < 1e-5 && g[0] < 1e-4);
    }
}
