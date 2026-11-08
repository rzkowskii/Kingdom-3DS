//! Render authored fog and mist checkpoints and save matching Azahar fixtures.
//! cargo run -p desktop --example atmosphere -- <assets> <output> [--saves-only | --fog-light | --fog-light-save-only]

#[path = "../src/assets.rs"]
mod assets;

use std::path::{Path, PathBuf};

use ::scene::atmosphere_data::{AtmosphereDef, FILE_NAME};
use ::scene::cloud_mesh::CloudMesh;
use ::scene::cloud_shadow;
use ::scene::clouds::AtmosphereKind;
use ::scene::render::{self, Draw, Renderer};
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
const FOGGY_NIGHT: &str = "Foggy Summer Night";
const SEED: u32 = 17;
const HOUR: f32 = 19.5;
const NIGHT_HOUR: f32 = 23.5;
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

fn stage(world: &WorldData, name: &str, hour: f32) -> (Session, u16) {
    let preset = world
        .lights
        .iter()
        .position(|light| light.name == name)
        .expect("authored weather") as u16;
    let mut s = Session::new(world.clone(), 0, SEED, Progress::everything());
    s.game.clock.day = 1;
    s.game.clock.time = hour;
    s.game.clock.speed = 0.0;
    s.game.days = DayCounter::new(&s.game.clock);
    s.god = true;
    let input = RawInput::default();
    s.step_ticks(input, 1);
    s.game.greed = Default::default();
    s.game.blender.add(&s.world, preset, 0.0);
    s.step_ticks(input, SETTLE_TICKS);
    assert_eq!(s.game.clock.time, hour);
    (s, preset)
}

fn save_fixtures(world: &WorldData, world_id: u32, out: &Path) -> Value {
    let mut reports = Vec::new();
    for (label, name) in [("foggy", FOGGY), ("clear", CLEAR)] {
        let (s, preset) = stage(world, name, HOUR);
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

fn night_save_path(out: &Path) -> PathBuf {
    out.join("azahar-fixtures")
        .join("foggy-night")
        .join("game.sav")
}

fn night_fixture(world: &WorldData, world_id: u32, out: &Path) -> Value {
    let (mut s, preset) = stage(world, FOGGY_NIGHT, NIGHT_HOUR);
    s.game.monarch.x = s.game.campfire + 0.75;
    let bytes = save_game(&s.game, world_id);
    let loaded = load_game(&bytes, world_id, world).expect("round-trip night fixture");
    assert!(loaded
        .blender
        .presets()
        .any(|(id, w)| id == preset && w == 1.0));
    assert_eq!(loaded.clock.time, NIGHT_HOUR);
    assert_eq!(loaded.clock.speed, 0.0);
    assert!(!loaded.days.is_daytime);
    let light = &loaded.blender.state;
    let fog = light.floats[float::FOG_DENSITY];
    assert!(fog > 0.0);
    assert!(world.steeds[usize::from(loaded.steed)].torch.is_some());
    assert!(light.ambient_grayscale() < world.fire.torch_threshold);

    let path = night_save_path(out);
    std::fs::create_dir_all(path.parent().unwrap()).expect("create night fixture folder");
    std::fs::write(&path, bytes).expect("write night fixture");
    json!({
        "world_fingerprint": format!("{world_id:08x}"),
        "path": path,
        "weather": FOGGY_NIGHT,
        "hour": loaded.clock.time,
        "seed": SEED,
        "fog_density": fog,
        "mist_density": light.floats[float::MIST_DENSITY],
        "ambient_grayscale": light.ambient_grayscale(),
        "torch_threshold": world.fire.torch_threshold,
        "monarch_x": loaded.monarch.x,
        "campfire_x": loaded.campfire,
    })
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

/// Suppress only the fog's local light while preserving the same sprites,
/// transparency, draw order, terrain lights and reflection inputs.
struct AmbientFog<'a>(&'a mut assets::Assets);

impl Renderer for AmbientFog<'_> {
    fn cloud_shadows(
        &mut self,
        camera: cloud_shadow::Camera,
        clouds: &[cloud_shadow::Sprite],
    ) -> bool {
        self.0.cloud_shadows(camera, clouds)
    }

    fn clear(&mut self, color: render::Color) {
        self.0.clear(color);
    }

    fn sprite(&mut self, id: u32, at: (f32, f32), draw: Draw) {
        self.0.sprite(id, at, draw);
    }

    fn atmosphere_sprite(
        &mut self,
        id: u32,
        mesh: &CloudMesh,
        at: (f32, f32),
        draw: Draw,
        vertex_tints: Option<&[render::Color]>,
    ) {
        assert!(
            vertex_tints.is_none() || draw.depth_write,
            "mist must not take fog lighting"
        );
        self.0.atmosphere_sprite(id, mesh, at, draw, None);
    }

    fn sliced(
        &mut self,
        id: u32,
        dest: render::Rect,
        border: (f32, f32, f32, f32),
        tint: render::Color,
    ) {
        self.0.sliced(id, dest, border, tint);
    }

    fn rect(&mut self, rect: render::Rect, color: render::Color) {
        self.0.rect(rect, color);
    }

    fn rect_lines(&mut self, rect: render::Rect, thickness: f32, color: render::Color) {
        self.0.rect_lines(rect, thickness, color);
    }

    fn gradient(&mut self, rect: render::Rect, top: render::Color, bottom: render::Color) {
        self.0.gradient(rect, top, bottom);
    }

    fn add(&mut self, rect: render::Rect, color: render::Color) {
        self.0.add(rect, color);
    }

    fn add_gradient(&mut self, rect: render::Rect, top: render::Color, bottom: render::Color) {
        self.0.add_gradient(rect, top, bottom);
    }

    fn circle(&mut self, x: f32, y: f32, radius: f32, color: render::Color) {
        self.0.circle(x, y, radius, color);
    }

    fn circle_lines(&mut self, x: f32, y: f32, radius: f32, thickness: f32, color: render::Color) {
        self.0.circle_lines(x, y, radius, thickness, color);
    }

    fn reflect(&mut self, dest: render::Rect, src: render::Rect, tint: render::Color) {
        self.0.reflect(dest, src, tint);
    }

    fn depth(&mut self, depth: f32) {
        self.0.depth(depth);
    }
}

impl Capture {
    async fn new(dir: &Path, world: &WorldData) -> Self {
        let capture = Self {
            renderer: assets::Assets::load(dir, world)
                .await
                .expect("load desktop assets"),
            screen: render_target(WIDTH, HEIGHT),
            previous: render_target(WIDTH, HEIGHT),
        };
        for target in [&capture.screen, &capture.previous] {
            target.texture.set_filter(FilterMode::Nearest);
        }
        capture
    }

    fn render(&mut self, s: &Session, fog_light: bool) -> Image {
        let sheet = self.renderer.sheet.clone();
        self.renderer.previous = None;
        set_camera(&camera(&self.screen));
        if fog_light {
            top::draw(s, &sheet, &mut self.renderer);
        } else {
            top::draw(s, &sheet, &mut AmbientFog(&mut self.renderer));
        }
        set_camera(&camera(&self.previous));
        draw_texture(&self.screen.texture, 0.0, 0.0, WHITE);
        top::draw_reflection_extras(s, &sheet, &mut self.renderer);
        self.renderer.previous = Some(self.previous.texture.clone());
        set_camera(&camera(&self.screen));
        if fog_light {
            top::draw(s, &sheet, &mut self.renderer);
        } else {
            top::draw(s, &sheet, &mut AmbientFog(&mut self.renderer));
        }
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
    let mut capture = Capture::new(&dir, &world).await;

    let (mut s, _) = stage(&world, FOGGY, HOUR);
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
        let image = capture.render(&s, true);
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

async fn capture_fog_light(world: WorldData, world_id: u32, dir: PathBuf, out: PathBuf) {
    let mut capture = Capture::new(&dir, &world).await;
    let bytes = std::fs::read(night_save_path(&out)).expect("read night fixture");
    let game = load_game(&bytes, world_id, &world).expect("load night fixture");
    let mut s = Session::new(world, 0, SEED, Progress::everything());
    s.resume(game);
    s.atmosphere = AtmosphereDef::load(&dir.join(FILE_NAME), &capture.renderer.sheet)
        .expect("read atmosphere sidecar");

    let mut checkpoints = Vec::new();
    let mut lit_pixels = 0;
    let mut previous = 0;
    for seconds in [16, 26, 38] {
        s.step_ticks(RawInput::default(), (seconds - previous) * 60);
        previous = seconds;
        assert_eq!(s.game.clock.time, NIGHT_HOUR);
        let def = s.atmosphere.as_ref().expect("authored fog and mist");
        let active_fog = s
            .clouds
            .atmosphere_samples(def, &s.game.blender.state)
            .filter(|p| p.kind == AtmosphereKind::Fog)
            .count();
        let ambient = capture.render(&s, false);
        let lit = capture.render(&s, true);
        let mut changed = 0;
        let mut peak = [0; 3];
        let mut bounds = [WIDTH, HEIGHT, 0, 0];
        for (i, (a, b)) in ambient
            .bytes
            .as_chunks::<4>()
            .0
            .iter()
            .zip(lit.bytes.as_chunks::<4>().0)
            .enumerate()
        {
            if a[..3] == b[..3] {
                continue;
            }
            changed += 1;
            for channel in 0..3 {
                peak[channel] = peak[channel].max(a[channel].abs_diff(b[channel]));
            }
            let i = i as u32;
            let x = i % WIDTH;
            let y = HEIGHT - 1 - i / WIDTH;
            bounds = [
                bounds[0].min(x),
                bounds[1].min(y),
                bounds[2].max(x),
                bounds[3].max(y),
            ];
        }
        for (label, image) in [("ambient", ambient), ("local", lit)] {
            let path = out.join(format!("night-{seconds:02}-{label}.png"));
            image.export_png(path.to_str().expect("UTF-8 capture path"));
        }
        checkpoints.push(json!({
            "simulated_seconds": seconds,
            "tick": s.game.ticks,
            "active_fog": active_fog,
            "changed_pixels": changed,
            "max_rgb8_delta": peak,
            "affected_bounds_inclusive": (changed > 0).then_some(bounds),
        }));
        lit_pixels += changed;
        next_frame().await;
    }
    let report = json!({"weather": FOGGY_NIGHT, "hour": NIGHT_HOUR, "checkpoints": checkpoints});
    write_report(&out.join("night-capture.json"), &report);
    assert!(
        lit_pixels > 0,
        "authored fog must pick up local light in at least one checkpoint"
    );
}

fn write_report(path: &Path, report: &Value) {
    std::fs::write(path, serde_json::to_vec_pretty(report).unwrap()).expect("write receipt");
}

fn main() {
    let mut args = std::env::args_os().skip(1);
    let dir = PathBuf::from(args.next().expect("converted assets path"));
    let out = PathBuf::from(args.next().expect("capture output path"));
    let mode = args.next();
    assert!(args.next().is_none(), "unexpected extra argument");
    std::fs::create_dir_all(&out).expect("create output folder");
    let world_bytes = std::fs::read(dir.join("world.bin")).expect("read converted world");
    let world = game::assetfile::decode_world(&world_bytes).expect("decode converted world");
    let world_id = fingerprint(&world_bytes);
    match mode.as_deref().and_then(std::ffi::OsStr::to_str) {
        None if mode.is_none() => {
            macroquad::Window::from_config(window(), capture(world, dir, out))
        }
        Some("--saves-only") => {
            let fixtures = save_fixtures(&world, world_id, &out);
            write_report(&out.join("fixtures.json"), &fixtures);
        }
        Some(flag @ ("--fog-light" | "--fog-light-save-only")) => {
            let fixture = night_fixture(&world, world_id, &out);
            write_report(&out.join("night-fixture.json"), &fixture);
            if flag == "--fog-light" {
                macroquad::Window::from_config(
                    window(),
                    capture_fog_light(world, world_id, dir, out),
                );
            }
        }
        _ => panic!("unknown argument"),
    }
}
