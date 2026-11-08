//! Lists sounds and the script fields that play them.
//! `cargo run -p extract --example sounds -- assets/world.bin [filter]`

use game::data::WorldData;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&args[1]).expect("read world.bin");
    let world: WorldData = game::assetfile::decode_world(&bytes).expect("decode");
    let filter = args.get(2).map(String::as_str).unwrap_or("");
    let a = &world.audio;
    for (key, id) in &a.refs {
        if !key.contains(filter) || (key.contains(':') && filter.is_empty()) {
            continue;
        }
        let s = &a.sounds[*id as usize];
        let clips: Vec<&str> = s
            .clips
            .iter()
            .map(|&c| a.clips[c as usize].name.as_str())
            .collect();
        println!(
            "{key} -> {} vol {:.2} spatial {:.1} {:?} loop {} max {} {:?}",
            s.name, s.volume, s.spatial, s.group, s.looping, s.max_distance, clips
        );
    }
}
