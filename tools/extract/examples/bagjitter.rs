//! How much a settled coin purse moves on the touch screen, and what the
//! pile costs: asks the real purse (`assets/world.bin`) for `n` coins, lets
//! every one drop in and settle for 20 s, then watches 10 s. As in the game
//! (`GameState::update_bag`), a coin that spills over the rim leaves the
//! wallet, so a big ask overflows down to what the purse holds. See
//! docs/perf.md, "Purse step size" and "Purse sleeping".
//! `cargo run --release -p extract --example bagjitter`
use game::bag::{Bag, BagEvent};
use game::data::WorldData;
use game::rng::Rng;

/// One game tick; returns the coins spilled (already taken off `n`).
fn tick(bag: &mut Bag, def: &game::data::BagDef, n: &mut u32, rng: &mut Rng) -> u32 {
    let mut spilled = 0;
    bag.update(1.0 / 60.0, def, *n, rng, |e| {
        if e == BagEvent::Spilled {
            spilled += 1;
        }
    });
    *n -= spilled.min(*n);
    spilled
}

fn main() {
    let world: WorldData =
        game::assetfile::decode_world(&std::fs::read("assets/world.bin").unwrap()).unwrap();
    let def = &world.bag;
    for &ask in &[5u32, 20, 60, 150, 300] {
        let mut n = ask;
        let mut bag = Bag::default();
        let mut rng = Rng::new(3);
        // Drop the coins in (timed), then give them 20 s to settle.
        let t1 = std::time::Instant::now();
        let mut fill = 0u32;
        while bag.coins.len() as u32 != n {
            tick(&mut bag, def, &mut n, &mut rng);
            fill += 1;
        }
        let fill_us = t1.elapsed().as_secs_f64() * 1e6 / fill.max(1) as f64;
        for _ in 0..20 * 60 {
            tick(&mut bag, def, &mut n, &mut rng);
        }
        // Then 10 s: per frame, coins whose drawn position (96 px/unit,
        // rounded) or angle (>0.5°) changed.
        let px = |c: &game::bag::BagCoin| ((c.x * 96.0).round(), (c.y * 96.0).round());
        let mut prev: Vec<_> = bag.coins.iter().map(|c| (px(c), c.angle)).collect();
        let (mut moved, mut turned, mut frames_moving, mut awake, mut spilled) = (0, 0, 0, 0, 0);
        let t0 = std::time::Instant::now();
        for _ in 0..10 * 60 {
            spilled += tick(&mut bag, def, &mut n, &mut rng);
            let now: Vec<_> = bag.coins.iter().map(|c| (px(c), c.angle)).collect();
            let m = now.iter().zip(&prev).filter(|(a, b)| a.0 != b.0).count();
            let t = now
                .iter()
                .zip(&prev)
                .filter(|(a, b)| (a.1 - b.1).abs() > 0.5)
                .count();
            moved += m;
            turned += t;
            frames_moving += (m + t > 0) as u32;
            awake += bag.awake();
            prev = now;
        }
        let us = t0.elapsed().as_secs_f64() * 1e6 / 600.0;
        println!(
            "{ask:3} asked, {n:3} kept: settled {us:6.1} µs/tick, filling {fill_us:6.1} µs/tick ({:4.1} s) | {:5.2} px-moves/frame, {:5.2} turns/frame, moving in {:3}/600 frames, {:5.1} awake, {spilled} spilled",
            fill as f32 / 60.0,
            moved as f32 / 600.0,
            turned as f32 / 600.0,
            frames_moving,
            awake as f32 / 600.0
        );
    }
}
