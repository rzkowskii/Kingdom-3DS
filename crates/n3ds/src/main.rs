//! Nintendo 3DS backend. `tools/extract`'s `pack3ds` fills `romfs/` from
//! your own extracted assets; then `cargo 3ds build --release` here makes
//! `kingdom.3dsx` with them inside.
//!
//! Controls: Circle Pad / D-Pad walk (double-tap to gallop), B sprint,
//! A, L, R or D-Pad up pay (tap drops a coin), touch the bag to drop a
//! coin, START pause.
//! After a run, pick an island on the touch screen (or ←/→ and A). The 3D
//! slider sets the depth. Sound plays from `sdmc:/3ds/kingdom-3ds/audio`
//! (`pack3ds` makes it) and needs `sdmc:/3ds/dspfirm.cdc`.

mod audio;
mod gpu;
mod savefile;
mod storage;

use std::rc::Rc;

use ctru::prelude::*;
use game::controls::RawInput;
use game::data::WorldData;
use game::save;
use game::state::TICK;
use scene::session::Session;
use scene::sheet::Sheet;
use scene::{top, touch};

const ROMFS: &str = "romfs:";
/// Eye separation of the farthest layers at full slider, in pixels.
const MAX_STEREO: f32 = 12.0;
/// Sound clips, copied onto the card by hand (see `pack3ds`).
const SD_DIR: &str = "sdmc:/3ds/kingdom-3ds";

/// Shows an error on the touch screen until START.
fn fail(gfx: &Gfx, hid: &mut Hid, apt: &Apt, msg: &str) {
    let _console = Console::new(gfx.bottom_screen.borrow_mut());
    println!("Kingdom 3DS could not start:\n\n{msg}\n\nPress START to exit.");
    while apt.main_loop() {
        hid.scan_input();
        if hid.keys_down().contains(KeyPad::START) {
            break;
        }
        gfx.wait_for_vblank();
    }
}

fn load() -> Result<(WorldData, u32, Sheet), String> {
    let bytes =
        std::fs::read(format!("{ROMFS}/world.bin")).map_err(|e| format!("world.bin: {e}"))?;
    let world: WorldData =
        game::assetfile::decode_world(&bytes).map_err(|e| format!("world.bin: {e}"))?;
    let world_id = save::fingerprint(&bytes);
    let sheet_bytes =
        std::fs::read(format!("{ROMFS}/sheet.bin")).map_err(|e| format!("sheet.bin: {e}"))?;
    let mut sheet: Sheet = game::assetfile::decode(game::assetfile::Kind::Sheet, &sheet_bytes)
        .map_err(|e| format!("sheet.bin: {e}"))?;
    sheet.validate()?;
    sheet.link(&world.sprites);
    let mesh_file = scene::cloud_mesh::FILE_NAME;
    match std::fs::read(format!("{ROMFS}/{mesh_file}")) {
        Ok(bytes) => sheet.link_cloud_meshes(&world, &bytes)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{mesh_file}: {e}")),
    }
    Ok((world, world_id, sheet))
}

fn main() {
    // Enable the New 3DS clock and L2 cache wherever they exist
    // (it does nothing on an Old 3DS).
    unsafe { ctru_sys::osSetSpeedupEnable(true) };
    let gfx = Gfx::new().expect("gfx");
    ctru::set_panic_hook(false);
    let mut hid = Hid::new().expect("hid");
    let apt = Apt::new().expect("apt");
    let _romfs = match ctru::services::romfs::RomFS::new() {
        Ok(r) => r,
        Err(_) => {
            fail(&gfx, &mut hid, &apt, "no romfs (build with pack3ds first)");
            return;
        }
    };
    let (world, world_id, sheet) = match load() {
        Ok(v) => v,
        Err(e) => {
            fail(&gfx, &mut hid, &apt, &e);
            return;
        }
    };
    let sheet = Rc::new(sheet);
    let cloud_ids = world
        .clouds
        .iter()
        .flat_map(game::data::CloudsDef::sprite_ids)
        .filter_map(|id| sheet.world(id));
    let mut gpu = match gpu::Gpu::new(sheet.clone(), ROMFS, cloud_ids) {
        Ok(g) => g,
        Err(e) => {
            fail(&gfx, &mut hid, &apt, &e);
            return;
        }
    };

    // Saves on the SD card. orig: PersistentManager's AUTO slot, UnlockManager's unlocks.
    let files = storage::SdCard::new();
    let progress = Session::saved_progress(&files);
    let slot = Session::last_slot(&files);
    let mut s = Session::new(world, 0, seed(), progress);
    s.storage = Some(Box::new(files));
    s.slot = slot;
    s.debug_lines = false;
    s.world_id = world_id;
    s.load_from(slot);
    let mut sound = audio::Audio::new(SD_DIR, &s.world.audio);
    s.warning = sound.files_problem().unwrap_or_default();
    let mut player = scene::sound::Player::new(&s.world);
    // Clips are stored 4× hot under a limiter (`scene::clipfile::DRIVE`): 4
    // keeps that loudness instead of dividing it back out, and 2 more for
    // the small speakers (each voice still stops at full volume).
    player.boost = 8.0;

    // 3D follows the slider; at 0 only the left eye is drawn.
    let mut stereo_on = false;
    let mut frames = 0u32;
    let mut avg = [0.0f32; 6];
    let mut acc = 0.0f32;
    // SELECT was part of the god-mode chord this press.
    let mut chorded = false;
    let mut chord_held = false;
    let mut prof_last: Vec<(String, f64)> = Vec::new();
    let mut ticks_since = 0u32;
    let mut last = std::time::Instant::now();
    while apt.main_loop() {
        hid.scan_input();
        let down = hid.keys_down();
        let held = hid.keys_held();
        let mut bag_tap = false;
        let mut go = false;
        if down.contains(KeyPad::TOUCH) {
            let (x, y) = hid.touch_position();
            match touch::tap(&mut s, &sheet, (x as f32, y as f32)) {
                touch::Tap::Bag => bag_tap = true,
                touch::Tap::Pause => s.set_paused(!s.paused),
                touch::Tap::Go => go = true,
                touch::Tap::Nothing => {}
            }
        }
        let map_open = s.map_shown();
        if map_open {
            if let Some(m) = &mut s.map {
                if down.intersects(KeyPad::LEFT) {
                    m.cycle(-1);
                }
                if down.intersects(KeyPad::RIGHT) {
                    m.cycle(1);
                }
            }
            go |= down.contains(KeyPad::A);
            if go {
                s.next_island();
            }
        } else if down.contains(KeyPad::START) {
            s.set_paused(!s.paused);
        }
        // SELECT + R + Y toggles god mode; SELECT alone (on release) the
        // debug overlay.
        let god = KeyPad::SELECT | KeyPad::R | KeyPad::Y;
        if held.contains(god) && !chord_held {
            s.god = !s.god;
            chorded = true;
        }
        chord_held = held.contains(god);
        if hid.keys_up().contains(KeyPad::SELECT) {
            if !chorded {
                s.debug = !s.debug;
            }
            chorded = false;
        }
        let select = held.contains(KeyPad::SELECT);
        let mut diagnostics = platform::diagnostics::RenderDiagnostics {
            painter: gpu.painter,
            reflection: gpu.reflect_mode,
        };
        diagnostics.update(
            s.debug,
            down.contains(KeyPad::X),
            down.contains(KeyPad::Y),
            select,
        );
        gpu.painter = diagnostics.painter;
        gpu.reflect_mode = diagnostics.reflection;
        let raw = if map_open || s.paused {
            RawInput::default()
        } else {
            RawInput {
                direction: held.intersects(KeyPad::RIGHT) as i8
                    - held.intersects(KeyPad::LEFT) as i8,
                sprint: held.contains(KeyPad::B),
                pay: !select
                    && held.intersects(KeyPad::A | KeyPad::L | KeyPad::R | KeyPad::DPAD_UP),
                bag_tap,
            }
        };
        // One tick per frame, as the screen refreshes; a slow frame runs
        // extra ticks (up to 4) to keep game time real-time. The screen
        // refreshes at ~59.83 Hz, not 60, and frame times jitter, so `acc`
        // (ticks owed) has slack: a plain rounding would sometimes run 0 or
        // 2 ticks in a row of smooth frames, which judders while scrolling.
        let now = std::time::Instant::now();
        acc += (now - last).as_secs_f32().min(0.1) / TICK;
        last = now;
        let ticks = if acc >= 2.0 { (acc as u32).min(4) } else { 1 };
        acc = (acc - ticks as f32).clamp(-1.0, 4.0);
        // The tick profiler reads the clock at every mark: only while the
        // overlay that shows it is up.
        game::prof::set_enabled(s.debug);
        let t0 = std::time::Instant::now();
        s.step_ticks(raw, ticks);
        ticks_since += ticks;
        let t1 = std::time::Instant::now();
        sound.update();
        player.update(&mut s, &sheet, &mut sound, ticks as f32 * TICK);
        let t_sound = std::time::Instant::now();
        let slider = unsafe { ctru_sys::osGet3DSliderState() };
        if (slider > 0.0) != stereo_on {
            stereo_on = slider > 0.0;
            unsafe { ctru_sys::gfxSet3D(stereo_on) };
        }
        gpu.stereo = if stereo_on { slider * MAX_STEREO } else { 0.0 };
        gpu.begin();
        let t2 = std::time::Instant::now();
        gpu.screen(true);
        top::draw(&s, &sheet, &mut gpu);
        gpu.capture_reflections(|gpu| top::draw_reflection_extras(&s, &sheet, gpu));
        gpu.screen(false);
        touch::draw(&s, &sheet, &mut gpu);
        let t_scene = std::time::Instant::now();
        // The GPU work is queued in end() now, so it counts as drawing.
        gpu.end();
        let t3 = std::time::Instant::now();
        // Debug overlay: simulation and drawing time (CPU), averaged.
        let ms = |a: std::time::Instant, b: std::time::Instant| (b - a).as_secs_f32() * 1000.0;
        avg[0] += (ms(t0, t1) - avg[0]) * 0.05;
        avg[1] += (ms(t2, t3) - avg[1]) * 0.05;
        avg[2] += (ms(t_sound, t2) - avg[2]) * 0.05;
        avg[3] += (ms(t1, t_sound) - avg[3]) * 0.05;
        avg[4] += (ms(t_scene, t3) - avg[4]) * 0.05;
        avg[5] += (gpu.gpu_ms() - avg[5]) * 0.05;
        frames = frames.wrapping_add(1);
        if s.debug && frames % 30 == 0 {
            // Per-system tick times since the last report (game "profile" feature).
            let now_prof = game::prof::report();
            let mut rows: Vec<(String, f64)> = now_prof
                .iter()
                .map(|(n, t)| {
                    let before = prof_last.iter().find(|p| &p.0 == n).map_or(0.0, |p| p.1);
                    (n.clone(), (t - before) * 1000.0 / ticks_since.max(1) as f64)
                })
                .collect();
            rows.sort_by(|a, b| b.1.total_cmp(&a.1));
            let top: Vec<String> = rows
                .iter()
                .take(5)
                .map(|(n, ms)| format!("{n} {ms:.2}"))
                .collect();
            prof_last = now_prof;
            ticks_since = 0;
            s.note = format!(
                "{}\n{}\n{}",
                top.join("  "),
                sound.status(),
                format!(
                "sim {:.1} ms ({ticks} ticks)  sound {:.1}  draw {:.1} (end {:.1})  wait {:.1}  gpu {:.1}  {} verts {} calls  {} reflection {}",
                avg[0],
                avg[3],
                avg[1],
                avg[4],
                avg[2],
                avg[5],
                gpu.vertices(),
                gpu.draws,
                if gpu.painter { "painter" } else { "depth" },
                gpu.reflect_mode,
            ));
        }
    }
    // orig: Menu.OnButtonQuit saves first; leaving through HOME does the same.
    s.save();
}

/// A fresh island seed from the clock.
fn seed() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |d| d.as_secs() as u32 ^ d.subsec_nanos())
}
