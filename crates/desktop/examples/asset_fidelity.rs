//! Bounded, silent 400x240 captures of injected asset states using the real
//! scene and desktop renderer. No storage is attached. This is a visual fixture,
//! not a playthrough or a performance measurement.
//! `cargo run -p desktop --example asset_fidelity -- <assets> <output>`

#[path = "../src/assets.rs"]
mod assets;

use ::scene::session::Session;
use game::data::{Layer, WorldData};
use game::level::{self, Level};
use macroquad::prelude::*;

fn window() -> Conf {
    Conf {
        window_title: "Kingdom asset fidelity capture".into(),
        window_width: 400,
        window_height: 240,
        high_dpi: false,
        ..Default::default()
    }
}

fn isolated(world: &WorldData) -> Session {
    let mut s = Session::new(world.clone(), 0, 1, game::progress::Progress::everything());
    s.game.level = Level::empty(-20.0, 40.0);
    s.game.population = Default::default();
    s.game.greed = Default::default();
    s.game.siege = Default::default();
    s.game.monarch.x = -100.0;
    s.camera_x = 0.0;
    s.debug = false;
    s
}

fn open_portal(s: &mut Session, object: u32) {
    use game::director::{Season, WaveKind};
    use game::greed::GreedCtx;
    let borders = game::kingdom::Borders {
        left: 9.0,
        right: 11.0,
    };
    let mut rng = game::rng::Rng::new(1);
    s.game.greed.schedule(
        &s.world,
        &s.game.level,
        0,
        WaveKind::Regular,
        Season::Summer,
        3,
        10.0,
        borders,
        &mut rng,
    );
    let portals = [(object, 0.0)];
    let ctx = GreedCtx {
        obstacles: &[],
        solid_obstacles: &[],
        world: &s.world,
        level_index: 0,
        campfire: 10.0,
        borders,
        monarch_x: -100.0,
        monarch_has_crown: true,
        targets: &[],
        citizens: &[],
        loot: &[],
        portals: &portals,
        portal_infos: &[],
        threats: &[],
        season: Season::Summer,
        day: 3,
        boat: false,
    };
    // Advance the production wave scheduler to its opening/spawning boundary.
    s.game
        .greed
        .update(0.0, 100.0, &ctx, &s.game.level, &mut [], &mut rng, |_| {});
    assert!(s.game.greed.portal_open(object));
}

#[macroquad::main(window)]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    assert_eq!(args.len(), 3, "usage: asset_fidelity <assets> <output>");
    let dir = std::path::Path::new(&args[1]);
    let out = std::path::Path::new(&args[2]);
    std::fs::create_dir_all(out).unwrap();
    let world: WorldData =
        game::assetfile::decode_world(&std::fs::read(dir.join("world.bin")).unwrap()).unwrap();
    let mut renderer = assets::Assets::load(dir, &world).await.unwrap();
    let sheet = renderer.sheet.clone();
    let portal = world
        .protos
        .iter()
        .position(|p| p.portal.as_ref().is_some_and(|p| p.cliff))
        .expect("cliff portal prototype");
    let counter = world
        .protos
        .iter()
        .position(|p| p.workshop_side.is_some())
        .expect("catapult workshop counter");
    let mut receipts = Vec::new();
    for mirror in [false, true] {
        let mut s = isolated(&world);
        let p = &world.protos[portal];
        let mut object = level::instantiate(
            &world,
            portal as u16,
            Layer::Game,
            0.0,
            1.0,
            p.root_z,
            if mirror { -1.0 } else { 1.0 },
        );
        object.build = None;
        let object = s.game.level.add(object);
        s.t = 0.4;
        s.game.ticks = 24;
        ::scene::top::draw(&s, &sheet, &mut renderer);
        let file = format!("portal-{}.png", if mirror { "mirrored" } else { "normal" });
        get_screen_data().export_png(out.join(&file).to_str().unwrap());
        receipts
            .push(serde_json::json!({"file":file,"prototype":p.name,"mirrored":mirror,"time":0.4}));
        next_frame().await;
        open_portal(&mut s, object);
        ::scene::top::draw(&s, &sheet, &mut renderer);
        let file = format!(
            "portal-open-{}.png",
            if mirror { "mirrored" } else { "normal" }
        );
        get_screen_data().export_png(out.join(&file).to_str().unwrap());
        receipts.push(serde_json::json!({"file":file,"prototype":p.name,
            "mirrored":mirror,"time":0.4,"portal_open":true}));
        next_frame().await;
    }
    for mirror in [false, true] {
        let mut s = isolated(&world);
        let object = level::instantiate(
            &world,
            counter as u16,
            Layer::Game,
            0.0,
            1.0,
            world.protos[counter].root_z,
            1.0,
        );
        let counter_id = s.game.level.add(object);
        s.game.siege.order(&world, &mut s.game.level, counter_id);
        let catapult_id = s.game.siege.catapults[0].object;
        let ci = s.game.level.index_of(catapult_id).unwrap();
        s.game.level.objects[0].x = -100.0;
        let o = &mut s.game.level.objects[ci];
        o.x = 0.0;
        o.build = None;
        o.flip = mirror;
        o.mode = game::siege::mode::FIRING;
        o.aim = 68.0;
        for (frame, time) in [0.0, 0.15, 0.16666667, 0.4, 1.1666666]
            .into_iter()
            .enumerate()
        {
            s.game.siege.catapults[0].fire_t = time;
            s.t = time;
            ::scene::top::draw(&s, &sheet, &mut renderer);
            let file = format!(
                "catapult-{}-{frame}.png",
                if mirror { "mirrored" } else { "normal" }
            );
            get_screen_data().export_png(out.join(&file).to_str().unwrap());
            receipts.push(serde_json::json!({"file":file,"mirrored":mirror,"fire_time":time}));
            next_frame().await;
        }
    }
    std::fs::write(out.join("capture.json"), serde_json::to_vec_pretty(&serde_json::json!({
        "method":"Injected states; actual scene::top and desktop Assets renderer; no simulation ticks, audio or storage",
        "width":400,"height":240,"frames":receipts
    })).unwrap()).unwrap();
}
