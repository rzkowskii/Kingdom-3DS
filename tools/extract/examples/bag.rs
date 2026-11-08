//! Fills the coin bag headlessly and reports how many coins stay in.
//! `cargo run --release -p extract --example bag -- assets/world.bin 60`

use game::bag::{Bag, BagEvent};
use game::data::WorldData;
use game::rng::Rng;

fn main() {
    let path = std::env::args().nth(1).expect("world.bin path");
    let world: WorldData =
        game::assetfile::decode_world(&std::fs::read(path).expect("read")).expect("decode");
    let mut coins: u32 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(40);
    let mut bag = Bag::default();
    let mut rng = Rng::new(3);
    let mut spills = 0;
    let t0 = std::time::Instant::now();
    for f in 0..60 * 40 {
        if f == 60 * 30 {
            println!("(timing the last 10 s)");
        }
        let mut e = Vec::new();
        bag.update(1.0 / 60.0, &world.bag, coins, &mut rng, |x| e.push(x));
        for x in e {
            if x == BagEvent::Spilled {
                spills += 1;
                coins -= 1;
            }
        }
        if f % 300 == 0 {
            let top = bag.coins.iter().map(|c| c.y).fold(f32::MIN, f32::max);
            println!(
                "t={:>3}s coins={} in bag={} spilled={} top={:.2} awake={} motion={:?}",
                f / 60,
                coins,
                bag.coins.len(),
                spills,
                top,
                bag.awake(),
                bag.motion()
            );
        }
    }
    for m in bag.movers() {
        println!(
            "  mover x={:.3} y={:.3} speed={:.3} spin={:.1}",
            m.0, m.1, m.2, m.3
        );
    }
    let fast = bag.movers().iter().filter(|m| m.2 > 0.02).count();
    println!("  moving faster than 0.02: {fast}+ of {}", bag.coins.len());
    let top = bag.coins.iter().map(|c| c.y).fold(f32::MIN, f32::max);
    println!(
        "end coins={} in bag={} spilled={} top={:.2} ({:?})",
        coins,
        bag.coins.len(),
        spills,
        top,
        t0.elapsed()
    );
}
