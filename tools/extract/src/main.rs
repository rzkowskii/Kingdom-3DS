//! Asset pipeline: AssetRipper export of the user's Kingdom_Data → platform-ready assets.
//!
//! ```text
//! cargo run -p extract -- --ripped reference/ripped/ExportedProject --out assets
//! ```
//!
//! Current outputs (desktop):
//! - `sprites/page_N.png` + `sprites.json`: every Sprite, cut out of Unity's
//!   tightly packed atlases by its own mesh and repacked into ≤1024² pages.
//! - `clips.json`: versioned sprite keys, transform/color curves and event markers.
//! - `animation-report.json`: retained coverage, duplicate aliases and unsupported features.
//! - `scenes/{main,blocks}.json`, `prefabs.json`, `data.json`: object trees
//!   and ScriptableObjects with all serialized fields (see `scenes.rs`).
//! - `world.bin`: versioned `game::data::WorldData` envelope (postcard payload).
//! - `cloud-meshes.bin`: authored tight sprite meshes keyed by world sprite ID.
//! - `atmosphere.bin`: scene-local fog and mist controllers and tight meshes.

mod audio;
mod boulder_fx;
mod clips;
mod fonts;
mod pack;
mod scenes;
mod sprites;
mod world;
mod yaml;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn usage() -> ExitCode {
    eprintln!("usage: extract --ripped <AssetRipper ExportedProject dir> --out <assets dir>");
    ExitCode::FAILURE
}

/// Unity GUID → asset path, from every `.meta` file under `dir`.
fn guid_map(dir: &Path) -> HashMap<String, PathBuf> {
    let mut map = HashMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "meta") {
                let Ok(text) = std::fs::read_to_string(&p) else {
                    continue;
                };
                if let Some(guid) = text.lines().find_map(|l| l.strip_prefix("guid: ")) {
                    map.insert(guid.trim().to_owned(), p.with_extension(""));
                }
            }
        }
    }
    map
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let arg = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
    };
    let (Some(ripped), Some(out)) = (arg("--ripped"), arg("--out")) else {
        return usage();
    };
    let assets = Path::new(ripped).join("Assets");
    let out = PathBuf::from(out);
    if !assets.is_dir() {
        eprintln!(
            "{} is not an AssetRipper ExportedProject (no Assets/)",
            ripped
        );
        return ExitCode::FAILURE;
    }
    if let Err(e) = std::fs::create_dir_all(&out) {
        eprintln!("{}: {e}", out.display());
        return ExitCode::FAILURE;
    }

    let guids = guid_map(&assets);
    println!("indexed {} assets", guids.len());

    let mut sheet = match sprites::extract_all(&assets.join("Sprite"), &guids) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sprites: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "cut {} sprites ({} skipped)",
        sheet.sprites.len(),
        sheet.skipped
    );
    let boulder_fx = match boulder_fx::extract(&assets, &guids, &mut sheet) {
        Ok(effect) => effect,
        Err(error) => {
            eprintln!("boulder particles: {error}");
            return ExitCode::FAILURE;
        }
    };

    // The game's pixel font, as glyph sprites.
    match fonts::extract(&assets.join("Resources/fonts"), "Kingdom", "Font Texture") {
        Ok((font, glyphs)) => {
            println!("font: {} glyphs", font.glyphs.len());
            sheet.sprites.extend(glyphs);
            let json = serde_json::to_string(&font).expect("font serialize");
            if let Err(e) = std::fs::write(out.join("font.json"), json) {
                eprintln!("font: {e}");
                return ExitCode::FAILURE;
            }
        }
        Err(e) => eprintln!("warning: font: {e}"),
    }

    let all_clips = match clips::extract_all(&assets.join("AnimationClip"), &sheet.names_by_guid) {
        Ok(all) => all,
        Err(e) => {
            eprintln!("clips: {e}");
            return ExitCode::FAILURE;
        }
    };
    let file = clips::ClipFile {
        version: clips::FORMAT_VERSION,
        clips: &all_clips,
    };
    let json = serde_json::to_string(&file).expect("clips serialize");
    if let Err(e) = std::fs::write(out.join("clips.json"), json) {
        eprintln!("clips: {e}");
        return ExitCode::FAILURE;
    }
    let (animations, report) = match clips::library(&assets, &guids, &all_clips) {
        Ok(result) => result,
        Err(e) => {
            eprintln!("animators: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = clips::validate_gameplay(&animations) {
        eprintln!("animation timelines: {e}");
        return ExitCode::FAILURE;
    }
    if let Err(e) = std::fs::write(
        out.join("animation-report.json"),
        serde_json::to_string_pretty(&report).expect("report serialize"),
    ) {
        eprintln!("animation report: {e}");
        return ExitCode::FAILURE;
    }
    println!("wrote {} clips; {} typed event markers; {} unsupported animation features (animation-report.json)", all_clips.len(), report.events, report.unsupported.len());

    if let Err(e) = export_scenes(&assets, &guids, &out, &mut sheet, animations, boulder_fx) {
        eprintln!("scenes: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn write_json(path: &Path, v: &serde_json::Value) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let text = serde_json::to_string(v).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

fn rel(assets: &Path, p: &Path) -> String {
    p.strip_prefix(assets)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

fn export_scenes(
    assets: &Path,
    guids: &HashMap<String, PathBuf>,
    out: &Path,
    sheet: &mut sprites::Sheet,
    animations: game::animation::Library,
    boulder_fx: boulder_fx::Pending,
) -> Result<(), String> {
    let r = scenes::Resolver::new(guids, assets);
    let main = scenes::export_file(&assets.join("main.unity"), &r)?;
    write_json(&out.join("scenes/main.json"), &main)?;
    let blocks = scenes::export_file(&assets.join("blocks.unity"), &r)?;
    write_json(&out.join("scenes/blocks.json"), &blocks)?;
    // The menu scene holds the island map.
    let loader = scenes::export_file(&assets.join("loader.unity"), &r)?;
    write_json(&out.join("scenes/loader.json"), &loader)?;

    let mut prefabs = serde_json::Map::new();
    for p in scenes::files_with_ext(assets, "prefab") {
        let v = scenes::export_file(&p, &r)?;
        if let Some(root) = v["roots"].get(0) {
            prefabs.insert(rel(assets, &p), root.clone());
        }
    }
    println!("exported {} prefabs", prefabs.len());
    write_json(
        &out.join("prefabs.json"),
        &serde_json::Value::Object(prefabs.clone()),
    )?;

    let mut data = serde_json::Map::new();
    for dir in ["Resources", "MonoBehaviour"] {
        for p in scenes::files_with_ext(&assets.join(dir), "asset") {
            // Some third-party assets (Rewired controller maps) aren't valid YAML
            // for our parser; they're irrelevant, so skip them.
            match scenes::export_data_asset(&p, &r) {
                Ok(Some(v)) => {
                    data.insert(rel(assets, &p), v);
                }
                Ok(None) => {}
                Err(e) => eprintln!("skip {e}"),
            }
        }
    }
    println!("exported {} data assets", data.len());
    write_json(
        &out.join("data.json"),
        &serde_json::Value::Object(data.clone()),
    )?;

    let sizes: world::SpriteSizes = sheet
        .sprites
        .iter()
        .map(|s| (s.name.clone(), (s.img.w, s.img.h, s.ppu)))
        .collect();
    let inputs = world::Inputs {
        main: &main,
        blocks: &blocks,
        loader: &loader,
        prefabs: &prefabs,
        data: &data,
        sprite_names: &sheet.names_by_path,
        sprite_sizes: &sizes,
        assets_dir: assets,
    };
    let (mut world, clip_paths) = world::build(&inputs)?;
    let atmosphere = world::atmosphere(&inputs)?;
    let bytes = game::assetfile::encode(game::assetfile::Kind::Atmosphere, &atmosphere)
        .map_err(|e| e.to_string())?;
    scene::atmosphere_data::AtmosphereDef::decode(&bytes)?;
    std::fs::write(out.join(scene::atmosphere_data::FILE_NAME), bytes)
        .map_err(|e| e.to_string())?;
    boulder_fx.install(&mut world)?;
    world.animations = animations;
    if !world.transforms_valid() {
        return Err("invalid extracted transform hierarchy".into());
    }
    let mut cloud_ids: HashMap<_, _> = world
        .clouds
        .iter()
        .flat_map(game::data::CloudsDef::sprite_ids)
        .map(|id| (world.sprites[usize::from(id)].as_str(), id))
        .collect();
    let mut meshes = scene::cloud_mesh::CloudMeshes::default();
    for sprite in &mut sheet.sprites {
        if let Some(id) = cloud_ids.remove(sprite.name.as_str()) {
            let (path, _) = sheet
                .names_by_path
                .iter()
                .find(|(_, name)| **name == sprite.name)
                .ok_or_else(|| format!("missing source path for cloud sprite {}", sprite.name))?;
            meshes
                .entries
                .push((id, sprites::cloud_mesh(&assets.join(path))?));
            sprites::bake_cloud_coverage(&mut sprite.img);
        }
    }
    if !cloud_ids.is_empty() {
        return Err("missing cloud sprites in the cut sheet".into());
    }
    meshes.entries.sort_by_key(|&(id, _)| id);
    let bytes = game::assetfile::encode(game::assetfile::Kind::CloudMeshes, &meshes)
        .map_err(|e| e.to_string())?;
    std::fs::write(out.join(scene::cloud_mesh::FILE_NAME), bytes).map_err(|e| e.to_string())?;
    pack::write_pages(sheet, &out.join("sprites"), &out.join("sprites.json"))
        .map_err(|e| format!("packing: {e}"))?;
    audio::convert(assets, out, &mut world.audio, &clip_paths)?;
    println!(
        "audio: {} sounds, {} clips ({:.0} s), {} script fields",
        world.audio.sounds.len(),
        world.audio.clips.len(),
        world.audio.clips.iter().map(|c| c.seconds).sum::<f32>(),
        world.audio.refs.len()
    );
    let bytes =
        game::assetfile::encode(game::assetfile::Kind::World, &world).map_err(|e| e.to_string())?;
    std::fs::write(out.join("world.bin"), &bytes).map_err(|e| e.to_string())?;
    println!(
        "world: {} blocks, {} protos, {} scatterables, {} islands, {} lights, {} days ({} KiB)",
        world.blocks.len(),
        world.protos.len(),
        world.scatterables.len(),
        world.levels.len(),
        world.lights.len(),
        world.days.len(),
        bytes.len() / 1024
    );
    Ok(())
}
