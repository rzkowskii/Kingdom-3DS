//! Extract the two non-looping Playground bursts referenced by Boulder.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use game::data::{BoulderEmitterDef, BoulderFxDef, BoulderGradientDef, SpriteId, WorldData};
use serde_json::Value;

use crate::{
    scenes::{export_file, Resolver},
    sprites::{Image, Sheet, Sprite},
    yaml::{field, guid_of},
};

pub struct Pending {
    definition: BoulderFxDef,
    names: Vec<String>,
}

impl Pending {
    pub fn install(mut self, world: &mut WorldData) -> Result<(), String> {
        let base = world.sprites.len();
        for emitter in [&mut self.definition.dust, &mut self.definition.fragments] {
            for id in &mut emitter.sprites {
                *id = SpriteId::try_from(base + usize::from(*id))
                    .map_err(|_| "too many sprite IDs for the boulder particles")?;
            }
        }
        if !self.definition.valid(base + self.names.len()) {
            return Err("invalid Boulder particle definition".into());
        }
        world.sprites.extend(self.names);
        world.boulder_fx = Some(self.definition);
        Ok(())
    }
}

pub fn extract(
    assets: &Path,
    guids: &HashMap<String, PathBuf>,
    sheet: &mut Sheet,
) -> Result<Pending, String> {
    let (definition, sprites) = load(assets, guids)?;
    let names = sprites.iter().map(|sprite| sprite.name.clone()).collect();
    sheet.sprites.extend(sprites);
    Ok(Pending { definition, names })
}

fn load(
    assets: &Path,
    guids: &HashMap<String, PathBuf>,
) -> Result<(BoulderFxDef, Vec<Sprite>), String> {
    let resolver = Resolver::new(guids, assets);
    let source = export_file(
        &assets.join("Resources/prefabs/objects/Boulder.prefab"),
        &resolver,
    )?;
    let root = &source["roots"][0];
    let boulder = component(root, "Boulder")?;
    if !root["scale"].as_array().is_some_and(|scale| {
        scale.len() == 3
            && scale
                .iter()
                .all(|axis| axis.as_f64().is_some_and(|v| (v - 1.0).abs() <= 1e-6))
    }) {
        return Err("unsupported Boulder lossy scale for particles".into());
    }
    let mut sprites = Vec::new();
    let mut emitter = |spawner_field: &str, prefix: &str| {
        let id = boulder[spawner_field]["local"]
            .as_i64()
            .ok_or("missing Boulder particle spawner")?;
        let spawner = root["components"]
            .as_array()
            .and_then(|components| {
                components.iter().find(|component| {
                    component["type"] == "PoolSpawner" && component["id"].as_i64() == Some(id)
                })
            })
            .ok_or("Boulder particle spawner reference must point to its root PoolSpawner")?;
        let prefab = spawner["fields"]["prefab"]["asset"]
            .as_str()
            .ok_or("missing particle prefab asset")?;
        let file = export_file(&assets.join(prefab), &resolver)?;
        let emitter = load_emitter(assets, guids, &file["roots"][0], prefix, &mut sprites)
            .map_err(|error| format!("{prefab}: {error}"))?;
        Ok::<_, String>(emitter)
    };
    let dust = emitter("dustSpawner", "dust")?;
    let fragments = emitter("fragmentsSpawner", "fragment")?;
    let definition = BoulderFxDef { dust, fragments };
    if !definition.valid(sprites.len()) {
        return Err("invalid Boulder particle definition".into());
    }
    Ok((definition, sprites))
}

fn component<'a>(root: &'a Value, name: &str) -> Result<&'a Value, String> {
    root["components"]
        .as_array()
        .and_then(|components| {
            components
                .iter()
                .find(|component| component["type"] == name)
        })
        .map(|component| &component["fields"])
        .ok_or_else(|| format!("missing {name}"))
}

fn number(fields: &Value, name: &str) -> Result<f32, String> {
    fields[name]
        .as_f64()
        .map(|v| v as f32)
        .filter(|v| v.is_finite())
        .ok_or_else(|| format!("invalid {name}"))
}

fn vector<const N: usize>(fields: &Value, name: &str, axes: [&str; N]) -> Result<[f32; N], String> {
    let mut out = [0.0; N];
    for (value, axis) in out.iter_mut().zip(axes) {
        *value = number(&fields[name], axis)?;
    }
    Ok(out)
}

fn gradient(fields: &Value) -> Result<BoulderGradientDef, String> {
    let color = vector(fields, "key0", ["r", "g", "b", "a"])?;
    let end = vector(fields, "key1", ["r", "g", "b", "a"])?;
    let at = |name| fields[name].as_u64();
    let start = at("atime0")
        .filter(|&start| start < 65535)
        .ok_or("invalid alpha start key")?;
    if at("m_Mode") != Some(0)
        || at("m_NumColorKeys") != Some(2)
        || at("m_NumAlphaKeys") != Some(2)
        || at("ctime0") != Some(0)
        || at("ctime1") != Some(65535)
        || at("atime1") != Some(65535)
        || color[..3] != end[..3]
        || end[3] != 0.0
    {
        return Err("unsupported particle gradient".into());
    }
    Ok(BoulderGradientDef {
        color,
        fade_start: start as f32 / 65535.0,
    })
}

fn load_emitter(
    assets: &Path,
    guids: &HashMap<String, PathBuf>,
    root: &Value,
    name: &str,
    sprites: &mut Vec<Sprite>,
) -> Result<BoulderEmitterDef, String> {
    let playground = component(root, "PlaygroundParticlesC")?;
    for (key, expected) in [
        ("source", 1),
        ("emit", 1),
        ("loop", 0),
        ("disableOnDone", 1),
        ("sorting", 2),
        ("emissionRate", 1),
        ("lifetimeOffset", 0),
        ("lifetimeValueMethod", 1),
        ("applySourceScatter", 1),
        ("sourceScatterMethod", 0),
        ("applyInitialLocalVelocity", 1),
        ("initialLocalVelocityMethod", 0),
        ("applyInitialVelocity", 0),
        ("applyLifetimeVelocity", 0),
        ("collision", 0),
        ("applyLifetimeSize", 0),
        ("applyParticleArraySize", 0),
        ("separateRotationAxes", 0),
        ("rotateTowardsDirection", 0),
        ("sizeMin", 1),
        ("sizeMax", 1),
        ("colorMethod", 0),
        ("velocityScale", 1),
        ("particleTimescale", 1),
        ("turbulenceType", 0),
        ("prewarm", 0),
    ] {
        if playground[key].as_i64() != Some(expected) {
            return Err(format!("unsupported Playground {key}"));
        }
    }
    if playground["sourceTransform"]["local"] != root["transform_id"]
        || playground["scatterScale"] != serde_json::json!({"x": 1, "y": 1, "z": 1})
    {
        return Err("unsupported Playground source transform or scatter scale".into());
    }
    let system = component(root, "ParticleSystem")?;
    let renderer = component(root, "ParticleSystemRenderer")?;
    if system["moveWithTransform"].as_i64() != Some(1)
        || renderer["m_RenderMode"].as_i64() != Some(0)
        || renderer["m_SortingOrder"].as_i64() != Some(0)
        || renderer["m_SortingLayer"].as_i64() != Some(0)
    {
        return Err("unsupported particle simulation space or billboard renderer".into());
    }
    let material = renderer["m_Materials"]
        .as_array()
        .and_then(|materials| materials.first())
        .and_then(|reference| reference["asset"].as_str())
        .ok_or("missing particle material")?;
    let material = std::fs::read_to_string(assets.join(material)).map_err(|e| e.to_string())?;
    let texture = field(&material, "m_Texture")
        .and_then(guid_of)
        .and_then(|guid| guids.get(guid))
        .ok_or("missing particle material texture")?;
    let image = Image::load_png(texture)?;

    let uv = &system["UVModule"];
    let tiles = |axis| {
        uv[axis]
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .filter(|v| (1..=16).contains(v))
            .ok_or("invalid particle atlas grid")
    };
    let (columns, rows) = (tiles("tilesX")?, tiles("tilesY")?);
    let frame = &uv["frameOverTime"];
    let animate = frame["minMaxState"].as_i64() == Some(1);
    if uv["enabled"].as_i64() != Some(1)
        || uv["mode"].as_i64() != Some(0)
        || uv["animationType"].as_i64() != Some(1)
        || uv["cycles"].as_i64() != Some(1)
        || uv["flipU"].as_i64() != Some(0)
        || uv["flipV"].as_i64() != Some(0)
        || uv["startFrame"]["minMaxState"].as_i64() != Some(0)
        || uv["startFrame"]["scalar"].as_i64() != Some(0)
        || image.w != columns * 8
        || image.h != rows * 8
    {
        return Err("unsupported particle atlas mode".into());
    }
    if animate {
        let curve = &frame["maxCurve"]["m_Curve"];
        if rows != 1
            || columns < 2
            || uv["randomRow"].as_i64() != Some(0)
            || curve.as_array().map(Vec::len) != Some(2)
            || number(&curve[0], "time")? != 0.0
            || number(&curve[0], "value")? != 0.0
            || number(&curve[1], "time")? != 1.0
            || number(&curve[1], "value")? != 1.0
            || number(&curve[0], "outSlope")? != 1.0
            || number(&curve[1], "inSlope")? != 1.0
        {
            return Err("unsupported particle atlas animation curve".into());
        }
    } else if columns != 1
        || uv["randomRow"].as_i64() != Some(1)
        || frame["minMaxState"].as_i64() != Some(0)
        || number(frame, "scalar")? != 0.0
    {
        return Err("unsupported particle atlas random-row settings".into());
    }

    let mut sprite_ids = Vec::new();
    for y in 0..rows {
        for x in 0..columns {
            let mut pixels = Vec::with_capacity(8 * 8 * 4);
            for row in y * 8..y * 8 + 8 {
                let start = ((row * image.w + x * 8) * 4) as usize;
                pixels.extend_from_slice(&image.px[start..start + 8 * 4]);
            }
            sprite_ids
                .push(SpriteId::try_from(sprites.len()).map_err(|_| "too many particle tiles")?);
            sprites.push(Sprite {
                name: format!("__boulder_{name}_{}", y * columns + x),
                img: Image {
                    w: 8,
                    h: 8,
                    px: pixels,
                },
                pivot: (4.0, 4.0),
                ppu: 32.0,
            });
        }
    }
    let colors = match playground["colorSource"].as_i64() {
        Some(1) => vec![gradient(&playground["lifetimeColor"])?],
        Some(2) => playground["lifetimeColors"]
            .as_array()
            .ok_or("missing lifetimeColors")?
            .iter()
            .map(|color| gradient(&color["gradient"]))
            .collect::<Result<_, _>>()?,
        _ => return Err("unsupported Playground color source".into()),
    };
    let pair = |lo, hi| Ok::<_, String>([number(playground, lo)?, number(playground, hi)?]);
    let velocity_min = vector(playground, "initialLocalVelocityMin", ["x", "y", "z"])?;
    let velocity_max = vector(playground, "initialLocalVelocityMax", ["x", "y", "z"])?;
    let gravity = vector(playground, "gravity", ["x", "y", "z"])?;
    if velocity_min[2] != 0.0 || velocity_max[2] != 0.0 || gravity[2] != 0.0 {
        return Err("unsupported particle motion along Z".into());
    }
    Ok(BoulderEmitterDef {
        count: playground["particleCount"]
            .as_u64()
            .and_then(|v| u8::try_from(v).ok())
            .ok_or("invalid particle count")?,
        sprites: sprite_ids,
        animate,
        size: number(playground, "scale")?,
        lifetime: pair("lifetimeMin", "lifetime")?,
        scatter_min: vector(playground, "sourceScatterMin", ["x", "y", "z"])?,
        scatter_max: vector(playground, "sourceScatterMax", ["x", "y", "z"])?,
        velocity_min: [velocity_min[0], velocity_min[1]],
        velocity_max: [velocity_max[0], velocity_max[1]],
        gravity: [gravity[0], gravity[1]],
        damping: number(playground, "damping")?,
        rotation: pair("initialRotationMin", "initialRotationMax")?,
        spin: pair("rotationSpeedMin", "rotationSpeedMax")?,
        colors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "set KINGDOM_REFERENCE_ASSETS to the PC ExportedProject/Assets folder"]
    fn private_reference_boulder_particles_match_the_authored_tiles_and_emitters() {
        let assets = PathBuf::from(std::env::var("KINGDOM_REFERENCE_ASSETS").expect("PC assets"));
        let guids = crate::guid_map(&assets);
        let (definition, sprites) = load(&assets, &guids).unwrap();
        let dust = &definition.dust;
        let fragments = &definition.fragments;
        assert_eq!((dust.count, fragments.count), (12, 4));
        assert_eq!((dust.lifetime, fragments.lifetime), ([1.0, 2.0], [1.0; 2]));
        assert_eq!((dust.sprites.len(), fragments.sprites.len()), (4, 2));
        assert!(dust.animate && !fragments.animate);
        assert_eq!(dust.colors.len(), 2);
        assert_eq!(fragments.spin, [180.0, 360.0]);
        assert_eq!(dust.gravity, [0.0, 8.0]);
        assert_eq!(fragments.velocity_min, [1.0, 2.0]);
        assert!(sprites.iter().all(|sprite| sprite
            .img
            .px
            .as_chunks::<4>()
            .0
            .iter()
            .any(|px| px[3] > 0)));
        assert_ne!(sprites[0].img.px, sprites[3].img.px);
        assert_ne!(sprites[4].img.px, sprites[5].img.px);
        let mut world = WorldData {
            sprites: vec!["existing".into()],
            ..WorldData::default()
        };
        Pending {
            definition,
            names: sprites.into_iter().map(|sprite| sprite.name).collect(),
        }
        .install(&mut world)
        .unwrap();
        let definition = world.boulder_fx.unwrap();
        assert_eq!(definition.dust.sprites, [1, 2, 3, 4]);
        assert_eq!(definition.fragments.sprites, [5, 6]);
    }
}
