//! Render the authored atmosphere at one game tick and save two Azahar fixtures.
//! cargo run -p desktop --example atmosphere -- <assets> <output> [--saves-only]

#[path = "../src/assets.rs"]
mod assets;

use std::path::{Path, PathBuf};

use ::scene::atmosphere_data::{AtmosphereDef, FILE_NAME};
use ::scene::clouds::AtmosphereKind;
use ::scene::{session::Session, top};
use game::controls::RawInput;
use game::data::WorldData;
use game::lighting::{color, float};
use game::progress::Progress;
use game::save::{fingerprint, load_game, save_game};
use game::time::DayCounter;
use macroquad::prelude::*;
use serde_json::{json, Value};

const WIDTH: u32 = 400;
const HEIGHT: u32 = 240;
const FOGGY: &str = "Foggy Summer Evening";
const CLEAR: &str = "Clear Summer Evening";
const SEED: u32 = 17;
const HOUR: f32 = 19.5;
const SETTLE_TICKS: u32 = 4 * 60;
const WARMUP_TICKS: u32 = 38 * 60;

fn window() -> Conf {
    Conf {
        window_title: "Kingdom fog and mist checkpoints".into(),
        window_width: WIDTH as i32,
        window_height: HEIGHT as i32,
        high_dpi: false,
        ..Default::default()
    }
}

fn stage(world: &WorldData, name: &str) -> (Session, u16) {
    let preset = world
        .lights
        .iter()
        .position(|light| light.name == name)
        .expect("authored evening weather") as u16;
    let mut s = Session::new(world.clone(), 0, SEED, Progress::everything());
    s.game.clock.day = 1;
    s.game.clock.time = HOUR;
    s.game.clock.speed = 0.0;
    s.game.days = DayCounter::new(&s.game.clock);
    s.god = true;
    let input = RawInput::default();
    s.step_ticks(input, 1);
    s.game.greed = Default::default();
    s.game.blender.add(&s.world, preset, 0.0);
    s.step_ticks(input, SETTLE_TICKS);
    assert_eq!(s.game.clock.time, HOUR);
    (s, preset)
}

fn save_fixtures(world: &WorldData, world_id: u32, out: &Path) -> Value {
    let mut reports = Vec::new();
    for (label, name) in [("foggy", FOGGY), ("clear", CLEAR)] {
        let (s, preset) = stage(world, name);
        let bytes = save_game(&s.game, world_id);
        let loaded = load_game(&bytes, world_id, world).expect("round-trip fixture");
        assert!(loaded
            .blender
            .presets()
            .any(|(id, w)| id == preset && w == 1.0));
        assert_eq!(loaded.clock.time, HOUR);
        assert_eq!(loaded.clock.speed, 0.0);
        let light = &loaded.blender.state;
        let [fog, mist] = [
            light.floats[float::FOG_DENSITY],
            light.floats[float::MIST_DENSITY],
        ];
        if label == "foggy" {
            assert!(fog > 0.0 && mist > 0.0);
        } else {
            assert_eq!([fog, mist], [0.0, 0.0]);
        }
        let folder = out.join("azahar-fixtures").join(label);
        std::fs::create_dir_all(&folder).expect("create fixture folder");
        let path = folder.join("game.sav");
        std::fs::write(&path, bytes).expect("write fixture");
        reports.push(json!({
            "path": path,
            "weather": name,
            "hour": loaded.clock.time,
            "seed": SEED,
            "fog_density": fog,
            "mist_density": mist,
        }));
    }
    json!({"world_fingerprint": format!("{world_id:08x}"), "fixtures": reports})
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
}

fn changed_pixels(a: &Image, b: &Image, first_pixels: usize) -> usize {
    a.bytes
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.bytes.as_chunks::<4>().0)
        .take(first_pixels)
        .filter(|(a, b)| a[..3] != b[..3])
        .count()
}

async fn capture(world: WorldData, dir: PathBuf, out: PathBuf) {
    let mut capture = Capture {
        renderer: assets::Assets::load(&dir, &world)
            .await
            .expect("load desktop assets"),
        screen: render_target(WIDTH, HEIGHT),
        previous: render_target(WIDTH, HEIGHT),
    };
    for target in [&capture.screen, &capture.previous] {
        target.texture.set_filter(FilterMode::Nearest);
    }

    let (mut s, _) = stage(&world, FOGGY);
    s.atmosphere = AtmosphereDef::load(&dir.join(FILE_NAME), &capture.renderer.sheet)
        .expect("read atmosphere sidecar");
    let input = RawInput::default();
    s.step_ticks(input, WARMUP_TICKS);
    assert_eq!(s.game.clock.time, HOUR);
    let def = s.atmosphere.as_ref().expect("authored fog and mist");
    let counts = [AtmosphereKind::Fog, AtmosphereKind::Mist].map(|kind| {
        s.clouds
            .atmosphere_samples(def, &s.game.blender.state)
            .filter(|p| p.kind == kind)
            .count()
    });
    assert!(counts.into_iter().all(|count| count > 0));

    let fog = s.game.blender.state.colors[color::FOG][3];
    let mist = s.game.blender.state.colors[color::MIST][3];
    let images = [
        ("fog-and-mist", fog, mist),
        ("fog-only", fog, 0.0),
        ("mist-only", 0.0, mist),
        ("no-atmosphere", 0.0, 0.0),
    ]
    .map(|(name, fog, mist)| {
        s.game.blender.state.colors[color::FOG][3] = fog;
        s.game.blender.state.colors[color::MIST][3] = mist;
        let image = capture.render(&s);
        let path = out.join(format!("{name}.png"));
        image.export_png(path.to_str().expect("UTF-8 capture path"));
        image
    });
    let [both, fog_only, mist_only, clear] = &images;
    let whole_frame = (WIDTH * HEIGHT) as usize;
    let fog_pixels = changed_pixels(fog_only, clear, whole_frame);
    let mist_pixels = changed_pixels(mist_only, clear, whole_frame);
    let both_pixels = changed_pixels(both, clear, whole_frame);
    let fog_on_mist = changed_pixels(both, mist_only, whole_frame);
    let mist_on_fog = changed_pixels(both, fog_only, whole_frame);
    assert!([fog_pixels, mist_pixels, fog_on_mist, mist_on_fog]
        .into_iter()
        .all(|n| n > 0));

    // Readback starts with the bottom row. A few pixels of margin keep
    // the shoreline outside the reflection comparison.
    let water_start = top::layer_to_screen(&s, 0.0, s.world.water.top, 0.0).1 as u32 + 4;
    let water_pixels = (WIDTH * HEIGHT.saturating_sub(water_start)) as usize;
    let reflected_mist_pixels = changed_pixels(mist_only, clear, water_pixels);
    assert!(
        reflected_mist_pixels > 0,
        "mist changes the reflected scene"
    );

    let report = json!({
        "weather": FOGGY,
        "seed": SEED,
        "hour": HOUR,
        "tick": s.game.ticks,
        "active_fog": counts[0],
        "active_mist": counts[1],
        "fog_pixels": fog_pixels,
        "mist_pixels": mist_pixels,
        "combined_pixels": both_pixels,
        "fog_over_mist_pixels": fog_on_mist,
        "mist_under_fog_pixels": mist_on_fog,
        "water_pixels_compared": water_pixels,
        "reflected_mist_pixels": reflected_mist_pixels,
    });
    write_report(&out.join("capture.json"), &report);
    next_frame().await;
}

fn write_report(path: &Path, report: &Value) {
    std::fs::write(path, serde_json::to_vec_pretty(report).unwrap()).expect("write receipt");
}

fn main() {
    let mut args = std::env::args_os().skip(1);
    let dir = PathBuf::from(args.next().expect("converted assets path"));
    let out = PathBuf::from(args.next().expect("capture output path"));
    let saves_only = match args.next() {
        None => false,
        Some(flag) => {
            assert_eq!(flag, "--saves-only", "unknown argument");
            true
        }
    };
    assert!(args.next().is_none(), "unexpected extra argument");
    std::fs::create_dir_all(&out).expect("create output folder");
    let world_bytes = std::fs::read(dir.join("world.bin")).expect("read converted world");
    let world = game::assetfile::decode_world(&world_bytes).expect("decode converted world");
    if saves_only {
        let fixtures = save_fixtures(&world, fingerprint(&world_bytes), &out);
        write_report(&out.join("fixtures.json"), &fixtures);
    } else {
        macroquad::Window::from_config(window(), capture(world, dir, out));
    }
}
