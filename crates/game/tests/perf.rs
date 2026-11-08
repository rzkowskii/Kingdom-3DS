//! Synthetic stress benchmark: many builders building + many coins dropped,
//! as on an Old 3DS mid-game. Run with:
//!   cargo test -p game --features profile --release --test perf -- --nocapture
//! Numbers are host-relative; on the o3ds everything is roughly 6-10x slower.
#![cfg(feature = "profile")]

use std::time::Instant;

use game::build::WALK_Y;
use game::coins::{Coins, Policy, Who};
use game::controls::Controls;
use game::data::*;
use game::level::instantiate;
use game::rng::Rng;
use game::state::GameState;

/// Buildings under construction at once.
const SITES: usize = 40;
/// Builders hammering them.
const BUILDERS: usize = 24;
/// Loose coins lying in the world.
const COINS: usize = 150;
/// Trees marked for chopping (forest churn per chop).
const TREES: usize = 24;

fn proto(name: &str) -> Proto {
    Proto {
        name: name.to_string(),
        tag: "Untagged".to_string(),
        ..Default::default()
    }
}

fn world() -> WorldData {
    // A building that only builders finish (no auto build).
    let mut site = proto("Site");
    site.building = Some(BuildingDef {
        build_points: 60,
        auto_build_rate: 0.0,
        scaffold: vec![
            ScaffoldModuleDef {
                transform: game::data::Affine2::IDENTITY,
                flip: false,
                flip_y: false,
                sprite: None,
                fill: None,
                x: 0.0,
                y: 0.0,
                z: 0.0,
            };
            3
        ],
    });
    site.payable = Some(PayableDef {
        price: 3,
        price_increase: 0,
        coin_offset: (0.0, 2.5),
        coin_spacing: 0.4,
        pay_point_offset: 0.0,
        pay_distance: 0.75,
    });
    site.upgrade = Some(UpgradeDef {
        next: None,
        only_in_buildable_region: false,
        requires_stone: false,
        cooldown: 0.0,
    });
    // A marked tree: chopping fires forest rebuilds.
    let mut tree = proto("Tree");
    tree.forest_item = Some(ForestItemDef {
        controls_forest_size: true,
        size_padding: 0.5,
        width: 2.0,
    });
    tree.tree_work = Some(TreeWorkDef {
        strength: 8,
        coin_yield: (0, 0),
        chop_mark: Some(0),
    });

    WorldData {
        layers: vec![LayerDef { ratio: 1.0, z: 0.0 }; 7],
        protos: vec![site, tree],
        // One island config: no blocks, so the level starts empty.
        levels: vec![LevelConfigDef {
            name: "bench".to_string(),
            autumn_start: (100, 1),
            winter_start: (200, 1),
            days_before_game_over: 40,
            starting_coins: 0,
            min_level_width: 40,
            group_counts: Vec::new(),
            waves: [[None; 3]; 3],
            wave_timed_multiplier: 1.0,
            wave_multiplier_timeframe: (0, 0),
            defence_waves: [None; 3],
            retaliation_waves: [None; 3],
            season_change_days: 10.0,
            income_multiplier: 1.0,
            free_boat_parts: 0,
            wave_boat_multiplier: 1.0,
            force_all_unlocks: false,
        }],
        // Two weeks of plain days (weather/lighting lookups index these).
        days: (0..14)
            .map(|i| DayDef {
                name: format!("day{i}"),
                seasons: 0xff,
                weathers: 1,
                tracks: Vec::new(),
            })
            .collect(),
        // A purse shaped like the real one (open top), scaled up so the pile
        // of a fat wallet fits: walls to y=1.5, coins spawn at y=1.0 inside.
        bag: BagDef {
            wall: vec![(-1.0, 1.5), (-1.0, 0.0), (1.0, 0.0), (1.0, 1.5)],
            spill: vec![(-3.0, -3.0), (3.0, -3.0), (3.0, -0.05), (-3.0, -0.05)],
            coin_shape: vec![(-0.114, 0.0), (0.114, 0.0), (0.114, 0.094), (-0.114, 0.094)],
            spawn_interval: 0.05,
            gravity_scale: 1.0,
            friction: 0.4,
            bounciness: 0.1,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn scaffold(world: &WorldData, x: f32) -> game::level::Object {
    let mut o = instantiate(world, 0, Layer::Game, x, 0.0, 0.0, 1.0);
    o.build = Some(0.0);
    o
}

fn tree(world: &WorldData, x: f32) -> game::level::Object {
    let mut o = instantiate(world, 1, Layer::Game, x, 0.0, 0.0, 1.0);
    o.marked = true;
    o.chops_left = 8;
    o
}

#[test]
fn bench_heavy_kingdom() {
    let world = world();
    let mut g = GameState::new(&world, 0, 42);
    let mut rng = Rng::new(7);

    // Sites and trees across the kingdom.
    for i in 0..SITES {
        let x = -20.0 + (i as f32 * 40.0 / SITES as f32);
        g.level.add(scaffold(&world, x));
    }
    for i in 0..TREES {
        let x = -21.0 + i as f32 * 1.7;
        g.level.add(tree(&world, x));
    }

    // Builders to hammer them.
    for i in 0..BUILDERS {
        g.population
            .spawn(game::data::Role::Worker, -19.0 + i as f32 * 1.6, &mut rng);
    }

    // A heavy purse: the bag simulates up to 300 pile coins.
    g.wallet.set(400);

    // Coins all over the ground for grabbers to fight over.
    let mut coins = Coins::default();
    for i in 0..COINS {
        let x = -20.0 + (i as f32 * 40.0 / COINS as f32);
        coins.drop(
            Who::Thing,
            (x, 1.0 - WALK_Y),
            (rng.range(-0.5, 0.5), 2.0),
            Policy::Anybody,
            None,
            None,
        );
    }
    g.coins.list = coins.list;

    eprintln!(
        "setup: {} objects, {} workers, {} coins, wallet {}",
        g.level.objects.len(),
        g.population.units.len(),
        g.coins.list.len(),
        g.wallet.coins()
    );

    let controls = Controls::default();
    let mut events = Vec::new();

    // Warm up (settle the bag, let assignment run).
    for _ in 0..120 {
        g.tick(&world, &controls, &mut events);
    }
    events.clear();

    let before = game::prof::report();
    let mut times = Vec::new();
    let frames = 600; // 10 s of game time at 60 Hz
    for _ in 0..frames {
        let t = Instant::now();
        g.tick(&world, &controls, &mut events);
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    let after = game::prof::report();

    times.sort_by(|a, b| a.total_cmp(b));
    let avg: f64 = times.iter().sum::<f64>() / times.len() as f64;
    let p95 = times[(times.len() as f32 * 0.95) as usize];
    let p99 = times[(times.len() as f32 * 0.99) as usize];
    eprintln!(
        "tick ms: avg {avg:.3}  p50 {:.3}  p95 {p95:.3}  p99 {p99:.3}  max {:.3}",
        times[times.len() / 2],
        times[times.len() - 1]
    );

    // o3ds scaling hint: the ARM11 @ 268 MHz is roughly 10x slower per core.
    eprintln!(
        "o3ds estimate (x10): avg {:.1} ms  p99 {:.1} ms  (budget 16.7 ms)",
        avg * 10.0,
        p99 * 10.0
    );

    let mut rows: Vec<(String, f64)> = after
        .iter()
        .map(|(n, t)| {
            let b = before.iter().find(|p| &p.0 == n).map_or(0.0, |p| p.1);
            (n.clone(), (t - b) * 1000.0 / frames as f64)
        })
        .collect();
    rows.sort_by(|a, b| b.1.total_cmp(&a.1));
    eprintln!("--- per-system ms/tick ---");
    for (n, ms) in &rows {
        eprintln!("{:12}  {:.3}", n, ms);
    }

    eprintln!(
        "end state: {} coins in world, {} sites left under construction, {} trees left",
        g.coins.list.len(),
        g.level
            .objects
            .iter()
            .filter(|o| game::build::under_construction(&world, o))
            .count(),
        g.level.objects.iter().filter(|o| o.chops_left > 0).count()
    );

    // Sanity: the sim kept running.
    assert!(!g.population.units.is_empty());

    // --- Burst reality check -------------------------------------------------
    // Real play: the purse sits settled, then the monarch pays or grabs coins
    // and the pile is disturbed. How big and how long is the spike?
    let burst_ticks = 240;
    let mut spike = Vec::new();
    for i in 0..(burst_ticks * 4) {
        // Every second: a payment (wallet drops, pile loses coins), then a
        // pickup (wallet rises, coins rain in).
        if i % 240 == 0 {
            let before = g.wallet.coins();
            g.wallet.set(before.saturating_sub(10));
        }
        if i % 240 == 120 {
            let before = g.wallet.coins();
            g.wallet.set((before + 10).min(500));
        }
        let t = Instant::now();
        g.tick(&world, &controls, &mut events);
        spike.push((t.elapsed().as_secs_f64() * 1000.0, g.bag.awake()));
    }
    let avg_s: f64 = spike.iter().map(|s| s.0).sum::<f64>() / spike.len() as f64;
    let worst = spike.iter().fold(0.0f64, |m, s| m.max(s.0));
    let awake_peak = spike.iter().map(|s| s.1).max().unwrap_or(0);
    let awake_avg: f64 = spike.iter().map(|s| s.1 as f64).sum::<f64>() / spike.len() as f64;
    eprintln!(
        "burst: avg {avg_s:.3} ms  worst {worst:.3} ms  awake peak {awake_peak}  awake avg {awake_avg:.0}/{}",
        g.bag.coins.len()
    );
}
