//! Desktop backend: runs the simulation laid out like a 3DS (400×240 top
//! screen over a 320×240 touch screen), drawing sprites produced by
//! `tools/extract` (set `KINGDOM_ASSETS` to override `assets/`).
//!
//! Keys: ←/→ or A/D walk (double-tap to gallop), Shift sprint (R),
//! Space or J pay/drop (A). Click the coin bag to drop a coin.
//! Dev: F1 debug overlay, 1–4 game speed ×1/×2/×4/×8, N skip one hour, C +10 coins,
//! B finish all construction, G spawn 3 greed, H fit the boat and bring it to the beach, I god mode (SELECT+R+Y on the 3DS). R restarts after a game over;
//! Enter sails on to the next island once the boat has left. F5 saves; the game
//! also saves when the kingdom becomes safe, on reaching a new island and on quitting.
//! `--screenshot <path> [--frame N]` runs an autopilot for N frames (default 200), saves a PNG, and exits.
//! `--island N` picks the island (0 = Land One); `--seed N` picks the island seed; `--speed N` starts at N× game speed; `--coins N` sets the purse;
//! `--greed-at N` spawns 3 greed at frame N; `--build` finishes construction every second;
//! `--prefer Castle,Farmhouse` steers the autopilot's purchases; `--goto X@F` sends it to x = X from frame F;
//! `--new` ignores the saved game, `--load` loads it even for a screenshot, `--no-save` turns saving off (screenshot runs never save);
//! `--weather "Clear Summer"` holds the lighting to that weather's presets; `--hour H` starts the clock at hour H;
//! `--boat-at N` does what H does at frame N; `--unlocked` starts with every unlock and island (otherwise a first playthrough); `--mute` turns sound off; `--paused` opens the pause menu, `--banner` the banner editor; `--tap X,Y` (repeatable) taps the touch screen at the start.

mod assets;
mod audio;
mod storage;

use ::scene::session::Session;
use ::scene::top::{self, draw_list, layer_to_screen, DrawItem};
use ::scene::touch::{self, Tap};
use assets::Assets;
use game::controls::RawInput;
use game::data::WorldData;
use game::progress::Progress;
use game::save;
use game::state::TICK;
use macroquad::prelude::*;
use platform::Storage;

pub const TOP_W: f32 = top::W;
pub const TOP_H: f32 = top::H;
const BOT_W: f32 = touch::W;
const BOT_H: f32 = touch::H;

fn arg<T: std::str::FromStr>(name: &str) -> Option<T> {
    std::env::args()
        .skip_while(|a| a != name)
        .nth(1)
        .and_then(|v| v.parse().ok())
}

fn window_conf() -> Conf {
    Conf {
        window_title: "kingdom-3ds (desktop)".to_owned(),
        window_width: (TOP_W * 2.0) as i32,
        window_height: ((TOP_H + BOT_H) * 2.0) as i32,
        high_dpi: false,
        ..Default::default()
    }
}

fn read_input(bag_tap: bool) -> RawInput {
    let left = is_key_down(KeyCode::Left) || is_key_down(KeyCode::A);
    let right = is_key_down(KeyCode::Right) || is_key_down(KeyCode::D);
    RawInput {
        direction: right as i8 - left as i8,
        sprint: is_key_down(KeyCode::LeftShift) || is_key_down(KeyCode::RightShift),
        pay: is_key_down(KeyCode::Space) || is_key_down(KeyCode::J),
        bag_tap,
    }
}

fn target(w: f32, h: f32) -> RenderTarget {
    let rt = render_target(w as u32, h as u32);
    rt.texture.set_filter(FilterMode::Nearest);
    rt
}

fn camera_for(rt: &RenderTarget, w: f32, h: f32) -> Camera2D {
    let mut cam = Camera2D::from_display_rect(Rect::new(0.0, 0.0, w, h));
    // Render targets are stored bottom-up; flip so (0,0) is top-left like the screens.
    cam.zoom.y = -cam.zoom.y;
    cam.render_target = Some(rt.clone());
    cam
}

#[macroquad::main(window_conf)]
async fn main() {
    let screenshot = std::env::args().skip_while(|a| a != "--screenshot").nth(1);
    let shot_frame: u32 = std::env::args()
        .skip_while(|a| a != "--frame")
        .nth(1)
        .and_then(|f| f.parse().ok())
        .unwrap_or(200);
    let top_rt = target(TOP_W, TOP_H);
    let bot_rt = target(BOT_W, BOT_H);
    // Last frame's top screen, which the water reflects.
    let previous_rt = target(TOP_W, TOP_H);
    let dir = std::env::var("KINGDOM_ASSETS").unwrap_or_else(|_| "assets".to_owned());
    let mut world_id = 0;
    let world: WorldData = match std::fs::read(std::path::Path::new(&dir).join("world.bin"))
        .map_err(|e| e.to_string())
        .and_then(|b| {
            world_id = save::fingerprint(&b);
            game::assetfile::decode_world(&b).map_err(|e| e.to_string())
        }) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("could not load {dir}/world.bin: {e} (re-run the extract tool)");
            std::process::exit(1);
        }
    };
    let mut assets = match Assets::load(std::path::Path::new(&dir), &world).await {
        Ok(a) => a,
        Err(e) => {
            eprintln!("could not load assets from {dir}/: {e}");
            eprintln!("run: cargo run -p extract -- --ripped reference/ripped/ExportedProject --out assets");
            std::process::exit(1);
        }
    };
    let sheet = assets.sheet.clone();
    let seed = std::env::args()
        .skip_while(|a| a != "--seed")
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let greed_at: Option<u32> = std::env::args()
        .skip_while(|a| a != "--greed-at")
        .nth(1)
        .and_then(|s| s.parse().ok());
    let island: usize = arg("--island")
        .unwrap_or(0)
        .min(world.levels.len().saturating_sub(1));
    let boat_at: Option<u32> = arg("--boat-at");
    let weather: Option<String> = arg("--weather");
    // Dev: start the clock at this hour (`--hour 23`).
    let start_hour: Option<f32> = arg("--hour");
    // Saves: off for screenshot runs and with --no-save. A saved run resumes
    // unless --new or --island is given.
    let storage = (screenshot.is_none() && !std::env::args().any(|a| a == "--no-save"))
        .then(storage::Files::new);
    let progress = if std::env::args().any(|a| a == "--unlocked") {
        Progress::everything()
    } else {
        storage
            .as_ref()
            .map(|s| Session::saved_progress(s))
            .unwrap_or_default()
    };
    let fresh = std::env::args().any(|a| a == "--new" || a == "--island");
    let slot = storage.as_ref().map_or(0, |s| Session::last_slot(s));
    // --load reads the saved game even in screenshot runs (still never writing).
    let saved = (storage.is_none() && !fresh && std::env::args().any(|a| a == "--load"))
        .then(|| Session::saved_game(&storage::Files::new(), slot, world_id, &world))
        .and_then(|r| match r {
            Ok(game) => game,
            Err(e) => {
                eprintln!("save preserved ({e:?})");
                None
            }
        });
    let mut app = Session::new(world, island, seed, progress);
    app.debug = std::env::args().any(|a| a == "--debug");
    if let Some(h) = start_hour {
        app.game.clock.time = h;
    }
    app.storage = storage.map(|s| Box::new(s) as Box<dyn Storage>);
    app.world_id = world_id;
    app.slot = slot;
    if app.storage.is_some() {
        if fresh {
            app.authorize_new_game();
        } else {
            app.load_from(slot);
        }
    }
    if std::env::args().any(|a| a == "--paused") {
        app.set_paused(true);
    }
    if std::env::args().any(|a| a == "--banner") {
        app.set_paused(true);
        app.editor = Some(Default::default());
    }
    let args: Vec<String> = std::env::args().collect();
    for w in args.windows(2).filter(|w| w[0] == "--tap") {
        if let Some((x, y)) = w[1].split_once(',') {
            let p = (x.parse().unwrap_or(0.0), y.parse().unwrap_or(0.0));
            if let Tap::Pause = touch::tap(&mut app, &sheet, p) {
                app.set_paused(!app.paused);
            }
        }
    }
    // Sound (off for screenshot runs and with --mute).
    let mut sound = (screenshot.is_none() && !std::env::args().any(|a| a == "--mute")).then(|| {
        (
            audio::Audio::new(std::path::Path::new(&dir), &app.world.audio),
            ::scene::sound::Player::new(&app.world),
        )
    });
    if let Some(g) = saved {
        app.resume(g);
    }
    if let Some(coins) = std::env::args()
        .skip_while(|a| a != "--coins")
        .nth(1)
        .and_then(|s| s.parse().ok())
    {
        app.game.wallet.set(coins);
    }
    if let Some(speed) = std::env::args()
        .skip_while(|a| a != "--speed")
        .nth(1)
        .and_then(|s| s.parse().ok())
    {
        app.speed = speed;
    }
    let pilot = game::autopilot::Autopilot {
        prefer: arg::<String>("--prefer")
            .map(|s| s.split(',').map(str::to_owned).collect())
            .unwrap_or_default(),
        goto: arg::<String>("--goto").and_then(|s| {
            let (x, f) = s.split_once('@')?;
            Some((x.parse().ok()?, f.parse().ok()?))
        }),
    };
    let build_every_second = std::env::args().any(|a| a == "--build");
    let mut acc = 0.0;
    let mut frame = 0u32;
    prevent_quit();

    loop {
        let scale = (screen_width() / TOP_W)
            .min(screen_height() / (TOP_H + BOT_H))
            .floor()
            .max(1.0);
        let top_origin = vec2(((screen_width() - TOP_W * scale) / 2.0).floor(), 0.0);
        let bot_origin = vec2(
            ((screen_width() - BOT_W * scale) / 2.0).floor(),
            TOP_H * scale,
        );

        if is_key_pressed(KeyCode::F1) {
            app.debug = !app.debug;
        }
        for (k, s) in [
            (KeyCode::Key1, 1),
            (KeyCode::Key2, 2),
            (KeyCode::Key3, 4),
            (KeyCode::Key4, 8),
        ] {
            if is_key_pressed(k) {
                app.speed = s;
            }
        }
        if is_key_pressed(KeyCode::N) {
            app.game.clock.time += 1.0;
        }
        // Dev: hold the lighting to one weather (`--weather "Clear Summer"`),
        // picking that weather's preset for the current part of the day.
        if let Some(w) = &weather {
            let part = match game::time::part_of_day(app.game.clock.time).0 {
                game::time::PartOfDay::Dawn => "Dawn",
                game::time::PartOfDay::Day => "Day",
                game::time::PartOfDay::Evening => "Evening",
                game::time::PartOfDay::Night => "Night",
            };
            let name = format!("{w} {part}");
            if let Some(p) = app.world.lights.iter().position(|l| l.name == name) {
                app.game.blender.add(&app.world, p as u16, 0.0);
            }
        }
        // The touch screen: the map after a run, otherwise the bag and pause.
        let mut bag_tap = false;
        let mut go = false;
        if is_mouse_button_pressed(MouseButton::Left) {
            let (mx, my) = mouse_position();
            let p = (vec2(mx, my) - bot_origin) / scale;
            match touch::tap(&mut app, &sheet, (p.x, p.y)) {
                Tap::Bag => bag_tap = true,
                Tap::Pause => app.set_paused(!app.paused),
                Tap::Go => go = true,
                Tap::Nothing => {}
            }
        }
        // The map by keys: ←/→ pick, Enter (or R after a loss) sets off.
        let map_open = app.map_shown();
        if map_open {
            if let Some(m) = &mut app.map {
                if is_key_pressed(KeyCode::Left) || is_key_pressed(KeyCode::A) {
                    m.cycle(-1);
                }
                if is_key_pressed(KeyCode::Right) || is_key_pressed(KeyCode::D) {
                    m.cycle(1);
                }
            }
            go |= is_key_pressed(KeyCode::Enter) || is_key_pressed(KeyCode::R);
        }
        if go && map_open {
            app.next_island();
        }
        // orig: the pause menu's Save.
        if is_key_pressed(KeyCode::F5) && !app.save() {
            eprintln!("saving failed");
        }
        // orig: Menu.OnButtonQuit saves first.
        if is_quit_requested() {
            app.save();
            break;
        }
        if is_key_pressed(KeyCode::G) || Some(frame) == greed_at {
            // Dev: a troll wave from the nearest edge of the screen.
            let x = app.camera_x + 7.0;
            for _ in 0..3 {
                app.game.greed.spawn_troll(
                    game::greed::EnemyKind::Troll,
                    x,
                    0,
                    false,
                    &mut app.game.rng,
                );
            }
        }
        if is_key_pressed(KeyCode::H) || (Some(frame) == boat_at && boat_at.is_some()) {
            app.game.dev_boat_at_beach(&app.world);
            app.camera_x = app.game.monarch.x;
        }
        if is_key_pressed(KeyCode::B) {
            app.game.complete_construction(&app.world);
        }
        if is_key_pressed(KeyCode::I) {
            app.god = !app.god;
        }
        if is_key_pressed(KeyCode::C) {
            let c = app.game.wallet.coins();
            app.game.wallet.set(c + 10);
        }
        let mut raw = read_input(bag_tap);
        if is_key_pressed(KeyCode::Escape) || (is_key_pressed(KeyCode::Enter) && !map_open) {
            app.set_paused(!app.paused);
        }
        if screenshot.is_some() {
            raw = pilot.input(&app.game, &app.world, frame);
        }
        if build_every_second && frame.is_multiple_of(60) {
            app.game.complete_construction(&app.world);
        }

        if !app.paused {
            acc += get_frame_time().min(0.1);
            if screenshot.is_some() {
                acc = TICK;
            }
            while acc >= TICK {
                app.step(raw);
                raw.bag_tap = false;
                acc -= TICK;
            }
        }

        match &mut sound {
            Some((mixer, player)) => {
                player.update(&mut app, &sheet, mixer, get_frame_time().min(0.1))
            }
            None => {
                app.cues.clear();
                app.music_cues.clear();
            }
        }
        set_camera(&camera_for(&top_rt, TOP_W, TOP_H));
        top::draw(&app, &sheet, &mut assets);
        set_camera(&camera_for(&previous_rt, TOP_W, TOP_H));
        draw_texture(&top_rt.texture, 0.0, 0.0, WHITE);
        top::draw_reflection_extras(&app, &sheet, &mut assets);
        assets.previous = Some(previous_rt.texture.clone());
        set_camera(&camera_for(&bot_rt, BOT_W, BOT_H));
        touch::draw(&app, &sheet, &mut assets);

        set_default_camera();
        clear_background(BLACK);
        for (rt, origin, w, h) in [
            (&top_rt, top_origin, TOP_W, TOP_H),
            (&bot_rt, bot_origin, BOT_W, BOT_H),
        ] {
            draw_texture_ex(
                &rt.texture,
                origin.x,
                origin.y,
                WHITE,
                DrawTextureParams {
                    dest_size: Some(vec2(w * scale, h * scale)),
                    ..Default::default()
                },
            );
        }

        frame += 1;
        if let Some(path) = &screenshot {
            if frame == shot_frame && std::env::var_os("KINGDOM_UNITS").is_some() {
                eprintln!("wallet {}", app.game.wallet.coins());
                for u in &app.game.population.units {
                    eprintln!("{:?} x={:.2} coins={} ai={:?}", u.role, u.x, u.coins, u.ai);
                }
                for t in &app.game.population.tools {
                    eprintln!(
                        "tool {:?} x={:.2} y={:.2} shop={:?}",
                        t.kind, t.x, t.y, t.shop
                    );
                }
            }
            if frame == shot_frame && std::env::var_os("KINGDOM_DUMP").is_some() {
                for (key, item) in draw_list(&app) {
                    if let DrawItem::Part(oi, pi) = item {
                        let o = &app.game.level.objects[oi];
                        let proto = &app.world.protos[o.proto as usize];
                        let sprite = o.parts[pi]
                            .sprite
                            .map(|s| app.world.sprites[s as usize].as_str());
                        let at = layer_to_screen(&app, o.x, o.y, app.world.layer(o.layer).ratio);
                        eprintln!(
                            "{:?} {:<28} {:<24} {:?} screen=({}, {}) z={:.2}",
                            o.layer,
                            proto.name,
                            sprite.unwrap_or("-"),
                            key,
                            at.0,
                            at.1,
                            -key.2
                        );
                    }
                }
            }
            if frame == shot_frame {
                get_screen_data().export_png(path);
                if std::env::var_os("KINGDOM_LIGHTS").is_some() {
                    for (p, w) in app.game.blender.presets() {
                        eprintln!("light {:<40} {w:.2}", app.world.lights[p as usize].name);
                    }
                }
                break;
            }
        }
        next_frame().await;
    }
}
