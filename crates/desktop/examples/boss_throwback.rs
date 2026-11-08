//! Scripted boss throwback captures using the desktop renderer and Session ticks.
//! A catapult makes the lodged stone. The fixture then places the boss beside it.
//! `cargo run -p desktop --example boss_throwback -- <assets> <output>`

#[path = "../src/assets.rs"]
mod assets;

use ::scene::boulder_fx::BoulderParticleKind;
use ::scene::session::Session;
use game::animation::EventKind;
use game::controls::{Controls, RawInput};
use game::data::{Layer, WorldData};
use game::greed::{EnemyKind, GreedEvent, TrollState};
use game::level::{self, Level};
use game::siege::mode;
use game::state::{GameEvent, TICK};
use macroquad::prelude::*;
use std::path::Path;

const FPS: u32 = 30;

fn window() -> Conf {
    Conf {
        window_title: "Kingdom boss throwback fixtures".into(),
        window_width: 400,
        window_height: 240,
        high_dpi: false,
        ..Default::default()
    }
}

fn isolated(world: &WorldData, seed: u32) -> Session {
    let mut s = Session::new(
        world.clone(),
        0,
        seed,
        game::progress::Progress::everything(),
    );
    s.game.level = Level::empty(-80.0, 160.0);
    s.game.population = Default::default();
    s.game.greed = Default::default();
    s.game.siege = Default::default();
    s.game.wildlife = Default::default();
    s.game.mounts = Default::default();
    s.game.hermits = Default::default();
    s.game.merchants = Default::default();
    s.game.treasure = Default::default();
    s.game.teleports = Default::default();
    s.game.dogs = Default::default();
    s.game.harbor = Default::default();
    s.game.forests.clear();
    s.game.tutorial = game::tutorial::Tutorial::new(world, &mut s.game.level, false);
    s.game.monarch.x = -45.0;
    s.debug = false;
    s
}

fn tick(s: &mut Session, events: &mut Vec<GameEvent>) {
    s.game.tick(&s.world, &Controls::default(), events);
    s.t = s.game.ticks as f32 * TICK;
}

fn lodge_stone(world: &WorldData, side: i8) -> (Session, u32) {
    let counter_proto = world
        .protos
        .iter()
        .position(|p| p.workshop_side.is_some_and(|(dir, _)| dir == side))
        .expect("workshop counter for this side");
    let p = &world.protos[counter_proto];
    // Keep the authored chance of lodging. Retry bounded seeds because a
    // friendly shot can legitimately burst on its second ground contact.
    for seed in 1..=16 {
        let mut s = isolated(world, seed);
        let counter = s.game.level.add(level::instantiate(
            world,
            counter_proto as u16,
            Layer::Game,
            0.0,
            1.0,
            p.root_z,
            1.0,
        ));
        s.game.siege.order(world, &mut s.game.level, counter);
        let catapult = s.game.siege.catapults[0].object;
        let ci = s.game.level.index_of(catapult).unwrap();
        let o = &mut s.game.level.objects[ci];
        o.build = None;
        o.mode = mode::RELOADING;
        let mut events = Vec::new();
        for _ in 0..120 {
            tick(&mut s, &mut events);
            events.clear();
        }
        let o = &mut s.game.level.objects[ci];
        o.mode = mode::FIRING;
        o.aim = 68.0;
        for _ in 0..360 {
            tick(&mut s, &mut events);
            events.clear();
            if s.game.siege.boulders.iter().any(|b| b.stuck.is_some()) {
                assert_eq!(s.game.siege.boulders.len(), 1);
                s.game.siege.remove(catapult);
                assert!(s.game.level.remove_any(catapult));
                assert!(s.game.level.remove_any(counter));
                return (s, seed);
            }
        }
    }
    panic!("no friendly boulder lodged in 16 seeded shots");
}

#[macroquad::main(window)]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    assert_eq!(args.len(), 3, "usage: boss_throwback <assets> <output>");
    let dir = Path::new(&args[1]);
    let out = Path::new(&args[2]);
    std::fs::create_dir_all(out).unwrap();
    let world = game::assetfile::decode_world(&std::fs::read(dir.join("world.bin")).unwrap())
        .expect("converted PC world");
    let mut renderer = assets::Assets::load(dir, &world).await.unwrap();
    let sheet = renderer.sheet.clone();
    let release_time = world
        .animations
        .clip("boss_throw")
        .and_then(|clip| clip.first_event_time(EventKind::BoulderRelease))
        .expect("authored boulder release marker")
        / world
            .animations
            .state_speed("boss", "Throw")
            .expect("authored Throw state speed");
    let mut receipts = Vec::new();
    let mut summaries = Vec::new();
    for (sequence, side, kind, frames, kill) in [
        ("throw_right", 1_i8, EnemyKind::Boss, 80, false),
        ("throw_left", -1_i8, EnemyKind::KillerBoss, 80, false),
        ("death_interrupt", 1_i8, EnemyKind::Boss, 60, true),
    ] {
        let (mut s, seed) = lodge_stone(&world, side);
        let dir = f32::from(side);
        let x = s.game.siege.boulders[0].x - dir * world.boss_throw.hand[0];
        // Hold the campfire goal beside the stone while the scanner warms up.
        // A crowned monarch would become the next Chase target at the center.
        s.game.campfire = x;
        s.game.monarch.x = x - dir * 15.0;
        s.game.has_crown = false;
        // Aim the normal crown camera into the throw lane. The crown stays
        // well away from the monarch and the boss cannot pick it up.
        s.game.greed.drop_crown(x + dir * 3.0, 0.0, 0.0);
        let boss = s.game.greed.spawn_troll(kind, x, 0, true, &mut s.game.rng);
        s.game.greed.trolls[0].mover.facing = side;
        s.resume(s.game.clone());
        let start_tick = s.game.ticks;
        let mut events = Vec::new();
        let mut pickup_frame = None;
        let mut released_frame = None;
        let mut impact_frame = None;
        let mut fragments_frame = None;
        let mut burst_frame = None;
        let mut death_frame = None;
        let mut caught_id = None;
        let mut peak_dust = 0;
        let mut peak_fragments = 0;
        let mut peak_shake = 0.0_f32;
        let mut saw_shake_request = false;
        for frame in 0..frames {
            if kill && pickup_frame.is_some_and(|picked| frame == picked + 22) {
                s.game.greed.hurt(0, 1000, &mut |_| {});
                death_frame = Some(frame);
            }
            let actor = s.game.greed.trolls.iter().find(|t| t.id == boss).unwrap();
            let boulder = s.game.siege.boulders.first();
            if let Some(b) = boulder.filter(|b| b.runtime.held_by == Some(boss)) {
                pickup_frame.get_or_insert(frame);
                caught_id.get_or_insert(b.runtime.id);
            }
            if let Some(b) = boulder.filter(|b| b.runtime.hostile) {
                assert_eq!(
                    Some(b.runtime.id),
                    caught_id,
                    "the boss relaunches the same stone"
                );
                assert!(b.vx * dir > 0.0, "the thrown stone follows boss facing");
            }
            let released = events
                .iter()
                .any(|e| matches!(e, GameEvent::Greed(GreedEvent::BoulderThrown { .. })));
            if released {
                assert!(released_frame.replace(frame).is_none(), "released twice");
            }
            let impact = events.iter().find_map(|e| match *e {
                GameEvent::BoulderImpact { x, y, side, .. } => Some(([x, y], side)),
                _ => None,
            });
            if let Some(([impact_x, _], impact_side)) = impact {
                assert!(impact_frame.replace(frame).is_none(), "impact twice");
                assert_eq!(impact_side, side, "impact dust is mirrored");
                assert!(
                    (impact_x - x) * dir > 0.0,
                    "impact follows the throw direction"
                );
            }
            let fragments = events.iter().find_map(|e| match *e {
                GameEvent::BoulderFragments { x, y, side, .. } => Some(([x, y], side)),
                _ => None,
            });
            if let Some((_, fragment_side)) = fragments {
                assert!(fragments_frame.replace(frame).is_none(), "fragmented twice");
                assert_eq!(fragment_side, side, "fragments are mirrored");
            }
            let burst = events
                .iter()
                .any(|e| matches!(e, GameEvent::BoulderBurst { .. }));
            if burst {
                assert!(burst_frame.replace(frame).is_none(), "burst twice");
            }
            let shake_requested = impact.is_some()
                || events
                    .iter()
                    .any(|e| matches!(e, GameEvent::CameraShake { .. }));
            let shake = s.camera_shake_offset();
            let (mut dust_samples, mut fragment_samples) = (0, 0);
            for particle in s.boulder_particles.samples(&s.world) {
                assert!(
                    sheet.world(particle.sprite).is_some(),
                    "particle sprite is packed"
                );
                if particle.color[3] <= 0.0 || particle.scale <= 0.0 {
                    continue;
                }
                match particle.kind {
                    BoulderParticleKind::Dust => dust_samples += 1,
                    BoulderParticleKind::Fragments => fragment_samples += 1,
                }
            }
            saw_shake_request |= shake_requested;
            peak_dust = peak_dust.max(dust_samples);
            peak_fragments = peak_fragments.max(fragment_samples);
            peak_shake = peak_shake.max(shake[0].abs().max(shake[1].abs()));
            if impact_frame.is_none() {
                assert_eq!(
                    dust_samples + fragment_samples,
                    0,
                    "no particles before contact"
                );
                assert_eq!(shake, [0.0, 0.0], "no shake before contact");
            }

            ::scene::top::draw(&s, &sheet, &mut renderer);
            let file = format!("frame-{:04}.png", receipts.len());
            let image = get_screen_data();
            assert_eq!((image.width, image.height), (400, 240));
            image.export_png(out.join(&file).to_str().unwrap());
            receipts.push(serde_json::json!({
                "file": file,
                "sequence": sequence,
                "frame": frame,
                "time": (s.game.ticks - start_tick) as f32 * TICK,
                "ticks": s.game.ticks,
                "boss": {
                    "id": boss,
                    "x": actor.x,
                    "y": actor.y,
                    "facing": actor.mover.facing,
                    "state": actor.state,
                    "throw_clip_time": actor.boss_throw_animation_time(&world),
                },
                "boulder": boulder.map(|b| serde_json::json!({
                    "id": b.runtime.id,
                    "pose": [b.x, b.y, b.vx, b.vy, b.spin],
                    "stuck": b.stuck.is_some(),
                    "held_by": b.runtime.held_by,
                    "hostile": b.runtime.hostile,
                })),
                "released": released,
                "impact": impact,
                "fragments": fragments,
                "burst": burst,
                "dust_samples": dust_samples,
                "fragment_samples": fragment_samples,
                "camera_x": s.camera_x,
                "camera_shake": shake,
                "shake_requested": shake_requested,
                "injected_death": death_frame == Some(frame),
            }));
            events.clear();
            for _ in 0..2 {
                s.step_ticks(RawInput::default(), 1);
                events.extend_from_slice(&s.events);
            }
            next_frame().await;
        }
        let pickup = pickup_frame.expect("boss must pick up the lodged stone");
        if kill {
            assert!(death_frame.is_some_and(|dead| dead > pickup));
            assert!(matches!(
                s.game.greed.trolls[0].state,
                TrollState::Dead { .. }
            ));
            assert_eq!(released_frame, None, "death must interrupt release");
            assert!(impact_frame.is_none() && fragments_frame.is_none() && burst_frame.is_none());
            assert!(!saw_shake_request);
        } else {
            let release = released_frame.expect("boss must release the held stone");
            let burst = burst_frame.expect("returned boulder must shatter");
            assert_eq!(impact_frame, Some(burst), "physical burst emits dust");
            assert_eq!(fragments_frame, Some(burst), "burst emits fragments");
            assert!(
                saw_shake_request && peak_shake > 0.0,
                "near impact shakes the camera"
            );
            assert!(
                peak_dust > 0 && peak_fragments > 0,
                "both effects reach the renderer"
            );
            assert!(pickup < release && release < burst);
            let sampled = (release - pickup) as f32 / FPS as f32;
            assert!((sampled - release_time).abs() <= 1.0 / FPS as f32 + TICK);
        }
        summaries.push(serde_json::json!({
            "sequence": sequence,
            "seed": seed,
            "side": side,
            "stage_ticks": start_tick,
            "pickup_frame": pickup,
            "release_frame": released_frame,
            "impact_frame": impact_frame,
            "fragments_frame": fragments_frame,
            "burst_frame": burst_frame,
            "peak_dust_samples": peak_dust,
            "peak_fragment_samples": peak_fragments,
            "peak_shake": peak_shake,
            "death_frame": death_frame,
        }));
    }
    assert_eq!(receipts.len(), 220);
    let report = serde_json::json!({
        "method": "Authored PC assets, staged catapult shot, boss and crown placement, production 60 Hz Session ticks sampled at 30 Hz with normal crown camera and effect handling, no audio or storage",
        "width": 400,
        "height": 240,
        "fps": FPS,
        "release_marker_seconds": release_time,
        "sequences": summaries,
        "frames": receipts,
    });
    std::fs::write(
        out.join("capture.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
}
