//! Runs an island headlessly with a mixer that logs what starts playing.
//! `cargo run --release -p extract --example listen -- assets [seconds]`

use std::path::Path;

use game::controls::RawInput;
use game::data::WorldData;
use scene::session::Session;
use scene::sheet::Sheet;
use scene::sound::{Mixer, Player, Voice};

struct Log {
    names: Vec<String>,
    t: f32,
    next: Voice,
    live: Vec<(Voice, f32)>,
    lens: Vec<f32>,
}

impl Mixer for Log {
    fn play(&mut self, clip: u16, gain: f32, pan: f32, looping: bool) -> Option<Voice> {
        println!(
            "{:7.2}s  {:<34} gain {gain:.3} pan {pan:+.2}{}",
            self.t,
            self.names[clip as usize],
            if looping { " (loop)" } else { "" }
        );
        self.next += 1;
        let end = if looping {
            f32::INFINITY
        } else {
            self.t + self.lens[clip as usize]
        };
        self.live.push((self.next, end));
        Some(self.next)
    }
    fn set(&mut self, _: Voice, _: f32, _: f32) {}
    fn stop(&mut self, v: Voice) {
        self.live.retain(|l| l.0 != v);
    }
    fn playing(&self, v: Voice) -> bool {
        self.live.iter().any(|l| l.0 == v && l.1 > self.t)
    }
    fn pause(&mut self, _: bool) {}
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "assets".into());
    let secs: f32 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(120.0);
    let dir = Path::new(&dir);
    let world: WorldData =
        game::assetfile::decode_world(&std::fs::read(dir.join("world.bin")).unwrap()).unwrap();
    let mut sheet = Sheet::from_json(
        &std::fs::read_to_string(dir.join("sprites.json")).unwrap(),
        &std::fs::read_to_string(dir.join("clips.json")).unwrap(),
        &std::fs::read_to_string(dir.join("font.json")).unwrap(),
    )
    .unwrap();
    sheet.link(&world.sprites);
    let mut log = Log {
        names: world.audio.clips.iter().map(|c| c.name.clone()).collect(),
        t: 0.0,
        next: 0,
        live: Vec::new(),
        lens: world.audio.clips.iter().map(|c| c.seconds).collect(),
    };
    for d in &world.days {
        println!("day {} tracks {:?}", d.name, d.tracks);
    }
    let mut s = Session::new(world, 0, 7, Default::default());
    println!("clock {} {}", s.game.clock.day, s.game.clock.time);
    for e in s.game.director.pending() {
        println!("  {:?}", e);
    }
    let mut player = Player::new(&s.world);
    let dt = 1.0 / 60.0;
    let mut f = 0u32;
    while log.t < secs {
        // Walk right for a while, then pay.
        let raw = RawInput {
            direction: if (f / 600).is_multiple_of(2) { 1 } else { -1 },
            sprint: false,
            pay: f % 300 < 60,
            bag_tap: false,
        };
        s.step(raw);
        player.update(&mut s, &sheet, &mut log, dt);
        log.t += dt;
        f += 1;
    }
}
