//! Times the simulation against building the frame (scene drawing into a
//! renderer that only counts), to see where the 3DS spends its frame.
//! `cargo run --release -p extract --example bench -- assets [island] [seconds]`

use std::time::{Duration, Instant};

use game::autopilot::Autopilot;
use game::data::WorldData;
use game::progress::Progress;
use scene::render::{Color, Draw, Rect, Renderer};
use scene::session::Session;
use scene::sheet::Sheet;
use scene::{top, touch};

#[derive(Default)]
struct Count {
    sprites: usize,
    other: usize,
    /// Additive fills (the fires' light on the ground, `PostFX`).
    adds: usize,
}

impl Count {
    fn quads(&self) -> usize {
        self.sprites + self.other
    }
}

impl Renderer for Count {
    fn clear(&mut self, _: Color) {
        self.other += 1;
    }
    fn sprite(&mut self, _: u32, _: (f32, f32), _: Draw) {
        self.sprites += 1;
    }
    fn sliced(&mut self, _: u32, _: Rect, _: (f32, f32, f32, f32), _: Color) {
        self.sprites += 9;
    }
    fn rect(&mut self, _: Rect, _: Color) {
        self.other += 1;
    }
    fn rect_lines(&mut self, _: Rect, _: f32, _: Color) {
        self.other += 4;
    }
    fn gradient(&mut self, _: Rect, _: Color, _: Color) {
        self.other += 1;
    }
    fn add(&mut self, _: Rect, _: Color) {
        self.other += 1;
        self.adds += 1;
    }
    fn add_gradient(&mut self, _: Rect, _: Color, _: Color) {
        self.other += 1;
        self.adds += 1;
    }
    fn circle(&mut self, _: f32, _: f32, _: f32, _: Color) {
        self.other += 16;
    }
    fn circle_lines(&mut self, _: f32, _: f32, _: f32, _: f32, _: Color) {
        self.other += 24;
    }
}

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or("assets".into()));
    let island: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let secs: u32 = std::env::args()
        .nth(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(600);
    let world: WorldData =
        game::assetfile::decode_world(&std::fs::read(dir.join("world.bin")).unwrap()).unwrap();
    let read = |n: &str| std::fs::read_to_string(dir.join(n)).unwrap();
    let mut sheet = Sheet::from_json(
        &read("sprites.json"),
        &read("clips.json"),
        &read("font.json"),
    )
    .unwrap();
    sheet.link(&world.sprites);
    let pilot = Autopilot::default();
    let mut s = Session::new(world, island, 1, Progress::everything());
    s.game.wallet.set(60);
    let (mut sim, mut draw) = (Duration::ZERO, Duration::ZERO);
    let mut r = Count::default();
    let frames = secs * 60;
    let mut worst = (Duration::ZERO, Duration::ZERO);
    // The busiest frame: (quads, of which additive fills).
    let mut peak = (0, 0);
    for f in 0..frames {
        let raw = pilot.input(&s.game, &s.world, f);
        let t0 = Instant::now();
        s.step(raw);
        let t1 = Instant::now();
        let before = (r.quads(), r.adds);
        top::draw(&s, &sheet, &mut r);
        touch::draw(&s, &sheet, &mut r);
        let t2 = Instant::now();
        let this = (r.quads() - before.0, r.adds - before.1);
        if this.0 > peak.0 {
            peak = this;
        }
        sim += t1 - t0;
        draw += t2 - t1;
        worst.0 = worst.0.max(t1 - t0);
        worst.1 = worst.1.max(t2 - t1);
    }
    let n = frames as f64;
    // Per-system tick times (with `--features game/profile`).
    for (name, t) in game::prof::report() {
        println!("  {name:14} {:6.1} µs", t * 1e6 / n);
    }
    println!(
        "island {island}: sim {:.1} µs/frame (worst {:.0}), draw {:.1} µs/frame (worst {:.0}), {:.0} sprites + {:.0} other per frame ({:.0} additive fills), busiest frame {} quads ({} additive)",
        sim.as_secs_f64() * 1e6 / n,
        worst.0.as_secs_f64() * 1e6,
        draw.as_secs_f64() * 1e6 / n,
        worst.1.as_secs_f64() * 1e6,
        r.sprites as f64 / n,
        r.other as f64 / n,
        r.adds as f64 / n,
        peak.0,
        peak.1
    );
}
