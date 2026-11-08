//! Bounded 400x240 scripted squid motion captures using real GameState ticks.
//! Initial locations, retreat requests and death are injected preconditions.
//! This is a native-port presentation fixture, not human play, a PC runtime
//! comparison, a performance benchmark, or an audio/storage test.
//! `cargo run -p desktop --example squid_motion -- <assets> <output>`
#[path = "../src/assets.rs"]
mod assets;
use ::scene::session::Session;
use game::data::{Layer, WorldData};
use game::level::{self, Level};
use macroquad::prelude::*;
const FPS: u32 = 30;
fn window() -> Conf {
    Conf {
        window_title: "Kingdom squid motion fixtures".into(),
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

fn object(s: &mut Session, proto: usize, x: f32, mirror: bool) -> u32 {
    let p = &s.world.protos[proto];
    let mut o = level::instantiate(
        &s.world,
        proto as u16,
        Layer::Game,
        x,
        1.0,
        p.root_z,
        if mirror { -1.0 } else { 1.0 },
    );
    o.build = None;
    let is_campfire = p.tag == "Campfire";
    let id = s.game.level.add(o);
    if is_campfire {
        // GameState caches this at level generation; replacing the fixture
        // level must update the cache alongside its authored campfire object.
        s.game.campfire = x;
        s.game.borders = game::kingdom::borders(&s.world, &s.game.level, x);
    }
    id
}
/// Real GPU shader/blend probe, independent of the gameplay capture's lighting.
/// get_screen_data flushes queued rendering; next_frame separates clear passes.
async fn overlay_probe(renderer: &mut assets::Assets, out: &std::path::Path) {
    use ::scene::render::{Color as SceneColor, Draw, Renderer};
    let sprite = renderer
        .sheet
        .clip("squid_move")
        .and_then(|c| c.sprite("", 0.0))
        .expect("squid_move root sprite for GPU overlay probe");
    let mut receipts = Vec::new();
    for (name, blue, overlay, alpha) in [
        ("baseline_black", false, false, 1.0),
        ("overlay_black", false, true, 1.0),
        ("overlay_blue", true, true, 1.0),
        ("overlay_half_alpha", true, true, 0.5),
    ] {
        renderer.clear(if blue {
            SceneColor::new(0.0, 0.0, 1.0, 1.0)
        } else {
            SceneColor::BLACK
        });
        let mut draw = Draw::tint(SceneColor::new(0.0, 0.0, 0.0, alpha)).scale(2.0);
        if overlay {
            draw = draw.overlay(SceneColor::new(1.0, 0.0, 0.0, 0.5));
        }
        renderer.sprite(sprite, (200.0, 170.0), draw);
        let image = get_screen_data();
        let pixels = image.get_image_data();
        let file = format!("probe-{name}.png");
        image.export_png(out.join(&file).to_str().unwrap());
        let nonblack = pixels
            .iter()
            .filter(|p| p[0] > 2 || p[1] > 2 || p[2] > 2)
            .count();
        let red = pixels
            .iter()
            .filter(|p| (124..=132).contains(&p[0]) && p[1] <= 2 && p[2] <= 2)
            .count();
        let half = pixels
            .iter()
            .filter(|p| (60..=68).contains(&p[0]) && p[1] <= 2 && (124..=132).contains(&p[2]))
            .count();
        let blue_count = pixels
            .iter()
            .filter(|p| p[0] <= 2 && p[1] <= 2 && p[2] >= 253)
            .count();
        let mut bounds = [usize::MAX, usize::MAX, 0, 0];
        for (index, p) in pixels.iter().enumerate() {
            if p[0] > 50 && p[1] <= 2 && p[2] < 200 {
                let x = index % image.width as usize;
                let y = index / image.width as usize;
                bounds[0] = bounds[0].min(x);
                bounds[1] = bounds[1].min(y);
                bounds[2] = bounds[2].max(x);
                bounds[3] = bounds[3].max(y);
            }
        }
        let mut transparent_inside = 0;
        if bounds[0] != usize::MAX {
            for y in bounds[1]..=bounds[3] {
                for x in bounds[0]..=bounds[2] {
                    let p = pixels[y * image.width as usize + x];
                    if p[0] <= 2 && p[1] <= 2 && p[2] >= 253 {
                        transparent_inside += 1;
                    }
                }
            }
        }
        match name {
            "baseline_black" => assert_eq!(nonblack, 0, "black-tinted baseline must remain black"),
            "overlay_black" => assert!(
                red > 100,
                "overlay must stay red under black lighting, got {red} pixels"
            ),
            "overlay_blue" => {
                assert!(
                    red > 100 && blue_count > 100,
                    "opaque overlay and unchanged blue background required"
                );
                assert!(
                    transparent_inside > 20,
                    "sprite silhouette transparency must remain inside its own bounding box"
                );
            }
            _ => assert!(
                half > 100,
                "sprite alpha must still blend half-strength overlay over blue, got {half}"
            ),
        }
        receipts.push(serde_json::json!({"file":file,"sprite":sprite,"lighting_tint_rgb":[0,0,0],"tint_alpha":alpha,
            "overlay_rgba":if overlay {Some([1.0,0.0,0.0,0.5])}else{None},"clear_blue":blue,
            "nonblack_pixels":nonblack,"opaque_half_red_pixels":red,"half_alpha_blend_pixels":half,
            "unchanged_blue_pixels":blue_count,"transparent_blue_inside_sprite_bounds":transparent_inside}));
        next_frame().await;
    }
    std::fs::write(out.join("overlay-probe.json"),serde_json::to_vec_pretty(&serde_json::json!({
        "method":"Real desktop GPU renderer, actual extracted squid frame; black lighting, RGB overlay, source coverage and half-alpha blending assertions; four separate flushed frames",
        "motion_frame_count_excludes_probe":true,"probes":receipts})).unwrap()).unwrap();
}

#[macroquad::main(window)]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    assert_eq!(args.len(), 3, "usage: squid_motion <assets> <output>");
    let dir = std::path::Path::new(&args[1]);
    let out = std::path::Path::new(&args[2]);
    std::fs::create_dir_all(out).unwrap();
    let world: WorldData =
        game::assetfile::decode_world(&std::fs::read(dir.join("world.bin")).unwrap()).unwrap();
    let mut renderer = assets::Assets::load(dir, &world).await.unwrap();
    let sheet = renderer.sheet.clone();
    overlay_probe(&mut renderer, out).await;
    let wall = world
        .protos
        .iter()
        .position(|p| p.wall && !p.obstacles.is_empty())
        .expect("authored physical wall");
    let portal = world
        .protos
        .iter()
        .position(|p| p.portal.is_some_and(|p| !p.cliff))
        .expect("regular portal");
    let campfire = world
        .protos
        .iter()
        .position(|p| p.tag == "Campfire")
        .expect("campfire");
    let cases = [
        ("wall_approach", false, 75),
        ("wall_approach", true, 75),
        ("portal_retreat", false, 45),
        ("portal_retreat", true, 45),
        ("no_portal_climb", false, 60),
        ("death_interrupt", false, 92),
    ];
    let mut receipts = Vec::new();
    for (sequence, mirror, frames) in cases {
        let side = if mirror { -1.0 } else { 1.0 };
        let mut s = isolated(&world);
        let (start, height) = match sequence {
            "wall_approach" => {
                object(&mut s, campfire, 8.0 * side, false);
                object(&mut s, wall, 0.0, mirror);
                (-1.8 * side, 0.0)
            }
            "portal_retreat" => {
                object(&mut s, campfire, 0.0, false);
                object(&mut s, portal, 3.5 * side, mirror);
                (1.5 * side, world.squid.base_altitude - game::build::WALK_Y)
            }
            "no_portal_climb" => {
                object(&mut s, campfire, 0.0, false);
                (3.0, world.squid.base_altitude - game::build::WALK_Y)
            }
            _ => {
                object(&mut s, campfire, 8.0, false);
                (0.0, 1.0)
            }
        };
        let enemy = s.game.greed.spawn_troll(
            game::greed::EnemyKind::Squid,
            start,
            0,
            false,
            &mut game::rng::Rng::new(90210),
        );
        s.game.greed.trolls[0].y = height;
        s.game.greed.trolls[0].mover.facing = if mirror { -1 } else { 1 };
        if sequence == "portal_retreat" || sequence == "no_portal_climb" {
            // Restored-retreat precondition: bypass only the initial Chase
            // transition wait so this bounded clip includes the full departure.
            s.game.greed.trolls[0].state = game::greed::TrollState::Retreat { t: 0.0 };
            s.game.greed.retreat_all();
        }
        if sequence == "death_interrupt" {
            // Let real updates start flight before interrupting it. The active
            // FlyUp coroutine is intentionally allowed to finish after death.
            for _ in 0..6 {
                tick_frame(&mut s);
            }
            s.game.greed.hurt(0, 1000, &mut |_| {});
        }
        for frame in 0..frames {
            ::scene::top::draw(&s, &sheet, &mut renderer);
            let file = format!("frame-{:04}.png", receipts.len());
            get_screen_data().export_png(out.join(&file).to_str().unwrap());
            let actor = s.game.greed.trolls.iter().find(|t| t.id == enemy);
            let visual = s.game.greed.squid_visual(enemy);
            receipts.push(serde_json::json!({"file":file,"sequence":sequence,"mirrored":mirror,"frame":frame,"time":frame as f32/FPS as f32,"ticks":s.game.ticks,
                "pose":actor.map(|t|[t.x,t.y,t.mover.vx,t.vy]),"actor_state":actor.map(|t|t.state),
                "runtime":s.game.greed.squid_states(),"overlay_elapsed":visual.as_ref().and_then(|v|v.overlay_elapsed),
                "death_alpha":visual.map(|v|v.death_alpha),"injected_retreat":sequence=="portal_retreat"||sequence=="no_portal_climb",
                "injected_death":sequence=="death_interrupt","warmup_ticks":if sequence=="death_interrupt"{12}else{0}}));
            tick_frame(&mut s);
            next_frame().await;
        }
    }
    assert_eq!(receipts.len(), 392);
    std::fs::write(out.join("capture.json"),serde_json::to_vec_pretty(&serde_json::json!({"method":"Injected initial poses/retreat/death; production60Hz GameState ticks sampled30fps; fixed camera; silent; no storage; no PC runtime or performance claim",
        "width":400,"height":240,"fps":FPS,"frames":receipts})).unwrap()).unwrap();
}
