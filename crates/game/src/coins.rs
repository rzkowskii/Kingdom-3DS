//! Wallets and coins in the world, picked up by the monarch and citizens.
//!
//! orig: `Wallet`, `Coin`, `Droppable`. The physics is a small stand-in for
//! Unity's Box2D using the original's materials and gravity; see
//! `docs/deviations.md`.

use alloc::vec::Vec;

use crate::monarch::Monarch;
use crate::payment::PayableId;
use crate::rng::Rng;

/// orig: `Wallet.maxCoins` (Player prefab).
pub const MAX_COINS: u32 = 1000;
/// orig: Physics2DSettings `m_Gravity.y` × coin `m_GravityScale` (1).
pub const GRAVITY: f32 = 15.0;
/// Box2D takes the larger restitution: coin material 0.5 vs ground default 0.
const BOUNCINESS: f32 = 0.5;
/// Below this impact speed Box2D doesn't bounce. orig: Physics2DSettings `m_VelocityThreshold`.
const BOUNCE_THRESHOLD: f32 = 1.0;
/// √(coin 0.4 × ground default 0.4) × gravity.
const GROUND_FRICTION_DECEL: f32 = 0.4 * GRAVITY;
/// Where coins leave the monarch, relative to their feet (x flips with facing).
/// orig: `Wallet.position` (Player prefab).
const WALLET_OFFSET: (f32, f32) = (0.125, 0.79);
/// orig: `Coin.moveSpeed` (Coin prefab).
const MOVE_SPEED: f32 = 5.0;
/// Fade time for coins spent on a purchase. orig: `SpriteFX.FadeOut(0.2f)` in `Player.UpdatePayState()`.
const SPENT_FADE: f32 = 0.2;
/// Fade time before an unclaimed coin disappears. orig: `Droppable.RunStateMachine()`.
const DESPAWN_FADE: f32 = 1.0;
/// Above this height no grabber's box reaches (they all stand on the ground),
/// so a flying coin can skip the pickup checks.
const COIN_REACH_Y: f32 = 1.8;

/// Pickup cooldowns from the Coin prefab.
mod cooldown {
    /// orig: `Droppable.nobodyCooldown`.
    pub const NOBODY: f32 = 0.7;
    /// orig: `Droppable.onlyReceiverCooldown`.
    pub const ONLY_CLAIMER: f32 = 5.0;
    /// orig: `Droppable.exceptDropperCooldown`.
    pub const EXCEPT_DROPPER: f32 = 5.0;
    /// Lifetime of a coin nobody picks up. orig: `Droppable.anybodyCooldown`.
    pub const ANYBODY: f32 = 120.0;
}

/// Monarch's coin-grab trigger, relative to their feet.
/// orig: Player prefab BoxCollider2D (trigger), offset (-0.05, 0.61), size (0.74, 1.14).
pub const MONARCH_BOX: Rect = Rect {
    x: -0.05 - 0.37,
    y: 0.61 - 0.57,
    w: 0.74,
    h: 1.14,
};
/// Coin trigger relative to its pivot. orig: Coin prefab, offset (0, 0.15625), size (0.25, 0.3125).
const COIN_BOX: Rect = Rect {
    x: -0.125,
    y: 0.0,
    w: 0.25,
    h: 0.3125,
};

/// Axis-aligned box relative to an owner's feet.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    /// A box centered on x with its bottom at `bottom`. orig: BoxCollider2D offset/size.
    pub const fn centered(offset_y: f32, w: f32, h: f32) -> Rect {
        Rect {
            x: -w / 2.0,
            y: offset_y - h / 2.0,
            w,
            h,
        }
    }

    pub fn at(self, x: f32, y: f32, flip: bool) -> Rect {
        let rx = if flip { -self.x - self.w } else { self.x };
        Rect {
            x: x + rx,
            y: y + self.y,
            ..self
        }
    }

    pub fn overlaps(&self, o: &Rect) -> bool {
        self.x < o.x + o.w && o.x < self.x + self.w && self.y < o.y + o.h && o.y < self.y + self.h
    }
}

/// orig: `Wallet`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Wallet {
    coins: u32,
    /// God mode: spending leaves the purse as it was (so the bag's pile
    /// stays at rest). Not saved.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub bottomless: bool,
}

impl Wallet {
    pub fn coins(&self) -> u32 {
        self.coins
    }

    /// orig: `Wallet.coins` setter clamps to `maxCoins`.
    pub fn set(&mut self, coins: u32) {
        self.coins = coins.min(MAX_COINS);
    }

    fn take(&mut self) -> bool {
        if self.bottomless && self.coins > 0 {
            true
        } else if self.coins > 0 {
            self.coins -= 1;
            true
        } else {
            false
        }
    }
}

/// Who dropped, claimed or picked up a coin.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Who {
    Monarch,
    Unit(u32),
    /// A tree, chest or other object; nobody's purse.
    Thing,
    /// A hunted animal. orig: `Coin.droppedByWildlife`.
    Wildlife,
    /// The castle's banker. orig: `Banker` (its `Wallet`).
    Banker,
}

/// Who may pick a coin up. orig: `Droppable.PickUpPolicy`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    Anybody,
    AnybodyExceptDropper,
    OnlyClaimer,
    Nobody,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Motion {
    /// Under physics. orig: `Droppable.fake == false`.
    Free,
    /// Flying along a straight line, physics off. orig: `Coin.MoveTo()`.
    Moving {
        from: (f32, f32),
        to: (f32, f32),
        t: f32,
        duration: f32,
    },
    /// Hovering above a payable as part of a payment.
    Floating { payable: PayableId },
    /// Being sucked into a purse, then removed.
    Grabbed {
        by: Who,
        from: (f32, f32),
        t: f32,
        duration: f32,
    },
    /// Fading out, then removed.
    Fading { t: f32, duration: f32 },
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct Coin {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub motion: Motion,
    /// Where a `Moving` coin ends up, as a payable slot.
    float_target: Option<PayableId>,
    pub policy: Policy,
    policy_timer: f32,
    /// The pickup state machine is running. Coins released from a cancelled
    /// payment never start it, so they never expire. orig: `Droppable._stateMachineRoutine`.
    state_machine: bool,
    /// orig: `Droppable.dropper`.
    pub dropper: Who,
    /// orig: `Droppable.friendlyClaimer`.
    pub claimer: Option<Who>,
    /// Time left on a `TryFriendlyClaim`, after which the claim lapses.
    claim_timeout: f32,
    nobody_cooldown: f32,
    inside_dropper: bool,
    pub alpha: f32,
    pub hit_ground: bool,
}

impl Coin {
    fn new(x: f32, y: f32, dropper: Who) -> Self {
        Self {
            x,
            y,
            vx: 0.0,
            vy: 0.0,
            motion: Motion::Free,
            float_target: None,
            policy: Policy::Anybody,
            policy_timer: 0.0,
            state_machine: false,
            dropper,
            claimer: None,
            claim_timeout: 0.0,
            nobody_cooldown: cooldown::NOBODY,
            inside_dropper: false,
            alpha: 1.0,
            hit_ground: false,
        }
    }

    /// orig: `Coin.droppedByPlayer`.
    pub fn dropped_by_monarch(&self) -> bool {
        self.dropper == Who::Monarch
    }

    pub fn is_free(&self) -> bool {
        self.motion == Motion::Free
    }

    /// orig: `Droppable.pickUpPolicy` setter: starts the cooldown state machine.
    fn set_policy(&mut self, policy: Policy) {
        self.policy = policy;
        self.policy_timer = 0.0;
        self.state_machine = true;
    }

    /// orig: `Droppable.CanBePickedUp(character)`.
    pub fn can_be_picked_up_by(&self, who: Who) -> bool {
        match self.policy {
            Policy::Anybody => true,
            Policy::AnybodyExceptDropper => who != self.dropper,
            Policy::OnlyClaimer => self.claimer == Some(who),
            Policy::Nobody => false,
        }
    }

    /// orig: `Droppable.TryFriendlyClaim()`: only one claimer at a time.
    pub fn try_claim(&mut self, who: Who, duration: f32) -> bool {
        if self.claimer.is_none() {
            self.claimer = Some(who);
            self.claim_timeout = duration;
            true
        } else {
            self.claimer == Some(who)
        }
    }

    /// orig: `Droppable.RunStateMachine()`. Returns true when the coin
    /// should start fading out for good.
    fn update_policy(&mut self, dt: f32) -> bool {
        if self.claim_timeout > 0.0 {
            self.claim_timeout -= dt;
            if self.claim_timeout <= 0.0 {
                self.claimer = None;
            }
        }
        if !self.state_machine {
            return false;
        }
        self.policy_timer += dt;
        let (limit, next) = match self.policy {
            Policy::AnybodyExceptDropper => (cooldown::EXCEPT_DROPPER, Policy::Anybody),
            Policy::OnlyClaimer => (cooldown::ONLY_CLAIMER, Policy::Anybody),
            Policy::Nobody => (
                self.nobody_cooldown,
                if self.claimer.is_some() {
                    Policy::OnlyClaimer
                } else {
                    Policy::Anybody
                },
            ),
            // Coin prefab has selfDestruct = 1.
            Policy::Anybody => {
                if self.policy_timer >= cooldown::ANYBODY {
                    self.state_machine = false;
                    return true;
                }
                return false;
            }
        };
        if self.policy_timer >= limit {
            self.policy = next;
            self.policy_timer = 0.0;
        }
        false
    }
}

/// Someone who picks coins up by touching them. orig: `Wallet` trigger.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grabber {
    pub who: Who,
    pub x: f32,
    pub y: f32,
    pub flip: bool,
    pub rect: Rect,
    /// Where grabbed coins fly to.
    pub purse: (f32, f32),
    /// orig: `Wallet.onlyGrabPlayerCoins`.
    pub only_monarch_coins: bool,
    /// Coins this grabber can still take (0 = none). orig: `canGrabCoins` / `maxCoins`.
    pub room: u32,
}

impl Grabber {
    pub fn monarch(m: &Monarch, wallet: &Wallet) -> Self {
        Grabber {
            who: Who::Monarch,
            x: m.x,
            y: 0.0,
            flip: m.facing < 0,
            rect: MONARCH_BOX,
            purse: monarch_purse(m),
            only_monarch_coins: false,
            room: MAX_COINS - wallet.coins,
        }
    }
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CoinEvent {
    /// Legacy serialized event; new collisions emit `HitGroundAt`.
    HitGround,
    /// orig: `Wallet.SuckCoin()` → `pickupSound`. The caller credits the wallet.
    PickedUp(Who),
    /// orig: `Coin.OnCollisionEnter2D()` plays at the coin's transform.
    HitGroundAt { x: f32 },
}

fn monarch_purse(m: &Monarch) -> (f32, f32) {
    (m.x + WALLET_OFFSET.0 * m.facing as f32, WALLET_OFFSET.1)
}

/// All coins in the world. orig: pooled `Coin` objects.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default)]
pub struct Coins {
    pub list: Vec<Coin>,
}

impl Coins {
    /// Drops a coin with an impulse. orig: `Wallet.DropCoin(force, policy, receiver, nobodyCooldown)`.
    #[allow(clippy::too_many_arguments)]
    pub fn drop(
        &mut self,
        from: Who,
        at: (f32, f32),
        velocity: (f32, f32),
        policy: Policy,
        receiver: Option<Who>,
        nobody_cooldown: Option<f32>,
    ) {
        let mut c = Coin::new(at.0, at.1, from);
        // orig: Droppable.Drop: AddForce(impulse) on a 1 kg body.
        (c.vx, c.vy) = velocity;
        c.claimer = receiver;
        if let Some(cd) = nobody_cooldown {
            c.nobody_cooldown = cd;
        }
        c.set_policy(policy);
        self.list.push(c);
    }

    /// Drops one coin from the monarch's purse. orig: `Wallet.DropCoin()`.
    pub fn drop_from(&mut self, m: &Monarch, wallet: &mut Wallet, rng: &mut Rng) {
        if !wallet.take() {
            return;
        }
        let v = (rng.range(-0.5, 0.5), 2.5);
        self.drop(
            Who::Monarch,
            monarch_purse(m),
            v,
            Policy::AnybodyExceptDropper,
            None,
            None,
        );
    }

    /// Sends one coin from the purse to a payable's price slot. orig: `Wallet.MoveCoin()`.
    pub fn send_to_payable(
        &mut self,
        m: &Monarch,
        wallet: &mut Wallet,
        id: PayableId,
        slot: (f32, f32),
    ) {
        if !wallet.take() {
            return;
        }
        let from = monarch_purse(m);
        let mut c = Coin::new(from.0, from.1, Who::Monarch);
        c.motion = move_to(from, slot);
        c.float_target = Some(id);
        self.list.push(c);
    }

    /// Floating coins fall back as real coins. orig: the `PayState.Cancelling` branch.
    pub fn release_floating(&mut self) {
        for c in &mut self.list {
            if c.float_target.is_some() {
                c.float_target = None;
                c.motion = Motion::Free;
                c.claimer = Some(Who::Monarch);
            }
        }
    }

    /// Floating coins were spent and fade away. orig: the `PayState.Completed` branch.
    pub fn spend_floating(&mut self) {
        for c in &mut self.list {
            if c.float_target.take().is_some() {
                c.motion = Motion::Fading {
                    t: 0.0,
                    duration: SPENT_FADE,
                };
                c.alpha = 0.5;
            }
        }
    }

    /// Steps all coins and lets grabbers take the ones they touch. Earlier
    /// grabbers win ties. Each `PickedUp` event means one coin for that wallet.
    pub fn update(&mut self, dt: f32, grabbers: &[Grabber], mut emit: impl FnMut(CoinEvent)) {
        let mut room: Vec<u32> = grabbers.iter().map(|g| g.room).collect();
        // `Who` → grabber index, for coins already being sucked in. A coin
        // each tick otherwise re-scans every grabber (coins × units is the
        // cost that spikes while a crowd is picking up a coin pile).
        let mut index: Vec<(Who, usize)> = grabbers
            .iter()
            .enumerate()
            .map(|(i, g)| (g.who, i))
            .collect();
        index.sort_unstable_by_key(|&(w, _)| w);
        for c in &mut self.list {
            match c.motion {
                Motion::Free => {
                    step_physics(c, dt, &mut emit);
                    if c.update_policy(dt) {
                        c.motion = Motion::Fading {
                            t: 0.0,
                            duration: DESPAWN_FADE,
                        };
                        continue;
                    }
                    // Nobody's box reaches this high (the ground folk's do).
                    if c.y > COIN_REACH_Y {
                        c.inside_dropper = false;
                        continue;
                    }
                    let me = COIN_BOX.at(c.x, c.y, false);
                    // One pass over the grabbers: leaving the dropper and
                    // the first taker, instead of a scan for each.
                    let mut inside = false;
                    let mut taker = None;
                    for (i, g) in grabbers.iter().enumerate() {
                        if !me.overlaps(&g.rect.at(g.x, g.y, g.flip)) {
                            continue;
                        }
                        if g.who == c.dropper {
                            inside = true;
                        }
                        if taker.is_none()
                            && room[i] > 0
                            && c.can_be_picked_up_by(g.who)
                            && (c.dropped_by_monarch() || !g.only_monarch_coins)
                        {
                            taker = Some(i);
                        }
                    }
                    // orig: Coin.OnTriggerExit2D: leaving the dropper frees the coin.
                    if c.inside_dropper && !inside && c.policy == Policy::AnybodyExceptDropper {
                        c.set_policy(Policy::Anybody);
                    }
                    c.inside_dropper = inside;
                    // orig: Wallet.OnTriggerStay2D → TryToGrabCoin → SuckCoin
                    if let Some(i) = taker {
                        let g = &grabbers[i];
                        room[i] -= 1;
                        let from = (c.x, c.y);
                        c.motion = Motion::Grabbed {
                            by: g.who,
                            from,
                            t: 0.0,
                            duration: dist(from, g.purse) / MOVE_SPEED,
                        };
                        emit(CoinEvent::PickedUp(g.who));
                    }
                }
                Motion::Moving {
                    from,
                    to,
                    t,
                    duration,
                } => {
                    let t = t + dt;
                    if t > duration {
                        (c.x, c.y) = to;
                        c.motion = match c.float_target {
                            Some(payable) => Motion::Floating { payable },
                            None => Motion::Free,
                        };
                    } else {
                        (c.x, c.y) = lerp(from, to, ease(t / duration));
                        c.motion = Motion::Moving {
                            from,
                            to,
                            t,
                            duration,
                        };
                    }
                }
                Motion::Floating { .. } => {}
                Motion::Grabbed {
                    by,
                    from,
                    t,
                    duration,
                } => {
                    // orig: Coin.Update with destroyAfter: follows the purse, shrinks/fades in the last quarter.
                    let t = t + dt;
                    let k = if duration > 0.0 {
                        (t / duration).min(1.0)
                    } else {
                        1.0
                    };
                    if let Ok(i) = index.binary_search_by(|&(w, _)| w.cmp(&by)) {
                        let g = &grabbers[index[i].1];
                        (c.x, c.y) = lerp(from, g.purse, ease(k));
                    }
                    c.alpha = 1.0 - 0.5 * ((k - 0.75) * 4.0).max(0.0);
                    c.motion = Motion::Grabbed {
                        by,
                        from,
                        t,
                        duration,
                    };
                }
                Motion::Fading { t, duration } => {
                    let t = t + dt;
                    c.alpha = (c.alpha - dt / duration).max(0.0);
                    c.motion = Motion::Fading { t, duration };
                }
            }
        }
        self.list.retain(|c| match c.motion {
            Motion::Grabbed { t, duration, .. } | Motion::Fading { t, duration } => t <= duration,
            _ => true,
        });
    }
}

fn move_to(from: (f32, f32), to: (f32, f32)) -> Motion {
    Motion::Moving {
        from,
        to,
        t: 0.0,
        duration: dist(from, to) / MOVE_SPEED,
    }
}

fn step_physics(c: &mut Coin, dt: f32, emit: &mut impl FnMut(CoinEvent)) {
    c.vy -= GRAVITY * dt;
    c.x += c.vx * dt;
    c.y += c.vy * dt;
    if c.y <= 0.0 {
        c.y = 0.0;
        if !c.hit_ground {
            c.hit_ground = true;
            emit(CoinEvent::HitGroundAt { x: c.x });
        }
        c.vy = if -c.vy > BOUNCE_THRESHOLD {
            -c.vy * BOUNCINESS
        } else {
            0.0
        };
        let step = GROUND_FRICTION_DECEL * dt;
        c.vx = if c.vx > 0.0 {
            (c.vx - step).max(0.0)
        } else {
            (c.vx + step).min(0.0)
        };
    }
}

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    libm::hypotf(b.0 - a.0, b.1 - a.1)
}

fn lerp(a: (f32, f32), b: (f32, f32), t: f32) -> (f32, f32) {
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
}

/// Stand-in for `Coin.moveCurve` until the extract tool exports the real curve.
fn ease(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monarch::SteedStats;

    const DT: f32 = 1.0 / 60.0;

    #[test]
    #[cfg(feature = "save")]
    fn old_coin_event_discriminants_remain_decodable() {
        assert_eq!(
            postcard::from_bytes::<CoinEvent>(&[0]).unwrap(),
            CoinEvent::HitGround
        );
        let bytes = postcard::to_allocvec(&CoinEvent::HitGroundAt { x: 7.0 }).unwrap();
        assert_eq!(bytes[0], 2, "new positioned variant is appended");
        assert_eq!(
            postcard::from_bytes::<CoinEvent>(&bytes).unwrap(),
            CoinEvent::HitGroundAt { x: 7.0 }
        );
    }

    #[test]
    fn first_ground_impact_retains_its_actual_position_after_coin_retirement() {
        let mut coins = Coins::default();
        coins.drop(
            Who::Unit(9),
            (25.0, 0.01),
            (3.0, -1.0),
            Policy::Nobody,
            None,
            None,
        );
        let mut events = Vec::new();
        coins.update(DT, &[], |event| events.push(event));
        let expected_x = 25.0 + 3.0 * DT;
        assert_eq!(events, [CoinEvent::HitGroundAt { x: expected_x }]);
        for _ in 0..180 {
            coins.update(DT, &[], |event| events.push(event));
        }
        coins.list.clear();
        assert_eq!(events, [CoinEvent::HitGroundAt { x: expected_x }]);
    }

    fn setup(coins: u32) -> (Coins, Monarch, Wallet, Rng) {
        let mut w = Wallet::default();
        w.set(coins);
        (
            Coins::default(),
            Monarch::new(0.0, SteedStats::HORSE_REGULAR),
            w,
            Rng::new(3),
        )
    }

    /// Runs with the monarch as the only grabber, crediting their wallet.
    fn run(c: &mut Coins, m: &Monarch, w: &mut Wallet, frames: u32) -> u32 {
        let mut total = 0;
        for _ in 0..frames {
            let g = [Grabber::monarch(m, w)];
            let mut picked = 0;
            c.update(DT, &g, |e| {
                if e == CoinEvent::PickedUp(Who::Monarch) {
                    picked += 1;
                }
            });
            w.set(w.coins() + picked);
            total += picked;
        }
        total
    }

    #[test]
    fn dropped_coin_lands_and_settles() {
        let (mut c, m, mut w, mut rng) = setup(5);
        c.drop_from(&m, &mut w, &mut rng);
        assert_eq!(w.coins(), 4);
        run(&mut c, &m, &mut w, 180);
        let coin = &c.list[0];
        assert_eq!(coin.y, 0.0);
        assert_eq!(coin.vy, 0.0);
        assert!(coin.hit_ground);
    }

    #[test]
    fn own_dropped_coin_is_not_regrabbed_while_standing_on_it() {
        let (mut c, m, mut w, mut rng) = setup(5);
        c.drop_from(&m, &mut w, &mut rng);
        assert_eq!(run(&mut c, &m, &mut w, 4 * 60), 0);
        assert_eq!(w.coins(), 4);
    }

    #[test]
    fn walking_away_and_back_picks_it_up() {
        let (mut c, mut m, mut w, mut rng) = setup(5);
        c.drop_from(&m, &mut w, &mut rng);
        run(&mut c, &m, &mut w, 60);
        m.x = 3.0;
        run(&mut c, &m, &mut w, 2);
        assert_eq!(c.list[0].policy, Policy::Anybody);
        m.x = c.list[0].x;
        assert_eq!(run(&mut c, &m, &mut w, 60), 1);
        assert_eq!(w.coins(), 5);
        assert!(c.list.is_empty());
    }

    #[test]
    fn someone_else_can_take_a_dropped_coin_at_once() {
        let (mut c, m, mut w, mut rng) = setup(5);
        c.drop_from(&m, &mut w, &mut rng);
        let beggar = Grabber {
            who: Who::Unit(7),
            x: m.x,
            y: 0.0,
            flip: false,
            rect: Rect::centered(0.35, 0.33, 0.7),
            purse: (m.x, 0.2),
            only_monarch_coins: true,
            room: 1,
        };
        let mut got = None;
        for _ in 0..60 {
            let g = [Grabber::monarch(&m, &w), beggar];
            c.update(DT, &g, |e| {
                if let CoinEvent::PickedUp(who) = e {
                    got = Some(who);
                }
            });
        }
        assert_eq!(got, Some(Who::Unit(7)));
    }

    #[test]
    fn unclaimed_coin_expires_after_cooldowns() {
        let (mut c, mut m, mut w, mut rng) = setup(1);
        c.drop_from(&m, &mut w, &mut rng);
        m.x = 100.0;
        run(&mut c, &m, &mut w, ((5.0 + 120.0 + 1.0) * 60.0) as u32 + 10);
        assert!(c.list.is_empty());
    }

    #[test]
    fn payment_coins_float_then_release_back_into_purse() {
        let (mut c, m, mut w, _) = setup(3);
        c.send_to_payable(&m, &mut w, PayableId(1), (0.0, 2.8));
        c.send_to_payable(&m, &mut w, PayableId(1), (0.4, 2.8));
        assert_eq!(w.coins(), 1);
        run(&mut c, &m, &mut w, 60);
        assert!(c
            .list
            .iter()
            .all(|k| matches!(k.motion, Motion::Floating { .. })));
        c.release_floating();
        // They fall straight back into the monarch's grab box.
        run(&mut c, &m, &mut w, 60);
        assert_eq!(w.coins(), 3);
    }

    #[test]
    fn spent_coins_fade_out() {
        let (mut c, m, mut w, _) = setup(1);
        c.send_to_payable(&m, &mut w, PayableId(1), (0.0, 2.8));
        run(&mut c, &m, &mut w, 60);
        c.spend_floating();
        run(&mut c, &m, &mut w, 20);
        assert!(c.list.is_empty());
        assert_eq!(w.coins(), 0);
    }

    #[test]
    fn wallet_caps_at_1000() {
        let mut w = Wallet::default();
        w.set(5000);
        assert_eq!(w.coins(), 1000);
    }
}
