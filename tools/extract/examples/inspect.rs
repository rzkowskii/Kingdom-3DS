//! Prints prototypes from `world.bin` whose name contains the given text.
//! `cargo run -p extract --example inspect -- assets/world.bin Wall`

use game::data::WorldData;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&args[1]).expect("read world.bin");
    let world: WorldData = game::assetfile::decode_world(&bytes).expect("decode");
    println!(
        "shield shop {:?} walls {:?} towers {:?} border {:?}",
        world.shield_shop, world.wall_prefabs, world.tower_prefabs, world.border
    );
    let filter = args.get(2).map(String::as_str).unwrap_or("");
    for (l, d) in game::data::Layer::ALL.iter().zip(&world.layers) {
        println!("layer {l:?}: ratio {} z {}", d.ratio, d.z);
    }
    if let Some(f) = &world.farmland {
        println!(
            "farmland: proto {} points {} yield {} frames {} back {} back_part {} width {}",
            f.proto,
            f.farm_points,
            f.coin_yield,
            f.frames.len(),
            f.back_frames.len(),
            f.back_part,
            f.width
        );
    }
    for (i, p) in world.protos.iter().enumerate() {
        if !p.name.contains(filter) {
            continue;
        }
        let next = p
            .upgrade
            .and_then(|u| u.next)
            .map(|n| world.protos[n as usize].name.clone());
        println!(
            "{i:4} {:24} tag={:16} payable={:?} next={:?} building={:?} forest={:?} castle={:?} z={} gaps={:?} farmhouse={:?} slots={:?} wall={:?} tower={:?} shields={:?} keep={} shop={:?} tk={} banker={:?} border={:?}",
            p.name,
            p.tag,
            p.payable.map(|d| (d.price, d.pay_distance, d.coin_offset)),
            next,
            p.building
                .as_ref()
                .map(|b| (b.build_points, b.auto_build_rate, b.scaffold.len())),
            p.forest_item.map(|f| (f.width, f.size_padding)),
            p.castle_level,
            p.root_z,
            p.gaps,
            p.farmhouse,
            p.guard_slots,
            p.wall_level,
            p.tower_level,
            p.shield_spots,
            p.keep_time,
            p.shield_shop,
            p.tower_knight,
            p.banker,
            p.border
        );
        if let Some(w) = &p.wharf {
            println!(
                "     wharf {:?} parts {} first {:?}",
                (w.work_per_part, w.skeleton, w.skeleton_at, w.boat),
                w.parts.len(),
                w.parts.first()
            );
        }
        if let Some(b) = &p.boat {
            println!("     boat {b:?}");
        }
        if p.beach || p.signpost.is_some() {
            println!("     beach {} signpost {:?}", p.beach, p.signpost);
        }
    }
    println!("unlockable on land {:?}", world.unlockable_on_land);
    if filter == "bag" {
        println!("{:#?}", world.bag);
        let m = &world.map;
        println!(
            "map back {:?} details {} boat {:?} pointer {:?}",
            m.back,
            m.details.len(),
            m.boat,
            m.pointer
        );
        for l in &m.lands {
            println!(
                "  land at {:?} boat {:?} clouds {} unlocks {:?}",
                l.at,
                l.boat,
                l.clouds.len(),
                l.unlocks
                    .iter()
                    .map(|u| (u.unlock, u.at, u.icon.is_some()))
                    .collect::<Vec<_>>()
            );
        }
    }
    for l in &world.levels {
        println!(
            "{} free parts {} boat mult {} force {}",
            l.name, l.free_boat_parts, l.wave_boat_multiplier, l.force_all_unlocks
        );
    }
}
