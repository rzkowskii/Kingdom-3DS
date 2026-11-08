//! Headless run of the real island with a simple autopilot, printing a
//! summary every in-game day. For checking behaviour without a window.
//!
//! `cargo run --release -p extract --example sim -- assets/world.bin [--days N] [--seed N] [--coins N] [--build] [--prefer Castle,Farmhouse]`
//! `--build` finishes construction instantly (as the desktop's B key).
//! `--boat` fits the boat and brings it to the beach at the start (the desktop's H key);
//! `--sail-on` lands on the next island after each win; `--fresh` starts with no unlocks;
//! `--trace-harbor` prints the wharf and boat every minute; `--night-check`
//! lists citizens outside the borders every night hour; `--jitter` lists
//! units turning back and forth (4+ reversals in a second).

use game::autopilot::Autopilot;
use game::controls::ControlMapper;
use game::data::{Role, WorldData};
use game::state::{GameEvent, GameState, TICK};

fn arg<T: std::str::FromStr>(name: &str) -> Option<T> {
    std::env::args()
        .skip_while(|a| a != name)
        .nth(1)
        .and_then(|v| v.parse().ok())
}

/// Farm diagnostics: what's around each farmhouse.
fn diagnose_farms(g: &GameState, world: &WorldData) {
    for o in &g.level.objects {
        let p = &world.protos[o.proto as usize];
        if p.farmhouse.is_none() {
            continue;
        }
        println!("farmhouse {} at {:.2} build={:?}", p.name, o.x, o.build);
        for q in &g.level.objects {
            let qp = &world.protos[q.proto as usize];
            if let Some(gp) = qp.gaps {
                let (l, r) = gp.span(q.x, q.flip);
                if r > o.x - 4.0 && l < o.x + 5.0 && gp.layers & game::data::Gaps::NOT_FARMABLE != 0
                {
                    println!(
                        "  blocker {} {:.2}..{:.2} build={:?}",
                        qp.name, l, r, q.build
                    );
                }
            }
        }
        for f in &g.forests {
            if f.1 > o.x - 4.0 && f.0 < o.x + 5.0 {
                println!("  forest {:.2}..{:.2}", f.0, f.1);
            }
        }
    }
}

fn main() {
    let path = std::env::args().nth(1).expect("world.bin path");
    let world: WorldData =
        game::assetfile::decode_world(&std::fs::read(path).expect("read")).expect("decode");
    let days: f32 = arg("--days").unwrap_or(5.0);
    let seed: u32 = arg("--seed").unwrap_or(1);
    let build = std::env::args().any(|a| a == "--build");
    let pilot = Autopilot {
        prefer: arg::<String>("--prefer")
            .map(|s| s.split(',').map(str::to_owned).collect())
            .unwrap_or_default(),
        goto: None,
    };
    let island: usize = arg("--island").unwrap_or(0);
    let mut progress = if std::env::args().any(|a| a == "--fresh") {
        game::progress::Progress::default()
    } else {
        game::progress::Progress::everything()
    };
    let mut g = GameState::start(&world, island, seed, &progress, None);
    let sail_on = std::env::args().any(|a| a == "--sail-on");
    let mut won_at: Option<u32> = None;
    if let Some(c) = arg::<u32>("--coins") {
        g.wallet.set(c);
    }
    if std::env::args().any(|a| a == "--count") {
        let n = g.level.objects.len();
        let game = g
            .level
            .objects
            .iter()
            .filter(|o| o.layer == game::data::Layer::Game)
            .count();
        let play = g
            .level
            .objects
            .iter()
            .filter(|o| world.protos[o.proto as usize].gameplay())
            .count();
        let forest = g
            .level
            .objects
            .iter()
            .filter(|o| world.protos[o.proto as usize].forest_item.is_some())
            .count();
        println!("objects {n}, game layer {game}, gameplay {play}, forest items {forest}");
        return;
    }
    if std::env::args().any(|a| a == "--boat") {
        g.dev_boat_at_beach(&world);
    }
    // --god: a bottomless purse, as SELECT+R+Y on the 3DS.
    g.god = std::env::args().any(|a| a == "--god");
    for o in &g.level.objects {
        let n = &world.protos[o.proto as usize].name;
        if n.starts_with("Farmhouse") {
            println!(
                "start: {} id={} x={:.2} layer={:?} in_forest={}",
                n,
                o.id,
                o.x,
                o.layer,
                game::build::in_forest(&g.forests, o.x)
            );
        }
    }
    if std::env::args().any(|a| a == "--forest-items") {
        for o in &g.level.objects {
            let p = &world.protos[o.proto as usize];
            if let Some(f) = p.forest_item {
                let ratio = world.layer(o.layer).ratio;
                let x = if ratio != 0.0 { o.x / ratio } else { o.x };
                if (-40.0..40.0).contains(&x) {
                    println!(
                        "forest item {} layer={:?} x={:.2} w={:.2} pad={:.2} controls={}",
                        p.name, o.layer, x, f.width, f.size_padding, f.controls_forest_size
                    );
                }
            }
        }
        println!("forests {:?}", g.forests);
    }
    println!(
        "campfire {:.2} monarch {:.2} level {:.1}..{:.1}",
        g.campfire,
        g.monarch.x,
        g.level.left,
        g.level.left + g.level.width
    );
    let mut mapper = ControlMapper::default();
    let mut events = Vec::new();
    let mut turns: std::collections::HashMap<u32, (f32, f32, u32)> =
        std::collections::HashMap::new();
    let mut states: std::collections::HashMap<u32, (String, u32)> =
        std::collections::HashMap::new();
    let mut anims: std::collections::HashMap<u32, (String, u32)> = std::collections::HashMap::new();
    let frames = (days * 240.0 / TICK) as u32;
    let mut harvests = 0;
    let mut kills = 0;
    let mut picked: std::collections::BTreeMap<String, u32> = Default::default();
    let mut purchases = 0;
    let mut taxes = 0;
    for frame in 0..frames {
        let raw = pilot.input(&g, &world, frame);
        let c = mapper.map(TICK, raw);
        events.clear();
        g.tick(&world, &c, &mut events);
        // --sail-on: five seconds after the boat leaves, land on the next island
        // (`--boat` brings each new island's boat to its beach straight away).
        if sail_on && g.won {
            let t = *won_at.get_or_insert(frame);
            if frame - t > 300 {
                let out = progress.finish(game::progress::Ending {
                    land: g.level_index,
                    won: true,
                    unlock: g.queued_unlock,
                });
                let mut v = g.voyage.clone().expect("voyage");
                v.land = out.next.unwrap_or(v.land);
                println!(">>> sailing to island {} with {:?}", v.land, v);
                g = GameState::start(&world, v.land, seed + frame, &progress, Some(&v));
                if std::env::args().any(|a| a == "--boat") {
                    g.dev_boat_at_beach(&world);
                }
                won_at = None;
            }
        }
        if build && frame % 60 == 0 {
            g.complete_construction(&world);
        }
        // --jitter: units reversing direction over and over.
        if std::env::args().any(|a| a == "--jitter") {
            for u in &g.population.units {
                let e = turns.entry(u.id).or_insert((u.x, 0.0f32, 0u32));
                let dx = u.x - e.0;
                if dx.abs() > 1e-4 {
                    if dx.signum() != e.1 && e.1 != 0.0 {
                        e.2 += 1;
                    }
                    e.1 = dx.signum();
                }
                e.0 = u.x;
                if std::env::var("TRACE_UNIT")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    == Some(u.id)
                {
                    println!(
                        "  t{frame} {:?} x={:.3} vx={:.3} y={:.2} {:?}",
                        u.anim(),
                        u.x,
                        u.mover.vx,
                        u.y,
                        u.ai
                    );
                }
                let anim = format!("{:?}", u.anim());
                let a = anims.entry(u.id).or_insert((anim.clone(), 0));
                if a.0 != anim {
                    a.0 = anim;
                    a.1 += 1;
                }
                let state = format!("{:?}", u.ai);
                let state = format!(
                    "{}{}{}",
                    state.split([' ', '{', '(']).next().unwrap_or(""),
                    if state.contains("working: true") {
                        "+"
                    } else {
                        ""
                    },
                    if u.y > 0.05 { "^" } else { "" }
                );
                let last = states.entry(u.id).or_insert((state.clone(), 0u32));
                if last.0 != state {
                    if std::env::var("TRACE").is_ok() && last.1 >= 2 {
                        println!(
                            "  {} {:?}@{:.2} {} -> {:?} goal",
                            u.id, u.role, u.x, last.0, u.ai
                        );
                    }
                    last.0 = state;
                    last.1 += 1;
                }
            }
            if frame % 60 == 0 {
                for u in &g.population.units {
                    if turns.get(&u.id).is_some_and(|e| e.2 >= 4)
                        || states.get(&u.id).is_some_and(|e| e.1 >= 4)
                        || anims.get(&u.id).is_some_and(|e| e.1 >= 6)
                    {
                        println!(
                            "jitter {:.1}h {}: #{} {:?}@{:.1} {:?} ({} turns, {} state changes, {} anim changes)",
                            g.clock.time,
                            if g.days.is_daytime { "day" } else { "night" }, u.id,
                            u.role,
                            u.x,
                            u.ai,
                            turns[&u.id].2,
                            states[&u.id].1,
                            anims[&u.id].1
                        );
                    }
                }
                for e in turns.values_mut() {
                    e.2 = 0;
                }
                for e in states.values_mut() {
                    e.1 = 0;
                }
                for e in anims.values_mut() {
                    e.1 = 0;
                }
            }
        }
        // --night-check: citizens outside the walls at night, once an hour.
        if std::env::args().any(|a| a == "--night-check")
            && !g.days.is_daytime
            && frame % (game::time::SECONDS_PER_HOUR / TICK) as u32 == 0
        {
            let b = g.borders;
            let out: Vec<String> = g
                .population
                .units
                .iter()
                .filter(|u| u.role != game::data::Role::Beggar && !b.contains(u.x))
                .map(|u| format!("{:?}@{:.1} {:?}", u.role, u.x, u.ai))
                .collect();
            println!(
                "night {:.1}h borders {:.1}..{:.1}: {} of {} outside {:?}",
                g.clock.time,
                b.left,
                b.right,
                out.len(),
                g.population.units.len(),
                out
            );
        }
        for e in &events {
            match e {
                GameEvent::Unit(game::units::UnitEvent::Farmed(_)) => harvests += 1,
                GameEvent::Wild(game::wildlife::WildEvent::Killed { .. }) => kills += 1,
                GameEvent::Coin(game::coins::CoinEvent::PickedUp(w)) => {
                    *picked
                        .entry(
                            format!("{:?}", std::mem::discriminant(w)).len().to_string()
                                + match w {
                                    game::coins::Who::Monarch => "monarch",
                                    game::coins::Who::Unit(_) => "unit",
                                    _ => "other",
                                },
                        )
                        .or_insert(0) += 1;
                }
                GameEvent::Pay(game::payment::PayEvent::Complete { .. }) => purchases += 1,
                GameEvent::Unit(game::units::UnitEvent::PaidTaxes(_)) => taxes += 1,
                GameEvent::Day(d) => {
                    println!("--- {:?} at {:.2}h day {}", d, g.clock.time, g.clock.day)
                }
                GameEvent::GameOver => println!("!!! game over"),
                GameEvent::Won => println!(
                    "!!! won at t={:.0} voyage {:?}",
                    frame as f32 * TICK,
                    g.voyage
                ),
                GameEvent::Harbor(h) => {
                    if !matches!(
                        h,
                        game::boat::HarborEvent::PartStacked | game::boat::HarborEvent::PartPlaced
                    ) {
                        println!("harbor {:?} at t={:.0}", h, frame as f32 * TICK);
                    }
                }
                GameEvent::Upgraded(u) => {
                    let name = g.level.index_of(u.new).map(|i| {
                        world.protos[g.level.objects[i].proto as usize]
                            .name
                            .as_str()
                    });
                    println!(
                        "upgraded {} -> {} {:?} at t={:.0}",
                        u.old,
                        u.new,
                        name,
                        frame as f32 * TICK
                    );
                }
                _ => {}
            }
        }
        if std::env::args().any(|a| a == "--trace-archers") && frame % 120 == 0 {
            for u in g.population.units.iter().filter(|u| u.role == Role::Archer) {
                let near: Vec<String> = g
                    .coins
                    .list
                    .iter()
                    .filter(|c| (c.x - u.x).abs() < 8.0)
                    .map(|c| format!("{:.1}/{:?}", c.x, c.dropper))
                    .collect();
                println!(
                    "  archer x={:.1} coins={} ai={:?} near={:?}",
                    u.x, u.coins, u.ai, near
                );
            }
        }
        if std::env::args().any(|a| a == "--trace-merchant") && frame % 600 == 0 {
            for m in &g.merchants.list {
                let x = g.level.index_of(m.object).map(|i| g.level.objects[i].x);
                println!(
                    "  merchant x={:?} mule={:.1} cargo={} {:?} t={:.0}",
                    x,
                    m.mule_x,
                    m.cargo,
                    m.state,
                    frame as f32 * TICK
                );
            }
        }
        if frame % (60 * 60) == 0 {
            let roles = [
                Role::Beggar,
                Role::Peasant,
                Role::Worker,
                Role::Archer,
                Role::Farmer,
                Role::Knight,
            ];
            let pop: Vec<String> = roles
                .iter()
                .map(|r| format!("{:?}={}", r, g.population.count(*r)))
                .collect();
            let names: Vec<&str> = g
                .level
                .objects
                .iter()
                .map(|o| world.protos[o.proto as usize].name.as_str())
                .filter(|n| {
                    n.starts_with("Castle") || n.starts_with("Farmhouse") || n.starts_with("Shop")
                })
                .collect();
            println!(
                "t={:5.0}s {:5.2}h coins={:3} {} farms={} (farmable {}) farm-strokes={} trolls={} deer={} bunnies={} kills={} grass={} thickets={} unit-coins={} ground-coins={}",
                frame as f32 * TICK,
                g.clock.time,
                g.wallet.coins(),
                pop.join(" "),
                g.farms.lands.len(),
                g.farms.lands.iter().filter(|l| l.farmable).count(),
                harvests,
                g.greed.trolls.len(),
                g.wildlife.animals.iter().filter(|a| a.species == game::wildlife::Species::Deer).count(),
                g.wildlife.animals.iter().filter(|a| a.species == game::wildlife::Species::Bunny).count(),
                kills,
                g.meadows.patches.iter().filter(|p| p.alive).count(),
                g.meadows.thickets().count(),
                g.population.units.iter().map(|u| u.coins).sum::<u32>(),
                g.coins.list.len(),
            );
            let _ = &names;
            if std::env::args().any(|a| a == "--trace-harbor") {
                let h = &g.harbor;
                let w = h
                    .wharf
                    .as_ref()
                    .map(|w| (w.stacked.len(), w.placing.len(), w.launched));
                let b = h
                    .boat_at(&g.level)
                    .map(|(x, d)| (x, d, h.boat_mode(&g.level)));
                let deck = h
                    .boat
                    .as_ref()
                    .map(|b| (b.workers, b.knights, b.archers, b.deck.len()));
                println!(
                    "  harbor wharf={w:?} boat={b:?} deck={deck:?} beach={:?}",
                    h.beach
                );
            }
        }
    }
    // --write-save DIR: leave the run where the desktop finds it (KINGDOM_SAVES=DIR).
    if let Some(dir) = arg::<String>("--write-save") {
        let wb = std::fs::read(std::env::args().nth(1).unwrap()).expect("world.bin");
        let bytes = game::save::save_game(&g, game::save::fingerprint(&wb));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(std::path::Path::new(&dir).join("game.sav"), bytes).expect("write");
        println!(
            "wrote {dir}/game.sav at monarch x={:.2} day {}",
            g.monarch.x, g.clock.day
        );
    }
    // --save-check: save, load, and check both copies stay identical for a day.
    if std::env::args().any(|a| a == "--save-check") {
        let bytes = game::save::save_game(&g, 1);
        let mut h = game::save::load_game(&bytes, 1, &world).expect("load");
        println!("save: {} bytes", bytes.len());
        let (mut ea, mut eb) = (Vec::new(), Vec::new());
        let (mut ma, mut mb) = (ControlMapper::default(), ControlMapper::default());
        for f in 0..(240.0 / TICK) as u32 {
            let ra = pilot.input(&g, &world, frames + f);
            let rb = pilot.input(&h, &world, frames + f);
            let (ca, cb) = (ma.map(TICK, ra), mb.map(TICK, rb));
            ea.clear();
            eb.clear();
            g.tick(&world, &ca, &mut ea);
            h.tick(&world, &cb, &mut eb);
        }
        let same = g.monarch.x == h.monarch.x
            && g.wallet.coins() == h.wallet.coins()
            && g.population.units.len() == h.population.units.len()
            && g.greed.trolls.len() == h.greed.trolls.len()
            && g.rng.clone().next_u32() == h.rng.clone().next_u32();
        println!(
            "save-check after a day: {}",
            if same { "identical" } else { "DIVERGED" }
        );
    }
    if let Some(t) = arg::<u32>("--deer-at") {
        let _ = t;
    }
    for a in &g.wildlife.animals {
        println!("animal {:?} x={:.1} state={:?}", a.species, a.x, a.state);
    }
    for f in &g.mounts.free {
        let x = g.level.index_of(f.object).map(|i| g.level.objects[i].x);
        println!(
            "free steed {} at {:?} {:?}",
            world.steeds[f.steed as usize].name, x, f.state
        );
    }
    println!("riding {}", world.steeds[g.steed as usize].name);
    println!("picked {picked:?} purchases {purchases} tax-events {taxes}");
    diagnose_farms(&g, &world);
    println!("monarch at {:.2}, wallet {}", g.monarch.x, g.wallet.coins());
    if let Some(name) = arg::<String>("--find") {
        for o in &g.level.objects {
            let n = &world.protos[o.proto as usize].name;
            if n.contains(&name) {
                println!(
                    "  object {} id={} x={:.1} layer={:?}",
                    n, o.id, o.x, o.layer
                );
            }
        }
        for p in &g.payables {
            if let Some(i) = g.level.index_of(p.id.0) {
                let o = &g.level.objects[i];
                let n = &world.protos[o.proto as usize].name;
                if n.contains(&name) {
                    println!(
                        "  {} x={:.1} price={} can_pay={} in_forest={} build={:?}",
                        n,
                        p.x,
                        p.price,
                        p.can_pay,
                        game::build::in_forest(&g.forests, o.x),
                        o.build
                    );
                }
            }
        }
    }
    for p in &g.payables {
        if let Some(i) = g.level.index_of(p.id.0) {
            let n = &world.protos[g.level.objects[i].proto as usize].name;
            if (p.x - g.campfire).abs() < 50.0
                && world.protos[g.level.objects[i].proto as usize]
                    .tree_work
                    .is_none()
            {
                println!(
                    "  payable {} x={:.1} price={} can_pay={}",
                    n, p.x, p.price, p.can_pay
                );
            }
        }
    }
}
