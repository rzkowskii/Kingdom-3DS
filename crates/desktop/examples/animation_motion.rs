//! Bounded silent 400x240 motion fixtures using real GameState ticks and assets.
//! Inputs are injected: portal wave/destruction and a ready-to-fire catapult.
//! No storage, audio, performance measurement or PC runtime capture is involved.
//! `cargo run -p desktop --example animation_motion -- <assets> <output>`

#[path = "../src/assets.rs"]
mod assets;

use ::scene::session::Session;
use game::data::{Layer, WorldData};
use game::level::{self, Level};
use macroquad::prelude::*;

const FRAMES_PER_SEQUENCE: usize = 90;
const FPS: u32 = 30;

fn window() -> Conf {
    Conf {
        window_title: "Kingdom animation motion fixtures".into(),
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
    s.game.monarch.x = -20.0;
    s.camera_x = 0.0;
    s.debug = false;
    s
}

fn tick_frame(s: &mut Session) {
    // 60 Hz production simulation, sampled at 30 Hz. Camera deliberately held
    // on the fixture; this is not a camera or reflection comparison.
    for _ in 0..2 {
        s.game.tick(
            &s.world,
            &game::controls::Controls::default(),
            &mut Vec::new(),
        );
    }
    s.t = s.game.ticks as f32 * game::state::TICK;
    s.camera_x = 0.0;
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
        monarch_x: -20.0,
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
    s.game
        .greed
        .update(0.0, 100.0, &ctx, &s.game.level, &mut [], &mut rng, |_| {});
    assert!(s.game.greed.portal_open(object));
    // Remove the scheduler's first enemy from this visual fixture. The portal
    // state and trigger are the production results; destruction clears its queue.
    s.game.greed.trolls.clear();
}

#[macroquad::main(window)]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    assert_eq!(args.len(), 3, "usage: animation_motion <assets> <output>");
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
        .position(|p| p.portal.as_ref().is_some_and(|p| !p.cliff))
        .expect("regular portal prototype");
    let counter = world
        .protos
        .iter()
        .position(|p| p.workshop_side.is_some())
        .expect("catapult workshop counter");
    let mut receipts = Vec::new();
    for mirror in [false, true] {
        let mut s = isolated(&world);
        let p = &world.protos[portal];
        let mut o = level::instantiate(
            &world,
            portal as u16,
            Layer::Game,
            0.0,
            1.0,
            p.root_z,
            if mirror { -1.0 } else { 1.0 },
        );
        o.build = None;
        let id = s.game.level.add(o);
        open_portal(&mut s, id);
        for frame in 0..FRAMES_PER_SEQUENCE {
            if frame == 6 {
                s.game.greed.portal_destroyed(id, false, 0.0, &mut |_| {});
            }
            ::scene::top::draw(&s, &sheet, &mut renderer);
            let file = format!("frame-{:04}.png", receipts.len());
            get_screen_data().export_png(out.join(&file).to_str().unwrap());
            receipts.push(
                serde_json::json!({"file":file,"sequence":"portal_open_destroy",
                "mirrored":mirror,"frame":frame,"time":frame as f32/FPS as f32,
                "portal_animation":s.game.greed.portal_animation(id),
                "destruction_trigger_frame":6,"ticks":s.game.ticks}),
            );
            tick_frame(&mut s);
            next_frame().await;
        }
    }
    for mirror in [false, true] {
        let mut s = isolated(&world);
        let o = level::instantiate(
            &world,
            counter as u16,
            Layer::Game,
            0.0,
            1.0,
            world.protos[counter].root_z,
            1.0,
        );
        let counter_id = s.game.level.add(o);
        s.game.siege.order(&world, &mut s.game.level, counter_id);
        let id = s.game.siege.catapults[0].object;
        let ci = s.game.level.index_of(id).unwrap();
        s.game.level.objects[0].x = -100.0;
        let o = &mut s.game.level.objects[ci];
        o.x = 0.0;
        o.build = None;
        o.flip = mirror;
        o.mode = game::siege::mode::RELOADING;
        s.game.siege.catapults[0].side = if mirror { -1 } else { 1 };
        // A normal firing catapult has already settled from its y=1 spawn.
        // Admit two seconds of real simulation before injecting the fire state.
        for _ in 0..60 {
            tick_frame(&mut s);
        }
        s.game.level.objects[ci].mode = game::siege::mode::FIRING;
        s.game.level.objects[ci].aim = 68.0;
        for frame in 0..FRAMES_PER_SEQUENCE {
            ::scene::top::draw(&s, &sheet, &mut renderer);
            let file = format!("frame-{:04}.png", receipts.len());
            get_screen_data().export_png(out.join(&file).to_str().unwrap());
            receipts.push(serde_json::json!({"file":file,"sequence":"catapult_recoil",
                "mirrored":mirror,"frame":frame,"time":frame as f32/FPS as f32,
                "settle_ticks":120,"root":[s.game.level.objects[ci].x,s.game.level.objects[ci].y],
                "fire_time":s.game.siege.catapults[0].fire_t,
                "recoil":s.game.siege.recoils.iter().find(|r| r.object==id),"ticks":s.game.ticks}));
            tick_frame(&mut s);
            next_frame().await;
        }
    }
    assert_eq!(receipts.len(), 360);
    std::fs::write(out.join("capture.json"), serde_json::to_vec_pretty(&serde_json::json!({
        "method":"Injected initial states and portal triggers; production GameState ticks; fixed camera; silent; no storage",
        "width":400,"height":240,"fps":FPS,"frames":receipts
    })).unwrap()).unwrap();
}
