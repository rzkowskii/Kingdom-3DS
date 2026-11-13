//! The day/night cycle and the day counter.

use std::collections::BTreeMap;

use game::state::TICK;
use game::time::DayEvent;

use super::island;
use crate::scenario::{no_metrics, Area, Checks, Class, Scenario};
use crate::script::Script;
use crate::sim::Sim;
use crate::tod::{self, LightSample};
use crate::worlds;

/// `(tick, event)` for every day event in the log.
fn day_events(s: &Sim) -> Vec<(u32, String)> {
    s.log
        .lines
        .iter()
        .filter(|l| l.contains(" Day."))
        .filter_map(|l| {
            let (tick, rest) = l.split_once(' ')?;
            Some((tick.parse().ok()?, rest.split_once(' ')?.1.to_string()))
        })
        .collect()
}

fn started(day: u32, show_counter: bool) -> String {
    format!(
        "{:?}",
        game::state::GameEvent::Day(DayEvent::Started { day, show_counter })
    )
}

fn ended() -> String {
    format!("{:?}", game::state::GameEvent::Day(DayEvent::Ended))
}

/// `(hours since day 0 00:00, ambient luma)` from the recorded lighting.
fn ambient_series(s: &Sim) -> Vec<(f64, f64)> {
    s.lights
        .iter()
        .map(|l| (l.hours(), tod::luma(l.ambient)))
        .collect()
}

/// Ambient luma of the summer preset for each part of the day.
fn preset_luma(s: &Sim, part: game::time::PartOfDay) -> f64 {
    let p = s
        .world
        .lights
        .iter()
        .find(|l| l.season == 1 && l.part_of_day == part as u8)
        .expect("lit_world has a preset per part of the day");
    tod::luma(p.colors[game::lighting::color::AMBIENT])
}

fn lit_island(seed: u32) -> Sim {
    let world = worlds::lit_world();
    let game = worlds::island(&world, seed, 0);
    let mut sim = Sim::new(world, game);
    sim.light_every = Some(1);
    sim
}

/// The lighting checks: every phase starts on schedule each day, nothing
/// else moves the light, the same hour looks the same every day, and
/// random times of day show the light their window promises.
fn check_lighting(s: &Sim, c: &mut Checks) {
    use game::time::{part_of_day, PartOfDay, TRANSITIONS};
    let series = ambient_series(s);
    let on = tod::onsets(&series);
    // Day 0 starts at 04:00 in the night light, so it has all four onsets
    // too; day 3 ends just after dawn.
    let mut want = Vec::new();
    for day in 0..3 {
        for (_, start, _) in TRANSITIONS {
            want.push(day as f64 * 24.0 + start as f64);
        }
    }
    want.push(3.0 * 24.0 + TRANSITIONS[0].1 as f64);
    c.eq("light changes", on.len(), want.len());
    for (got, want) in on.iter().zip(&want) {
        c.near(
            &format!("light change at {want} h"),
            *got as f32,
            *want as f32,
            0.02,
        );
    }

    // The same time of day looks the same on days 1 and 2.
    let worst = series
        .iter()
        .filter(|p| (24.0..48.0).contains(&p.0))
        .map(|p| (p.1 - tod::luma_at(&series, p.0 + 24.0)).abs())
        .fold(0.0, f64::max);
    c.that(worst < 0.01, || {
        format!("day 2's light differs from day 1's by up to {worst:.4} at the same hour")
    });

    // Random times (they differ per seed: the state hash picks them): a
    // settled phase shows its preset, a blend lies between the two presets.
    let luma = |p| preset_luma(s, p);
    let prev = |p| match p {
        PartOfDay::Dawn => PartOfDay::Night,
        PartOfDay::Day => PartOfDay::Dawn,
        PartOfDay::Evening => PartOfDay::Day,
        PartOfDay::Night => PartOfDay::Evening,
    };
    for t in tod::pick_times(s.state_hash() as u32, 16, 2) {
        let got = tod::luma_at(&series, t.hours());
        let (part, progress) = part_of_day(t.hour);
        let (to, from) = (luma(part), luma(prev(part)));
        if progress >= 1.0 {
            c.near(
                &format!("ambient at {} ({})", t.label(), t.window),
                got as f32,
                to as f32,
                1e-3,
            );
        } else {
            let (lo, hi) = (from.min(to) - 1e-3, from.max(to) + 1e-3);
            c.range(
                &format!("ambient at {} ({})", t.label(), t.window),
                got,
                lo,
                hi,
            );
        }
    }

    // Lit and dark follow the presets: brightest at midday, darkest at night.
    let noon = tod::luma_at(&series, 24.0 + 14.0);
    let night = tod::luma_at(&series, 24.0 + 2.0);
    c.near(
        "midday ambient",
        noon as f32,
        luma(PartOfDay::Day) as f32,
        1e-3,
    );
    c.near(
        "night ambient",
        night as f32,
        luma(PartOfDay::Night) as f32,
        1e-3,
    );
    // Stars: out once the night has settled, gone by midday.
    let stars_at = |h: f64| {
        s.lights
            .iter()
            .find(|l: &&LightSample| l.hours() >= h)
            .map_or(f32::NAN, |l| l.stars)
    };
    c.near("stars at night", stars_at(24.0 + 2.0), 1.0, 1e-3);
    c.near("stars at midday", stars_at(24.0 + 14.0), 0.0, 1e-3);
}

pub fn scenarios() -> Vec<Scenario> {
    vec![
        time_of_day_lighting(),
        day_night_cycle(),
        boss_wave_red_moon(),
    ]
}

fn red_moon_island(seed: u32) -> Sim {
    use game::data::{LightPreset, PortalDef, Proto, WaveDef, LIGHT_COLORS, LIGHT_FLOATS};
    use game::director::WaveKind;
    use game::lighting::{color, float};
    use game::time::DayCounter;

    let mut world = worlds::lit_world();
    // Keep the underlying stars constant even during the dawn transition,
    // so the sampled value directly measures the overlay's fade.
    for preset in &mut world.lights {
        preset.floats[float::STARS] = 1.0;
    }
    let mut colors = [[0.0; 4]; LIGHT_COLORS.len()];
    colors[color::MOON_TINT] = [0.6, 0.2, 0.1, 1.0];
    world.red_moon = Some(world.lights.len() as u16);
    world.lights.push(LightPreset {
        name: "Red Moon".into(),
        category: 0,
        weather: 0,
        season: 0,
        part_of_day: 0,
        test_only: false,
        enabled: 0x0040_6c00,
        colors,
        floats: [0.0; LIGHT_FLOATS.len()],
    });

    let portal = world.protos.len() as u16;
    world.protos.push(Proto {
        name: "Portal".into(),
        portal: Some(PortalDef {
            cliff: false,
            hit_points: 100,
            by_arrows: true,
            by_knights: true,
            hit_box: (-1.0, 1.0, 0.0, 2.0),
            defense_range: 0.0,
            defense_delay: (0.0, 0.0),
            defense_cooldown: 0.0,
            rebuild_days: 0.0,
            rift: None,
        }),
        ..Default::default()
    });
    world.waves.push(WaveDef {
        boss_wave: true,
        ..Default::default()
    });
    world.levels[0].waves[1] = [Some(0); 3];

    let mut game = worlds::island(&world, seed, 0);
    game.clock.time = 17.0;
    game.days = DayCounter::new(&game.clock);
    worlds::place(&mut game, &world, portal, 25.0, false);
    game.greed.schedule(
        &world,
        &game.level,
        0,
        WaveKind::Boss,
        game.season,
        game.clock.day,
        game.campfire,
        game.borders,
        &mut game.rng,
    );
    let mut sim = Sim::new(world, game);
    sim.light_every = Some(1);
    sim
}

fn check_red_moon(s: &Sim, c: &mut Checks) {
    const OPEN: &str = "Greed.PortalOpened";
    const ADMISSION: &str = "Greed.BossWaveSpawned";
    const DAWN: &str = "Day.Started";
    c.eq("opened portals", s.log.count(OPEN), 1);
    c.eq("admitted boss waves", s.log.count(ADMISSION), 1);
    c.eq("day starts", s.log.count(DAWN), 1);
    c.that(s.game.greed.trolls.is_empty(), || {
        "an empty boss wave spawned enemies".into()
    });

    let (Some(&open), Some(&admission), Some(&dawn)) = (
        s.log.first.get(OPEN),
        s.log.first.get(ADMISSION),
        s.log.first.get(DAWN),
    ) else {
        return;
    };
    c.that(open < admission && admission + 660 < dawn, || {
        format!("expected opening, full fade and then dawn, got {open}, {admission}, {dawn}")
    });

    for (label, tick, stars) in [
        ("opening leaves stars alone", open, 1.0),
        ("before admission", admission.saturating_sub(1), 1.0),
        ("five seconds after admission", admission + 300, 0.5),
        ("full overlay", admission + 660, 0.0),
        ("overlay still active at dawn", dawn, 0.0),
        ("five seconds after dawn", dawn + 300, 0.5),
        ("overlay removed after dawn", dawn + 660, 1.0),
    ] {
        let sample = s.lights.iter().find(|sample| sample.tick == tick);
        c.near(label, sample.map_or(f32::NAN, |l| l.stars), stars, 0.003);
    }

    let ambient = s
        .lights
        .iter()
        .find(|sample| sample.tick == admission + 660)
        .map_or(f32::NAN, |l| tod::luma(l.ambient) as f32);
    c.near(
        "boss overlay keeps night ambient",
        ambient,
        preset_luma(s, game::time::PartOfDay::Night) as f32,
        1e-3,
    );
}

fn boss_wave_red_moon() -> Scenario {
    Scenario {
        name: "boss_wave_red_moon",
        area: Area::Time,
        class: Class::Internal,
        summary: "An admitted empty boss wave fades the stars out in ten seconds and dawn brings them back over ten seconds",
        seed: 1,
        setup: red_moon_island,
        script: || Script::new().idle(135.0),
        checkpoint_every: 30 * 60,
        check: check_red_moon,
        metrics: no_metrics,
        known_issue: None,
    }
}

fn time_of_day_lighting() -> Scenario {
    Scenario {
        name: "time_of_day_lighting",
        area: Area::Time,
        class: Class::Parity(&["time-of-day-lighting"]),
        summary: "Three days of lighting: each phase (dawn, day, evening, night) blends in on schedule, every day, and random times of day show the right light",
        seed: 1,
        setup: lit_island,
        script: || Script::new().idle(3.0 * 240.0 + 30.0),
        checkpoint_every: 60 * 60,
        check: check_lighting,
        metrics: |s| {
            let mut m = tod::light_metrics(&ambient_series(s));
            let edges: Vec<(f64, bool)> = s
                .lights
                .windows(2)
                .filter(|w| w[0].daytime != w[1].daytime)
                .map(|w| (w[1].hours(), w[1].daytime))
                .collect();
            let (day, night) = tod::daytime_lengths(&edges);
            m.insert("daytime_s".into(), day);
            m.insert("night_s".into(), night);
            m
        },
        known_issue: None,
    }
}

fn day_night_cycle() -> Scenario {
    Scenario {
        name: "day_night_cycle",
        area: Area::Time,
        class: Class::Parity(&["day-night-cycle"]),
        summary: "Three days from 04:00 on day 0: dawn, dusk and the day counter on schedule",
        seed: 1,
        setup: |seed| island(seed, 0),
        script: || Script::new().idle(3.0 * 240.0 + 10.0),
        checkpoint_every: 60 * 60,
        check: |s, c| {
            let got: Vec<String> = day_events(s).into_iter().map(|e| e.1).collect();
            c.eq(
                "day events",
                got,
                vec![
                    started(1, false),
                    ended(),
                    started(2, true),
                    ended(),
                    started(3, true),
                    ended(),
                    started(4, true),
                ],
            );
            c.eq("clock day", s.game.clock.day, 3);
            c.eq("day shown", s.game.days.day, 4);
        },
        metrics: |s| {
            let e = day_events(s);
            let secs = |i: usize| e.get(i).map_or(f64::NAN, |x| x.0 as f64 * TICK as f64);
            BTreeMap::from([
                ("first_dawn_s".into(), secs(0)),
                ("daytime_s".into(), secs(1) - secs(0)),
                ("night_s".into(), secs(2) - secs(1)),
                ("day_length_s".into(), secs(2) - secs(0)),
                ("day_length_3_avg_s".into(), (secs(6) - secs(0)) / 3.0),
            ])
        },
        known_issue: None,
    }
}
