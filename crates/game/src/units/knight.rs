//! Knights, their shield shops and the archers who follow them.
//!
//! orig: `Knight`, `PayableShield`, `DroppableShield`,
//! `Kingdom.FetchArchersForJob()` (knights), `Archer.FollowKnight()`.

use alloc::vec::Vec;

use super::{tuning, Ai, Ctx, Unit, UnitEvent};
use crate::coins::{Coins, Who};
use crate::data::Role;
use crate::rng::Rng;

/// orig: Knight prefab.
pub(super) const WALK: f32 = 0.6;
pub(super) const RUN: f32 = 1.4;
const ATTACK_DAMAGE: i32 = 6;
const SLASH_RANGE: f32 = 0.4;
const SLASH_COOLDOWN: f32 = 1.2;
/// orig: the enemy scanner's range (`powerSlashRange`) and `rangeBehind`.
const SCAN_FRONT: f32 = 2.5;
const SCAN_BEHIND: f32 = 0.5;
/// orig: `Slash()` keeps its hit box up for 4/15 s.
const SLASH_TIME: f32 = 4.0 / 15.0;
/// orig: `hitBox` (x −0.1, width 0.6) in front of the knight.
const HIT_BOX: (f32, f32) = (-0.1, 0.5);
const COIN_SCAN: f32 = 7.0;
const DISTANCE_FROM_WALL: f32 = 1.3;
const DISTANCE_FROM_PORTAL: (f32, f32) = (2.5, 4.0);
pub(super) const MAX_ARCHERS: usize = 4;
/// orig: `Kingdom.FetchArchersForJob(knight, n, 10)` every 10 s.
const ARCHER_FETCH_RANGE: f32 = 10.0;
const ARCHER_FETCH_INTERVAL: f32 = 10.0;
/// orig: Archer prefab `distanceFromKnight`.
pub(super) const ARCHER_DISTANCE: (f32, f32) = (0.3, 1.5);
/// orig: Knight prefab `powerSlashCooldown`, `powerSlashJumpForce`; the hit box is up 0.7 s.
const POWER_SLASH_COOLDOWN: f32 = 3.0;
const POWER_SLASH_JUMP: (f32, f32) = (7.0, 3.0);
const POWER_SLASH_TIME: f32 = 0.7;
/// Marks portal ids among the trolls a slash already hit.
const PORTAL_TAG: u32 = 1 << 31;
/// orig: Troll trigger half width.
const TROLL_HALF_WIDTH: f32 = 0.15;

/// A knight's own state. orig: `Knight` fields.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct KnightData {
    /// Which wall it defends. orig: `side`.
    pub side: i8,
    /// Its place in the line on that side. orig: `_rank`.
    pub rank: u8,
    /// The shield shop it came from. orig: `_shieldShop`.
    pub shop: Option<u32>,
    should_charge: bool,
    /// orig: `_chargingWithoutArchers`.
    alone: bool,
    /// Its archers wait while it walks out past its post. orig: `holdFire`.
    pub hold_fire: bool,
    cooldown: f32,
    /// A slash in progress: (time left, trolls already hit).
    slash: Option<(f32, Vec<u32>)>,
    fetch_t: f32,
    /// Upward speed during a power slash leap.
    vy: f32,
    /// A power slash just started: its archers should hold. Taken by the population.
    pub hold_archers: bool,
}

impl KnightData {
    pub fn new(side: i8, rank: u8, shop: Option<u32>) -> Self {
        KnightData {
            side,
            rank,
            shop,
            should_charge: false,
            alone: false,
            hold_fire: false,
            cooldown: 0.0,
            slash: None,
            fetch_t: 0.0,
            vy: 0.0,
            hold_archers: false,
        }
    }

    pub fn is_slashing(&self) -> bool {
        self.slash.is_some()
    }

    /// orig: `Knight.DoCharge()` (from the border banner).
    pub fn charge(&mut self, archers: usize) {
        self.should_charge = true;
        self.alone = archers == 0;
    }

    pub fn charging(ai: &Ai) -> bool {
        matches!(ai, Ai::Charge { .. } | Ai::Board { .. })
    }
}

/// orig: `PayableShield.Status`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShieldStatus {
    Empty,
    /// A shield waits on the rack.
    Available,
    /// A knight carries it; its banner flies.
    Taken,
    /// Its knight fell; the banner is torn.
    Broken,
}

/// orig: `PayableShield`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShieldShop {
    pub object: u32,
    pub status: ShieldStatus,
    /// Seconds until the castle keep is done. orig: `castleConstructing`.
    pub constructing: f32,
    /// orig: its banner's `recolorA` / `recolorB`.
    pub colors: ([f32; 3], [f32; 3]),
    pub side: i8,
}

/// orig: `Color.grayscale`.
fn grayscale(c: [f32; 3]) -> f32 {
    0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]
}

impl ShieldShop {
    /// orig: `PayableShield.Start()` → `Banner.AssignColorAndPattern()`.
    pub fn new(
        object: u32,
        x: f32,
        campfire: f32,
        coat: crate::kingdom::CoatOfArms,
        constructing: f32,
        rng: &mut Rng,
    ) -> Self {
        let side: i8 = if x < campfire { -1 } else { 1 };
        let solid = (grayscale(coat.primary) < 0.5) ^ (side < 0);
        let inverted = rng.value() < 0.5;
        let (a, b) = if inverted {
            (coat.secondary, coat.primary)
        } else {
            (coat.primary, coat.secondary)
        };
        ShieldShop {
            object,
            status: ShieldStatus::Empty,
            constructing,
            colors: (a, if solid { a } else { b }),
            side,
        }
    }

    /// orig: `PayableShield.CanPay()`.
    pub fn can_pay(&self) -> bool {
        self.constructing <= 0.0
            && matches!(self.status, ShieldStatus::Empty | ShieldStatus::Broken)
    }
}

/// What knights need to know about the kingdom this tick.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct KnightCtx {
    /// The outermost intact wall on each side (left, right). orig: `Kingdom.intactWall`.
    pub intact_wall: [Option<f32>; 2],
    /// orig: `Kingdom.borderBanner[side]`.
    pub banner: [Option<f32>; 2],
}

fn side_index(side: i8) -> usize {
    usize::from(side > 0)
}

/// The knight's FSM. orig: `Knight` (states and transitions from `Awake`):
/// Charge interrupts anything; from Stand: GrabCoin (interrupting), then
/// Assemble by safe day, GoToWall otherwise; Assemble loops and is
/// interrupted by GrabCoin and GoToWall.
#[allow(clippy::too_many_arguments)]
pub(super) fn knight_ai(
    u: &mut Unit,
    dt: f32,
    check: bool,
    ctx: &Ctx,
    followers: usize,
    coins: &mut Coins,
    rng: &mut Rng,
    events: &mut impl FnMut(UnitEvent),
) {
    let Some(k) = u.knight.as_mut() else { return };
    k.cooldown -= dt;
    k.fetch_t -= dt;
    let should_wall = !ctx.daytime || !ctx.safe;
    let should_assemble = ctx.daytime && ctx.safe;

    // orig: ShouldCharge (any state, interrupts).
    if k.should_charge && !matches!(u.ai, Ai::Charge { .. }) {
        k.should_charge = false;
        k.hold_fire = false;
        let x = u.x;
        // orig: Kingdom.ClosestSpawnerOnSameSideOfCampfire.
        let portal = ctx
            .portals
            .iter()
            .filter(|p| (p.x < ctx.campfire) == (x < ctx.campfire))
            .min_by(|a, b| (a.x - x).abs().total_cmp(&(b.x - x).abs()));
        let portal = portal.filter(|p| p.alive);
        let boat = ctx
            .boat
            .filter(|b| b.dir == k.side && b.knight_room && b.mode != crate::boat::mode::SAILING);
        u.ai = match portal.map(|p| (p.id, p.x)) {
            None if boat.is_some() => Ai::Board {
                waiting: false,
                t: 0.0,
            },
            Some((id, px)) => {
                let dir = if px > x { 1.0 } else { -1.0 };
                let off = if k.alone {
                    0.75
                } else {
                    rng.range(DISTANCE_FROM_PORTAL.0, DISTANCE_FROM_PORTAL.1)
                };
                Ai::Charge {
                    portal: id,
                    target: px - dir * off,
                    t: 0.0,
                }
            }
            None => Ai::Stand,
        };
    }

    let wants_coin = |u: &Unit, coins: &Coins| -> Option<usize> {
        if !ctx.daytime || u.coins >= tuning(Role::Knight).max_coins {
            return None;
        }
        coins
            .list
            .iter()
            .position(|c| c.is_free() && (c.x - u.x).abs() <= COIN_SCAN && c.claimer.is_none())
    };

    match u.ai {
        Ai::Stand => {
            if let Some(ci) = wants_coin(u, coins) {
                coins.list[ci].try_claim(Who::Unit(u.id), 20.0);
                u.mover.set_goal(coins.list[ci].x, RUN);
                u.ai = Ai::GrabCoin;
            } else if should_assemble {
                u.ai = assemble(u, ctx, rng);
            } else if should_wall {
                u.ai = go_to_wall(u, ctx);
            }
        }
        Ai::Assemble { t } => {
            if check {
                if let Some(ci) = wants_coin(u, coins) {
                    coins.list[ci].try_claim(Who::Unit(u.id), 20.0);
                    u.mover.set_goal(coins.list[ci].x, RUN);
                    u.ai = Ai::GrabCoin;
                    return slash(u, dt, ctx, events);
                }
                if should_wall {
                    u.ai = go_to_wall(u, ctx);
                    return slash(u, dt, ctx, events);
                }
            }
            if u.mover.moving_to_goal {
                u.ai = Ai::Assemble { t };
            } else {
                let t = t + dt;
                // orig: wait 10 s, then Assemble again (nextState).
                u.ai = if t >= 10.0 {
                    assemble(u, ctx, rng)
                } else {
                    Ai::Assemble { t }
                };
            }
        }
        Ai::KnightWall => {
            if !u.mover.moving_to_goal {
                let dir = if ctx.campfire < u.x { 1 } else { -1 };
                u.mover.set_speed(0.0, dir);
                if let Some(k) = u.knight.as_mut() {
                    k.hold_fire = false;
                }
                u.ai = Ai::Stand;
            }
        }
        Ai::Charge { portal, target, t } => {
            let alive = ctx.portals.iter().any(|p| p.id == portal && p.alive);
            let alone = u.knight.as_ref().is_some_and(|k| k.alone);
            if !alive || (followers == 0 && !alone) {
                u.ai = Ai::Stand;
            } else {
                // orig: re-aim every 2 s, facing the portal.
                let t = t - dt;
                if t <= 0.0 {
                    u.mover.set_goal(target, RUN);
                    let px = ctx
                        .portals
                        .iter()
                        .find(|p| p.id == portal)
                        .map_or(target, |p| p.x);
                    if !u.mover.moving_to_goal {
                        u.mover.facing = if px > u.x { 1 } else { -1 };
                    }
                    u.ai = Ai::Charge {
                        portal,
                        target,
                        t: 2.0,
                    };
                } else {
                    u.ai = Ai::Charge { portal, target, t };
                }
            }
        }
        Ai::Board { waiting, t } => {
            use crate::boat::mode;
            u.ai = match ctx.boat {
                // orig: while it's pushed, run 2–4 ahead of it every 5 s.
                Some(b) if b.mode == mode::PUSHING => {
                    let t = t - dt;
                    if t <= 0.0 {
                        u.mover
                            .set_goal(b.x + rng.range(2.0, 4.0) * b.dir as f32, RUN);
                        Ai::Board {
                            waiting: false,
                            t: 5.0,
                        }
                    } else {
                        Ai::Board { waiting, t }
                    }
                }
                Some(b) if b.mode == mode::WAITING && b.knight_room => {
                    if !waiting {
                        u.mover.set_goal(b.x + rng.range(-0.5, 0.5), RUN);
                        Ai::Board {
                            waiting: true,
                            t: 0.0,
                        }
                    } else if !u.mover.moving_to_goal {
                        events(UnitEvent::Embark(u.id));
                        Ai::Stand
                    } else {
                        Ai::Board { waiting, t }
                    }
                }
                _ => Ai::Stand,
            };
        }
        Ai::GrabCoin => {
            if !u.mover.moving_to_goal {
                u.ai = Ai::Stand;
            }
        }
        _ => u.ai = Ai::Stand,
    }
    slash(u, dt, ctx, events);
}

/// orig: `Knight.Assemble()`: by the border banner (or 4 past the border).
fn assemble(u: &mut Unit, ctx: &Ctx, rng: &mut Rng) -> Ai {
    let Some(k) = u.knight.as_mut() else {
        return Ai::Stand;
    };
    k.hold_fire = false;
    let side = k.side;
    let pos = match ctx.knights.banner[side_index(side)] {
        Some(b) => b + side as f32 * 3.0,
        None => ctx.borders.side(side) + side as f32 * 4.0,
    };
    u.mover.set_goal(pos + rng.range(-1.0, 1.0), WALK);
    Ai::Assemble { t: 0.0 }
}

/// orig: `Knight.GoToWall()`: just inside the outermost intact wall, by rank.
fn go_to_wall(u: &mut Unit, ctx: &Ctx) -> Ai {
    let Some(k) = u.knight.as_mut() else {
        return Ai::Stand;
    };
    let side = k.side;
    let wall = ctx.knights.intact_wall[side_index(side)].unwrap_or(ctx.borders.side(side));
    let target = wall - side as f32 * (DISTANCE_FROM_WALL + k.rank as f32 * 0.25);
    if (side > 0 && u.x > target) || (side < 0 && u.x < target) {
        k.hold_fire = true;
    }
    u.mover.set_goal(target, RUN);
    Ai::KnightWall
}

/// orig: `Knight.ShouldSlash()` / `Slash()`: greed in reach gets cut.
/// With the knight statue: `ShouldPowerSlash()` / `PowerSlash()`, a leap
/// at anything in scanning range.
fn slash(u: &mut Unit, dt: f32, ctx: &Ctx, events: &mut impl FnMut(UnitEvent)) {
    let x = u.x;
    let facing = u.mover.facing as f32;
    let at_wall = u.ai == Ai::KnightWall;
    let Some(k) = u.knight.as_mut() else { return };
    // The leap. orig: Mover.Jump(powerSlashJumpForce) under gravity.
    if u.y > 0.0 || k.vy > 0.0 {
        k.vy -= crate::coins::GRAVITY * dt;
        u.y = (u.y + k.vy * dt).max(0.0);
        if u.y <= 0.0 {
            k.vy = 0.0;
        }
    }
    if let Some((t, hits)) = &mut k.slash {
        *t -= dt;
        let (a, b) = (x + HIT_BOX.0 * facing, x + HIT_BOX.1 * facing);
        let (lo, hi) = (a.min(b), a.max(b));
        for e in ctx.enemies {
            if e.x + TROLL_HALF_WIDTH > lo && e.x - TROLL_HALF_WIDTH < hi && !hits.contains(&e.id) {
                hits.push(e.id);
                events(UnitEvent::Strike {
                    troll: e.id,
                    damage: ATTACK_DAMAGE,
                    from_x: x,
                });
            }
        }
        // Portals share the Enemies layer. Their ids are tagged so they don't clash with trolls'.
        for p in ctx.portals {
            let tag = p.id | PORTAL_TAG;
            if p.alive && p.by_knights && p.x1 > lo && p.x0 < hi && !hits.contains(&tag) {
                hits.push(tag);
                events(UnitEvent::StrikePortal {
                    portal: p.id,
                    damage: ATTACK_DAMAGE,
                });
            }
        }
        if *t <= 0.0 {
            k.slash = None;
        }
        return;
    }
    if k.cooldown > 0.0 {
        return;
    }
    // orig: the scanner sees 2.5 ahead and 0.5 behind; strike within 0.4
    // of the closest one's position.
    let in_scan = |ex: f32| {
        let d = (ex - x) * facing;
        d < SCAN_FRONT && d > -SCAN_BEHIND
    };
    let closest = ctx
        .enemies
        .iter()
        .map(|e| e.x)
        .chain(ctx.portals.iter().filter(|p| p.alive).map(|p| p.x))
        .filter(|&ex| in_scan(ex))
        .min_by(|a, b| (a - x).abs().total_cmp(&(b - x).abs()));
    if let Some(ex) = closest {
        if ctx.knight_statue && !at_wall {
            let dir = if ex < x { -1 } else { 1 };
            k.cooldown = POWER_SLASH_COOLDOWN;
            k.slash = Some((POWER_SLASH_TIME, Vec::new()));
            k.vy = POWER_SLASH_JUMP.1;
            k.hold_archers = true;
            u.mover.pause(1.5);
            u.mover.facing = dir;
            u.mover.vx = POWER_SLASH_JUMP.0 * dir as f32;
            u.anim_t = 0.0;
        } else if (ex - x).abs() < SLASH_RANGE {
            k.cooldown = SLASH_COOLDOWN;
            k.slash = Some((SLASH_TIME, Vec::new()));
            u.mover.pause(0.5);
            u.mover.facing = if ex < x { -1 } else { 1 };
            u.anim_t = 0.0;
        }
    }
}

/// How often a knight looks for archers. orig: `Knight.Update()`.
pub(super) fn wants_archers(u: &mut Unit, followers: usize) -> Option<usize> {
    let k = u.knight.as_mut()?;
    let n = MAX_ARCHERS.saturating_sub(followers);
    if k.fetch_t <= 0.0 && n > 0 {
        k.fetch_t = ARCHER_FETCH_INTERVAL;
        return Some(n);
    }
    None
}

pub(super) const FETCH_RANGE: f32 = ARCHER_FETCH_RANGE;
