//! Coins: dropping, picking up, the purse's limit, paying, cancelling, and
//! many coins at once. Every one runs with the coin ledger closed: no
//! scenario here has income, so no coin may appear or vanish.

use game::coins::{Policy, Who};
use game::data::Role;

use super::island;
use crate::scenario::{no_metrics, Area, Class, Scenario};
use crate::script::Script;
use crate::sim::Sim;
use crate::worlds::{self, proto};

pub fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "coin_drop",
            area: Area::Economy,
            class: Class::Internal,
            summary: "Three bag taps drop three coins; the monarch can't take them straight back",
            seed: 1,
            setup: |seed| {
                let mut s = island(seed, 5);
                s.closed_economy();
                s
            },
            script: || {
                Script::new()
                    .idle(1.0)
                    .tap(6)
                    .idle(0.5)
                    .tap(6)
                    .idle(0.5)
                    .tap(6)
                    .idle(3.0)
            },
            checkpoint_every: 60,
            check: |s, c| {
                c.eq("wallet", s.game.wallet.coins(), 2);
                c.eq("coins on the ground", s.game.coins.list.len(), 3);
                c.eq("drop events", s.log.count("Pay.DropCoin"), 3);
                c.eq("pickups", s.log.count("Coin.PickedUp"), 0);
            },
            metrics: no_metrics,
            known_issue: None,
        },
        Scenario {
            name: "purse_overflow",
            area: Area::Economy,
            class: Class::Internal,
            summary: "An overfull purse spills coins over the rim until the pile fits (about 55), half of them into the water",
            seed: 1,
            setup: |seed| {
                let mut s = island(seed, 90);
                s.closed_economy();
                s
            },
            script: || Script::new().idle(40.0),
            checkpoint_every: 300,
            check: |s, c| {
                let spilled = s.log.count("Bag.Spilled");
                let picked = s.log.count("Coin.PickedUp");
                let wallet = s.game.wallet.coins();
                // Spills that land on dry ground can be picked up again.
                c.eq("purse = start - spills + pickups", wallet + spilled, 90 + picked);
                c.range("purse after spilling (docs: about 55)", wallet, 45, 65);
                // Settled: nothing spilled in the last 10 s.
                let late = s.checkpoints.iter().rev().nth(2).map(|cp| cp.events.get("Bag.Spilled").copied().unwrap_or(0));
                c.eq("spills in the last 10 s", late.map(|l| spilled - l), Some(0));
                let l = s.ledger.as_ref().expect("closed economy");
                // Half go in the water (overflowCoinsWaterProbability 0.5): binomial, so wide bounds.
                c.that(spilled < 8 || (l.faded > 0 && l.faded < spilled as u64), || {
                    format!("{} of {spilled} spilled coins fell in the water", l.faded)
                });
            },
            metrics: |s| {
                let last_spill = s
                    .log
                    .lines
                    .iter()
                    .filter(|l| l.contains(" Bag.Spilled "))
                    .filter_map(|l| l.split(' ').next()?.parse::<u32>().ok())
                    .max()
                    .unwrap_or(0);
                std::collections::BTreeMap::from([
                    ("purse_after_overflow".into(), s.game.wallet.coins() as f64),
                    ("overflow_spills".into(), s.log.count("Bag.Spilled") as f64),
                    ("last_spill_s".into(), last_spill as f64 * game::state::TICK as f64),
                ])
            },
            known_issue: None,
        },
        Scenario {
            name: "purse_full_pickup",
            area: Area::Economy,
            class: Class::Internal,
            summary: "A settled 50-coin purse picks up 10 more coins and spills the excess, conserving every coin",
            seed: 1,
            setup: |seed| {
                let mut s = island(seed, 50);
                rain(&mut s, 10, -0.5, 0.1);
                s.closed_economy();
                s
            },
            script: || Script::new().idle(20.0),
            checkpoint_every: 300,
            check: |s, c| {
                let picked = s.log.count("Coin.PickedUp");
                c.that(picked >= 10, || format!("only {picked} pickups for 10 coins"));
                c.eq("coins left on the ground", s.game.coins.list.iter().filter(|c| crate::sim::is_loose(c)).count() as u32 + s.ledger.as_ref().map_or(0, |l| l.faded as u32), s.log.count("Bag.Spilled") + 10 - picked);
                c.eq(
                    "purse = start - spills + pickups",
                    s.game.wallet.coins() + s.log.count("Bag.Spilled"),
                    50 + picked,
                );
            },
            metrics: no_metrics,
            known_issue: None,
        },
        Scenario {
            name: "pay_complete",
            area: Area::Economy,
            class: Class::Internal,
            summary: "Holding pay at a 3-coin post spends exactly 3 and upgrades it once",
            seed: 1,
            setup: |seed| {
                let mut s = island(seed, 5);
                worlds::place(&mut s.game, &s.world, proto::POST3, 0.0, false);
                s.closed_economy();
                s
            },
            script: || Script::new().idle(0.5).hold_pay(4.0).idle(2.0),
            checkpoint_every: 60,
            check: |s, c| {
                c.eq("wallet", s.game.wallet.coins(), 2);
                c.eq("completions", s.log.count("Pay.Complete"), 1);
                c.eq("coins into slots", s.log.count("Pay.CoinToPayable"), 3);
                c.eq("upgrades", s.log.count("Upgraded"), 1);
                c.eq("releases", s.log.count("Pay.ReleaseCoins"), 0);
            },
            metrics: no_metrics,
            known_issue: None,
        },
        Scenario {
            name: "pay_cancel",
            area: Area::Economy,
            class: Class::Internal,
            summary: "Letting go of pay part-way releases the coins, and the monarch gets them all back",
            seed: 1,
            setup: |seed| {
                let mut s = island(seed, 3);
                worlds::place(&mut s.game, &s.world, proto::POST3, 0.0, false);
                s.closed_economy();
                s
            },
            script: || Script::new().idle(0.5).hold_pay(0.5).idle(8.0),
            checkpoint_every: 60,
            check: |s, c| {
                c.eq("completions", s.log.count("Pay.Complete"), 0);
                c.eq("upgrades", s.log.count("Upgraded"), 0);
                c.that(s.log.count("Pay.CoinToPayable") >= 1, || {
                    "no coin reached the payable before letting go".into()
                });
                c.eq("releases", s.log.count("Pay.ReleaseCoins"), 1);
                c.eq("wallet (all coins recovered)", s.game.wallet.coins(), 3);
            },
            metrics: no_metrics,
            known_issue: None,
        },
        Scenario {
            name: "pay_unaffordable",
            area: Area::Economy,
            class: Class::Internal,
            summary: "Two coins can't buy a 3-coin post; nothing is spent and both come back",
            seed: 1,
            setup: |seed| {
                let mut s = island(seed, 2);
                worlds::place(&mut s.game, &s.world, proto::POST3, 0.0, false);
                s.closed_economy();
                s
            },
            script: || Script::new().idle(0.5).hold_pay(4.0).idle(8.0),
            checkpoint_every: 60,
            check: |s, c| {
                c.eq("completions", s.log.count("Pay.Complete"), 0);
                c.eq("upgrades", s.log.count("Upgraded"), 0);
                c.eq("wallet", s.game.wallet.coins(), 2);
            },
            metrics: no_metrics,
            known_issue: None,
        },
        Scenario {
            name: "mass_coins",
            area: Area::Economy,
            class: Class::Internal,
            summary: "150 loose coins, 24 workers and the monarch: purses fill to their limits, no coin is duplicated or lost",
            seed: 7,
            setup: |seed| {
                let mut s = island(seed, 0);
                worlds::spawn(&mut s.game, Role::Worker, 24, -19.0, 1.6);
                rain(&mut s, 150, -20.0, 40.0 / 150.0);
                s.closed_economy();
                s
            },
            script: || Script::new().walk(1, 6.0).walk(-1, 12.0).idle(6.0),
            checkpoint_every: 300,
            check: |s, c| {
                let worker_cap = 3;
                for u in &s.game.population.units {
                    c.that(u.coins <= worker_cap, || {
                        format!("worker {} holds {} coins (cap {worker_cap})", u.id, u.coins)
                    });
                }
                let held: u32 = s.game.population.units.iter().map(|u| u.coins).sum();
                c.that(held > 0, || "no worker picked up a coin".into());
                c.that(s.game.wallet.coins() > 0, || "the monarch picked up nothing".into());
            },
            metrics: no_metrics,
            known_issue: None,
        },
    ]
}

/// Drops `n` coins anybody may take, from `x0` every `dx`, falling from
/// a metre up.
fn rain(s: &mut Sim, n: usize, x0: f32, dx: f32) {
    for i in 0..n {
        let x = x0 + i as f32 * dx;
        let vx = s.game.rng.range(-0.5, 0.5);
        s.game.coins.drop(
            Who::Thing,
            (x, 1.0 - game::build::WALK_Y),
            (vx, 2.0),
            Policy::Anybody,
            None,
            None,
        );
    }
}
