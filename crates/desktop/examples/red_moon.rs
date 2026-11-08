//! Red moon checkpoints through normal portal admission and session ticks.
//! cargo run -p desktop --example red_moon -- <assets> <output>

#[path = "../src/assets.rs"]
mod assets;

use ::scene::{session::Session, top};
use game::controls::RawInput;
use game::data::{Layer, WorldData};
use game::director::{Season, WaveKind};
use game::greed::GreedEvent;
use game::level::instantiate;
use game::lighting::{color, float};
use game::progress::Progress;
use game::state::GameEvent;
use game::time::{DayCounter, DayEvent, DAY_START};
use macroquad::prelude::*;
use serde_json::{json, Value};
use std::path::Path;

const WIDTH: u32 = 400;
const HEIGHT: u32 = 240;
const SEED: u32 = 17;

fn window() -> Conf {
    Conf {
        window_title: "Kingdom red moon checkpoints".into(),
        window_width: WIDTH as i32,
        window_height: HEIGHT as i32,
        high_dpi: false,
        ..Default::default()
    }
}

fn stage(world: &WorldData, boss: bool) -> (Session, u32) {
    let mut world = world.clone();
    let wave = world.levels[0].waves[1][0].expect("summer boss wave");
    assert!(world.waves[wave as usize].boss_wave);
    assert!(!world.waves[wave as usize].both_sides);
    world.waves[wave as usize].boss_wave = boss;
    world.levels[0].waves[0] = [None; 3];
    world.levels[0].waves[2] = [None; 3];
    let night = world
        .lights
        .iter()
        .position(|light| light.name == "Clear Summer Night")
        .expect("clear summer night") as u16;
    let mut s = Session::new(world, 0, SEED, Progress::everything());
    s.game.clock.day = 1;
    s.game.clock.time = 0.995;
    s.game.clock.speed = 0.0;
    s.game.days = DayCounter::new(&s.game.clock);
    // Let the director apply the events leading to the staged night, then
    // select the same authored weather preset in both branches.
    s.step_ticks(RawInput::default(), 1);
    s.game.blender.add(&s.world, night, 0.0);
    s.step_ticks(RawInput::default(), 360);
    s.game.greed = Default::default();
    // Open a view of the moon in both branches. Keep the generated game
    // layer's terrain and low details, and drop the tall tree scenery.
    let visible_ground = |o: &game::level::Object| {
        let proto = &s.world.protos[o.proto as usize];
        o.layer == Layer::Game
            && proto.portal.is_none()
            && proto.tree.is_none()
            && proto.tree_work.is_none()
            && !proto
                .forest_item
                .is_some_and(|forest| forest.controls_forest_size)
    };
    s.game.level.objects.retain(visible_ground);
    s.game.level.scenery.retain(visible_ground);
    s.game.forests.clear();
    let portal_proto = s
        .world
        .protos
        .iter()
        .position(|p| p.portal.is_some_and(|portal| !portal.cliff))
        .expect("regular portal prefab") as u16;
    let proto = &s.world.protos[portal_proto as usize];
    let portal_x = s.game.campfire + 32.0;
    let portal = s.game.level.add(instantiate(
        &s.world,
        portal_proto,
        Layer::Game,
        portal_x,
        1.0,
        proto.root_z,
        1.0,
    ));
    // With no travel between the staged border and portal, the normal
    // scheduler admits the previous day's wave at 01:00.
    s.game.borders.right = portal_x;
    s.game.greed.schedule(
        &s.world,
        &s.game.level,
        0,
        WaveKind::Boss,
        Season::Summer,
        0,
        s.game.campfire,
        s.game.borders,
        &mut s.game.rng,
    );
    s.game.clock.speed = 1.0;
    (s, portal)
}

fn overlay(s: &Session, preset: u16) -> Option<f32> {
    let mut matching = s.game.blender.presets().filter(|(id, _)| *id == preset);
    let weight = matching.next().map(|(_, weight)| weight);
    assert!(
        matching.next().is_none(),
        "one red moon fader in this fixture"
    );
    weight
}

fn assert_palette(boss: &Session, control: &Session, preset: u16, weight: f32) {
    let a = boss.game.blender.state;
    let b = control.game.blender.state;
    let p = &boss.world.lights[preset as usize];
    for (i, (boss_color, base_color)) in a.colors.iter().zip(b.colors).enumerate() {
        for (channel, (&actual, base)) in boss_color.iter().zip(base_color).enumerate() {
            let expected = if p.enabled & (1 << i) != 0 {
                base + (p.colors[i][channel] - base) * weight
            } else {
                base
            };
            assert!((actual - expected).abs() < 0.0001, "color {i}:{channel}");
        }
    }
    for (i, (&actual, base)) in a.floats.iter().zip(b.floats).enumerate() {
        let expected = if p.enabled & (1 << (a.colors.len() + i)) != 0 {
            base + (p.floats[i] - base) * weight
        } else {
            base
        };
        assert!((actual - expected).abs() < 0.0001, "float {i}");
    }
}

fn camera(rt: &RenderTarget) -> Camera2D {
    let mut camera = Camera2D::from_display_rect(Rect::new(0.0, 0.0, WIDTH as f32, HEIGHT as f32));
    // Match the desktop's bottom-up render targets.
    camera.zoom.y = -camera.zoom.y;
    camera.render_target = Some(rt.clone());
    camera
}

async fn capture(
    s: &Session,
    renderer: &mut assets::Assets,
    targets: [&RenderTarget; 2],
    out: &Path,
    file: &str,
) {
    let [screen, previous] = targets;
    let sheet = renderer.sheet.clone();
    renderer.previous = None;
    // Seed a previous frame of the same checkpoint for the normal water
    // reflection pass. Reflection-only geometry stays off the main screen.
    set_camera(&camera(screen));
    top::draw(s, &sheet, renderer);
    set_camera(&camera(previous));
    draw_texture(&screen.texture, 0.0, 0.0, WHITE);
    top::draw_reflection_extras(s, &sheet, renderer);
    renderer.previous = Some(previous.texture.clone());
    set_camera(&camera(screen));
    top::draw(s, &sheet, renderer);
    set_default_camera();
    clear_background(BLACK);
    draw_texture(&screen.texture, 0.0, 0.0, WHITE);
    let image = get_screen_data();
    assert_eq!((image.width, image.height), (WIDTH as u16, HEIGHT as u16));
    image.export_png(out.join(file).to_str().expect("capture path is UTF-8"));
    next_frame().await;
}

fn receipt(s: &Session, file: &str, phase: &str, boss: bool, preset: u16) -> Value {
    let light = s.game.blender.state;
    json!({
        "file": file,
        "phase": phase,
        "boss_wave_flag": boss,
        "day": s.game.clock.day,
        "hour": s.game.clock.time,
        "ticks": s.game.ticks,
        "red_moon_weight": overlay(s, preset),
        "moon_tint": light.colors[color::MOON_TINT],
        "moon_glow": light.colors[color::MOON_GLOW],
        "stars": light.floats[float::STARS],
        "visible_stars": s.visible_stars(),
        "ambient": light.colors[color::AMBIENT],
        "sky_top": light.colors[color::SKY_TOP],
        "sky_bottom": light.colors[color::SKY_BOTTOM],
        "water": light.colors[color::WATER],
        "rain": light.floats[float::RAIN],
        "snow": light.floats[float::SNOW],
    })
}

#[macroquad::main(window)]
async fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    assert_eq!(args.len(), 3, "usage: red_moon <assets> <output>");
    let dir = Path::new(&args[1]);
    let out = Path::new(&args[2]);
    std::fs::create_dir_all(out).unwrap();
    let world = game::assetfile::decode_world(&std::fs::read(dir.join("world.bin")).unwrap())
        .expect("converted PC world");
    let preset = world.red_moon.expect("EnemyManager.redMoon preset");
    let mut renderer = assets::Assets::load(dir, &world).await.unwrap();
    let screen = render_target(WIDTH, HEIGHT);
    let previous = render_target(WIDTH, HEIGHT);
    for target in [&screen, &previous] {
        target.texture.set_filter(FilterMode::Nearest);
    }
    let (mut boss, portal) = stage(&world, true);
    let (mut control, control_portal) = stage(&world, false);
    assert_eq!(portal, control_portal);
    let input = RawInput::default();
    let mut frames = Vec::new();
    let mut admission_tick = None;
    for phase in ["before", "fade-in", "full", "dawn-fade", "dawn-cleared"] {
        match phase {
            "before" => assert_eq!(overlay(&boss, preset), None),
            "fade-in" => {
                for _ in 0..8 {
                    boss.step_ticks(input, 1);
                    control.step_ticks(input, 1);
                    let accepted = |s: &Session| {
                        s.events.iter().any(|e| {
                            matches!(e, GameEvent::Greed(GreedEvent::BossWaveSpawned { portal: id }) if *id == portal)
                        })
                    };
                    assert!(!accepted(&control));
                    if accepted(&boss) {
                        admission_tick = Some(boss.game.ticks);
                        break;
                    }
                }
                assert!(admission_tick.is_some(), "scheduled boss wave accepted");
                boss.game.clock.speed = 0.0;
                control.game.clock.speed = 0.0;
                boss.step_ticks(input, 300);
                control.step_ticks(input, 300);
                assert!((overlay(&boss, preset).unwrap() - 0.5).abs() < 0.004);
            }
            "full" => {
                boss.step_ticks(input, 610);
                control.step_ticks(input, 610);
                assert_eq!(overlay(&boss, preset), Some(1.0));
                assert_eq!(boss.visible_stars(), 0);
                assert!(control.visible_stars() > 0);
            }
            "dawn-fade" => {
                for s in [&mut boss, &mut control] {
                    s.game.clock.time = DAY_START + 0.001;
                    s.step_ticks(input, 1);
                    assert!(s
                        .events
                        .iter()
                        .any(|e| matches!(e, GameEvent::Day(DayEvent::Started { .. }))));
                    s.step_ticks(input, 300);
                }
                assert!((overlay(&boss, preset).unwrap() - 0.5).abs() < 0.004);
            }
            "dawn-cleared" => {
                boss.step_ticks(input, 310);
                control.step_ticks(input, 310);
                assert_eq!(overlay(&boss, preset), None);
            }
            _ => unreachable!(),
        }
        assert_eq!(overlay(&control, preset), None);
        assert_palette(
            &boss,
            &control,
            preset,
            overlay(&boss, preset).unwrap_or(0.0),
        );
        for (s, enabled, suffix) in [(&boss, true, "boss"), (&control, false, "control")] {
            let file = format!("{phase}-{suffix}.png");
            capture(s, &mut renderer, [&screen, &previous], out, &file).await;
            frames.push(receipt(s, &file, phase, enabled, preset));
        }
    }
    let report = json!({
        "method": "Converted PC assets, generated game-layer terrain with the tree canopy removed, staged clear night, one portal and border, production wave scheduling and 60 Hz Session ticks. Night clock holds during the fade. Staged dawn crosses the actual DayCounter boundary. Control changes the wave's boss flag.",
        "seed": SEED,
        "width": WIDTH,
        "height": HEIGHT,
        "red_moon_preset": world.lights[preset as usize].name,
        "portal": portal,
        "admission_tick": admission_tick,
        "frames": frames,
    });
    std::fs::write(
        out.join("capture.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
}
