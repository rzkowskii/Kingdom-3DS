//! Capture the authored sky clouds through a boss wave and its dawn fade.
//! cargo run -p desktop --example clouds -- <assets> <output>

#[path = "../src/assets.rs"]
mod assets;

use ::scene::{
    clouds::{CloudKind, CloudSample},
    render::Color as Tint,
    session::Session,
    top,
};
use game::controls::RawInput;
use game::data::{Layer, WorldData};
use game::director::{Season, WaveKind};
use game::greed::GreedEvent;
use game::level::instantiate;
use game::lighting::{color, float, Light};
use game::progress::Progress;
use game::state::GameEvent;
use game::time::{DayCounter, DayEvent, DAY_START};
use macroquad::prelude::*;
use serde_json::{json, Value};
use std::path::Path;

const WIDTH: u32 = 400;
const HEIGHT: u32 = 240;
const SEED: u32 = 17;
const NIGHT: &str = "Cloudy Autumn Night";
const WARMUP: u32 = 16 * 60;

fn window() -> Conf {
    Conf {
        window_title: "Kingdom cloud checkpoints".into(),
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
        .position(|light| light.name == NIGHT)
        .expect("authored cloudy night") as u16;
    let mut s = Session::new(world, 0, SEED, Progress::everything());
    s.game.clock.day = 1;
    s.game.clock.time = 0.995;
    s.game.clock.speed = 0.0;
    s.game.days = DayCounter::new(&s.game.clock);
    let input = RawInput::default();
    s.step_ticks(input, 1);

    // Keep the ground and low details while opening a view of the sky.
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
    s.game.blender.add(&s.world, night, 0.0);
    s.step_ticks(input, WARMUP);
    s.game.greed = Default::default();

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
    assert!(matching.next().is_none(), "one red moon fader");
    weight
}

fn rgba(c: Tint) -> [f32; 4] {
    [c.r, c.g, c.b, c.a]
}

fn near(actual: f32, expected: f32) {
    assert!((actual - expected).abs() < 0.0001, "{actual} != {expected}");
}

fn assert_clouds(
    boss: &Session,
    control: &Session,
    preset: u16,
    weight: f32,
) -> (Vec<CloudSample>, Vec<CloudSample>) {
    let a = &boss.game.blender.state;
    let b = &control.game.blender.state;
    let p = &boss.world.lights[preset as usize];
    assert_eq!(a.colors[color::AMBIENT], b.colors[color::AMBIENT]);
    assert_eq!(a.colors[color::CLOUD_BASE], b.colors[color::CLOUD_BASE]);
    for field in [float::CLOUD_DENSITY, float::CLOUD_STRATUS_DENSITY] {
        assert_eq!(a.floats[field], b.floats[field]);
    }
    for field in [color::CLOUD_HIGHLIGHT, color::CLOUD_STRATUS] {
        for ch in 0..4 {
            let base = b.colors[field][ch];
            near(
                a.colors[field][ch],
                base + (p.colors[field][ch] - base) * weight,
            );
        }
    }

    let boss_clouds: Vec<_> = boss.clouds.samples(&boss.world, a).collect();
    let control_clouds: Vec<_> = control.clouds.samples(&control.world, b).collect();
    assert_eq!(boss_clouds.len(), control_clouds.len());
    for kind in [CloudKind::Stratus, CloudKind::Cumulus] {
        assert!(
            boss_clouds.iter().any(|cloud| cloud.kind == kind),
            "{kind:?}"
        );
    }
    for (red, plain) in boss_clouds.iter().zip(&control_clouds) {
        assert_eq!(red.kind, plain.kind);
        assert_eq!(red.sprite, plain.sprite);
        assert_eq!(red.position, plain.position);
        assert_eq!(red.scale, plain.scale);
        assert_eq!(red.flip, plain.flip);
        let (base, highlight) = match red.kind {
            CloudKind::Stratus => (color::CLOUD_STRATUS, [0.0; 4]),
            CloudKind::Cumulus => (color::CLOUD_BASE, a.colors[color::CLOUD_HIGHLIGHT]),
        };
        let opacity = plain.tint.a / b.colors[base][3];
        assert!(opacity > 0.0 && opacity <= 1.0);
        near(red.tint.a, a.colors[base][3] * opacity);
        for (channel, actual) in rgba(red.tint)[..3].iter().enumerate() {
            near(
                *actual,
                a.colors[base][channel] * a.colors[color::AMBIENT][channel],
            );
        }
        assert_eq!(rgba(red.light), highlight);
    }
    (boss_clouds, control_clouds)
}

fn camera(rt: &RenderTarget) -> Camera2D {
    let mut camera = Camera2D::from_display_rect(Rect::new(0.0, 0.0, WIDTH as f32, HEIGHT as f32));
    camera.zoom.y = -camera.zoom.y;
    camera.render_target = Some(rt.clone());
    camera
}

struct Capture {
    renderer: assets::Assets,
    screen: RenderTarget,
    previous: RenderTarget,
}

impl Capture {
    fn render(&mut self, s: &Session) -> Image {
        let sheet = self.renderer.sheet.clone();
        self.renderer.previous = None;
        set_camera(&camera(&self.screen));
        top::draw(s, &sheet, &mut self.renderer);
        set_camera(&camera(&self.previous));
        draw_texture(&self.screen.texture, 0.0, 0.0, WHITE);
        top::draw_reflection_extras(s, &sheet, &mut self.renderer);
        self.renderer.previous = Some(self.previous.texture.clone());
        set_camera(&camera(&self.screen));
        top::draw(s, &sheet, &mut self.renderer);
        set_default_camera();
        clear_background(BLACK);
        draw_texture(&self.screen.texture, 0.0, 0.0, WHITE);
        let image = get_screen_data();
        assert_eq!((image.width, image.height), (WIDTH as u16, HEIGHT as u16));
        image
    }

    async fn save(
        &mut self,
        s: &mut Session,
        control: &Light,
        path: &Path,
        unshadowed_path: Option<&Path>,
    ) -> (usize, [usize; 2], usize) {
        let image = self.render(s);
        self.renderer.cloud_shadows_enabled = false;
        let unshadowed = self.render(s);
        self.renderer.cloud_shadows_enabled = true;
        let shadow_pixels = shadowed_sky_pixels(&image, &unshadowed);
        if let Some(path) = unshadowed_path {
            unshadowed.export_png(path.to_str().expect("capture path is UTF-8"));
        }
        let defs = s.world.clouds.take().expect("authored cloud decks");
        let cloud_pixels = changed_sky_pixels(&image, &self.render(s));
        s.world.clouds = Some(defs);
        assert!(cloud_pixels > 0, "clouds affect the rendered sky");
        let tint_pixels = [color::CLOUD_HIGHLIGHT, color::CLOUD_STRATUS].map(|field| {
            let current = s.game.blender.state.colors[field];
            if current == control.colors[field] {
                return 0;
            }
            s.game.blender.state.colors[field] = control.colors[field];
            let changed = changed_sky_pixels(&image, &self.render(s));
            s.game.blender.state.colors[field] = current;
            changed
        });
        image.export_png(path.to_str().expect("capture path is UTF-8"));
        next_frame().await;
        (cloud_pixels, tint_pixels, shadow_pixels)
    }
}

fn changed_sky_pixels(a: &Image, b: &Image) -> usize {
    // get_screen_data stores rows bottom-up. Compare the upper half, where
    // water reflection and foreground lighting cannot contribute.
    let start = (WIDTH * (HEIGHT / 2) * 4) as usize;
    a.bytes[start..]
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.bytes[start..].as_chunks::<4>().0)
        .filter(|(a, b)| a[..3] != b[..3])
        .count()
}

fn shadowed_sky_pixels(shadowed: &Image, unshadowed: &Image) -> usize {
    let start = (WIDTH * (HEIGHT / 2) * 4) as usize;
    shadowed.bytes[start..]
        .as_chunks::<4>()
        .0
        .iter()
        .zip(unshadowed.bytes[start..].as_chunks::<4>().0)
        .filter(|(a, b)| {
            assert_eq!(a[3], b[3], "exposure must preserve coverage");
            assert!(
                (0..3).all(|c| a[c] <= b[c]),
                "exposure only attenuates the additive rim: {a:?} > {b:?}"
            );
            a[..3] != b[..3]
        })
        .count()
}

fn cloud_receipt(clouds: &[CloudSample], kind: CloudKind) -> Value {
    let mut matching = clouds.iter().filter(|c| c.kind == kind);
    let first = matching.next().expect("cloud sample");
    json!({
        "count": matching.count() + 1,
        "first_sprite": first.sprite,
        "first_position": first.position,
        "first_scale": first.scale,
        "first_flip": first.flip,
        "first_tint": rgba(first.tint),
        "first_light": rgba(first.light),
    })
}

#[macroquad::main(window)]
async fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    assert_eq!(args.len(), 3, "usage: clouds <assets> <output>");
    let dir = Path::new(&args[1]);
    let out = Path::new(&args[2]);
    std::fs::create_dir_all(out).unwrap();
    let world = game::assetfile::decode_world(&std::fs::read(dir.join("world.bin")).unwrap())
        .expect("converted PC world");
    let preset = world.red_moon.expect("EnemyManager.redMoon preset");
    let mut capture = Capture {
        renderer: assets::Assets::load(dir, &world).await.unwrap(),
        screen: render_target(WIDTH, HEIGHT),
        previous: render_target(WIDTH, HEIGHT),
    };
    for target in [&capture.screen, &capture.previous] {
        target.texture.set_filter(FilterMode::Nearest);
    }

    let (mut boss, portal) = stage(&world, true);
    let (mut control, control_portal) = stage(&world, false);
    assert_eq!(portal, control_portal);
    let input = RawInput::default();
    let mut admission_tick = None;
    let mut frames = Vec::new();
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
                for s in [&mut boss, &mut control] {
                    s.game.clock.speed = 0.0;
                    s.step_ticks(input, 300);
                }
                assert!((overlay(&boss, preset).unwrap() - 0.5).abs() < 0.004);
            }
            "full" => {
                boss.step_ticks(input, 610);
                control.step_ticks(input, 610);
                assert_eq!(overlay(&boss, preset), Some(1.0));
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
        let weight = overlay(&boss, preset).unwrap_or(0.0);
        let (boss_clouds, control_clouds) = assert_clouds(&boss, &control, preset, weight);
        let control_light = control.game.blender.state;
        for (s, clouds, suffix) in [
            (&mut boss, boss_clouds, "boss"),
            (&mut control, control_clouds, "control"),
        ] {
            let file = format!("{phase}-{suffix}.png");
            let unshadowed =
                (phase == "full").then(|| out.join(format!("full-{suffix}-unshadowed.png")));
            let (cloud_pixels, [highlight_pixels, stratus_pixels], shadow_pixels) = capture
                .save(s, &control_light, &out.join(&file), unshadowed.as_deref())
                .await;
            if phase == "full" {
                assert!(shadow_pixels > 0, "{suffix} clouds occlude visible rims");
            }
            if suffix == "boss" && phase == "full" {
                assert!(
                    highlight_pixels > 0,
                    "cumulus highlight affects the rendered sky"
                );
                assert!(stratus_pixels > 0, "stratus tint affects the rendered sky");
            }
            frames.push(json!({
                "file": file,
                "phase": phase,
                "boss_wave_flag": suffix == "boss",
                "ticks": s.game.ticks,
                "red_moon_weight": overlay(s, preset),
                "camera_x": s.camera_x,
                "stratus": cloud_receipt(&clouds, CloudKind::Stratus),
                "cumulus": cloud_receipt(&clouds, CloudKind::Cumulus),
                "cloud_pixels": cloud_pixels,
                "shadow_pixels": shadow_pixels,
                "cumulus_highlight_pixels": highlight_pixels,
                "stratus_tint_pixels": stratus_pixels,
            }));
        }
    }
    let report = json!({
        "weather": NIGHT,
        "warmup_ticks": WARMUP,
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
