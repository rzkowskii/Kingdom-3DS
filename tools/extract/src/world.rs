//! Exported scene/prefab/data JSON → `game::data::WorldData` (`world.bin`).
//!
//! This is where Unity components are mapped onto the game's own data
//! model. It grows as more systems are ported.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use game::data::{
    Affine2, BlockDef, BuildingDef, CampDef, DayDef, DirectorDef, FixedTransform, ForestItemDef,
    Kind, Layer, LayerDef, LevelConfigDef, LightPreset, Part, PayableDef, Placed, Proto, ProtoId,
    RandomSprite, Role, ScaffoldModuleDef, Scatter, ShopDef, Side, SpriteId, ToolKind,
    TransformNode, TreeWorkDef, UpgradeDef, WorldData, LIGHT_COLORS, LIGHT_FLOATS,
    TILE_SCATTER_FLAGS,
};
use game::data::{CloudLayerDef, CloudsDef, Curve, PortalDef, WallDef, WaveDef};
use serde_json::{Map, Value};

/// Components deriving from `Payable` (for overlap regions and half widths).
const PAYABLES: &[&str] = &[
    "Beach",
    "Cabin",
    "ChristmasTree",
    "Hermit",
    "Merchant",
    "PayableBorder",
    "PayableShield",
    "PayableShop",
    "PayableTeleporter",
    "PayableTree",
    "PayableUpgrade",
    "PayableWorkshop",
    "PayableXbox",
    "Signpost",
    "Snowman",
    "Statue",
    "Steed",
    "Wharf",
];

/// Sprite size info from the cutter: (width px, height px, ppu).
pub type SpriteSizes = HashMap<String, (u32, u32, f32)>;

pub struct Inputs<'a> {
    pub main: &'a Value,
    pub blocks: &'a Value,
    /// The menu scene (the island map).
    pub loader: &'a Value,
    pub prefabs: &'a Map<String, Value>,
    pub data: &'a Map<String, Value>,
    pub sprite_names: &'a HashMap<String, String>,
    pub sprite_sizes: &'a SpriteSizes,
    pub assets_dir: &'a Path,
}

fn comp<'v>(node: &'v Value, ty: &str) -> Option<&'v Map<String, Value>> {
    node["components"]
        .as_array()?
        .iter()
        .find(|c| c["type"] == ty)
        .and_then(|c| c["fields"].as_object())
}

/// The `id` of the first component of type `ty` (what `{"local": id}` refers to).
fn comp_id(node: &Value, ty: &str) -> Option<i64> {
    node["components"]
        .as_array()?
        .iter()
        .find(|c| c["type"] == ty)
        .and_then(|c| c["id"].as_i64())
}

fn has_any(node: &Value, types: &[&'static str]) -> Option<&'static str> {
    let comps = node["components"].as_array()?;
    for c in comps {
        if let Some(t) = c["type"].as_str() {
            if let Some(found) = types.iter().find(|x| **x == t) {
                return Some(found);
            }
        }
    }
    None
}

fn camera_def(fields: &Map<String, Value>) -> Result<game::data::CameraDef, String> {
    use game::animation::{Key, Tangent};
    let keys = |name: &str| -> Result<Vec<Key>, String> {
        fields[name]["m_Curve"]
            .as_array()
            .ok_or_else(|| format!("camera {name}: no curve"))?
            .iter()
            .map(|k| {
                let f = |field: &str| {
                    k[field]
                        .as_f64()
                        .map(|v| v as f32)
                        .filter(|v| v.is_finite())
                        .ok_or_else(|| format!("camera {name}.{field}: invalid key"))
                };
                Ok(Key {
                    time: f("time")?,
                    value: f("value")?,
                    in_tangent: Tangent::Slope(f("inSlope")?),
                    out_tangent: Tangent::Slope(f("outSlope")?),
                })
            })
            .collect()
    };
    let number = |name: &str| {
        fields
            .get(name)
            .and_then(Value::as_f64)
            .map(|v| v as f32)
            .ok_or_else(|| format!("missing MainCamera.{name}"))
    };
    let strength = |axis| {
        fields
            .get("shakeStrength")
            .and_then(|strength| strength[axis].as_f64())
            .map(|v| v as f32)
            .ok_or_else(|| format!("missing MainCamera.shakeStrength.{axis}"))
    };
    let def = game::data::CameraDef {
        lead: flag(fields, "lead"),
        max_lead_speed: num(fields, "maxLeadSpeed"),
        lead_over_time: keys("leadOverTime")?,
        seek_speed_by_distance: keys("seekSpeedByDistance")?,
        shake: game::data::CameraShakeDef {
            duration: number("shakeDuration")?,
            strength: [strength("x")?, strength("y")?],
            vibrato: fields
                .get("shakeVibrato")
                .and_then(Value::as_u64)
                .and_then(|v| u8::try_from(v).ok())
                .ok_or("invalid shakeVibrato")?,
            randomness: number("shakeRandomness")?,
        },
    };
    if def.lead_over_time.is_empty() || def.seek_speed_by_distance.is_empty() || !def.valid() {
        return Err("invalid MainCamera curves or shake settings".into());
    }
    Ok(def)
}

fn num(f: &Map<String, Value>, k: &str) -> f32 {
    f.get(k).and_then(Value::as_f64).unwrap_or(0.0) as f32
}

fn flag(f: &Map<String, Value>, k: &str) -> bool {
    f.get(k).and_then(Value::as_i64).unwrap_or(0) != 0
}

fn vec3(v: &Value) -> [f32; 3] {
    [0, 1, 2].map(|i| v[i].as_f64().unwrap_or(0.0) as f32)
}

fn local_pose(node: &Value) -> game::animation::Pose {
    let mut p = game::animation::Pose {
        position: vec3(&node["pos"]),
        scale: [0, 1, 2].map(|i| node["scale"][i].as_f64().unwrap_or(1.0) as f32),
        rotation: [0, 1, 2, 3].map(|i| {
            node["rot"][i]
                .as_f64()
                .unwrap_or(if i == 3 { 1.0 } else { 0.0 }) as f32
        }),
        ..Default::default()
    };
    if let Some(sr) = comp(node, "SpriteRenderer") {
        p.color = ["r", "g", "b", "a"]
            .map(|k| sr.get("m_Color").and_then(|c| c[k].as_f64()).unwrap_or(1.0) as f32);
        p.visible = flag(sr, "m_Enabled");
    }
    p
}

fn asset_path(v: Option<&Value>) -> Option<&str> {
    v?.get("asset")?.as_str()
}

/// Materials with the `Recolor` toggle on.
fn recolor_materials(assets: &Path) -> HashSet<String> {
    let mut out = HashSet::new();
    let Ok(rd) = std::fs::read_dir(assets.join("Material")) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "mat") {
            if let Ok(t) = std::fs::read_to_string(&p) {
                if t.lines().any(|l| l.trim() == "- Recolor: 1") {
                    out.insert(format!(
                        "Material/{}",
                        p.file_name().unwrap().to_string_lossy()
                    ));
                }
            }
        }
    }
    out
}

struct Builder<'a> {
    inp: &'a Inputs<'a>,
    world: WorldData,
    sprite_ids: HashMap<String, SpriteId>,
    prefab_protos: HashMap<String, ProtoId>,
    recolor: HashSet<String>,
    errors: Vec<String>,
}

impl<'a> Builder<'a> {
    fn sprite(&mut self, r: Option<&Value>) -> Option<SpriteId> {
        let name = self.inp.sprite_names.get(asset_path(r)?)?.clone();
        if let Some(&id) = self.sprite_ids.get(&name) {
            return Some(id);
        }
        let id = self.world.sprites.len() as SpriteId;
        self.world.sprites.push(name.clone());
        self.sprite_ids.insert(name, id);
        Some(id)
    }

    fn sprite_width(&self, id: Option<SpriteId>) -> f32 {
        id.and_then(|i| self.inp.sprite_sizes.get(&self.world.sprites[i as usize]))
            .map_or(0.0, |&(w, _, ppu)| w as f32 / ppu)
    }

    fn clouds(&mut self, level: &Value) -> Result<CloudsDef, String> {
        let (children, level_position) = cloud_parent(self.inp.main, level)?;
        let (_, world) = find_component(self.inp.main, "World").ok_or("main scene has no World")?;
        let inp = self.inp;
        let mut layer = |name: &str, color, density, highlight| {
            let node = cloud_child(children, name)?;
            Self::cloud_layer(
                inp,
                node,
                color,
                density,
                highlight,
                |dir, path| cloud_material(dir, path).map(|_| ()),
                |sprite| self.sprite(Some(sprite)),
            )
            .map_err(|e| format!("{name}: {e}"))
        };
        let mut stratus = layer(
            "CloudsStratus",
            "cloudStratusColor",
            "cloudStratusDensity",
            None,
        )?;
        let mut cumulus = layer(
            "CloudsCumulus",
            "cloudBaseColor",
            "cloudDensity",
            Some("cloudHighlightColor"),
        )?;
        for layer in [&mut stratus, &mut cumulus] {
            for (axis, offset) in layer.origin.iter_mut().zip(level_position) {
                *axis += offset;
            }
        }
        let def = CloudsDef {
            stratus,
            cumulus,
            wind_change_cooldown: cloud_number(
                world.get("windChangeCooldown"),
                "windChangeCooldown",
            )?,
        };
        if !def.valid(self.world.sprites.len()) {
            return Err("invalid sky cloud definition".into());
        }
        Ok(def)
    }

    fn cloud_layer(
        inp: &Inputs<'_>,
        node: &Value,
        color: &str,
        density: &str,
        highlight: Option<&str>,
        check_material: impl FnOnce(&Path, &str) -> Result<(), String>,
        mut sprite_id: impl FnMut(&Value) -> Option<SpriteId>,
    ) -> Result<CloudLayerDef, String> {
        let controller = comp(node, "CloudController").ok_or("missing CloudController")?;
        let parallax = comp(node, "Parallax2D").ok_or("missing parent Parallax2D")?;
        let parameter = |key| {
            controller
                .get(key)
                .and_then(|v| v["parameterName"].as_str())
        };
        if node["active"] != true
            || !unrotated(node)
            || !flag(controller, "m_Enabled")
            || !flag(parallax, "m_Enabled")
            || !parallax
                .get("ratio")
                .is_some_and(|r| r["x"] == 0 && r["y"] == 1)
            || parameter("baseColor") != Some(color)
            || parameter("density") != Some(density)
            || parameter("highlightColor") != highlight
            || !flag(controller, "useBlenderBaseColor")
            || flag(controller, "useHighlight") != highlight.is_some()
        {
            return Err(
                "unsupported CloudController transform, parallax, or lighting bindings".into(),
            );
        }
        let reference = controller.get("cloudPrefab").ok_or("missing cloudPrefab")?;
        let path = asset_path(Some(reference)).ok_or("unlinked cloudPrefab")?;
        let prefab = inp.prefabs.get(path).ok_or("missing cloud prefab")?;
        let cloud = comp(prefab, "Cloud").ok_or("missing Cloud component")?;
        let renderer = comp(prefab, "SpriteRenderer").ok_or("missing cloud SpriteRenderer")?;
        let randomizer = comp(prefab, "RandomizeSprite").ok_or("missing cloud RandomizeSprite")?;
        if !unrotated(prefab)
            || reference["id"]
                .as_i64()
                .is_none_or(|id| comp_id(prefab, "Cloud") != Some(id))
            || !flag(cloud, "m_Enabled")
            || !flag(renderer, "m_Enabled")
            || [
                "m_SortingLayer",
                "m_SortingLayerID",
                "m_SortingOrder",
                "m_FlipX",
                "m_FlipY",
                "m_DrawMode",
                "m_MaskInteraction",
            ]
            .iter()
            .any(|key| renderer.get(*key).and_then(Value::as_i64) != Some(0))
            || !flag(randomizer, "m_Enabled")
            || !flag(randomizer, "randomizeOnAwake")
            || flag(randomizer, "matchParent")
            || cloud_number(
                randomizer.get("deactivationProbability"),
                "deactivationProbability",
            )? != 0.0
            || cloud_number(randomizer.get("shiftX"), "shiftX")? != 0.0
            || prefab["children"]
                .as_array()
                .is_some_and(|children| children.iter().any(|child| child["active"] == true))
        {
            return Err("unsupported cloud prefab, renderer, or randomization settings".into());
        }
        let material = renderer
            .get("m_Materials")
            .and_then(Value::as_array)
            .filter(|materials| materials.len() == 1)
            .and_then(|materials| asset_path(materials.first()))
            .ok_or("cloud renderer must reference one material")?;
        check_material(inp.assets_dir, material)?;
        let sprites = randomizer
            .get("sprites")
            .and_then(Value::as_array)
            .ok_or("missing cloud variants")?;
        let sprites = sprites
            .iter()
            .map(|r| sprite_id(r).ok_or("unresolved cloud sprite"))
            .collect::<Result<Vec<_>, _>>()?;
        let setting = |key| cloud_number(controller.get(key), key);
        Ok(CloudLayerDef {
            origin: vec3(&node["pos"]),
            depth: setting("depth")?,
            width: setting("width")?,
            wind_multiplier: setting("windMultiplier")?,
            lifetime: cloud_number(cloud.get("lifetime"), "lifetime")?,
            fade_time: cloud_number(cloud.get("fadeTime"), "fadeTime")?,
            vertical_motion: cloud_number(cloud.get("verticalMotion"), "verticalMotion")?,
            max_clouds: controller
                .get("maxClouds")
                .and_then(Value::as_u64)
                .and_then(|v| u16::try_from(v).ok())
                .ok_or("invalid maxClouds")?,
            sprites,
            prefab_active: prefab["active"]
                .as_bool()
                .ok_or("missing cloud prefab active state")?,
            allow_flip: flag(randomizer, "allowFlip"),
            y_curve: cloud_curve(controller.get("yCurve"), "yCurve")?,
            scale_curve: cloud_curve(controller.get("scaleCurve"), "scaleCurve")?,
            parallax_curve: cloud_curve(
                controller.get("parallaxRatioCurve"),
                "parallaxRatioCurve",
            )?,
        })
    }

    fn prefab(&mut self, path: &str) -> Option<ProtoId> {
        if let Some(&id) = self.prefab_protos.get(path) {
            return Some(id);
        }
        let node = self.inp.prefabs.get(path)?;
        // Reserve the id first: upgrade chains reference prefabs recursively.
        let id = self.reserve();
        self.prefab_protos.insert(path.to_owned(), id);
        self.fill(id, node);
        Some(id)
    }

    fn reserve(&mut self) -> ProtoId {
        let id = self.world.protos.len() as ProtoId;
        self.world.protos.push(Proto::default());
        id
    }

    fn proto(&mut self, node: &Value) -> ProtoId {
        let id = self.reserve();
        self.fill(id, node);
        id
    }

    /// Scaffold modules of a WorkableBuilding, relative to the building root.
    fn scaffold(&mut self, node: &Value) -> Vec<ScaffoldModuleDef> {
        fn find<'v>(n: &'v Value, path: &mut Vec<&'v Value>) -> bool {
            path.push(n);
            if comp(n, "Scaffolding").is_some() {
                return true;
            }
            for c in n["children"].as_array().into_iter().flatten() {
                if find(c, path) {
                    return true;
                }
            }
            path.pop();
            false
        }
        let mut path = Vec::new();
        if !find(node, &mut path) {
            return Vec::new();
        }
        // Keep parent scale/rotation, including non-rendering ancestors.
        let mut root = local_pose(node);
        root.position = [0.0; 3];
        root.scale[0] = root.scale[0].abs();
        let mut at = Affine2::from_pose(root);
        let mut z = 0.0;
        for n in &path[1..] {
            let local = local_pose(n);
            at = at.compose(Affine2::from_pose(local));
            z += local.position[2];
        }
        let scaffolding = path[path.len() - 1];
        let mut modules: Vec<(f32, ScaffoldModuleDef)> = Vec::new();
        fn collect<'v>(
            n: &'v Value,
            at: Affine2,
            z: f32,
            out: &mut Vec<(&'v Value, Affine2, f32)>,
        ) {
            for child in n["children"].as_array().into_iter().flatten() {
                let local = local_pose(child);
                let composed = at.compose(Affine2::from_pose(local));
                let cz = z + local.position[2];
                out.push((child, composed, cz));
                collect(child, composed, cz, out);
            }
        }
        let mut nodes = Vec::new();
        collect(scaffolding, at, z, &mut nodes);
        for (n, transform, z) in nodes {
            let Some(m) = comp(n, "ScaffoldModule") else {
                continue;
            };
            let renderer = comp(n, "SpriteRenderer");
            let sprite = renderer.and_then(|sr| self.sprite(sr.get("m_Sprite")));
            let fill = self.sprite(m.get("fillSprite"));
            let local = vec3(&n["pos"]);
            let order = local[0] * local[0] + local[1] * local[1] + local[2] * local[2];
            modules.push((
                order,
                ScaffoldModuleDef {
                    sprite,
                    fill,
                    transform,
                    flip: renderer.is_some_and(|sr| flag(sr, "m_FlipX")),
                    flip_y: renderer.is_some_and(|sr| flag(sr, "m_FlipY")),
                    x: transform.translation[0],
                    y: transform.translation[1],
                    z,
                },
            ));
        }
        modules.sort_by(|a, b| a.0.total_cmp(&b.0));
        modules.into_iter().map(|m| m.1).collect()
    }

    /// Preserve every Transform ancestor while collecting renderers in their
    /// original depth-first order. Gameplay uses cached composed positions.
    #[allow(clippy::too_many_arguments)]
    fn parts(
        &mut self,
        node: &Value,
        at: [f32; 3],
        scale: (f32, f32),
        parent: Option<u16>,
        out: &mut Vec<Part>,
        ids: &mut Vec<i64>,
        transforms: &mut Vec<TransformNode>,
    ) {
        let mut local = local_pose(node);
        local.position = at;
        local.scale[0] = scale.0;
        local.scale[1] = scale.1;
        self.parts_in(
            node,
            local,
            Affine2::IDENTITY,
            0.0,
            None,
            "",
            parent,
            out,
            ids,
            transforms,
            false,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn parts_in(
        &mut self,
        node: &Value,
        local: game::animation::Pose,
        ancestor: Affine2,
        parent_z: f32,
        parent_node: Option<u16>,
        path: &str,
        parent: Option<u16>,
        out: &mut Vec<Part>,
        ids: &mut Vec<i64>,
        transforms: &mut Vec<TransformNode>,
        in_scaffold: bool,
    ) {
        let in_scaffold = in_scaffold || comp(node, "Scaffolding").is_some();
        // Scaffold modules have a separate draw path. Reflection membership
        // belongs to the renderer, so retain those nodes and their hierarchy.
        if comp(node, "ScaffoldModule").is_some() {
            return;
        }
        let composed = ancestor.compose(Affine2::from_pose(local));
        let z = parent_z + local.position[2];
        let node_index = transforms.len() as u16;
        let animator = comp(node, "Animator")
            .filter(|a| flag(a, "m_Enabled"))
            .and_then(|a| asset_path(a.get("m_Controller")))
            .map(str::to_owned);
        transforms.push(TransformNode {
            path: path.into(),
            parent: parent_node,
            local,
            animator,
        });
        let mut me = parent;
        if let Some(sr) = comp(node, "SpriteRenderer") {
            let sprite = self.sprite(sr.get("m_Sprite"));
            let random = comp(node, "RandomizeSprite")
                .filter(|r| flag(r, "m_Enabled"))
                .map(|r| {
                    let sprites = r
                        .get("sprites")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|s| self.sprite(Some(s)))
                        .collect();
                    RandomSprite {
                        sprites,
                        allow_flip: flag(r, "allowFlip"),
                        match_parent: flag(r, "matchParent"),
                        deactivation_probability: num(r, "deactivationProbability"),
                        shift_x: num(r, "shiftX"),
                    }
                });
            let c = sr.get("m_Color");
            let ch = |k: &str| (c.and_then(|c| c[k].as_f64()).unwrap_or(1.0) * 255.0).round() as u8;
            let material = sr
                .get("m_Materials")
                .and_then(Value::as_array)
                .and_then(|m| asset_path(m.first()));
            let recolor = material.is_some_and(|p| self.recolor.contains(p));
            // Project materials are lit unless unlit/additive; built-in sprite materials are unlit.
            let lit = material.is_some_and(|p| {
                p.starts_with("Material/") && !p.contains("Unlit") && !p.contains("Additive")
            });
            me = Some(out.len() as u16);
            ids.push(comp_id(node, "SpriteRenderer").unwrap_or(0));
            out.push(Part {
                sprite,
                reflection_only: node["layer"].as_i64() == Some(29),
                x: composed.translation[0],
                y: composed.translation[1],
                z,
                flip: flag(sr, "m_FlipX"),
                flip_y: flag(sr, "m_FlipY"),
                transform: composed,
                node: node_index,
                color: [ch("r"), ch("g"), ch("b"), ch("a")],
                sort_layer: sr
                    .get("m_SortingLayer")
                    .and_then(Value::as_i64)
                    .unwrap_or(0) as i16,
                sort_order: sr
                    .get("m_SortingOrder")
                    .and_then(Value::as_i64)
                    .unwrap_or(0) as i16,
                random,
                parent,
                recolor,
                lit,
                scaffold: in_scaffold,
            });
        }
        for child in node["children"].as_array().into_iter().flatten() {
            if child["active"] != true {
                continue;
            }
            let name = child["name"].as_str().unwrap_or_default();
            let child_path = if path.is_empty() {
                name.to_owned()
            } else {
                format!("{path}/{name}")
            };
            self.parts_in(
                child,
                local_pose(child),
                composed,
                z,
                Some(node_index),
                &child_path,
                me,
                out,
                ids,
                transforms,
                in_scaffold,
            );
        }
    }

    fn fill(&mut self, id: ProtoId, node: &Value) {
        let root_scale = vec3(&node["scale"]);
        let mut parts = Vec::new();
        let mut part_ids = Vec::new();
        let mut transforms = Vec::new();
        self.parts(
            node,
            [0.0; 3],
            (root_scale[0].abs(), root_scale[1]),
            None,
            &mut parts,
            &mut part_ids,
            &mut transforms,
        );

        let root_sprite = comp(node, "SpriteRenderer").and_then(|sr| {
            let r = sr.get("m_Sprite");
            asset_path(r)
                .and_then(|p| self.inp.sprite_names.get(p))
                .cloned()
        });
        let root_width = root_sprite
            .and_then(|n| self.inp.sprite_sizes.get(&n))
            .map(|&(w, _, ppu)| w as f32 / ppu * root_scale[0].abs());

        let payable = has_any(node, PAYABLES).and_then(|t| comp(node, t));
        let pay_region = payable.map(|p| {
            let off = p
                .get("playerPayPointOffset")
                .and_then(|o| o["x"].as_f64())
                .unwrap_or(0.0) as f32;
            (off, num(p, "playerPayDistance"))
        });
        let half_width = match (root_width, pay_region, comp(node, "GAPS")) {
            (Some(w), _, _) => w / 2.0,
            (None, Some((_, d)), _) => d,
            (None, None, Some(g)) => {
                g.get("region")
                    .and_then(|r| r["width"].as_f64())
                    .unwrap_or(0.0) as f32
                    / 2.0
            }
            _ => 0.0,
        };

        let kind = if let Some(t) = comp(node, "Tile") {
            let width =
                self.sprite_width(parts.first().and_then(|p| p.sprite)) * root_scale[0].abs();
            let mut scatter = 0;
            for (bit, name) in TILE_SCATTER_FLAGS.iter().enumerate() {
                if !name.is_empty() && flag(t, name) {
                    scatter |= 1 << bit;
                }
            }
            Kind::Tile {
                width,
                no_ground: flag(t, "noGround"),
                scatter,
            }
        } else if let Some(p) = comp(node, "PrefabPlaceholder") {
            let target = asset_path(p.get("prefab"))
                .map(str::to_owned)
                .and_then(|path| self.prefab(&path));
            match target {
                Some(proto) if flag(p, "doNotSwap") => Kind::Dormant { proto },
                Some(proto) => Kind::Placeholder {
                    proto,
                    spawn_in_game_layer: flag(p, "spawnInGameLayer"),
                },
                None => Kind::Scenery,
            }
        } else if comp(node, "HintText").is_some() {
            Kind::Hint
        } else {
            Kind::Scenery
        };

        let fixed = comp(node, "FixedTransform").map(|f| FixedTransform {
            fix_z: flag(f, "fixZ").then(|| num(f, "z")),
            fix_y: flag(f, "fixY").then(|| num(f, "y")),
            random_offset_range: num(f, "randomOffsetRange"),
            symmetric: flag(f, "symmetric"),
            flipped: flag(f, "flipped"),
        });
        let pos = vec3(&node["pos"]);
        let scatter = comp(node, "ScatteredObject").map(|s| Scatter {
            layer: Layer::from_content_index(s.get("layer").and_then(Value::as_i64).unwrap_or(2))
                .unwrap_or(Layer::Game),
            adjacent: flag(s, "adjacent"),
            spacing: num(s, "spacing"),
            irregularity: num(s, "irregularity"),
            dist_from_region_edge: num(s, "distFromRegionEdge"),
            avoid: s
                .get("avoidOverlapWith")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|t| t.as_str().map(str::to_owned))
                .collect(),
            y: pos[1],
            z: pos[2],
        });

        let tree = comp(node, "RandomizeTree")
            .filter(|t| flag(t, "m_Enabled"))
            .map(|t| {
                let c = &t["backColor"];
                let ch = |k: &str| (c[k].as_f64().unwrap_or(1.0) * 255.0).round() as u8;
                game::data::TreeDepth {
                    z_front: num(t, "zFront"),
                    z_back: num(t, "zBack"),
                    back_color: [ch("r"), ch("g"), ch("b"), ch("a")],
                    random_offset_range: num(t, "randomOffsetRange"),
                }
            });
        let payable_def = payable.map(|p| {
            let pair = |k: &str| {
                (
                    p[k]["x"].as_f64().unwrap_or(0.0) as f32,
                    p[k]["y"].as_f64().unwrap_or(0.0) as f32,
                )
            };
            PayableDef {
                price: p.get("price").and_then(Value::as_i64).unwrap_or(1) as u8,
                price_increase: p.get("priceIncrease").and_then(Value::as_i64).unwrap_or(0) as u8,
                coin_offset: pair("floatingCoinPosition"),
                coin_spacing: num(p, "floatingCoinSpacing"),
                pay_point_offset: pair("playerPayPointOffset").0,
                pay_distance: num(p, "playerPayDistance"),
            }
        });
        let upgrade = match comp(node, "PayableUpgrade") {
            Some(u) => {
                let next = asset_path(u.get("nextPrefab"))
                    .map(str::to_owned)
                    .and_then(|path| self.prefab(&path));
                Some(UpgradeDef {
                    next,
                    only_in_buildable_region: flag(u, "onlyInBuildableRegion"),
                    requires_stone: flag(u, "requiresStoneTech"),
                    cooldown: num(u, "cooldown"),
                })
            }
            None => None,
        };
        let building = comp(node, "WorkableBuilding").map(|w| BuildingDef {
            build_points: w.get("buildPoints").and_then(Value::as_i64).unwrap_or(0) as i32,
            auto_build_rate: num(w, "autoBuildRate"),
            scaffold: self.scaffold(node),
        });
        let forest_item = comp(node, "ForestItem")
            .filter(|f| flag(f, "m_Enabled"))
            .map(|f| ForestItemDef {
                controls_forest_size: flag(f, "controlsForestSize"),
                size_padding: num(f, "sizePadding"),
                width: root_width.unwrap_or(0.0),
            });
        let shop = comp(node, "PayableShop").and_then(|sh| {
            let item = self.inp.prefabs.get(asset_path(sh.get("itemPrefab"))?)?;
            let tool = item["tag"].as_str().and_then(ToolKind::from_tag)?;
            let place = &sh["itemPlacement"];
            Some(ShopDef {
                tool,
                item_placement: (
                    place["x"].as_f64().unwrap_or(0.0) as f32,
                    place["y"].as_f64().unwrap_or(0.0) as f32,
                ),
                item_spacing: num(sh, "itemSpacing"),
                max_items: sh.get("maxItems").and_then(Value::as_i64).unwrap_or(1) as u8,
            })
        });
        let tree_work = comp(node, "WorkableTree").map(|t| {
            let mark = t
                .get("chopMark")
                .and_then(|r| r.get("local"))
                .and_then(Value::as_i64);
            TreeWorkDef {
                strength: t.get("strength").and_then(Value::as_i64).unwrap_or(3) as i32,
                coin_yield: (
                    t["coinYield"]["min"].as_i64().unwrap_or(0) as i32,
                    t["coinYield"]["max"].as_i64().unwrap_or(0) as i32,
                ),
                chop_mark: mark
                    .and_then(|m| part_ids.iter().position(|&i| i == m))
                    .map(|i| i as u16),
            }
        });
        let camp = comp(node, "BeggarCamp").map(|c| CampDef {
            max_beggars: c.get("maxBeggars").and_then(Value::as_i64).unwrap_or(2) as u8,
            spawn_interval: num(c, "spawnInterval"),
            hunger_interval: num(c, "hungerInterval"),
        });
        let character = comp(node, "Character")
            .and_then(|_| node["tag"].as_str())
            .and_then(Role::from_tag);
        let wall = comp(node, "Wall").is_some();
        let damageable = comp(node, "Damageable");
        let hp = damageable
            .and_then(|d| d.get("initialHitPoints"))
            .and_then(Value::as_i64)
            .unwrap_or(1) as i32;
        let wall_def = match comp(node, "Wall") {
            Some(w) => {
                let wreck = asset_path(w.get("wreckPrefab"))
                    .map(str::to_owned)
                    .and_then(|path| self.prefab(&path));
                Some(WallDef {
                    hit_points: hp,
                    repair_rate: w.get("repairRate").and_then(Value::as_i64).unwrap_or(1) as i32,
                    repair_rate_above_max: w
                        .get("repairRateAboveMax")
                        .and_then(Value::as_i64)
                        .unwrap_or(1) as i32,
                    bonus_hit_points_multiplier: num(w, "bonusHitPointsMultiplier"),
                    minimum_intact: num(w, "minimumIntactHitPoints"),
                    wreck,
                })
            }
            None => None,
        };
        let damaged_by = damageable
            .and_then(|d| d.get("damagedBy"))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let portal = match comp(node, "Portal") {
            Some(p) => {
                // The root trigger polygon's bounds (+ collider offset).
                let hit_box = node["components"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|c| c["type"] == "PolygonCollider2D")
                    .map_or((-1.0, 1.0, 0.0, 3.0), |c| {
                        let f = &c["fields"];
                        let off = (
                            f["m_Offset"]["x"].as_f64().unwrap_or(0.0) as f32,
                            f["m_Offset"]["y"].as_f64().unwrap_or(0.0) as f32,
                        );
                        let mut b = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
                        for path in f["m_Points"]["m_Paths"].as_array().into_iter().flatten() {
                            for pt in path.as_array().into_iter().flatten() {
                                let x = pt["x"].as_f64().unwrap_or(0.0) as f32 + off.0;
                                let y = pt["y"].as_f64().unwrap_or(0.0) as f32 + off.1;
                                b = (b.0.min(x), b.1.max(x), b.2.min(y), b.3.max(y));
                            }
                        }
                        b
                    });
                let rift = asset_path(p.get("teleporterRiftPrefab"))
                    .map(str::to_owned)
                    .and_then(|path| self.prefab(&path));
                Some(PortalDef {
                    cliff: p.get("type").and_then(Value::as_i64) == Some(1),
                    hit_points: hp,
                    by_arrows: damaged_by & 2 != 0,
                    by_knights: damaged_by & 16 != 0,
                    hit_box,
                    defense_range: num(p, "defenseTriggerRange"),
                    defense_delay: (
                        p["defenseSpawnDelay"]["min"].as_f64().unwrap_or(2.0) as f32,
                        p["defenseSpawnDelay"]["max"].as_f64().unwrap_or(16.0) as f32,
                    ),
                    defense_cooldown: num(p, "defenseSpawnCooldown"),
                    rebuild_days: num(p, "rebuildDays"),
                    rift,
                })
            }
            None => None,
        };
        let castle_level =
            comp(node, "Castle").map(|c| c.get("level").and_then(Value::as_i64).unwrap_or(0) as u8);
        let statue = comp(node, "Statue").map(|st| {
            // The charge indicators are SpriteFX components; their objects' renderers are the parts.
            let charge_parts = st
                .get("charges")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|r| r.get("local").and_then(Value::as_i64))
                .filter_map(|fx| {
                    fn find(n: &Value, id: i64) -> Option<&Value> {
                        if n["components"]
                            .as_array()
                            .is_some_and(|cs| cs.iter().any(|c| c["id"].as_i64() == Some(id)))
                        {
                            return Some(n);
                        }
                        n["children"].as_array()?.iter().find_map(|c| find(c, id))
                    }
                    find(node, fx)
                })
                .filter_map(|n| comp_id(n, "SpriteRenderer"))
                .filter_map(|sr| part_ids.iter().position(|&i| i == sr))
                .map(|i| i as u16)
                .collect();
            game::data::StatueDef {
                deity: match st.get("deity").and_then(Value::as_i64).unwrap_or(0) {
                    1 => game::data::Deity::Worker,
                    2 => game::data::Deity::Knight,
                    3 => game::data::Deity::Farmer,
                    4 => game::data::Deity::Time,
                    _ => game::data::Deity::Archer,
                },
                charge_days: num(st, "chargeDurationInDays"),
                charge_multiplier: num(st, "chargeDurationMultiplier"),
                max_charge: num(st, "MaxCharge"),
                overgrown: self.sprite(st.get("overgrownSprite")),
                clean: self.sprite(st.get("cleanSprite")),
                charge_parts,
            }
        });
        let ballista = comp(node, "Ballista").map(|b| {
            let ball = node["children"].as_array().and_then(|cs| {
                cs.iter()
                    .find(|c| comp(c, "Ballista").is_some() || c["name"] == "Ballista")
            });
            let base = ball.map_or([0.0; 3], |n| vec3(&n["pos"]));
            let bolt = ball
                .and_then(|n| n["children"].as_array())
                .and_then(|cs| cs.iter().find(|c| c["name"] == "Bolt Position"))
                .map_or([0.0; 3], |n| vec3(&n["pos"]));
            let body_part = b
                .get("bodySprite")
                .and_then(|r| r.get("local"))
                .and_then(Value::as_i64)
                .and_then(|sr| part_ids.iter().position(|&i| i == sr))
                .map(|i| i as u16);
            let bolt_def = asset_path(b.get("boltPrefab"))
                .and_then(|p| self.inp.prefabs.get(p))
                .and_then(|n| comp(n, "Bolt"));
            let frames = b
                .get("reloadFrames")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|s| self.sprite(Some(s)))
                .collect();
            game::data::BallistaDef {
                bolt_pos: (base[0] + bolt[0], base[1] + bolt[1]),
                range: num(b, "shootRange"),
                min_range: num(b, "minRange"),
                angle: (
                    b["angleRange"]["min"].as_f64().unwrap_or(-15.0) as f32,
                    b["angleRange"]["max"].as_f64().unwrap_or(15.0) as f32,
                ),
                reload_work: b.get("reloadWork").and_then(Value::as_i64).unwrap_or(20) as i32,
                damage: bolt_def
                    .and_then(|f| f.get("damage"))
                    .and_then(Value::as_i64)
                    .unwrap_or(12) as i32,
                max_hits: bolt_def
                    .and_then(|f| f.get("maxHits"))
                    .and_then(Value::as_i64)
                    .unwrap_or(8) as u8,
                body_part,
                frames,
                work_pos: (-0.5, 2.0625),
            }
        });
        let shield_shop = comp(node, "PayableShield").map(|s| {
            let shield_pos = s
                .get("shieldPosition")
                .and_then(|r| r.get("local"))
                .and_then(Value::as_i64)
                .and_then(|id| child_by_transform(node, id))
                .map_or((0.0, 1.7), |c| {
                    let p = vec3(&c["pos"]);
                    (p[0], p[1])
                });
            // The Banner component's SpriteRenderer is the banner part.
            let banner_part = s
                .get("banner")
                .and_then(|r| r.get("local"))
                .and_then(Value::as_i64)
                .and_then(|bid| {
                    fn find(n: &Value, bid: i64) -> Option<&Value> {
                        if n["components"]
                            .as_array()
                            .is_some_and(|cs| cs.iter().any(|c| c["id"].as_i64() == Some(bid)))
                        {
                            return Some(n);
                        }
                        n["children"].as_array()?.iter().find_map(|c| find(c, bid))
                    }
                    find(node, bid)
                })
                .and_then(|n| comp_id(n, "SpriteRenderer"))
                .and_then(|sr| part_ids.iter().position(|&i| i == sr))
                .map(|i| i as u16);
            game::data::ShieldShopDef {
                shield_pos,
                banner_part,
                banner: self.sprite(s.get("bannerSprite")),
                torn: self.sprite(s.get("bannerSpriteTorn")),
            }
        });
        let gaps = comp(node, "GAPS").map(|g| {
            let r = &g["region"];
            game::data::Gaps {
                x: r["x"].as_f64().unwrap_or(0.0) as f32,
                width: r["width"].as_f64().unwrap_or(0.0) as f32,
                layers: g["layer"]["m_Bits"].as_u64().unwrap_or(0) as u32,
            }
        });
        let farmhouse = comp(node, "Farmhouse").map(|f| game::data::FarmhouseDef {
            update_interval: num(f, "updateInterval"),
            max_farmlands: f.get("maxFarmlands").and_then(Value::as_i64).unwrap_or(0) as u8,
            statue_extra_farmlands: f
                .get("farmerStatueExtraFarmlands")
                .and_then(Value::as_i64)
                .unwrap_or(0) as u8,
            left_seed_offset: num(f, "leftSeedOffset"),
            right_seed_offset: num(f, "rightSeedOffset"),
            level: f.get("level").and_then(Value::as_i64).unwrap_or(0) as u8,
        });
        let part_index = |sr: i64| part_ids.iter().position(|&i| i == sr).map(|i| i as u16);
        let wharf = comp(node, "Wharf").map(|w| {
            // orig: GetComponentsInChildren<BoatPart>: depth-first order.
            fn boat_parts<'v>(n: &'v Value, out: &mut Vec<&'v Value>) {
                for c in n["children"].as_array().into_iter().flatten() {
                    if comp(c, "BoatPart").is_some() {
                        out.push(c);
                    }
                    boat_parts(c, out);
                }
            }
            let mut nodes = Vec::new();
            boat_parts(node, &mut nodes);
            let assets = self.inp.assets_dir;
            let parts = nodes
                .into_iter()
                .filter_map(|c| {
                    let i = comp_id(c, "SpriteRenderer").and_then(part_index)?;
                    let sprite =
                        comp(c, "SpriteRenderer").and_then(|sr| asset_path(sr.get("m_Sprite")));
                    let (cx, cy) = sprite.map_or((0.0, 0.0), |p| sprite_center(assets, p));
                    Some((i, cx, cy))
                })
                .collect();
            let skeleton = w
                .get("skeletonBase")
                .and_then(|r| r["local"].as_i64())
                .and_then(|id| child_by_id(node, id).or_else(|| child_by_transform(node, id)));
            let boat = asset_path(w.get("boatPrefab"))
                .map(str::to_owned)
                .and_then(|p| self.prefab(&p));
            game::data::WharfDef {
                work_per_part: w.get("workPerPart").and_then(Value::as_i64).unwrap_or(4) as u8,
                parts,
                skeleton: skeleton
                    .and_then(|n| comp_id(n, "SpriteRenderer"))
                    .and_then(part_index),
                skeleton_at: skeleton.map_or((0.0, 0.0), |n| {
                    let p = vec3(&n["pos"]);
                    (p[0], p[1])
                }),
                boat,
            }
        });
        let boat = comp(node, "Boat").map(|b| {
            let pushes = node["children"]
                .as_array()
                .and_then(|cs| cs.iter().find(|c| c["name"] == "Push Positions"))
                .and_then(|n| n["children"].as_array())
                .into_iter()
                .flatten()
                .filter(|c| comp(c, "BoatPushPosition").is_some())
                .map(|c| vec3(&c["pos"])[0])
                .collect();
            fn named<'v>(n: &'v Value, name: &str) -> Option<&'v Value> {
                n["children"].as_array()?.iter().find_map(|c| {
                    if c["name"] == name {
                        Some(c)
                    } else {
                        named(c, name)
                    }
                })
            }
            game::data::BoatDef {
                water_height: num(b, "waterHeight"),
                push_speed: num(b, "pushSpeed"),
                forest_speed_multiplier: num(b, "forestSpeedMultiplier"),
                sail_speed: num(b, "sailSpeed"),
                max_workers: b.get("maxWorkers").and_then(Value::as_i64).unwrap_or(3) as u8,
                max_knights: b.get("maxKnights").and_then(Value::as_i64).unwrap_or(3) as u8,
                push_positions: pushes,
                sail: named(node, "Sail")
                    .and_then(|n| comp_id(n, "SpriteRenderer"))
                    .and_then(part_index),
            }
        });
        let obstacles = match obstacle_shapes(node) {
            Ok(shapes) => shapes,
            Err(error) => {
                self.errors.push(format!("{}: {error}", node["name"]));
                Vec::new()
            }
        };
        self.world.protos[id as usize] = Proto {
            obstacles,
            fires: fires_under(node),
            name: node["name"].as_str().unwrap_or_default().to_owned(),
            tag: node["tag"].as_str().unwrap_or("Untagged").to_owned(),
            kind,
            parts,
            transforms,
            fixed,
            scatter,
            tree,
            half_width,
            pay_region,
            root_z: pos[2],
            payable: payable_def,
            upgrade,
            building,
            forest_item,
            castle_level,
            shop,
            tree_work,
            camp,
            character,
            wall,
            wall_def,
            portal,
            gaps,
            farmhouse,
            grass: comp(node, "Grass").is_some(),
            clear_grass: comp(node, "ClearSpaceOnStart").is_some_and(|c| flag(c, "clearGrass")),
            guard_slots: guard_slots(node),
            wall_level: comp(node, "Wall")
                .map(|c| c.get("level").and_then(Value::as_i64).unwrap_or(0) as u8),
            tower_level: comp(node, "Tower")
                .map(|c| c.get("level").and_then(Value::as_i64).unwrap_or(0) as u8),
            shield_spots: comp(node, "Castle")
                .and_then(|c| c.get("shieldShopPlaceholders"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|r| r.get("local").and_then(Value::as_i64))
                .filter_map(|id| child_by_id(node, id))
                .map(|c| vec3(&c["pos"])[0])
                .collect(),
            keep_time: comp(node, "Castle")
                .and(child_with(node, "WorkableBuilding"))
                .and_then(|k| comp(k, "WorkableBuilding"))
                .map_or(0.0, |w| {
                    let rate = num(w, "autoBuildRate");
                    if rate > 0.0 {
                        num(w, "buildPoints") / rate
                    } else {
                        0.0
                    }
                }),
            shield_shop,
            tower_knight: comp(node, "TowerKnight").is_some(),
            banker: comp(node, "Castle")
                .and_then(|c| asset_path(c.get("bankerPrefab")))
                .map(str::to_owned)
                .and_then(|p| self.prefab(&p)),
            statue,
            mason_shrine: comp(node, "MasonShrine").is_some(),
            steed: None,
            cabin: comp(node, "Cabin")
                .and_then(|c| asset_path(c.get("hermitPrefab")))
                .map(str::to_owned)
                .and_then(|p| self.prefab(&p)),
            hermit: comp(node, "Hermit")
                .and_then(|_| node["tag"].as_str())
                .and_then(game::data::HermitKind::from_tag),
            passenger_upgrades: comp(node, "PayableUpgrade")
                .and_then(|u| u.get("passengerUpgrades"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|r| {
                    let kind = game::data::HermitKind::from_tag(r["tag"].as_str()?)?;
                    let path = asset_path(r.get("prefab"))?.to_owned();
                    Some((kind, self.prefab(&path)?))
                })
                .collect(),
            baker: comp(node, "Baker").map(|b| {
                (
                    b.get("maxBeggars").and_then(Value::as_i64).unwrap_or(3) as u8,
                    num(b, "callBeggarInterval"),
                )
            }),
            ballista,
            merchant_spawner: comp(node, "MerchantSpawner")
                .and_then(|m| asset_path(m.get("merchantPrefab")))
                .map(str::to_owned)
                .and_then(|p| self.prefab(&p)),
            workshop: node["children"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|c| comp(c, "PayableWorkshop").is_some())
                .map(|c| {
                    let x = vec3(&c["pos"])[0];
                    (self.proto(c), x)
                })
                .collect(),
            workshop_side: comp(node, "PayableWorkshop").and_then(|w| {
                let side = w.get("side").and_then(Value::as_i64).unwrap_or(1) as i8;
                let path = asset_path(w.get("catapultPrefab"))?.to_owned();
                Some((side, self.prefab(&path)?))
            }),
            catapult: comp(node, "Catapult").map(|c| {
                let boulder = asset_path(c.get("boulderPrefab"))
                    .and_then(|p| self.inp.prefabs.get(p))
                    .and_then(|n| comp(n, "Boulder"));
                // Spoon: Flippable/Arm/Spoon.
                let mut spoon = (0.19, 1.03);
                fn find_pos(n: &Value, name: &str, at: (f32, f32)) -> Option<(f32, f32)> {
                    for ch in n["children"].as_array()? {
                        let p = vec3(&ch["pos"]);
                        let here = (at.0 + p[0], at.1 + p[1]);
                        if ch["name"] == name {
                            return Some(here);
                        }
                        if let Some(r) = find_pos(ch, name, here) {
                            return Some(r);
                        }
                    }
                    None
                }
                if let Some(s) = find_pos(node, "Spoon", (0.0, 0.0)) {
                    spoon = s;
                }
                game::data::CatapultDef {
                    crank_rate: num(c, "crankRate"),
                    force: (
                        c["projectileForce"]["min"].as_f64().unwrap_or(7.0) as f32,
                        c["projectileForce"]["max"].as_f64().unwrap_or(8.0) as f32,
                    ),
                    scan: (
                        c["scanRange"]["min"].as_f64().unwrap_or(7.0) as f32,
                        c["scanRange"]["max"].as_f64().unwrap_or(8.0) as f32,
                    ),
                    hit_points: hp,
                    boulder_damage: boulder
                        .and_then(|b| b.get("hitDamage"))
                        .and_then(Value::as_i64)
                        .unwrap_or(15) as i32,
                    stuck_probability: boulder.map_or(0.5, |b| num(b, "StuckProbability")),
                    spoon,
                    recoil: None, // resolved with project physics settings after prefab discovery
                }
            }),
            teleporter: comp(node, "PayableTeleporter").map(|t| num(t, "cooldown")),
            dog_spawn: comp(node, "DogSpawn").map(|d| {
                let trapped = node["children"]
                    .as_array()
                    .and_then(|cs| cs.iter().find(|c| c["name"] == "Trapped Dog"));
                game::data::DogSpawnDef {
                    colors: d
                        .get("colorOptions")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .map(|c| {
                            [c["r"].as_f64(), c["g"].as_f64(), c["b"].as_f64()]
                                .map(|v| v.unwrap_or(1.0) as f32)
                        })
                        .collect(),
                    trapped_part: trapped
                        .and_then(|t| comp_id(t, "SpriteRenderer"))
                        .and_then(|sr| part_ids.iter().position(|&i| i == sr))
                        .map(|i| i as u16),
                    trapped_x: trapped.map_or(0.4, |t| vec3(&t["pos"])[0]),
                }
            }),
            rift: comp(node, "TeleporterRift").is_some(),
            chest: comp(node, "Chest").map(|c| {
                (
                    c.get("coins").and_then(Value::as_i64).unwrap_or(0) as u8,
                    (
                        c["spillRange"]["min"].as_f64().unwrap_or(-3.0) as f32,
                        c["spillRange"]["max"].as_f64().unwrap_or(3.0) as f32,
                    ),
                )
            }),
            banker_def: comp(node, "Banker").map(|b| game::data::BankerDef {
                walk: num(b, "walkSpeed"),
                run: num(b, "runSpeed"),
                wander_range: num(b, "wanderRange"),
                coin_scan_range: num(b, "coinScanRange"),
                payout_portion: num(b, "payoutPortion"),
                daily_interest: num(b, "dailyInterest"),
                max_interest: b.get("maxInterest").and_then(Value::as_i64).unwrap_or(8) as i32,
                player_max_coins: b
                    .get("playerMaxCoins")
                    .and_then(Value::as_i64)
                    .unwrap_or(30) as u32,
                payout_interval: num(b, "payoutInterval"),
                max_coins: comp(node, "Wallet")
                    .and_then(|w| w.get("maxCoins"))
                    .and_then(Value::as_i64)
                    .unwrap_or(10) as u32,
            }),
            merchant: comp(node, "Merchant").map(|m| {
                (
                    m.get("numCoins").and_then(Value::as_i64).unwrap_or(8) as u8,
                    num(m, "supplyTimeInHours"),
                )
            }),
            steed_spawn: comp(node, "SteedSpawn").map(|s| {
                let picks = s
                    .get("steeds")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|r| asset_path(Some(r)).map(str::to_owned))
                    .filter_map(|p| self.steed(&p))
                    .collect();
                (
                    s.get("amount").and_then(Value::as_i64).unwrap_or(1) as u8,
                    picks,
                )
            }),
            border: comp(node, "PayableBorder").map(|b| game::data::BorderDef {
                distance_from_wall: num(b, "distanceFromWall"),
                min_campfire_distance: num(b, "minCampfireDistance"),
                knight_range: num(b, "knightRange"),
            }),
            wharf,
            boat,
            beach: comp(node, "Beach").is_some(),
            signpost: comp(node, "Signpost")
                .and_then(|s| s.get("map"))
                .and_then(|m| m["local"].as_i64())
                .and_then(|sfx| {
                    // The map's SpriteFX sits beside its renderer.
                    node["children"].as_array()?.iter().find_map(|c| {
                        (comp_id(c, "SpriteFX") == Some(sfx))
                            .then(|| comp_id(c, "SpriteRenderer"))
                            .flatten()
                    })
                })
                .and_then(part_index),
        };
    }

    /// orig: the main scene's `CoinBag` and `BagCoin.prefab`.
    /// orig: `Holder.firePrefab` (`Fire.prefab`), its flames and glow, and
    /// `Player.torchBrightnessThreshold`.
    fn fire_style(&mut self) {
        let inp = self.inp;
        let keys = |f: &Map<String, Value>, k: &str| -> Vec<(f32, f32)> {
            f.get(k)
                .and_then(|c| c["m_Curve"].as_array())
                .into_iter()
                .flatten()
                .map(|k| {
                    (
                        k["time"].as_f64().unwrap_or(0.0) as f32,
                        k["value"].as_f64().unwrap_or(0.0) as f32,
                    )
                })
                .collect()
        };
        let mut style = game::data::FireStyleDef {
            glow_scale: 1.0,
            torch_threshold: 0.5,
            ..Default::default()
        };
        if let Some((node, f)) = inp
            .prefabs
            .values()
            .find_map(|n| comp(n, "Fire").map(|f| (n, f)))
        {
            style.strength = keys(f, "sizeToStrengthCurve");
            style.intensity = keys(f, "intensityCurve");
            style.range = keys(f, "rangeCurve");
            style.ambient = keys(f, "ambientIntensity");
            for c in node["children"].as_array().into_iter().flatten() {
                if let Some(l) = comp(c, "Class108") {
                    let col = &l["m_Color"];
                    style.light_color =
                        ["r", "g", "b"].map(|k| col[k].as_f64().unwrap_or(1.0) as f32);
                }
                if let Some(sr) = comp(c, "SpriteRenderer") {
                    style.glow = self.sprite(sr.get("m_Sprite"));
                    let col = &sr["m_Color"];
                    style.glow_color =
                        ["r", "g", "b", "a"].map(|k| col[k].as_f64().unwrap_or(1.0) as f32);
                    style.glow_scale = vec3(&c["scale"])[0];
                }
            }
            for flame in f
                .get("flamePrefabs")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(fnode) = asset_path(Some(flame)).and_then(|p| inp.prefabs.get(p)) else {
                    continue;
                };
                fn first_sprite(n: &Value) -> Option<&Value> {
                    comp(n, "SpriteRenderer")
                        .and_then(|sr| sr.get("m_Sprite"))
                        .filter(|s| s.get("asset").is_some())
                        .or_else(|| n["children"].as_array()?.iter().find_map(first_sprite))
                }
                let size = comp(fnode, "Flame").map_or((0.25, 0.25), |fl| {
                    (
                        fl["size"]["x"].as_f64().unwrap_or(0.25) as f32,
                        fl["size"]["y"].as_f64().unwrap_or(0.25) as f32,
                    )
                });
                let sprite = first_sprite(fnode).and_then(|s| self.sprite(Some(s)));
                style.flames.push((size, sprite));
            }
        } else {
            eprintln!("warning: no Fire prefab");
        }
        if let Some(p) = inp.prefabs.values().find_map(|n| comp(n, "Player")) {
            style.torch_threshold = num(p, "torchBrightnessThreshold");
        }
        self.world.fire = style;
    }

    /// orig: `Level/Water` with its `Water` script and `Water Material`.
    fn water(&mut self) {
        let inp = self.inp;
        let w = &mut self.world.water;
        if let Some((node, f)) = find_component(inp.main, "Water") {
            let pos = vec3(&node["pos"]);
            let scale = vec3(&node["scale"]);
            let half = scale[1].abs() / 2.0;
            (w.top, w.bottom, w.z) = (pos[1] + half, pos[1] - half, pos[2]);
            w.reflection_scale = num(f, "reflectionScale");
            w.max_displacement = (
                f["maxDisplacement"]["x"].as_f64().unwrap_or(16.0) as f32,
                f["maxDisplacement"]["y"].as_f64().unwrap_or(16.0) as f32,
            );
        } else {
            eprintln!("warning: no Water in the main scene");
        }
        // The material's maps: red and green channels of each.
        let load = |name: &str| -> Option<(u16, u16, Vec<[u8; 2]>)> {
            let img = crate::sprites::Image::load_png(&inp.assets_dir.join("Texture2D").join(name))
                .ok()?;
            let (iw, ih) = (img.w as usize, img.h as usize);
            // PNG rows run top-down; Unity's v runs bottom-up.
            let texels = (0..ih)
                .rev()
                .flat_map(|y| (0..iw).map(move |x| (y * iw + x) * 4))
                .map(|i| [img.px[i], img.px[i + 1]])
                .collect();
            Some((iw as u16, ih as u16, texels))
        };
        match (load("water_coarse.png"), load("water_fine.png")) {
            (Some((cw, ch, coarse)), Some((fw, fh, fine))) if (cw, ch) == (fw, fh) => {
                w.size = (cw, ch);
                w.coarse = coarse;
                w.fine = fine;
            }
            _ => eprintln!("warning: water displacement maps missing"),
        }
        if let Some(f) = std::fs::read_to_string(inp.assets_dir.join("Material/Water Material.mat"))
            .ok()
            .and_then(|t| {
                let i = t.find("_Displacement1Tex:")?;
                let j = t[i..].find("m_Scale: {x: ")? + i + "m_Scale: {x: ".len();
                t[j..].split(',').next()?.trim().parse::<f32>().ok()
            })
        {
            w.repeat.0 = f;
        }
    }

    fn bag(&mut self) {
        let inp = self.inp;
        let Some((node, cb)) = find_component(inp.main, "CoinBag") else {
            return;
        };
        let path = |n: &Value| -> Vec<(f32, f32)> {
            let Some(c) = comp(n, "PolygonCollider2D") else {
                return Vec::new();
            };
            let off = vec3(&n["pos"]);
            c["m_Points"]["m_Paths"][0]
                .as_array()
                .into_iter()
                .flatten()
                .map(|p| {
                    (
                        p["x"].as_f64().unwrap_or(0.0) as f32 + off[0],
                        p["y"].as_f64().unwrap_or(0.0) as f32 + off[1],
                    )
                })
                .collect()
        };
        let child = |name: &str| {
            node["children"]
                .as_array()
                .and_then(|cs| cs.iter().find(|c| c["name"] == name))
        };
        let sr = |b: &mut Self, name: &str| {
            child(name)
                .and_then(|c| comp(c, "SpriteRenderer"))
                .and_then(|r| r.get("m_Sprite").cloned())
                .and_then(|s| b.sprite(Some(&s)))
        };
        let mut def = game::data::BagDef {
            back: sr(self, "Back"),
            front: sr(self, "Front"),
            closed: sr(self, "Closed"),
            wall: child("Collider").map(path).unwrap_or_default(),
            spill: child("Ground").map(path).unwrap_or_default(),
            spawn_interval: num(cb, "coinSpawnInterval"),
            ..Default::default()
        };
        let coin = asset_path(cb.get("bagCoinPrefab")).and_then(|p| inp.prefabs.get(p));
        if let Some(coin) = coin {
            def.coin = comp(coin, "SpriteRenderer")
                .and_then(|r| r.get("m_Sprite").cloned())
                .and_then(|s| self.sprite(Some(&s)));
            if let Some(bc) = comp(coin, "BagCoin") {
                def.coin_colors = bc["colors"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|c| {
                        [c["r"].as_f64(), c["g"].as_f64(), c["b"].as_f64()]
                            .map(|v| v.unwrap_or(1.0) as f32)
                    })
                    .collect();
            }
            if let Some(rb) = comp(coin, "Rigidbody2D") {
                def.gravity_scale = num(rb, "m_GravityScale");
                def.linear_drag = num(rb, "m_LinearDrag");
                def.angular_drag = num(rb, "m_AngularDrag");
            }
            if let Some(col) = child_with(coin, "PolygonCollider2D") {
                def.coin_shape = path(col);
                let mat = comp(col, "PolygonCollider2D")
                    .and_then(|c| asset_path(c.get("m_Material")))
                    .and_then(|p| std::fs::read_to_string(inp.assets_dir.join(p)).ok())
                    .unwrap_or_default();
                let field = |k: &str| {
                    mat.lines()
                        .find_map(|l| l.trim().strip_prefix(k))
                        .and_then(|v| v.trim().parse::<f32>().ok())
                };
                def.friction = field("friction:").unwrap_or(0.1);
                def.bounciness = field("bounciness:").unwrap_or(0.5);
            }
        }
        self.world.bag = def;
    }

    /// orig: the loader scene's `Map`.
    fn map(&mut self) {
        let inp = self.inp;
        let Some((node, _)) = find_component(inp.loader, "Map") else {
            return;
        };
        let spr = |b: &mut Self, n: &Value| {
            n["components"]
                .as_array()
                .into_iter()
                .flatten()
                .find_map(|c| c["fields"].get("m_Sprite").cloned())
                .and_then(|s| b.sprite(Some(&s)))
        };
        let pos = |n: &Value| {
            let p = vec3(&n["pos"]);
            (p[0], p[1])
        };
        let flipped = |n: &Value| vec3(&n["scale"])[0] < 0.0;
        let named = |n: &'a Value, name: &str| -> Option<&'a Value> {
            n["children"].as_array()?.iter().find(|c| c["name"] == name)
        };
        let Some(back) = named(node, "Background") else {
            return;
        };
        let Some(group) = named(back, "Group") else {
            return;
        };
        let mut def = game::data::MapDef {
            back: spr(self, back),
            ..Default::default()
        };
        if let Some(d) = named(group, "Detail") {
            for c in d["children"].as_array().into_iter().flatten() {
                if let Some(s) = spr(self, c) {
                    def.details.push((s, pos(c)));
                }
            }
        }
        let mut lands: Vec<(i64, game::data::MapLandDef)> = Vec::new();
        for l in group["children"].as_array().into_iter().flatten() {
            let Some(ml) = comp(l, "MapLand") else {
                continue;
            };
            let mut land = game::data::MapLandDef {
                sprite: spr(self, l),
                at: pos(l),
                ..Default::default()
            };
            for c in l["children"].as_array().into_iter().flatten() {
                let name = c["name"].as_str().unwrap_or("");
                if let Some(u) = comp(c, "MapUnlock") {
                    let icon = named(c, "Icon");
                    land.unlocks.push(game::data::MapUnlockDef {
                        unlock: u.get("unlockID").and_then(Value::as_i64).unwrap_or(0) as u8,
                        at: pos(c),
                        icon: icon.and_then(|i| spr(self, i)),
                        icon_at: icon.map_or((0.0, 0.0), pos),
                        map_at: named(c, "Map").map_or((0.0, 10.0), pos),
                    });
                } else if name == "MapBoat" {
                    land.boat = (pos(c), flipped(c));
                } else if name == "Pointer" {
                    land.pointer = (pos(c), flipped(c));
                } else if name.starts_with("Cloud") {
                    if let Some(s) = spr(self, c) {
                        land.clouds.push((s, pos(c)));
                    }
                }
            }
            lands.push((ml.get("land").and_then(Value::as_i64).unwrap_or(0), land));
        }
        lands.sort_by_key(|l| l.0);
        def.lands = lands.into_iter().map(|l| l.1).collect();
        let frames = |b: &mut Self, base: &str| -> Vec<SpriteId> {
            (0..8)
                .map_while(|i| {
                    let name = format!("{base}_{i}");
                    inp.sprite_sizes
                        .contains_key(&name)
                        .then(|| b.named_sprite(&name))
                })
                .collect()
        };
        def.boat = frames(self, "map_boat");
        def.wreck = frames(self, "map_boat_wreck");
        def.pointer = frames(self, "map_pointer");
        def.light_patch = Some(self.named_sprite("map_lightpatch"));
        def.map_icon = Some(self.named_sprite("map_icon_map"));
        def.button = Some(self.named_sprite("map_button_normal"));
        // The loader's RectTransforms aren't exported; the map's is 300×200.
        def.back_size = (300.0, 200.0);
        let border = |name: &str| {
            let text = std::fs::read_to_string(inp.assets_dir.join(format!("Sprite/{name}.asset")))
                .unwrap_or_default();
            let line = text
                .lines()
                .find(|l| l.trim_start().starts_with("m_Border:"))
                .unwrap_or("");
            let v = |k: &str| {
                line.split(k)
                    .nth(1)
                    .and_then(|r| r.split([',', '}']).next())
                    .and_then(|n| n.trim().parse::<f32>().ok())
                    .unwrap_or(0.0)
            };
            (v("x:"), v("y:"), v("z:"), v("w:"))
        };
        def.back_border = border("map_back");
        def.button_border = border("map_button_normal");
        self.world.map = def;
    }

    /// A sprite by its exported name.
    fn named_sprite(&mut self, name: &str) -> SpriteId {
        if let Some(&id) = self.sprite_ids.get(name) {
            return id;
        }
        let id = self.world.sprites.len() as SpriteId;
        self.world.sprites.push(name.to_owned());
        self.sprite_ids.insert(name.to_owned(), id);
        id
    }

    /// A steed prefab, registered once in `WorldData::steeds`.
    fn steed(&mut self, path: &str) -> Option<u16> {
        let proto = self.prefab(path)?;
        if let Some(i) = self.world.steeds.iter().position(|s| s.proto == proto) {
            return Some(i as u16);
        }
        let node = self.inp.prefabs.get(path)?;
        let f = comp(node, "Steed")?;
        let color = |k: &str| {
            [f[k]["r"].as_f64(), f[k]["g"].as_f64(), f[k]["b"].as_f64()]
                .map(|c| c.unwrap_or(1.0) as f32)
        };
        let clips = comp(node, "Animator")
            .and_then(|a| asset_path(a.get("m_Controller")))
            .and_then(|p| Path::new(p).file_stem())
            .map(|s| {
                s.to_string_lossy()
                    .split('.')
                    .next()
                    .unwrap_or("")
                    .to_owned()
            })
            .unwrap_or_else(|| "player_horse".to_owned());
        let rider = node["children"]
            .as_array()
            .and_then(|cs| cs.iter().find(|c| c["name"] == "King"))
            .map_or((0.0, 0.0), |k| {
                let p = vec3(&k["pos"]);
                (p[0], p[1])
            });
        let hb = &f["hitBox"];
        let def = game::data::SteedDef {
            name: node["name"].as_str().unwrap_or_default().to_owned(),
            proto,
            clips,
            walk_speed: num(f, "walkSpeed"),
            run_speed: num(f, "runSpeed"),
            forest_speed_multiplier: num(f, "forestSpeedMultiplier"),
            walk_stamina_rate: num(f, "walkStaminaRate"),
            run_stamina_rate: num(f, "runStaminaRate"),
            stand_stamina_rate: num(f, "standStaminaRate"),
            eat_delay: num(f, "eatDelay"),
            eat_duration: num(f, "eatDuration"),
            rearing_cooldown: num(f, "rearingCooldown"),
            rearing_duration: num(f, "rearingDuration"),
            well_fed_duration: num(f, "wellFedDuration"),
            tired_duration: num(f, "tiredDuration"),
            reserve_stamina: num(f, "reserveStamina"),
            reserve_probability: num(f, "reserveProbability"),
            attacks_while_running: flag(f, "attacksWhileRunning"),
            hit_box: (
                hb["x"].as_f64().unwrap_or(0.0) as f32,
                hb["y"].as_f64().unwrap_or(0.0) as f32,
                hb["width"].as_f64().unwrap_or(0.0) as f32,
                hb["height"].as_f64().unwrap_or(0.0) as f32,
            ),
            attack_damage: f.get("attackDamage").and_then(Value::as_i64).unwrap_or(0) as i32,
            attack_stamina_cost: num(f, "attackStaminaCost"),
            attracts_deer: flag(f, "attractsDeer"),
            poop_coins: f
                .get("poopCoinsWhenEating")
                .and_then(Value::as_i64)
                .unwrap_or(0) as u8,
            inspire_citizens: flag(f, "inspireCitizens"),
            inspired_duration: num(f, "inspiredDuration"),
            inspired_range: num(f, "inspiredRange"),
            ability_cooldown: num(f, "abilityCooldown"),
            recolor_to_coat: flag(f, "recolorToCoatOfArms"),
            disable_switching: flag(f, "disableSteedSwitching"),
            does_not_need_grass: flag(f, "doesNotNeedGrass"),
            wander_range: num(f, "wanderRange"),
            coat: color("coatColor"),
            mane: color("maneColor"),
            rider,
            torch: node["children"]
                .as_array()
                .and_then(|cs| cs.iter().find(|c| c["name"] == "King"))
                .and_then(|k| {
                    let at = vec3(&k["pos"]);
                    fires_under(k).into_iter().next().map(|mut f| {
                        f.at = (f.at.0 + at[0], f.at.1 + at[1], f.at.2 + at[2]);
                        f
                    })
                }),
            passenger: node["children"]
                .as_array()
                .and_then(|cs| cs.iter().find(|c| c["name"] == "Passenger"))
                .map_or((-0.28125, 0.5625), |k| {
                    let p = vec3(&k["pos"]);
                    (p[0], p[1])
                }),
        };
        let i = self.world.steeds.len() as u16;
        self.world.steeds.push(def);
        self.world.protos[proto as usize].steed = Some(i);
        Some(i)
    }

    /// orig: `Holder.farmlandPrefab`.
    fn farmland(&mut self, path: &str) -> Option<game::data::FarmlandDef> {
        let proto = self.prefab(path)?;
        let node = self.inp.prefabs.get(path)?;
        let f = comp(node, "Farmland")?;
        let sprites = |b: &mut Self, k: &str| -> Vec<SpriteId> {
            f.get(k)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|s| b.sprite(Some(s)))
                .collect()
        };
        let frames = sprites(self, "frames");
        let back_frames = sprites(self, "backgroundFrames");
        // Parts are flattened depth-first: the root renderer, then children in order.
        let back_id = f
            .get("backgroundRenderer")
            .and_then(|r| r.get("local"))
            .and_then(Value::as_i64);
        let mut ids = Vec::new();
        let mut parts = Vec::new();
        self.parts(
            node,
            [0.0; 3],
            (1.0, 1.0),
            None,
            &mut parts,
            &mut ids,
            &mut Vec::new(),
        );
        let back_part = back_id
            .and_then(|b| ids.iter().position(|&i| i == b))
            .unwrap_or(0) as u16;
        let width = comp(node, "GAPS")
            .and_then(|g| g["region"]["width"].as_f64())
            .unwrap_or(2.0) as f32;
        Some(game::data::FarmlandDef {
            proto,
            farm_points: f.get("farmPoints").and_then(Value::as_i64).unwrap_or(10) as i32,
            coin_yield: f.get("coinYield").and_then(Value::as_i64).unwrap_or(0) as i32,
            decay_interval: num(f, "decayInterval"),
            coin_drop_force: (
                f["coinDropForce"]["x"].as_f64().unwrap_or(0.0) as f32,
                f["coinDropForce"]["y"].as_f64().unwrap_or(0.0) as f32,
            ),
            frames,
            back_frames,
            decayed: (
                self.sprite(f.get("decayedFrame")),
                self.sprite(f.get("decayedBackgroundFrame")),
            ),
            frozen: (
                self.sprite(f.get("frozenFrame")),
                self.sprite(f.get("frozenBackgroundFrame")),
            ),
            stages: curve(&f["stages"]),
            spacing: num(f, "spacing"),
            front_part: 0,
            back_part,
            width,
        })
    }

    fn block(&mut self, node: &Value) -> Option<BlockDef> {
        let lb = comp(node, "LevelBlock")?;
        let mut items = Vec::new();
        let mut tiles: Vec<(f32, f32)> = Vec::new();
        for layer_node in node["children"].as_array().into_iter().flatten() {
            let Some(layer) = layer_node["name"].as_str().and_then(Layer::from_name) else {
                continue;
            };
            let layer_active = layer_node["active"] == true;
            for item in layer_node["children"].as_array().into_iter().flatten() {
                let pos = vec3(&item["pos"]);
                let proto = self.proto(item);
                // Width counts every tile, active or not. orig: GetComponentsInChildren<Tile>(true).
                if let Kind::Tile { width, .. } = self.world.protos[proto as usize].kind {
                    tiles.push((pos[0], pos[0] + width));
                }
                if layer_active && item["active"] == true {
                    items.push(Placed {
                        proto,
                        layer,
                        x: pos[0],
                        y: pos[1],
                        z: pos[2],
                        scale_x: vec3(&item["scale"])[0].signum(),
                    });
                }
            }
        }
        tiles.sort_by(|a, b| a.0.total_cmp(&b.0));
        let left = tiles.first().map_or(0.0, |t| t.0);
        let right = tiles.last().map_or(0.0, |t| t.1);
        let group = |k: &str| lb.get(k).and_then(Value::as_i64).unwrap_or(0) as u8;
        Some(BlockDef {
            name: node["name"].as_str().unwrap_or_default().to_owned(),
            needs_unlock: flag(lb, "needsUnlock"),
            unlock_id: lb.get("unlockID").and_then(Value::as_i64).unwrap_or(0) as i32,
            min_distance: num(lb, "minDistance"),
            max_distance: num(lb, "maxDistance"),
            maximize_mutual_distance: flag(lb, "maximizeMutualDistance"),
            side: match lb.get("side").and_then(Value::as_i64) {
                Some(1) => Side::Left,
                Some(2) => Side::Right,
                Some(3) => Side::Distribute,
                _ => Side::Any,
            },
            absolute_center: flag(lb, "absoluteCenter"),
            groups: [group("groupOne"), group("groupTwo"), group("groupThree")],
            left,
            width: (right - left).floor() as i32,
            items,
        })
    }
}

/// The child GameObject with this file id.
fn child_by_id(node: &Value, id: i64) -> Option<&Value> {
    node["children"].as_array()?.iter().find_map(|c| {
        if c["id"].as_i64() == Some(id) {
            Some(c)
        } else {
            child_by_id(c, id)
        }
    })
}

/// The child whose Transform has this file id.
fn child_by_transform(node: &Value, id: i64) -> Option<&Value> {
    node["children"].as_array()?.iter().find_map(|c| {
        if comp_id(c, "Transform") == Some(id) {
            Some(c)
        } else {
            child_by_transform(c, id)
        }
    })
}

/// The first descendant with a component of type `ty`.
fn child_with<'v>(node: &'v Value, ty: &str) -> Option<&'v Value> {
    node["children"].as_array()?.iter().find_map(|c| {
        if comp(c, ty).is_some() {
            Some(c)
        } else {
            child_with(c, ty)
        }
    })
}

/// The middle of a sprite's rect relative to its pivot, in units (stands in
/// for the centroid of a `BoatPart`'s outline).
fn sprite_center(assets: &Path, path: &str) -> (f32, f32) {
    let Ok(text) = std::fs::read_to_string(assets.join(path)) else {
        return (0.0, 0.0);
    };
    let field = |key: &str, after: &str| -> f32 {
        text.find(after)
            .and_then(|i| {
                let rest = &text[i..];
                let j = rest.find(key)? + key.len();
                let v = rest[j..].trim_start();
                let end = v.find([',', '}', '\n']).unwrap_or(v.len());
                v[..end].trim().parse().ok()
            })
            .unwrap_or(0.0)
    };
    let (w, h) = (field("width:", "m_Rect:"), field("height:", "m_Rect:"));
    let (px, py) = (field("x:", "m_Pivot:"), field("y:", "m_Pivot:"));
    let ppu = match field("m_PixelsToUnits:", "m_PixelsToUnits:") {
        v if v > 0.0 => v,
        _ => 32.0,
    };
    ((0.5 - px) * w / ppu, (0.5 - py) * h / ppu)
}

/// `FirePlaceholder`s under `node`, positioned relative to it.
fn fires_under(node: &Value) -> Vec<game::data::FireDef> {
    fn walk(n: &Value, at: [f32; 3], out: &mut Vec<game::data::FireDef>) {
        for c in n["children"].as_array().into_iter().flatten() {
            let p = vec3(&c["pos"]);
            let here = [at[0] + p[0], at[1] + p[1], at[2] + p[2]];
            if let Some(f) = comp(c, "FirePlaceholder") {
                if flag(f, "m_Enabled") || f.get("m_Enabled").is_none() {
                    out.push(game::data::FireDef {
                        at: (here[0], here[1], here[2]),
                        size: (
                            f["size"]["x"].as_f64().unwrap_or(0.25) as f32,
                            f["size"]["y"].as_f64().unwrap_or(0.25) as f32,
                        ),
                        night_only: flag(f, "burnsAtNightOnly"),
                        auto_start: flag(f, "autoStart"),
                        real_light: flag(f, "forceRealLight"),
                    });
                }
            }
            walk(c, here, out);
        }
    }
    let mut out = Vec::new();
    walk(node, [0.0; 3], &mut out);
    out
}

/// `GuardSlot` positions under `node`, relative to it.
fn guard_slots(node: &Value) -> Vec<(f32, f32)> {
    fn walk(n: &Value, at: (f32, f32), out: &mut Vec<(f32, f32)>) {
        for c in n["children"].as_array().into_iter().flatten() {
            if c["active"] != true {
                continue;
            }
            let p = vec3(&c["pos"]);
            let here = (at.0 + p[0], at.1 + p[1]);
            if comp(c, "GuardSlot").is_some() {
                out.push(here);
            }
            walk(c, here, out);
        }
    }
    let mut out = Vec::new();
    walk(node, (0.0, 0.0), &mut out);
    out
}

fn find_component<'v>(roots: &'v Value, ty: &str) -> Option<(&'v Value, &'v Map<String, Value>)> {
    fn walk<'v>(n: &'v Value, ty: &str) -> Option<(&'v Value, &'v Map<String, Value>)> {
        if let Some(c) = comp(n, ty) {
            return Some((n, c));
        }
        n["children"].as_array()?.iter().find_map(|c| walk(c, ty))
    }
    roots["roots"].as_array()?.iter().find_map(|r| walk(r, ty))
}

fn unrotated(node: &Value) -> bool {
    vec3(&node["scale"]) == [1.0; 3]
        && [0.0, 0.0, 0.0, 1.0]
            .into_iter()
            .enumerate()
            .all(|(i, v)| node["rot"][i].as_f64() == Some(v))
}

fn cloud_parent<'v>(main: &Value, level: &'v Value) -> Result<(&'v [Value], [f32; 3]), String> {
    if !unrotated(level)
        || !main["roots"]
            .as_array()
            .is_some_and(|roots| roots.iter().any(|root| root["id"] == level["id"]))
    {
        return Err("clouds require a scene-root Level without rotation or scale".into());
    }
    let children = level["children"]
        .as_array()
        .ok_or("Level has no children")?;
    Ok((children, vec3(&level["pos"])))
}

fn cloud_child<'v>(children: &'v [Value], name: &str) -> Result<&'v Value, String> {
    children
        .iter()
        .find(|child| child["name"] == name)
        .ok_or_else(|| format!("Level has no {name}"))
}

pub fn atmosphere(inp: &Inputs<'_>) -> Result<scene::atmosphere_data::AtmosphereDef, String> {
    let (level, _) = find_component(inp.main, "Level").ok_or("main scene has no Level")?;
    let (children, level_position) = cloud_parent(inp.main, level)?;
    let mut sprites = Vec::new();
    let mut paths = Vec::new();
    let mut sprite_id = |reference: &Value| {
        let path = asset_path(Some(reference))?;
        let name = inp.sprite_names.get(path)?;
        let (_, _, ppu) = inp.sprite_sizes.get(name)?;
        if *ppu != 32.0 {
            return None;
        }
        if let Some(index) = sprites.iter().position(|sprite| sprite == name) {
            return SpriteId::try_from(index).ok();
        }
        let id = SpriteId::try_from(sprites.len()).ok()?;
        sprites.push(name.clone());
        paths.push(path.to_owned());
        Some(id)
    };
    let mut fog = Builder::cloud_layer(
        inp,
        cloud_child(children, "Fogs")?,
        "fogColor",
        "fogDensity",
        None,
        |dir, path| atmosphere_material(dir, path, "Shader/Custom_PowerSprite2.shader.meta", true),
        &mut sprite_id,
    )
    .map_err(|e| format!("Fogs: {e}"))?;
    let mut mist = Builder::cloud_layer(
        inp,
        cloud_child(children, "Mists")?,
        "mistColor",
        "mistDensity",
        None,
        |dir, path| {
            atmosphere_material(
                dir,
                path,
                "Shader/Custom_Blended Texture.shader.meta",
                false,
            )
        },
        &mut sprite_id,
    )
    .map_err(|e| format!("Mists: {e}"))?;
    for layer in [&mut fog, &mut mist] {
        for (axis, offset) in layer.origin.iter_mut().zip(level_position) {
            *axis += offset;
        }
    }
    let meshes = paths
        .iter()
        .map(|path| crate::sprites::cloud_mesh(&inp.assets_dir.join(path)))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(scene::atmosphere_data::AtmosphereDef {
        fog,
        mist,
        sprites,
        meshes,
    })
}

fn atmosphere_material(
    dir: &Path,
    path: &str,
    shader: &str,
    power_sprite: bool,
) -> Result<(), String> {
    use crate::yaml::{field, guid_of};

    let read =
        |path: &str| std::fs::read_to_string(dir.join(path)).map_err(|e| format!("{path}: {e}"));
    let shader = read(shader)?;
    let guid = field(&shader, "guid").ok_or("missing atmosphere shader GUID")?;
    let material = read(path)?;
    let ambient = if power_sprite {
        field(&material, "_AmbientResponse").and_then(|value| value.parse::<f32>().ok())
            == Some(1.0)
    } else {
        field(&material, "m_ShaderKeywords").is_some_and(|keywords| {
            keywords
                .split_whitespace()
                .any(|keyword| keyword == "USEAMBIENT_ON")
        })
    };
    if field(&material, "m_Shader").and_then(guid_of) != Some(guid) || !ambient {
        return Err(format!("{path}: unsupported atmosphere shader"));
    }
    Ok(())
}

fn cloud_number(value: Option<&Value>, field: &str) -> Result<f32, String> {
    value
        .and_then(Value::as_f64)
        .map(|n| n as f32)
        .filter(|n| n.is_finite())
        .ok_or_else(|| format!("invalid cloud {field}"))
}

fn cloud_curve(value: Option<&Value>, field: &str) -> Result<Curve, String> {
    let value = value.ok_or_else(|| format!("missing cloud {field}"))?;
    let keys = value["m_Curve"]
        .as_array()
        .ok_or_else(|| format!("missing cloud {field}"))?;
    for key in keys {
        for axis in ["time", "value"] {
            cloud_number(key.get(axis), field)?;
        }
        for tangent in ["inSlope", "outSlope"] {
            if !matches!(key[tangent].as_str(), Some("Infinity" | "-Infinity")) {
                cloud_number(key.get(tangent), field)?;
            }
        }
    }
    Ok(curve(value))
}

fn cloud_material(dir: &Path, path: &str) -> Result<[f32; 4], String> {
    use crate::yaml::{field, guid_of, inline_map};

    let read =
        |path: &str| std::fs::read_to_string(dir.join(path)).map_err(|e| format!("{path}: {e}"));
    let shader = read("Shader/Custom_ShadowReceiver.shader.meta")?;
    let guid = field(&shader, "guid").ok_or("missing Custom/ShadowReceiver GUID")?;
    let material = read(path)?;
    if field(&material, "m_Shader").and_then(guid_of) != Some(guid)
        || !field(&material, "m_ShaderKeywords").is_some_and(|keywords| {
            keywords
                .split_whitespace()
                .any(|keyword| keyword == "USEAMBIENT_ON")
        })
    {
        return Err(format!(
            "{path}: expected the ambient ShadowReceiver shader"
        ));
    }
    field(&material, "_LightColor")
        .and_then(|value| inline_map(value, &["r", "g", "b", "a"]))
        .and_then(|values| values.try_into().ok())
        .ok_or_else(|| format!("{path}: missing or invalid _LightColor"))
}

/// orig: `AnimationCurve` keys.
fn curve(v: &Value) -> Curve {
    let f = |k: &Value| match k {
        Value::String(s) if s == "Infinity" => f32::INFINITY,
        Value::String(s) if s == "-Infinity" => f32::NEG_INFINITY,
        other => other.as_f64().unwrap_or(0.0) as f32,
    };
    Curve {
        keys: v["m_Curve"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|k| {
                [
                    f(&k["time"]),
                    f(&k["value"]),
                    f(&k["inSlope"]),
                    f(&k["outSlope"]),
                ]
            })
            .collect(),
    }
}

fn level_config(name: &str, f: &Map<String, Value>, waves: &HashMap<&str, u16>) -> LevelConfigDef {
    let wave = |k: &str| asset_path(f.get(k)).and_then(|p| waves.get(p).copied());
    // orig: Director/EnemyManager.StoreWaveReferences: seasonal waves only when overridden.
    let seasonal = |base: &str| {
        let summer = wave(base);
        let autumn = if flag(f, "overrideAutumnWaves") {
            wave(&format!("{base}Autumn"))
        } else {
            summer
        };
        let winter = if flag(f, "overrideWinterWaves") {
            wave(&format!("{base}Winter"))
        } else {
            summer
        };
        [summer, autumn, winter]
    };
    let timeframe = &f["waveMultiplierTimeframe"];
    let range = |k: &str| {
        let r = &f[k];
        (
            r["min"].as_i64().unwrap_or(0) as i32,
            r["max"].as_i64().unwrap_or(0) as i32,
        )
    };
    let mut group_counts = vec![(0, 0); 64];
    for g in f
        .get("groupCounts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let idx = g["group"].as_i64().unwrap_or(0) as usize;
        if idx < group_counts.len() {
            group_counts[idx] = (
                g["count"]["min"].as_i64().unwrap_or(0) as i32,
                g["count"]["max"].as_i64().unwrap_or(0) as i32,
            );
        }
    }
    LevelConfigDef {
        name: name.to_owned(),
        autumn_start: range("autumnStart"),
        winter_start: range("winterStart"),
        days_before_game_over: f
            .get("daysBeforeGameOver")
            .and_then(Value::as_i64)
            .unwrap_or(-1) as i32,
        starting_coins: f.get("startingCoins").and_then(Value::as_i64).unwrap_or(0) as i32,
        min_level_width: f.get("minLevelWidth").and_then(Value::as_i64).unwrap_or(0) as i32,
        group_counts,
        waves: [
            seasonal("regularWave"),
            seasonal("bossWave"),
            seasonal("recoveryWave"),
        ],
        defence_waves: seasonal("portalDefenceWave"),
        retaliation_waves: seasonal("retaliationWave"),
        wave_timed_multiplier: f
            .get("waveTimedMultiplier")
            .and_then(Value::as_f64)
            .unwrap_or(1.0) as f32,
        wave_multiplier_timeframe: (
            timeframe["min"].as_i64().unwrap_or(0) as i32,
            timeframe["max"].as_i64().unwrap_or(-1) as i32,
        ),
        season_change_days: f
            .get("seasonChangeDays")
            .and_then(Value::as_f64)
            .unwrap_or(3.0) as f32,
        income_multiplier: f
            .get("incomeMultiplier")
            .and_then(Value::as_f64)
            .unwrap_or(1.0) as f32,
        free_boat_parts: f.get("freeBoatParts").and_then(Value::as_i64).unwrap_or(0) as u8,
        wave_boat_multiplier: f
            .get("waveBoatMultiplier")
            .and_then(Value::as_f64)
            .unwrap_or(1.0) as f32,
        force_all_unlocks: flag(f, "forceAllUnlocks"),
    }
}

/// The world, and the asset path of each audio clip (`AudioDef::clips`).
pub fn build(inp: &Inputs) -> Result<(WorldData, Vec<String>), String> {
    let mut b = Builder {
        inp,
        world: WorldData::default(),
        sprite_ids: HashMap::new(),
        prefab_protos: HashMap::new(),
        recolor: recolor_materials(inp.assets_dir),
        errors: Vec::new(),
    };

    let (_, camera) =
        find_component(inp.main, "MainCamera").ok_or("main scene has no MainCamera")?;
    b.world.camera = camera_def(camera)?;

    // Layers from the main scene's Level object.
    let (level_node, level) = find_component(inp.main, "Level").ok_or("main scene has no Level")?;
    b.world.clouds = Some(b.clouds(level_node)?);
    b.world.layers = Layer::ALL
        .iter()
        .map(|l| {
            let node = level_node["children"].as_array().and_then(|cs| {
                cs.iter()
                    .find(|c| c["name"].as_str().and_then(Layer::from_name) == Some(*l))
            });
            let ratio = node
                .and_then(|n| comp(n, "Parallax2D"))
                .and_then(|p| p["ratio"]["x"].as_f64())
                .unwrap_or(1.0) as f32;
            let z = node.map_or(0.0, |n| vec3(&n["pos"])[2]);
            LayerDef { ratio, z }
        })
        .collect();

    // Scatterables, matched to the Tile flag named "scatter" + prefab name without spaces.
    for r in level
        .get("scatterableObjects")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(path) = asset_path(Some(r)).map(str::to_owned) else {
            continue;
        };
        let Some(proto) = b.prefab(&path) else {
            continue;
        };
        let flag_name = format!(
            "scatter{}",
            b.world.protos[proto as usize].name.replace(' ', "")
        );
        match TILE_SCATTER_FLAGS.iter().position(|f| *f == flag_name) {
            Some(bit) => b.world.scatterables.push((proto, bit as u8)),
            None => eprintln!("warning: scatterable {path} has no Tile flag {flag_name}"),
        }
    }

    // Blocks: LevelBlocks/Blocks/<block>.
    let container = inp.blocks["roots"]
        .as_array()
        .and_then(|r| r.first())
        .and_then(|r| r["children"].as_array())
        .and_then(|c| c.first())
        .ok_or("blocks scene layout unexpected")?;
    for node in container["children"].as_array().into_iter().flatten() {
        if let Some(block) = b.block(node) {
            b.world.blocks.push(block);
        }
    }

    // Waves. orig: Resources/waves/*.asset.
    let waves = assets_of(inp.data, "Wave");
    let wave_index: HashMap<&str, u16> = waves
        .iter()
        .enumerate()
        .map(|(i, (p, _))| (*p, i as u16))
        .collect();
    b.world.waves = waves
        .iter()
        .map(|(p, f)| WaveDef {
            name: Path::new(p)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            trolls: curve(&f["numTrolls"]),
            tough_trolls: curve(&f["numToughTrolls"]),
            squids: curve(&f["numSquids"]),
            bosses: curve(&f["numBosses"]),
            killer_bosses: curve(&f["numKillerBosses"]),
            both_sides: flag(f, "spawnOnBothSides"),
            boss_wave: flag(f, "bossWave"),
        })
        .collect();

    // Island configs in Land order. orig: Holder.levelConfigs.
    let (_, holder) = find_component(inp.main, "Holder").ok_or("main scene has no Holder")?;
    for r in holder
        .get("levelConfigs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let path = asset_path(Some(r)).ok_or("level config ref")?;
        let asset = inp
            .data
            .get(path)
            .ok_or_else(|| format!("missing {path}"))?;
        let name = asset["name"].as_str().unwrap_or(path);
        let fields = asset["fields"].as_object().ok_or("level config fields")?;
        b.world.levels.push(level_config(name, fields, &wave_index));
    }

    lighting_and_days(&mut b.world, inp)?;

    let grass_path = asset_path(holder.get("grassPrefab")).map(str::to_owned);
    if let Some(node) = grass_path.as_deref().and_then(|p| inp.prefabs.get(p)) {
        let g = comp(node, "Grass").ok_or("grass prefab")?;
        let thicket = asset_path(g.get("thicketPrefab")).map(str::to_owned);
        let (_, world_comp) = find_component(inp.main, "World").ok_or("main scene has no World")?;
        let sprites = holder
            .get("grassSprites")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|s| b.sprite(Some(s)).unwrap_or(0))
            .collect();
        b.world.grass = Some(game::data::GrassDef {
            sprites,
            spacing: num(g, "spacing"),
            update_interval: num(g, "updateInterval"),
            z: comp(node, "FixedTransform").map_or(-0.01, |f| num(f, "z")),
            thicket: thicket.and_then(|p| b.prefab(&p)),
            thicket_spacing: num(world_comp, "thicketSpacing"),
        });
    }
    let list = |b: &mut Builder, k: &str| -> Vec<game::data::ProtoId> {
        holder
            .get(k)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|r| asset_path(Some(r)).map(str::to_owned))
            .filter_map(|p| b.prefab(&p))
            .collect()
    };
    b.world.wall_prefabs = list(&mut b, "wallPrefabs");
    b.bag();
    b.map();
    b.water();
    b.fire_style();
    // orig: UnlockManager.unlockableOnLand, serialized as little-endian int32 hex.
    if let Some((_, u)) = find_component(inp.main, "UnlockManager") {
        let hex = u
            .get("unlockableOnLand")
            .and_then(Value::as_str)
            .unwrap_or("");
        b.world.unlockable_on_land = hex
            .as_bytes()
            .chunks(8)
            .filter_map(|c| {
                let s = std::str::from_utf8(c).ok()?;
                let v = u32::from_str_radix(s, 16).ok()?;
                Some(v.swap_bytes() as u8)
            })
            .collect();
    }
    b.world.tower_prefabs = list(&mut b, "towerPrefabs");
    let (_, kingdom) = find_component(inp.main, "Kingdom").ok_or("main scene has no Kingdom")?;
    let border = asset_path(kingdom.get("borderPrefab")).map(str::to_owned);
    b.world.border = border.and_then(|p| b.prefab(&p));
    b.world.shield_shop = b.prefab("Resources/prefabs/buildings and interactive/ShopShield.prefab");
    b.world.bolt_sprite = inp
        .prefabs
        .get("Resources/prefabs/objects/Bolt.prefab")
        .and_then(|n| {
            // The sprite may sit on the root or a child.
            comp(n, "SpriteRenderer")
                .or_else(|| child_with(n, "SpriteRenderer").and_then(|c| comp(c, "SpriteRenderer")))
        })
        .and_then(|sr| sr.get("m_Sprite").cloned())
        .and_then(|s| b.sprite(Some(&s)));
    b.world.boulder_sprite = inp
        .prefabs
        .get("Resources/prefabs/objects/Boulder.prefab")
        .and_then(|n| comp(n, "SpriteRenderer"))
        .and_then(|sr| sr.get("m_Sprite").cloned())
        .and_then(|s| b.sprite(Some(&s)));
    b.world.rift_proto =
        b.prefab("Resources/prefabs/buildings and interactive/TeleporterRift.prefab");
    // orig: Player.standardSteedPrefab.
    let player = inp
        .prefabs
        .get("Resources/prefabs/characters/Player.prefab")
        .ok_or("no Player prefab")?;
    let standard = comp(player, "Player")
        .and_then(|p| asset_path(p.get("standardSteedPrefab")))
        .map(str::to_owned)
        .ok_or("Player.standardSteedPrefab")?;
    b.world.default_steed = b.steed(&standard).ok_or("standard steed")?;
    let farmland = asset_path(holder.get("farmlandPrefab")).map(str::to_owned);
    b.world.farmland = farmland.and_then(|p| b.farmland(&p));

    // Tool sprites. orig: Resources/prefabs/objects/Tool*.prefab.
    b.world.tool_sprites = ["ToolBow", "ToolHammer", "ToolScythe", "ToolShield", "Bread"]
        .iter()
        .map(|name| {
            let path = format!("Resources/prefabs/objects/{name}.prefab");
            let node = inp.prefabs.get(&path)?;
            let sr = comp(node, "SpriteRenderer")?;
            b.sprite(sr.get("m_Sprite"))
        })
        .collect();

    // Sun and moon. orig: CelestialBody children of Level/Heaven.
    let heaven_z = level_node["children"]
        .as_array()
        .and_then(|cs| cs.iter().find(|c| c["name"] == "Heaven"))
        .map_or(0.0, |h| vec3(&h["pos"])[2]);
    let heaven = level_node["children"]
        .as_array()
        .and_then(|cs| cs.iter().find(|c| c["name"] == "Heaven"))
        .ok_or("no Heaven")?;
    for body in heaven["children"].as_array().into_iter().flatten() {
        let Some(cb) = comp(body, "CelestialBody") else {
            continue;
        };
        let sprite = comp(body, "SpriteRenderer").and_then(|sr| b.sprite(sr.get("m_Sprite")));
        let z = heaven_z + vec3(&body["pos"])[2];
        // Every SpriteRenderer child is a glow halo, each with its own
        // sprite, scale, depth and tinting colour.
        let glows: Vec<game::data::CelestialGlow> = body["children"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|g| {
                let sr = comp(g, "SpriteRenderer")?;
                let sprite = b.sprite(sr.get("m_Sprite"))?;
                let scale = vec3(&g["scale"])[0];
                let col = &sr["m_Color"];
                Some(game::data::CelestialGlow {
                    sprite,
                    z: z + vec3(&g["pos"])[2],
                    scale: if scale > 0.0 { scale } else { 1.0 },
                    color: ["r", "g", "b", "a"].map(|k| col[k].as_f64().unwrap_or(1.0) as f32),
                })
            })
            .collect();
        let pair = |k: &str| {
            (
                cb[k]["x"].as_f64().unwrap_or(0.0) as f32,
                cb[k]["y"].as_f64().unwrap_or(0.0) as f32,
            )
        };
        b.world.celestials.push(game::data::CelestialDef {
            sprite,
            glows,
            is_sun: body["name"] == "Sun",
            orbit_scale: pair("orbitScale"),
            orbit_center: pair("orbitCenter"),
            rises_at: num(cb, "risesAt"),
            sets_at: num(cb, "setsAt"),
            z,
        });
    }

    // Sounds, and the soundtrack each day queues.
    let found = crate::audio::find(inp);
    for ((_, f), day) in assets_of(inp.data, "Day").iter().zip(&mut b.world.days) {
        day.tracks = f
            .get("tracks")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|t| {
                let sound = *found.by_prefab.get(asset_path(t.get("soundTrack"))?)?;
                Some(game::data::TrackDef {
                    anchor_time: t["anchorTime"].as_f64().unwrap_or(0.0) as f32,
                    anchor_seconds: t["trackAnchorInSeconds"].as_f64().unwrap_or(0.0) as f32,
                    sound,
                })
            })
            .collect();
    }
    {
        let settings = inp
            .assets_dir
            .parent()
            .ok_or("assets have no project directory")?
            .join("ProjectSettings");
        let physics =
            read_physics_settings(&settings, "Physics2DSettings.asset", "Physics2DSettings")?;
        let time = read_physics_settings(&settings, "TimeManager.asset", "TimeManager")?;
        let ground_height = catapult_ground(inp.main)?;
        let wall_material = read_physics_settings(
            &inp.assets_dir.join("PhysicsMaterial2D"),
            "Walls.physicsMaterial2D",
            "PhysicsMaterial2D",
        )?;
        if physics_number(&wall_material, "friction")? != 0.0
            || physics_number(&wall_material, "bounciness")? != 0.0
        {
            return Err("unsupported nonzero wall contact material".into());
        }
        let squid = inp
            .prefabs
            .get("Resources/prefabs/enemies/Squid.prefab")
            .ok_or("missing authored Squid prefab")?;
        b.world.squid = squid_definition(squid, &physics, &time, ground_height)?;
        let boss = inp
            .prefabs
            .get("Resources/prefabs/enemies/Boss.prefab")
            .ok_or("missing authored Boss prefab")?;
        let boulder = inp
            .prefabs
            .get("Resources/prefabs/objects/Boulder.prefab")
            .ok_or("missing authored Boulder prefab")?;
        b.world.boss_throw = boss_throw_definition(
            boss,
            boulder,
            &physics,
            &time,
            ground_height,
            inp.assets_dir,
        )?;
        for (path, &id) in &b.prefab_protos {
            if let Some(catapult) = &mut b.world.protos[id as usize].catapult {
                catapult.recoil = Some(
                    catapult_recoil(&inp.prefabs[path], &physics, &time, ground_height)
                        .map_err(|e| format!("{path}: recoil physics: {e}"))?,
                );
            }
        }
        if b.world
            .protos
            .iter()
            .any(|p| p.catapult.as_ref().is_some_and(|c| c.recoil.is_none()))
        {
            return Err("catapult physics missing from an unlinked scene prototype".into());
        }
    }
    if !b.errors.is_empty() {
        return Err(b.errors.join("; "));
    }
    b.world.audio = found.audio;
    Ok((b.world, found.clip_paths))
}

/// Preserve the actual Obstacles-layer geometry independently of attack targets.
fn obstacle_shapes(node: &Value) -> Result<Vec<game::data::ObstacleDef>, String> {
    fn walk(
        node: &Value,
        at: Affine2,
        out: &mut Vec<game::data::ObstacleDef>,
    ) -> Result<(), String> {
        if node["active"].as_bool() == Some(false) {
            return Ok(());
        }
        if node["layer"].as_u64() == Some(20) {
            for c in node["components"].as_array().into_iter().flatten() {
                let ty = c["type"].as_str().unwrap_or("");
                if !ty.ends_with("Collider2D") {
                    continue;
                }
                let f = c["fields"].as_object().ok_or("missing obstacle fields")?;
                if !flag(f, "m_Enabled") {
                    continue;
                }
                if flag(f, "m_UsedByComposite")
                    || flag(f, "m_UsedByEffector")
                    || flag(f, "m_IsTrigger")
                {
                    return Err("unsupported obstacle composite, effector or trigger".into());
                }
                if asset_path(f.get("m_Material"))
                    != Some("PhysicsMaterial2D/Walls.physicsMaterial2D")
                {
                    return Err("unsupported obstacle material".into());
                }
                let offset = physics_xy(&f["m_Offset"])?;
                let paths: Vec<Vec<[f32; 2]>> = match ty {
                    "PolygonCollider2D" => f["m_Points"]["m_Paths"]
                        .as_array()
                        .ok_or("missing obstacle polygon")?
                        .iter()
                        .map(|path| {
                            path.as_array()
                                .ok_or_else(|| "missing obstacle vertices".to_owned())?
                                .iter()
                                .map(physics_xy)
                                .collect()
                        })
                        .collect::<Result<_, _>>()?,
                    "BoxCollider2D" => {
                        let size = physics_xy(&f["m_Size"])?;
                        if size.iter().any(|v| *v <= 0.0) {
                            return Err("invalid obstacle box".into());
                        }
                        vec![vec![
                            [-size[0] * 0.5, -size[1] * 0.5],
                            [size[0] * 0.5, -size[1] * 0.5],
                            [size[0] * 0.5, size[1] * 0.5],
                            [-size[0] * 0.5, size[1] * 0.5],
                        ]]
                    }
                    _ => return Err(format!("unsupported obstacle collider {ty}")),
                };
                for path in paths {
                    let points: Vec<_> = path
                        .iter()
                        .map(|v| at.point([v[0] + offset[0], v[1] + offset[1]]))
                        .collect();
                    if points.len() < 3 || points.iter().flatten().any(|v| !v.is_finite()) {
                        return Err("invalid obstacle vertices".into());
                    }
                    out.push(game::data::ObstacleDef {
                        origin: at.translation,
                        points,
                    });
                }
            }
        }
        for child in node["children"].as_array().into_iter().flatten() {
            walk(
                child,
                at.compose(Affine2::from_pose(local_pose(child))),
                out,
            )?;
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(node, Affine2::IDENTITY, &mut out)?;
    Ok(out)
}

fn squid_definition(
    node: &Value,
    physics: &Value,
    time: &Value,
    ground_height: f32,
) -> Result<game::flight::Definition, String> {
    let squid = comp(node, "Squid").ok_or("missing Squid component")?;
    let body = comp(node, "Rigidbody2D").ok_or("missing Squid body")?;
    let mover = comp(node, "Mover").ok_or("missing Squid mover")?;
    let field = |f: &Map<String, Value>, key: &str| -> Result<f32, String> {
        f.get(key)
            .and_then(Value::as_f64)
            .map(|v| v as f32)
            .filter(|v| v.is_finite())
            .ok_or_else(|| format!("missing/invalid Squid {key}"))
    };
    let physical = node["children"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|n| n["tag"] == "PhysicalCollider")
        .ok_or("missing Squid physical collider child")?;
    let pose = local_pose(physical);
    let circle = comp(physical, "CircleCollider2D").ok_or("missing Squid physical circle")?;
    let sensor = comp(node, "PolygonCollider2D").ok_or("missing Squid hit sensor")?;
    if num(body, "m_BodyType") != 0.0
        || !flag(body, "m_Simulated")
        || flag(body, "m_UseAutoMass")
        || num(body, "m_Constraints") != 4.0
        || num(body, "m_CollisionDetection") != 0.0
        || !default_physics_material(&body["m_Material"])
        || !default_physics_material(&circle["m_Material"])
        || !default_physics_material(&physics["m_DefaultMaterial"])
        || pose.position != [0.0; 3]
        || pose.rotation != [0.0, 0.0, 0.0, 1.0]
        || pose.scale != [1.0; 3]
        || !flag(circle, "m_Enabled")
        || flag(circle, "m_IsTrigger")
        || flag(circle, "m_UsedByComposite")
        || flag(circle, "m_UsedByEffector")
        || !flag(sensor, "m_IsTrigger")
        || !flag(sensor, "m_Enabled")
        || physical["layer"].as_u64() != Some(21)
    {
        return Err("unsupported Squid rigidbody/collider topology or material".into());
    }
    for (a, b, collides) in [
        (21, 0, true),
        (21, 20, true),
        (17, 0, true),
        (17, 20, false),
    ] {
        if physics_layers_collide(physics, a, b)? != collides
            || physics_layers_collide(physics, b, a)? != collides
        {
            return Err("unsupported Squid chase/retreat collision layers".into());
        }
    }
    if physics_xy(&physics["m_Gravity"])?[0] != 0.0
        || physics_number(physics, "m_AutoSimulation")? != 1.0
        || physics_number(time, "m_TimeScale")? != 1.0
    {
        return Err("unsupported Squid simulation mode".into());
    }
    let positions = squid["carryPositions"]
        .as_array()
        .ok_or("missing Squid carry positions")?;
    if positions.is_empty() || positions.len() > u8::MAX as usize {
        return Err("invalid Squid carry capacity".into());
    }
    for reference in positions {
        let id = reference["local"]
            .as_i64()
            .ok_or("unlinked Squid carry position")?;
        if !node["children"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|n| n["transform_id"].as_i64() == Some(id))
        {
            return Err("missing direct Squid carry position".into());
        }
    }
    let iterations = |key| -> Result<u8, String> {
        physics[key]
            .as_u64()
            .filter(|v| *v > 0 && *v <= 255)
            .map(|v| v as u8)
            .ok_or_else(|| format!("invalid Squid {key}"))
    };
    let hit = &squid["hitBox"];
    let d = game::flight::Definition {
        mass: field(body, "m_Mass")?,
        gravity: -physics_xy(&physics["m_Gravity"])?[1],
        gravity_scale: field(body, "m_GravityScale")?,
        // Squid.OnEnable overrides the authored body drag to .5 every spawn.
        linear_drag: 0.5,
        angular_drag: field(body, "m_AngularDrag")?,
        fixed_step: physics_number(time, "Fixed Timestep")?,
        max_linear_speed: physics_number(physics, "m_MaxTranslationSpeed")?,
        max_acceleration_per_frame: field(mover, "maxAccelerationPerFrame")?,
        fly_lift: field(squid, "flyLift")?,
        fly_duration: field(squid, "flyDuration")?,
        fly_cooldown: field(squid, "flyCooldown")?,
        base_altitude: field(squid, "baseAltitude")?,
        move_speed: field(squid, "moveSpeed")?,
        scan_range: field(squid, "scanRange")?,
        attack_cooldown: field(squid, "attackCooldown")?,
        physical_radius: field(circle, "m_Radius")?,
        physical_offset: physics_xy(&circle["m_Offset"])?,
        dead_radius: field(squid, "deadRadius")?,
        grab_bounds: [
            physics_number(hit, "x")?,
            physics_number(hit, "y")?,
            physics_number(hit, "width")?,
            physics_number(hit, "height")?,
        ],
        carry_capacity: positions.len() as u8,
        spawn_overlay_color: ["r", "g", "b", "a"]
            .map(|key| physics_number(&squid["spawnOverlayColor"], key))
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?
            .try_into()
            .map_err(|_| "invalid overlay color")?,
        ground_height,
        // Unity 2017.1 null collider/body/global material defaults.
        friction: 0.4,
        restitution: 0.0,
        contact_offset: physics_number(physics, "m_DefaultContactOffset")?,
        velocity_threshold: physics_number(physics, "m_VelocityThreshold")?,
        max_linear_correction: physics_number(physics, "m_MaxLinearCorrection")?,
        baumgarte: physics_number(physics, "m_BaumgarteScale")?,
        velocity_iterations: iterations("m_VelocityIterations")?,
        position_iterations: iterations("m_PositionIterations")?,
    };
    if !d.valid() {
        return Err("invalid Squid flight definition".into());
    }
    Ok(d)
}

/// Resolve Boss.hand through its authored reference and follow the projectile's
/// actual collider material. The throwback solver expects a circular body with
/// a centered collider and the game's normal and ghost collision layers.
fn boss_throw_definition(
    boss: &Value,
    boulder: &Value,
    physics: &Value,
    time: &Value,
    ground_y: f32,
    assets_dir: &Path,
) -> Result<game::data::BossThrowDef, String> {
    let boss_script = comp(boss, "Boss").ok_or("missing Boss component")?;
    let hand_id = boss_script
        .get("hand")
        .and_then(|hand| hand["local"].as_i64())
        .ok_or("unlinked Boss.hand")?;
    let hand = boss["children"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|node| node["transform_id"].as_i64() == Some(hand_id))
        .ok_or("missing direct Boss.hand child")?;
    let boss_pose = local_pose(boss);
    let hand_pose = local_pose(hand);
    if hand["name"] != "Hand"
        || !hand["pos"].as_array().is_some_and(|position| {
            position.len() == 3 && position.iter().all(|value| value.as_f64().is_some())
        })
        || hand_pose.scale != [1.0; 3]
        || hand_pose.rotation != [0.0, 0.0, 0.0, 1.0]
        || boss_pose.scale != [1.0; 3]
        || boss_pose.rotation != [0.0, 0.0, 0.0, 1.0]
    {
        return Err("unsupported Boss.hand hierarchy or pose".into());
    }
    if comp(boss, "Animator").and_then(|a| asset_path(a.get("m_Controller")))
        != Some("AnimatorController/boss.controller")
    {
        return Err("unsupported Boss animator controller".into());
    }

    let script = comp(boulder, "Boulder").ok_or("missing Boulder component")?;
    let body = comp(boulder, "Rigidbody2D").ok_or("missing Boulder body")?;
    let circle = comp(boulder, "CircleCollider2D").ok_or("missing Boulder circle")?;
    let value = |fields: &Map<String, Value>, key: &str| -> Result<f32, String> {
        fields
            .get(key)
            .and_then(Value::as_f64)
            .map(|v| v as f32)
            .filter(|v| v.is_finite())
            .ok_or_else(|| format!("missing or invalid Boulder {key}"))
    };
    if boulder["layer"].as_u64() != Some(11)
        || !flag(body, "m_Simulated")
        || value(body, "m_BodyType")? != 0.0
        || value(body, "m_GravityScale")? != 1.0
        || value(body, "m_Constraints")? != 0.0
        || value(body, "m_CollisionDetection")? != 0.0
        || !flag(circle, "m_Enabled")
        || flag(circle, "m_IsTrigger")
        || flag(circle, "m_UsedByComposite")
        || flag(circle, "m_UsedByEffector")
        || physics_xy(
            circle
                .get("m_Offset")
                .ok_or("missing Boulder collider offset")?,
        )? != [0.0; 2]
    {
        return Err("unsupported Boulder body or collider".into());
    }
    for layer in [0, 8, 9, 10, 16, 20] {
        if !physics_layers_collide(physics, 11, layer)?
            || !physics_layers_collide(physics, layer, 11)?
            || physics_layers_collide(physics, 17, layer)? != (layer == 0)
            || physics_layers_collide(physics, layer, 17)? != (layer == 0)
        {
            return Err("unsupported Boulder normal/ghost collision layers".into());
        }
    }
    if physics_xy(&physics["m_Gravity"])?[0] != 0.0
        || physics_number(physics, "m_AutoSimulation")? != 1.0
        || physics_number(time, "m_TimeScale")? != 1.0
    {
        return Err("unsupported Boulder simulation mode".into());
    }
    let material = asset_path(circle.get("m_Material")).ok_or("unlinked Boulder material")?;
    let material = read_physics_settings(assets_dir, material, "PhysicsMaterial2D")?;
    let force = boss_script
        .get("throwForce")
        .ok_or("missing Boss.throwForce")?;
    let def = game::data::BossThrowDef {
        force: (physics_number(force, "min")?, physics_number(force, "max")?),
        hand: hand_pose.position,
        boulder_radius: value(circle, "m_Radius")?,
        boulder_ghost: value(script, "ghostTime")?,
        boulder_damage: script
            .get("hitDamage")
            .and_then(Value::as_i64)
            .and_then(|x| i32::try_from(x).ok())
            .ok_or("invalid Boulder.hitDamage")?,
        boulder_max_hits: script
            .get("maxHitCitizens")
            .and_then(Value::as_u64)
            .and_then(|x| u8::try_from(x).ok())
            .ok_or("invalid Boulder.maxHitCitizens")?,
        boulder_linear_drag: value(body, "m_LinearDrag")?,
        boulder_angular_drag: value(body, "m_AngularDrag")?,
        boulder_restitution: physics_number(&material, "bounciness")?,
        boulder_friction: physics_number(&material, "friction")?,
        gravity: -physics_xy(&physics["m_Gravity"])?[1],
        fixed_step: physics_number(time, "Fixed Timestep")?,
        ground_y,
    };
    if !def.valid() {
        return Err("invalid Boss throwback definition".into());
    }
    Ok(def)
}

fn read_physics_settings(dir: &Path, name: &str, key: &str) -> Result<Value, String> {
    let text = std::fs::read_to_string(dir.join(name)).map_err(|e| format!("{name}: {e}"))?;
    let body = text
        .lines()
        .filter(|l| !l.starts_with('%') && !l.starts_with("---"))
        .collect::<Vec<_>>()
        .join("\n");
    let value: Value = serde_yaml::from_str(&body).map_err(|e| format!("{name}: {e}"))?;
    Ok(value[key].clone())
}

fn physics_number(v: &Value, key: &str) -> Result<f32, String> {
    v[key]
        .as_f64()
        .map(|v| v as f32)
        .filter(|v| v.is_finite())
        .ok_or_else(|| format!("missing or invalid physics {key}"))
}

fn physics_xy(v: &Value) -> Result<[f32; 2], String> {
    Ok([physics_number(v, "x")?, physics_number(v, "y")?])
}

fn default_physics_material(v: &Value) -> bool {
    v.is_null() || v["fileID"].as_i64() == Some(0)
}

fn physics_layers_collide(physics: &Value, a: u32, b: u32) -> Result<bool, String> {
    if a >= 32 || b >= 32 {
        return Err("invalid physics layer".into());
    }
    let matrix = physics["m_LayerCollisionMatrix"]
        .as_str()
        .ok_or("missing physics layer collision matrix")?;
    if matrix.len() != 256 || !matrix.is_ascii() {
        return Err("invalid physics layer collision matrix".into());
    }
    let row = matrix
        .get(a as usize * 8..a as usize * 8 + 8)
        .ok_or("invalid physics layer collision matrix")?;
    let mut bytes = [0; 4];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&row[i * 2..i * 2 + 2], 16)
            .map_err(|_| "invalid physics layer collision matrix")?;
    }
    Ok(u32::from_le_bytes(bytes) & (1 << b) != 0)
}

/// Level.GenerateInternal (original CIL 0x06000fe5) places the ground transform
/// at world Y=0 and replaces its edge points with Y=.875*.5. Collider offset is
/// still applied. The workshop's spawn Y=1 is above this surface, not the floor.
fn catapult_ground(main: &Value) -> Result<f32, String> {
    let (level_node, level) = find_component(main, "Level").ok_or("missing ground Level")?;
    let id = level["ground"]["local"]
        .as_i64()
        .ok_or("unlinked Level.ground")?;
    let ground = level_node["children"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|c| c["id"].as_i64() == Some(id))
        .ok_or("missing direct ground child")?;
    for node in [level_node, ground] {
        let pose = local_pose(node);
        if pose.rotation != [0.0, 0.0, 0.0, 1.0] || pose.scale != [1.0; 3] {
            return Err("unsupported transformed catapult ground".into());
        }
    }
    if comp(ground, "Rigidbody2D").is_some() {
        return Err("catapult ground must be static".into());
    }
    if ground["layer"].as_u64() != Some(0) {
        return Err("unsupported catapult ground layer".into());
    }
    let edge = comp(ground, "EdgeCollider2D").ok_or("missing ground edge")?;
    if !flag(edge, "m_Enabled")
        || flag(edge, "m_IsTrigger")
        || flag(edge, "m_UsedByComposite")
        || flag(edge, "m_UsedByEffector")
        || num(edge, "m_EdgeRadius") != 0.0
        || !default_physics_material(&edge["m_Material"])
    {
        return Err("unsupported ground collider or material".into());
    }
    Ok(physics_xy(&edge["m_Offset"])?[1] + 0.875 * 0.5)
}

/// Extract the three dynamic bodies, free wheel hinges and flat-ground settings.
/// Fail unsupported topology/materials instead of silently changing PC physics.
fn catapult_recoil(
    node: &Value,
    physics: &Value,
    time: &Value,
    ground_height: f32,
) -> Result<game::recoil::Definition, String> {
    let p = |key| physics_number(physics, key);
    let body_settings = |body: &Map<String, Value>| -> Result<(), String> {
        if num(body, "m_BodyType") != 0.0
            || !flag(body, "m_Simulated")
            || flag(body, "m_UseAutoMass")
            || num(body, "m_GravityScale") != 1.0
            || num(body, "m_Constraints") != 0.0
            || num(body, "m_SleepingMode") != 1.0
            || num(body, "m_CollisionDetection") != 0.0
            || num(body, "m_Interpolate") != 0.0
            || !default_physics_material(&body["m_Material"])
        {
            return Err("unsupported dynamic body settings or material".into());
        }
        Ok(())
    };
    if physics_xy(&physics["m_Gravity"])?[0] != 0.0
        || !default_physics_material(&physics["m_DefaultMaterial"])
        || p("m_AutoSimulation")? != 1.0
        || physics_number(time, "m_TimeScale")? != 1.0
    {
        return Err("unsupported project gravity, material or simulation mode".into());
    }
    let iterations = |key| -> Result<u8, String> {
        physics[key]
            .as_u64()
            .filter(|v| *v > 0 && *v <= 255)
            .map(|v| v as u8)
            .ok_or_else(|| format!("unsupported physics {key}"))
    };
    let cat = comp(node, "Catapult").ok_or("missing Catapult")?;
    fn count_physics(node: &Value, counts: &mut [u32; 3]) {
        for c in node["components"].as_array().into_iter().flatten() {
            let ty = c["type"].as_str().unwrap_or("");
            if ty == "Rigidbody2D" {
                counts[0] += 1;
            }
            if ty.ends_with("Collider2D") {
                counts[1] += 1;
            }
            if ty.ends_with("Joint2D") {
                counts[2] += 1;
            }
        }
        for child in node["children"].as_array().into_iter().flatten() {
            count_physics(child, counts);
        }
    }
    let mut counts = [0; 3];
    count_physics(node, &mut counts);
    if counts != [3, 3, 2] {
        return Err("unsupported extra/missing catapult bodies, colliders or joints".into());
    }
    let body = comp(node, "Rigidbody2D").ok_or("missing chassis body")?;
    let body_id = comp_id(node, "Rigidbody2D").ok_or("missing chassis id")?;
    body_settings(body)?;
    // Catapult.Start forces root scale to one; direction only mirrors Flippable.
    if local_pose(node).rotation != [0.0, 0.0, 0.0, 1.0] {
        return Err("unsupported initial chassis rotation".into());
    }
    let children = node["children"]
        .as_array()
        .ok_or("missing catapult children")?;
    let flippable = cat["flippable"]["local"]
        .as_i64()
        .ok_or("unlinked flippable transform")?;
    let shape_node = children
        .iter()
        .find(|c| c["transform_id"].as_i64() == Some(flippable))
        .ok_or("missing direct flippable child")?;
    let mut wheels = Vec::new();
    let mut wheel_layers = Vec::new();
    for name in ["frontWheel", "rearWheel"] {
        let id = cat[name]["local"].as_i64().ok_or("unlinked wheel body")?;
        let wheel = children
            .iter()
            .find(|c| comp_id(c, "Rigidbody2D") == Some(id))
            .ok_or("wheel must be a direct root child outside Flippable")?;
        let rb = comp(wheel, "Rigidbody2D").ok_or("missing wheel body")?;
        let layer = wheel["layer"]
            .as_u64()
            .filter(|v| *v < 32)
            .ok_or("invalid wheel layer")? as u32;
        if !physics_layers_collide(physics, layer, 0)?
            || !physics_layers_collide(physics, 0, layer)?
        {
            return Err("wheel does not collide with the ground layer".into());
        }
        wheel_layers.push(layer);
        let hinge = comp(wheel, "HingeJoint2D").ok_or("missing wheel hinge")?;
        let circle = comp(wheel, "CircleCollider2D").ok_or("missing wheel circle")?;
        body_settings(rb)?;
        if wheel["transform_id"].as_i64() == Some(flippable)
            || !flag(hinge, "m_Enabled")
            || flag(hinge, "m_EnableCollision")
            || flag(hinge, "m_UseMotor")
            || flag(hinge, "m_UseLimits")
            || flag(hinge, "m_AutoConfigureConnectedAnchor")
            || hinge["m_BreakForce"].as_str() != Some("Infinity")
            || hinge["m_BreakTorque"].as_str() != Some("Infinity")
            || !flag(circle, "m_Enabled")
            || flag(circle, "m_IsTrigger")
            || flag(circle, "m_UsedByComposite")
            || flag(circle, "m_UsedByEffector")
            || !default_physics_material(&circle["m_Material"])
            || hinge["m_ConnectedRigidBody"]["local"].as_i64() != Some(body_id)
        {
            return Err("unsupported wheel collider, material or hinge settings".into());
        }
        let pose = local_pose(wheel);
        let anchor = physics_xy(&hinge["m_ConnectedAnchor"])?;
        if [pose.position[0], pose.position[1]] != anchor
            || pose.rotation != [0.0, 0.0, 0.0, 1.0]
            || pose.scale != [1.0; 3]
            || physics_xy(&hinge["m_Anchor"])? != [0.0; 2]
            || physics_xy(&circle["m_Offset"])? != [0.0; 2]
        {
            return Err("unsupported wheel or hinge transform".into());
        }
        wheels.push((
            num(rb, "m_Mass"),
            num(circle, "m_Radius"),
            anchor,
            num(rb, "m_LinearDrag"),
            num(rb, "m_AngularDrag"),
        ));
    }
    if wheels[0].0 != wheels[1].0
        || wheels[0].1 != wheels[1].1
        || wheels[0].3 != wheels[1].3
        || wheels[0].4 != wheels[1].4
    {
        return Err("asymmetric wheel mass, radius or drag".into());
    }
    if physics_layers_collide(physics, wheel_layers[0], wheel_layers[1])?
        || physics_layers_collide(physics, wheel_layers[1], wheel_layers[0])?
    {
        return Err("unsupported wheel-to-wheel contacts".into());
    }
    let pose = local_pose(shape_node);
    if pose.position != [0.0; 3] || pose.rotation != [0.0, 0.0, 0.0, 1.0] || pose.scale != [1.0; 3]
    {
        return Err("unsupported transformed chassis collider".into());
    }
    let shape = comp(shape_node, "PolygonCollider2D").ok_or("missing flippable chassis polygon")?;
    // This sensor contributes mass, but the wheels alone contact the ground.
    if !flag(shape, "m_Enabled")
        || !flag(shape, "m_IsTrigger")
        || flag(shape, "m_UsedByComposite")
        || flag(shape, "m_UsedByEffector")
        || !default_physics_material(&shape["m_Material"])
    {
        return Err("unsupported chassis sensor or material".into());
    }
    let paths = shape["m_Points"]["m_Paths"]
        .as_array()
        .ok_or("missing chassis polygon paths")?;
    if paths.len() != 1 || physics_xy(&shape["m_Offset"])? != [0.0; 2] {
        return Err("unsupported compound/offset chassis polygon".into());
    }
    let vertices = paths[0]
        .as_array()
        .ok_or("missing chassis vertices")?
        .iter()
        .map(physics_xy)
        .collect::<Result<Vec<_>, _>>()?;
    // Catapult.set_direction mirrors the polygon before moving centerOfMass to
    // the physical rear wheel. Translating the unmirrored polygon is incorrect.
    let reflected: Vec<_> = vertices.iter().map(|v| [-v[0], v[1]]).collect();
    let mass = num(body, "m_Mass");
    let d = game::recoil::Definition {
        body_mass: mass,
        body_inertia: game::recoil::polygon_inertia(&vertices, mass, wheels[0].2)
            .ok_or("invalid chassis inertia")?,
        rear_body_inertia: game::recoil::polygon_inertia(&reflected, mass, wheels[1].2)
            .ok_or("invalid reflected chassis inertia")?,
        body_linear_drag: num(body, "m_LinearDrag"),
        body_angular_drag: num(body, "m_AngularDrag"),
        wheel_linear_drag: wheels[0].3,
        wheel_angular_drag: wheels[0].4,
        wheel_mass: wheels[0].0,
        wheel_radius: wheels[0].1,
        front_anchor: wheels[0].2,
        rear_anchor: wheels[1].2,
        gravity: -physics_xy(&physics["m_Gravity"])?[1],
        ground_height,
        fixed_step: physics_number(time, "Fixed Timestep")?,
        // Unity 2017.1 Rigidbody2D.sharedMaterial docs: absent collider, body
        // and global materials use friction .4 and bounciness 0.
        // https://docs.unity3d.com/2017.1/Documentation/ScriptReference/Rigidbody2D-sharedMaterial.html
        friction: 0.4,
        restitution: 0.0,
        velocity_iterations: iterations("m_VelocityIterations")?,
        position_iterations: iterations("m_PositionIterations")?,
        velocity_threshold: p("m_VelocityThreshold")?,
        max_linear_correction: p("m_MaxLinearCorrection")?,
        max_angular_correction: p("m_MaxAngularCorrection")?.to_radians(),
        max_linear_speed: p("m_MaxTranslationSpeed")?,
        max_angular_speed: p("m_MaxRotationSpeed")?.to_radians(),
        baumgarte: p("m_BaumgarteScale")?,
        contact_offset: p("m_DefaultContactOffset")?,
        sleep_time: p("m_TimeToSleep")?,
        linear_sleep_tolerance: p("m_LinearSleepTolerance")?,
        angular_sleep_tolerance: p("m_AngularSleepTolerance")?.to_radians(),
    };
    if !d.valid() {
        return Err("invalid or unsupported recoil geometry/settings".into());
    }
    Ok(d)
}

#[cfg(test)]
mod recoil_tests {
    use super::*;
    use serde_json::json;

    fn prefab() -> Value {
        let component = |id, name, fields| json!({"id":id,"type":name,"fields":fields});
        let rb = |mass, drag| {
            json!({"m_Mass":mass,"m_BodyType":0,"m_LinearDrag":drag,"m_Simulated":1,
                "m_UseAutoMass":0,"m_SleepingMode":1,"m_Material":null,
                "m_AngularDrag":0,"m_GravityScale":1,"m_Constraints":0})
        };
        let wheel = |id, x| {
            json!({"transform_id":id+100,"layer":17,"pos":[x,0.171875,0.0],"components":[
                component(id,"Rigidbody2D",rb(1.0,0.0)),
                component(id+10,"CircleCollider2D",json!({"m_Radius":0.171875,"m_IsTrigger":0,
                    "m_Enabled":1,"m_Material":null,"m_Offset":{"x":0,"y":0}})),
                component(id+20,"HingeJoint2D",json!({"m_UseMotor":0,"m_UseLimits":0,
                    "m_Enabled":1,"m_BreakForce":"Infinity","m_BreakTorque":"Infinity",
                    "m_ConnectedRigidBody":{"local":1},"m_Anchor":{"x":0,"y":0},
                    "m_ConnectedAnchor":{"x":x,"y":0.171875}}))],"children":[]})
        };
        json!({"components":[component(1,"Rigidbody2D",rb(5.0,1.0)),
            component(2,"Catapult",json!({"frontWheel":{"local":3},"rearWheel":{"local":4},
                "flippable":{"local":105}}))],
            "children":[wheel(3,0.421875),wheel(4,-0.421875),
            {"transform_id":105,"pos":[0,0,0],"rot":[0,0,0,1],"scale":[1,1,1],
                "children":[],"components":[component(5,"PolygonCollider2D",json!({
                    "m_Enabled":1,"m_IsTrigger":1,"m_Material":null,"m_Offset":{"x":0,"y":0},
                    "m_Points":{"m_Paths":[[
                        {"x":0.29107857,"y":0.80594754},{"x":0.6120415,"y":0.03996277},
                        {"x":-0.256279,"y":0.0318774},{"x":-0.35272598,"y":0.18773437}]]}}))]}]})
    }

    pub(super) fn physics() -> Value {
        json!({"m_Gravity":{"x":0,"y":-15},"m_DefaultMaterial":{"fileID":0},
            "m_VelocityIterations":8,"m_PositionIterations":3,"m_VelocityThreshold":1,
            "m_MaxLinearCorrection":0.2,"m_MaxAngularCorrection":8,"m_MaxTranslationSpeed":100,
            "m_MaxRotationSpeed":360,"m_BaumgarteScale":0.2,"m_TimeToSleep":0.5,
            "m_LinearSleepTolerance":0.01,"m_AngularSleepTolerance":2,"m_DefaultContactOffset":0.01,
            "m_AutoSimulation":1,
            "m_LayerCollisionMatrix":"c98bbf00c8000000c8000000ffffffffc8000000c8000000ffffffffffffffffc95c0100c9080000c8590100c9071100c8250500c8100100c8051100c9800000c97d1000c9000000c9100000c9000800c9482100c9001000c8008000c9004000c8000000c8000000c8000000c8000000c8000000c8000000c8000000c8000000"})
    }
    pub(super) fn time() -> Value {
        json!({"Fixed Timestep":0.02,"m_TimeScale":1})
    }
    fn extract(node: &Value) -> Result<game::recoil::Definition, String> {
        catapult_recoil(node, &physics(), &time(), 0.8675)
    }
    fn main_scene() -> Value {
        json!({"roots":[{"components":[{"type":"Level","fields":{"ground":{"local":59}}}],
            "children":[{"id":59,"layer":0,"pos":[10,0,0],"components":[{"type":"EdgeCollider2D", "fields":{
                "m_Enabled":1,"m_IsTrigger":0,"m_Material":null,"m_EdgeRadius":0,
                "m_Offset":{"x":0,"y":0.43},"m_Points":[{"x":-0.5,"y":0},{"x":0.5,"y":0}]}}]}]}]})
    }

    #[test]
    fn catapult_physics_retains_three_body_geometry_drag_and_settings() {
        let d = extract(&prefab()).unwrap();
        assert_eq!(d.body_mass, 5.0);
        assert_eq!(d.front_anchor, [0.421875, 0.171875]);
        assert_eq!(d.rear_anchor, [-0.421875, 0.171875]);
        assert_eq!(
            (d.wheel_mass, d.wheel_radius, d.gravity, d.fixed_step),
            (1.0, 0.171875, 15.0, 0.02)
        );
        assert!((d.body_inertia - 0.8364514).abs() < 1e-6);
        assert!((d.body_inertia - d.rear_body_inertia).abs() < 1e-6);
        assert_eq!(
            (
                d.body_linear_drag,
                d.body_angular_drag,
                d.wheel_linear_drag,
                d.wheel_angular_drag
            ),
            (1.0, 0.0, 0.0, 0.0)
        );
        assert_eq!((d.velocity_iterations, d.position_iterations), (8, 3));
        assert_eq!(
            (d.friction, d.restitution, d.velocity_threshold),
            (0.4, 0.0, 1.0)
        );
        assert_eq!(
            (d.max_linear_correction, d.max_linear_speed, d.baumgarte),
            (0.2, 100.0, 0.2)
        );
        assert!((d.max_angular_correction - 8f32.to_radians()).abs() < 1e-6);
        assert!((d.max_angular_speed - core::f32::consts::TAU).abs() < 1e-6);
        assert_eq!(
            (d.contact_offset, d.sleep_time, d.linear_sleep_tolerance),
            (0.01, 0.5, 0.01)
        );
        assert!((d.angular_sleep_tolerance - 2f32.to_radians()).abs() < 1e-6);
        assert_eq!(d.ground_height, 0.8675);
        let mut changed = prefab();
        changed["components"][0]["fields"]["m_Mass"] = json!(10.0);
        let heavier = extract(&changed).unwrap();
        assert!((heavier.body_inertia - d.body_inertia * 2.0).abs() < 1e-6);
        assert!((heavier.rear_body_inertia - d.rear_body_inertia * 2.0).abs() < 1e-6);
    }

    #[test]
    fn extraction_rejects_topology_that_the_three_body_solver_cannot_represent() {
        for (pointer, value) in [
            ("/children/0/components/2/fields/m_UseMotor", json!(1)),
            ("/children/0/components/2/fields/m_UseLimits", json!(1)),
            ("/children/0/components/2/fields/m_BreakForce", json!(100)),
            (
                "/children/0/components/2/fields/m_EnableCollision",
                json!(1),
            ),
            (
                "/children/0/components/1/fields/m_Material",
                json!({"asset":"custom"}),
            ),
            ("/components/0/fields/m_Material", json!({"asset":"custom"})),
            ("/components/0/fields/m_Simulated", json!(0)),
            ("/components/0/fields/m_Constraints", json!(1)),
            ("/children/2/components/0/fields/m_IsTrigger", json!(0)),
            ("/children/2/transform_id", json!(999)),
            ("/children/0/pos/0", json!(0.5)),
        ] {
            let mut node = prefab();
            if let Some(slot) = node.pointer_mut(pointer) {
                *slot = value;
            } else {
                let (parent, key) = pointer.rsplit_once('/').unwrap();
                node.pointer_mut(parent).unwrap()[key] = value;
            }
            assert!(extract(&node).is_err(), "accepted {pointer}");
        }
        let mut node = prefab();
        let wheel = node["children"].as_array_mut().unwrap().remove(0);
        node["children"][1]["children"]
            .as_array_mut()
            .unwrap()
            .push(wheel);
        assert!(
            extract(&node).is_err(),
            "wheel under Flippable would mirror physical bodies"
        );
        let mut node = prefab();
        node["components"]
            .as_array_mut()
            .unwrap()
            .push(json!({"type":"BoxCollider2D","fields":{}}));
        assert!(
            extract(&node).is_err(),
            "an extra shape changes mass/contact geometry"
        );
    }

    #[test]
    fn extraction_retains_changed_solver_settings_and_rejects_unsupported_defaults() {
        let mut p = physics();
        p["m_VelocityIterations"] = json!(12);
        p["m_PositionIterations"] = json!(6);
        p["m_Gravity"]["y"] = json!(-12);
        p["m_MaxAngularCorrection"] = json!(4);
        let d = catapult_recoil(&prefab(), &p, &time(), 0.8675).unwrap();
        assert_eq!(
            (d.velocity_iterations, d.position_iterations, d.gravity),
            (12, 6, 12.0)
        );
        assert_eq!(d.max_angular_correction, 4f32.to_radians());
        p["m_DefaultMaterial"] = json!({"fileID":123});
        assert!(catapult_recoil(&prefab(), &p, &time(), 0.8675).is_err());
        let mut t = time();
        t["Fixed Timestep"] = json!(0);
        assert!(catapult_recoil(&prefab(), &physics(), &t, 0.8675).is_err());
        p = physics();
        p["m_VelocityIterations"] = json!(256);
        assert!(catapult_recoil(&prefab(), &p, &time(), 0.8675).is_err());
        p = physics();
        p["m_LayerCollisionMatrix"] = json!("00000000".repeat(32));
        assert!(catapult_recoil(&prefab(), &p, &time(), 0.8675).is_err());
        p["m_LayerCollisionMatrix"] = json!("ffffffff".repeat(32));
        assert!(catapult_recoil(&prefab(), &p, &time(), 0.8675).is_err());
    }

    #[test]
    fn physical_ground_uses_runtime_edge_points_plus_authored_offset() {
        let mut main = main_scene();
        assert_eq!(catapult_ground(&main).unwrap(), 0.8675);
        // Serialized editor points and Y position are replaced by GenerateInternal.
        main["roots"][0]["children"][0]["pos"][1] = json!(99);
        main["roots"][0]["children"][0]["components"][0]["fields"]["m_Points"][0]["y"] = json!(88);
        assert_eq!(catapult_ground(&main).unwrap(), 0.8675);
        main["roots"][0]["children"][0]["components"][0]["fields"]["m_Offset"]["y"] = json!(0.5);
        assert_eq!(catapult_ground(&main).unwrap(), 0.9375);
        main["roots"][0]["children"][0]["components"][0]["fields"]["m_IsTrigger"] = json!(1);
        assert!(catapult_ground(&main).is_err());
    }

    #[test]
    #[ignore = "requires the user's private PC asset export; set KINGDOM_RECOIL_REFERENCE_ASSETS"]
    fn local_reference_catapult_physics_export() {
        let assets =
            std::path::PathBuf::from(std::env::var("KINGDOM_RECOIL_REFERENCE_ASSETS").unwrap());
        let guids = crate::guid_map(&assets);
        let resolver = crate::scenes::Resolver::new(&guids, &assets);
        let tree = crate::scenes::export_file(
            &assets.join("Resources/prefabs/objects/Catapult.prefab"),
            &resolver,
        )
        .unwrap();
        let main = crate::scenes::export_file(&assets.join("main.unity"), &resolver).unwrap();
        let settings = assets.parent().unwrap().join("ProjectSettings");
        let p = read_physics_settings(&settings, "Physics2DSettings.asset", "Physics2DSettings")
            .unwrap();
        let t = read_physics_settings(&settings, "TimeManager.asset", "TimeManager").unwrap();
        let def =
            catapult_recoil(&tree["roots"][0], &p, &t, catapult_ground(&main).unwrap()).unwrap();
        assert_eq!(def.body_mass, 5.0);
        assert!((def.body_inertia - 0.8364514).abs() < 1e-6);
        assert!((def.rear_body_inertia - 0.8364514).abs() < 1e-6);
        assert_eq!(def.ground_height, 0.8675);
        assert_eq!((def.velocity_iterations, def.position_iterations), (8, 3));
        assert_eq!(
            (def.body_linear_drag, def.friction, def.restitution),
            (1.0, 0.4, 0.0)
        );
    }
}

#[cfg(test)]
mod squid_tests {
    use super::*;
    use serde_json::json;
    fn prefab() -> Value {
        json!({"components":[
            {"type":"Squid","fields":{"scanRange":5,"attackCooldown":4,"moveSpeed":1.3,
                "deadRadius":0.22,"spawnOverlayColor":{"r":0.5686275,"g":0.0627451,"b":0.3921569,"a":1},
                "hitBox":{"x":-0.2,"y":0.3,"width":1,"height":0.6},
                "flyLift":8,"flyDuration":0.6,"flyCooldown":0.4,"baseAltitude":2.2,
                "carryPositions":[{"local":3},{"local":4}]}},
            {"type":"Rigidbody2D","fields":{"m_BodyType":0,"m_Simulated":1,"m_UseAutoMass":0,
                "m_Constraints":4,"m_Mass":1,"m_GravityScale":0.3,"m_LinearDrag":0.5,"m_AngularDrag":0.1,
                "m_Material":null,"m_CollisionDetection":0}},
            {"type":"Mover","fields":{"maxAccelerationPerFrame":0}},
            {"type":"PolygonCollider2D","fields":{"m_IsTrigger":1,"m_Enabled":1}}],
            "children":[{"tag":"PhysicalCollider","layer":21,"components":[
                {"type":"CircleCollider2D","fields":{"m_Enabled":1,"m_IsTrigger":0,"m_Material":null,
                    "m_Offset":{"x":0,"y":0.875},"m_Radius":0.58}}]},
                {"transform_id":3},{"transform_id":4}]})
    }
    fn extract(node: &Value) -> Result<game::flight::Definition, String> {
        squid_definition(
            node,
            &super::recoil_tests::physics(),
            &super::recoil_tests::time(),
            0.8675,
        )
    }
    #[test]
    fn squid_uses_prefab_overrides_and_runtime_drag_not_constructor_defaults() {
        let mut node = prefab();
        let d = extract(&node).unwrap();
        assert_eq!(
            (d.move_speed, d.scan_range, d.base_altitude),
            (1.3, 5.0, 2.2)
        );
        assert_eq!(
            (d.fly_lift, d.fly_duration, d.fly_cooldown),
            (8.0, 0.6, 0.4)
        );
        assert_eq!(
            (d.mass, d.gravity, d.gravity_scale, d.linear_drag),
            (1.0, 15.0, 0.3, 0.5)
        );
        assert_eq!(
            (d.angular_drag, d.fixed_step, d.max_linear_speed),
            (0.1, 0.02, 100.0)
        );
        assert_eq!(
            (d.physical_radius, d.dead_radius, d.physical_offset),
            (0.58, 0.22, [0.0, 0.875])
        );
        assert_eq!(d.grab_bounds, [-0.2, 0.3, 1.0, 0.6]);
        assert_eq!(d.carry_capacity, 2);
        assert_eq!(
            d.spawn_overlay_color,
            [0.5686275, 0.0627451, 0.3921569, 1.0]
        );
        assert_eq!((d.velocity_iterations, d.position_iterations), (8, 3));
        assert_eq!(
            (d.friction, d.restitution, d.contact_offset),
            (0.4, 0.0, 0.01)
        );
        assert_eq!(d.ground_height, 0.8675);
        node["components"][0]["fields"]["flyLift"] = json!(10);
        node["components"][0]["fields"]["baseAltitude"] = json!(3.0);
        // OnEnable resets drag even when authored Rigidbody2D drag differs.
        node["components"][1]["fields"]["m_LinearDrag"] = json!(7);
        let d = extract(&node).unwrap();
        assert_eq!(
            (d.fly_lift, d.base_altitude, d.linear_drag),
            (10.0, 3.0, 0.5)
        );
    }
    #[test]
    fn squid_missing_fields_and_unsupported_collision_modes_fail_extraction() {
        for (pointer, value) in [
            ("/components/0/fields/flyLift", Value::Null),
            ("/components/0/fields/carryPositions", json!([])),
            ("/components/0/fields/hitBox/height", json!(0)),
            ("/components/1/fields/m_Constraints", json!(0)),
            ("/children/0/components/0/fields/m_IsTrigger", json!(1)),
            (
                "/children/0/components/0/fields/m_Material",
                json!({"asset":"custom"}),
            ),
        ] {
            let mut node = prefab();
            *node.pointer_mut(pointer).unwrap() = value;
            assert!(extract(&node).is_err(), "accepted {pointer}");
        }
        let mut p = super::recoil_tests::physics();
        p["m_LayerCollisionMatrix"] = json!("ffffffff".repeat(32));
        assert!(squid_definition(&prefab(), &p, &super::recoil_tests::time(), 0.8675).is_err());
    }
    fn obstacle() -> Value {
        json!({"pos":[100,80,0],"children":[{"pos":[2,3,0],"scale":[2,3,1],
            "children":[{"layer":20,"pos":[1,2,0],"components":[{"type":"PolygonCollider2D",
                "fields":{"m_Enabled":1,"m_IsTrigger":0,"m_Offset":{"x":0.5,"y":0.25},
                    "m_Material":{"asset":"PhysicsMaterial2D/Walls.physicsMaterial2D"},
                    "m_Points":{"m_Paths":[[{"x":0,"y":0},{"x":1,"y":0},{"x":0,"y":1}]]}}}]}]}]})
    }
    #[test]
    fn obstacle_polygons_keep_child_transforms_and_origin_separate_from_bounds() {
        let mut node = obstacle();
        let shapes = obstacle_shapes(&node).unwrap();
        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes[0].origin, [4.0, 9.0]);
        assert_eq!(
            shapes[0].points,
            vec![[5.0, 9.75], [7.0, 9.75], [5.0, 12.75]]
        );
        node["children"][0]["children"][0]["active"] = json!(false);
        assert!(obstacle_shapes(&node).unwrap().is_empty());
        node["children"][0]["children"][0]["active"] = json!(true);
        node["children"][0]["children"][0]["components"][0]["type"] = json!("CircleCollider2D");
        assert!(obstacle_shapes(&node).is_err());
    }
    #[test]
    #[ignore = "requires the user's private PC asset export; set KINGDOM_RECOIL_REFERENCE_ASSETS"]
    fn local_reference_squid_and_obstacles_export() {
        let assets =
            std::path::PathBuf::from(std::env::var("KINGDOM_RECOIL_REFERENCE_ASSETS").unwrap());
        let guids = crate::guid_map(&assets);
        let resolver = crate::scenes::Resolver::new(&guids, &assets);
        let read = |path: &str| crate::scenes::export_file(&assets.join(path), &resolver).unwrap();
        let settings = assets.parent().unwrap().join("ProjectSettings");
        let p = read_physics_settings(&settings, "Physics2DSettings.asset", "Physics2DSettings")
            .unwrap();
        let t = read_physics_settings(&settings, "TimeManager.asset", "TimeManager").unwrap();
        let tree = read("Resources/prefabs/enemies/Squid.prefab");
        let d = squid_definition(
            &tree["roots"][0],
            &p,
            &t,
            catapult_ground(&read("main.unity")).unwrap(),
        )
        .unwrap();
        assert_eq!(d, extract(&prefab()).unwrap());
        for level in 0..=4 {
            let path = format!("Resources/prefabs/buildings and interactive/Wall{level}.prefab");
            let wall = read(&path);
            let shapes = obstacle_shapes(&wall["roots"][0]).unwrap();
            if level == 0 {
                assert!(shapes.is_empty());
            } else {
                assert_eq!(shapes.len(), 1);
                assert_eq!(shapes[0].origin, [0.0; 2]);
                assert_eq!(shapes[0].points.len(), [0, 7, 8, 8, 6][level]);
            }
        }
    }
}

#[cfg(test)]
mod boss_throw_tests {
    use super::*;

    #[test]
    #[ignore = "requires the user's private PC asset export; set KINGDOM_RECOIL_REFERENCE_ASSETS"]
    fn local_reference_boss_throwback_export() {
        let assets =
            std::path::PathBuf::from(std::env::var_os("KINGDOM_RECOIL_REFERENCE_ASSETS").unwrap());
        let guids = crate::guid_map(&assets);
        let resolver = crate::scenes::Resolver::new(&guids, &assets);
        let read = |path: &str| crate::scenes::export_file(&assets.join(path), &resolver).unwrap();
        let settings = assets.parent().unwrap().join("ProjectSettings");
        let physics =
            read_physics_settings(&settings, "Physics2DSettings.asset", "Physics2DSettings")
                .unwrap();
        let time = read_physics_settings(&settings, "TimeManager.asset", "TimeManager").unwrap();
        let ground = catapult_ground(&read("main.unity")).unwrap();
        let boss = read("Resources/prefabs/enemies/Boss.prefab")["roots"][0].clone();
        let boulder = read("Resources/prefabs/objects/Boulder.prefab")["roots"][0].clone();
        let extract =
            |boss: &Value| boss_throw_definition(boss, &boulder, &physics, &time, ground, &assets);
        assert_eq!(extract(&boss).unwrap(), game::data::BossThrowDef::default());
        let mut altered = boss;
        let script = altered["components"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|component| component["type"] == "Boss")
            .unwrap();
        script["fields"]["throwForce"]["min"] = Value::from(5);
        assert_eq!(extract(&altered).unwrap().force, (5.0, 7.0));
        let script = altered["components"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|component| component["type"] == "Boss")
            .unwrap();
        script["fields"]["hand"]["local"] = Value::from(-1);
        assert!(extract(&altered).unwrap_err().contains("Boss.hand"));
    }
}

/// Data assets of `ty`, sorted by path, as (path, fields).
fn assets_of<'v>(data: &'v Map<String, Value>, ty: &str) -> Vec<(&'v str, &'v Map<String, Value>)> {
    let mut v: Vec<_> = data
        .iter()
        .filter(|(_, a)| a["type"] == ty)
        .filter_map(|(p, a)| Some((p.as_str(), a["fields"].as_object()?)))
        .collect();
    v.sort_by_key(|(p, _)| *p);
    v
}

fn light_preset(path: &str, f: &Map<String, Value>) -> LightPreset {
    let int = |k: &str| f.get(k).and_then(Value::as_i64).unwrap_or(0);
    let mut enabled = 0u32;
    let mut colors = [[0.0f32; 4]; LIGHT_COLORS.len()];
    for (i, name) in LIGHT_COLORS.iter().enumerate() {
        let field = f.get(*name).unwrap_or(&Value::Null);
        if field["enabled"].as_i64() == Some(1) {
            enabled |= 1 << i;
        }
        let c = &field["value"];
        colors[i] = ["r", "g", "b", "a"].map(|k| c[k].as_f64().unwrap_or(0.0) as f32);
    }
    let mut floats = [0.0f32; LIGHT_FLOATS.len()];
    for (j, name) in LIGHT_FLOATS.iter().enumerate() {
        let field = f.get(*name).unwrap_or(&Value::Null);
        if field["enabled"].as_i64() == Some(1) {
            enabled |= 1 << (LIGHT_COLORS.len() + j);
        }
        floats[j] = field["value"].as_f64().unwrap_or(0.0) as f32;
    }
    LightPreset {
        name: Path::new(path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        category: int("category") as u8,
        weather: int("weather") as u16,
        season: int("season") as u8,
        part_of_day: int("partOfDay") as u8,
        test_only: int("testOnly") != 0,
        enabled,
        colors,
        floats,
    }
}

fn red_moon_preset(main: &Value, lights: &[(&str, &Map<String, Value>)]) -> Result<u16, String> {
    let (_, manager) =
        find_component(main, "EnemyManager").ok_or("main scene has no EnemyManager")?;
    let path =
        asset_path(manager.get("redMoon")).ok_or("EnemyManager.redMoon has no asset reference")?;
    let (index, (_, fields)) = lights
        .iter()
        .enumerate()
        .find(|(_, (candidate, _))| *candidate == path)
        .ok_or_else(|| format!("EnemyManager.redMoon is not a converted BlenderState: {path}"))?;
    let preset = light_preset(path, fields);
    if fields.get("category").and_then(Value::as_i64) != Some(0)
        || fields
            .get("placeholder")
            .and_then(Value::as_i64)
            .is_some_and(|v| v != 0)
        || !preset.is_red_moon_overlay()
    {
        return Err(format!(
            "EnemyManager.redMoon is not a moon lighting overlay: {path}"
        ));
    }
    // General lighting extraction tolerates missing values. An explicitly
    // referenced overlay must provide the values it enables.
    for (i, name) in LIGHT_COLORS.iter().enumerate() {
        if preset.enabled & (1 << i) != 0
            && ["r", "g", "b", "a"].iter().any(|channel| {
                fields
                    .get(*name)
                    .and_then(|field| field["value"][*channel].as_f64())
                    .is_none()
            })
        {
            return Err(format!(
                "EnemyManager.redMoon.{name} has no RGBA value: {path}"
            ));
        }
    }
    if fields
        .get("stars")
        .and_then(|field| field["value"].as_f64())
        .is_none()
    {
        return Err(format!("EnemyManager.redMoon.stars has no value: {path}"));
    }
    u16::try_from(index).map_err(|_| "EnemyManager.redMoon index exceeds the asset format".into())
}

/// orig: `BlenderState` and `Day` assets, the `Director` day lists and `Blender.initialStack`.
fn lighting_and_days(world: &mut WorldData, inp: &Inputs) -> Result<(), String> {
    let lights = assets_of(inp.data, "BlenderState");
    world.red_moon = Some(red_moon_preset(inp.main, &lights)?);
    let light_index: HashMap<&str, u16> = lights
        .iter()
        .enumerate()
        .map(|(i, (p, _))| (*p, i as u16))
        .collect();
    world.lights = lights.iter().map(|(p, f)| light_preset(p, f)).collect();

    let days = assets_of(inp.data, "Day");
    let day_index: HashMap<&str, u16> = days
        .iter()
        .enumerate()
        .map(|(i, (p, _))| (*p, i as u16))
        .collect();
    world.days = days
        .iter()
        .map(|(p, f)| DayDef {
            name: Path::new(p)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            seasons: f.get("seasonOptions").and_then(Value::as_i64).unwrap_or(7) as u8,
            weathers: f.get("weatherOptions").and_then(Value::as_i64).unwrap_or(1) as u16,
            tracks: Vec::new(),
        })
        .collect();

    let (_, dir) = find_component(inp.main, "Director").ok_or("main scene has no Director")?;
    let day = |k: &str| -> Result<u16, String> {
        asset_path(dir.get(k))
            .and_then(|p| day_index.get(p).copied())
            .ok_or_else(|| format!("Director.{k} not found"))
    };
    let day_list = |k: &str| -> Vec<u16> {
        dir.get(k)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|r| asset_path(Some(r)).and_then(|p| day_index.get(p).copied()))
            .collect()
    };
    let n = &dir["numRegularDays"];
    let (_, blender) = find_component(inp.main, "Blender").ok_or("main scene has no Blender")?;
    world.director = DirectorDef {
        first_day: day("firstDay")?,
        first_autumn_day: day("firstAutumnDay")?,
        first_winter_day: day("firstWinterDay")?,
        regular_days: day_list("regularDays"),
        boss_days: day_list("bossDays"),
        recovery_days: day_list("recoveryDays"),
        num_regular_days: (
            n["min"].as_i64().unwrap_or(3) as i32,
            n["max"].as_i64().unwrap_or(4) as i32,
        ),
        initial_lights: blender
            .get("initialStack")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|r| asset_path(Some(r)).and_then(|p| light_index.get(p).copied()))
            .collect(),
    };
    Ok(())
}

#[cfg(test)]
mod cloud_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cloud_curve_preserves_steps_and_rejects_missing_tangents() {
        let mut curve = json!({"m_Curve": [
            {"time": 0.4, "value": 0.2, "inSlope": 0, "outSlope": "Infinity"},
            {"time": 0.8, "value": 0.1, "inSlope": 0, "outSlope": 0}
        ]});
        let parsed = cloud_curve(Some(&curve), "parallaxRatioCurve").unwrap();
        assert_eq!(parsed.eval(0.0), 0.2);
        assert_eq!(parsed.eval(0.6), 0.2);
        assert_eq!(parsed.eval(0.8), 0.1);
        curve["m_Curve"][0]["outSlope"] = Value::Null;
        assert!(cloud_curve(Some(&curve), "parallaxRatioCurve").is_err());
    }

    #[test]
    #[ignore = "requires the user's private PC asset export; set KINGDOM_REFERENCE_ASSETS"]
    fn local_reference_clouds_follow_prefab_material_and_scene_links() {
        let assets =
            std::path::PathBuf::from(std::env::var_os("KINGDOM_REFERENCE_ASSETS").unwrap());
        let guids = crate::guid_map(&assets);
        let resolver = crate::scenes::Resolver::new(&guids, &assets);
        let main = crate::scenes::export_file(&assets.join("main.unity"), &resolver).unwrap();
        let prefabs = [
            "GameObject/CloudStratus.prefab",
            "GameObject/CloudCumulus.prefab",
        ]
        .into_iter()
        .map(|path| {
            let file = crate::scenes::export_file(&assets.join(path), &resolver).unwrap();
            (path.into(), file["roots"][0].clone())
        })
        .collect();
        let sheet = crate::sprites::extract_all(&assets.join("Sprite"), &guids).unwrap();
        let unused = Value::Null;
        let inp = Inputs {
            main: &main,
            blocks: &unused,
            loader: &unused,
            prefabs: &prefabs,
            data: &Map::new(),
            sprite_names: &sheet.names_by_path,
            sprite_sizes: &SpriteSizes::new(),
            assets_dir: &assets,
        };
        let mut builder = Builder {
            inp: &inp,
            world: WorldData::default(),
            sprite_ids: HashMap::new(),
            prefab_protos: HashMap::new(),
            recolor: HashSet::new(),
            errors: Vec::new(),
        };
        let (level, _) = find_component(&main, "Level").unwrap();
        let def = builder.clouds(level).unwrap();
        assert_eq!(def.wind_change_cooldown, 60.0);
        let names = |layer: &CloudLayerDef| {
            layer
                .sprites
                .iter()
                .map(|&id| builder.world.sprites[usize::from(id)].as_str())
                .collect::<Vec<_>>()
        };
        let stratus = def.stratus;
        assert_eq!(stratus.origin, [0.0, 3.5, 50.0]);
        assert_eq!(stratus.max_clouds, 10);
        assert_eq!(stratus.depth, 2.0);
        assert_eq!(stratus.y_curve.eval(0.5), -0.75);
        assert_eq!(stratus.scale_curve.eval(1.0), 1.0);
        assert_eq!(names(&stratus), ["clouds_stratus_01", "clouds_stratus_02"]);
        let cumulus = def.cumulus;
        assert_eq!(cumulus.origin, [0.0, 5.5, 42.0]);
        assert_eq!(cumulus.max_clouds, 160);
        assert_eq!(cumulus.depth, 10.0);
        assert_eq!(cumulus.y_curve.eval(0.5), -2.1236117);
        assert_eq!(cumulus.scale_curve.eval(0.5), 1.25);
        assert_eq!(cumulus.parallax_curve.eval(0.5), 0.16669999);
        assert_eq!(cumulus.parallax_curve.eval(0.9), 0.099999994);
        assert_eq!(cumulus.parallax_curve.eval(1.0), 0.0);
        assert_eq!(
            names(&cumulus),
            ["clouds_01", "clouds_02", "clouds_03", "clouds_04"]
        );
        for layer in [stratus, cumulus] {
            assert_eq!(layer.width, 16.0);
            assert_eq!(layer.wind_multiplier, 1.0);
            assert_eq!(layer.lifetime, 3600.0);
            assert_eq!(layer.fade_time, 8.0);
            assert_eq!(layer.vertical_motion, 0.0);
            assert!(layer.allow_flip && layer.prefab_active);
        }
    }

    #[test]
    #[ignore = "requires the user's private PC asset export; set KINGDOM_REFERENCE_ASSETS"]
    fn local_reference_atmosphere_preserves_controller_layers_and_tight_meshes() {
        let assets =
            std::path::PathBuf::from(std::env::var_os("KINGDOM_REFERENCE_ASSETS").unwrap());
        let guids = crate::guid_map(&assets);
        let resolver = crate::scenes::Resolver::new(&guids, &assets);
        let main = crate::scenes::export_file(&assets.join("main.unity"), &resolver).unwrap();
        let prefabs = ["GameObject/Fog.prefab", "GameObject/Mist.prefab"]
            .into_iter()
            .map(|path| {
                let file = crate::scenes::export_file(&assets.join(path), &resolver).unwrap();
                (path.into(), file["roots"][0].clone())
            })
            .collect();
        let sheet = crate::sprites::extract_all(&assets.join("Sprite"), &guids).unwrap();
        let sizes = sheet
            .sprites
            .iter()
            .map(|sprite| {
                (
                    sprite.name.clone(),
                    (sprite.img.w, sprite.img.h, sprite.ppu),
                )
            })
            .collect();
        let unused = Value::Null;
        let def = atmosphere(&Inputs {
            main: &main,
            blocks: &unused,
            loader: &unused,
            prefabs: &prefabs,
            data: &Map::new(),
            sprite_names: &sheet.names_by_path,
            sprite_sizes: &sizes,
            assets_dir: &assets,
        })
        .unwrap();
        let fog = &def.fog;
        let mist = &def.mist;
        assert_eq!(fog.origin, [0.0, 1.0, -5.0]);
        assert_eq!(mist.origin, [0.0, 0.5, 1.5]);
        assert_eq!(
            (
                fog.max_clouds,
                fog.lifetime,
                fog.fade_time,
                fog.wind_multiplier
            ),
            (30, 3600.0, 8.0, 1.0)
        );
        assert_eq!(
            (
                mist.max_clouds,
                mist.lifetime,
                mist.fade_time,
                mist.wind_multiplier
            ),
            (25, 12.0, 2.0, 1.5)
        );
        assert_eq!(fog.parallax_curve.eval(0.65), 1.25);
        assert_eq!(fog.parallax_curve.eval(0.65000004), 1.0);
        assert_eq!(mist.parallax_curve.eval(0.49), 1.0);
        assert_eq!(mist.parallax_curve.eval(0.5), 0.75);
        assert_eq!(mist.vertical_motion, 0.1);
        assert_eq!(fog.sprites, [0, 1, 2, 3]);
        assert_eq!(mist.sprites, [4, 5, 6]);
        assert_eq!(
            def.sprites,
            ["fog_01", "fog_02", "fog_03", "fog_04", "mist_0", "mist_1", "mist_2"]
        );
        assert_eq!(
            def.meshes
                .iter()
                .map(|m| (m.vertices.len(), m.indices.len() / 3))
                .collect::<Vec<_>>(),
            [(14, 13), (8, 6), (16, 14), (7, 5), (7, 5), (7, 5), (11, 9)]
        );
        let bytes = game::assetfile::encode(game::assetfile::Kind::Atmosphere, &def).unwrap();
        assert_eq!(
            scene::atmosphere_data::AtmosphereDef::decode(&bytes).unwrap(),
            def
        );
    }
}

#[cfg(test)]
mod red_moon_tests {
    use super::*;
    use serde_json::json;

    fn scene(reference: Value) -> Value {
        json!({"roots": [{"components": [{"type": "EnemyManager", "fields": {
            "redMoon": reference,
        }}]}]})
    }

    fn overlay() -> Map<String, Value> {
        let mut fields = Map::new();
        fields.insert("category".into(), json!(0));
        for name in [
            "moonTint",
            "moonGlowColor",
            "cloudHighlightColor",
            "cloudStratusColor",
        ] {
            fields.insert(
                name.into(),
                json!({"enabled": 1, "value": {"r": 0.5, "g": 0.1, "b": 0.2, "a": 1}}),
            );
        }
        fields.insert("stars".into(), json!({"enabled": 1, "value": 0}));
        fields
    }

    #[test]
    fn extracts_the_manager_reference_instead_of_matching_a_preset_name() {
        let decoy = Map::new();
        let authored = overlay();
        let lights = [
            ("Red Moon.asset", &decoy),
            ("another path.asset", &authored),
        ];
        assert_eq!(
            red_moon_preset(&scene(json!({"asset": "another path.asset"})), &lights),
            Ok(1)
        );
        assert!(red_moon_preset(&scene(Value::Null), &lights).is_err());
        assert!(red_moon_preset(&scene(json!({"asset": "unknown.asset"})), &lights).is_err());
        assert!(red_moon_preset(&scene(json!({"asset": "Red Moon.asset"})), &lights).is_err());
    }

    #[test]
    fn rejects_weather_replacements_and_incomplete_or_invalid_active_channels() {
        let main = scene(json!({"asset": "overlay.asset"}));
        for (field, value) in [
            ("category", json!(1)),
            ("placeholder", json!(1)),
            ("stars", json!({"enabled": 1})),
            (
                "moonTint",
                json!({"enabled": 1, "value": {"r": 0.5, "g": 0.1, "b": 0.2}}),
            ),
            (
                "moonTint",
                json!({"enabled": 1, "value": {"r": 1.5, "g": 0.1, "b": 0.2, "a": 1}}),
            ),
            ("rain", json!({"enabled": 1, "value": 0.631})),
        ] {
            let mut fields = overlay();
            fields.insert(field.into(), value);
            assert!(
                red_moon_preset(&main, &[("overlay.asset", &fields)]).is_err(),
                "accepted {field}"
            );
        }
    }

    #[test]
    #[ignore = "requires the user's private PC asset export; set KINGDOM_REFERENCE_ASSETS"]
    fn local_reference_red_moon_preserves_the_scene_link_and_enabled_values() {
        const MOON: &str = "Resources/blender/Red Moon.asset";
        const RAIN: &str = "Resources/blender/Night Rain Red Moon.asset";
        let assets = Path::new(&std::env::var("KINGDOM_REFERENCE_ASSETS").unwrap()).to_path_buf();
        let guids = crate::guid_map(&assets);
        let resolver = crate::scenes::Resolver::new(&guids, &assets);
        let main = crate::scenes::export_file(&assets.join("main.unity"), &resolver).unwrap();
        let data = [RAIN, MOON]
            .into_iter()
            .map(|path| {
                let asset = crate::scenes::export_data_asset(&assets.join(path), &resolver)
                    .unwrap()
                    .unwrap();
                (path.into(), asset)
            })
            .collect();
        let lights = assets_of(&data, "BlenderState");
        let (path, fields) = lights[red_moon_preset(&main, &lights).unwrap() as usize];
        assert_eq!(path, MOON);
        let preset = light_preset(path, fields);
        assert_eq!(preset.enabled, 0x0040_6c00);
        assert_eq!(preset.colors[10], [0.58823526, 0.2292089, 0.14705881, 1.0]);
        assert_eq!(
            preset.colors[11],
            [0.97794116, 0.5033521, 0.5720853, 0.2509804]
        );
        assert_eq!(
            preset.colors[13],
            [0.38235295, 0.1040225, 0.1040225, 0.5686275]
        );
        assert_eq!(preset.colors[14], [0.7647059, 0.35986158, 0.35986158, 1.0]);
        assert_eq!(preset.floats[5], 0.0);
        assert!(red_moon_preset(&scene(json!({"asset": RAIN})), &lights).is_err());
    }
}

#[cfg(test)]
mod transform_tests {
    use super::*;
    use serde_json::json;

    fn extract(node: &Value) -> Proto {
        let empty = json!({});
        let maps = Map::new();
        let names = HashMap::from([("sprite".into(), "panel".into())]);
        let sizes = HashMap::from([("panel".into(), (4, 4, 32.0))]);
        let inp = Inputs {
            main: &empty,
            blocks: &empty,
            loader: &empty,
            prefabs: &maps,
            data: &maps,
            sprite_names: &names,
            sprite_sizes: &sizes,
            assets_dir: Path::new("."),
        };
        let mut builder = Builder {
            inp: &inp,
            world: WorldData::default(),
            sprite_ids: HashMap::new(),
            prefab_protos: HashMap::new(),
            recolor: HashSet::new(),
            errors: Vec::new(),
        };
        let index = builder.proto(node);
        builder.world.protos.remove(index as usize)
    }
    fn node(
        name: &str,
        pos: [f32; 3],
        scale: [f32; 3],
        rot: [f32; 4],
        children: Vec<Value>,
        sprite: bool,
    ) -> Value {
        json!({"name":name,"active":true,"pos":pos,"scale":scale,"rot":rot,"layer":0,
            "components":if sprite { vec![json!({"id":31,"type":"SpriteRenderer","fields":{
                "m_Enabled":1,"m_Sprite":{"asset":"sprite"},"m_FlipX":1,"m_FlipY":1}})] } else { vec![] },
            "children":children})
    }
    fn close(a: f32, b: f32) {
        assert!((a - b).abs() < 1e-5, "{a} != {b}");
    }

    #[test]
    fn reflection_layer_is_retained_with_its_ancestor_transform() {
        let mut reflection = node(
            "Reflection",
            [2.0, 1.0, 0.0],
            [1.0, -1.0, 1.0],
            [0.0, 0.0, 0.0, 1.0],
            vec![],
            true,
        );
        reflection["layer"] = json!(29);
        let normal = node(
            "Hull",
            [0.0; 3],
            [1.0; 3],
            [0.0, 0.0, 0.0, 1.0],
            vec![],
            true,
        );
        let root = node(
            "Boat",
            [0.0; 3],
            [2.0, 3.0, 1.0],
            [0.0, 0.0, 0.0, 1.0],
            vec![normal, reflection],
            false,
        );
        let p = extract(&root);
        assert_eq!(p.parts.len(), 2);
        assert!(!p.parts[0].reflection_only);
        assert!(p.parts[1].reflection_only);
        assert_eq!((p.parts[1].x, p.parts[1].y), (4.0, 3.0));
        assert_eq!(p.parts[1].transform.basis, [2.0, 0.0, 0.0, -3.0]);
        assert_eq!(p.transforms[p.parts[1].node as usize].path, "Reflection");
    }

    #[test]
    fn composed_parts_keep_nonrendered_parent_rotation_scale_and_renderer_flips() {
        let c = core::f32::consts::FRAC_1_SQRT_2;
        let panel = node(
            "Panel",
            [2.0, 0.0, 0.0],
            [-1.0, 1.0, 1.0],
            [0.0, 0.0, 0.0, 1.0],
            vec![],
            true,
        );
        let pivot = node(
            "Pivot",
            [1.0, 2.0, 0.0],
            [1.0, 3.0, 1.0],
            [
                0.0,
                0.0,
                (core::f32::consts::PI / 8.0).sin(),
                (core::f32::consts::PI / 8.0).cos(),
            ],
            vec![panel],
            false,
        );
        let root = node(
            "Root",
            [7.0, 8.0, 0.0],
            [2.0, 1.0, 1.0],
            [0.0, 0.0, c, c],
            vec![pivot],
            false,
        );
        let p = extract(&root);
        assert_eq!(
            p.transforms
                .iter()
                .map(|n| n.path.as_str())
                .collect::<Vec<_>>(),
            vec!["", "Pivot", "Pivot/Panel"]
        );
        assert_eq!(p.transforms[2].parent, Some(1));
        assert_eq!(p.parts.len(), 1);
        let part = &p.parts[0];
        assert_eq!(part.node, 2);
        assert!(
            part.flip && part.flip_y,
            "renderer flips must not absorb hierarchy signs"
        );
        close(part.x, -2.0 - 2.0 * c);
        close(part.y, 2.0 + 4.0 * c);
        for (a, b) in part
            .transform
            .basis
            .into_iter()
            .zip([c, -3.0 * c, -2.0 * c, -6.0 * c])
        {
            close(a, b);
        }
        let sampled = p.sampled_transform(part.node, |n| n.local).unwrap().0;
        assert_eq!(sampled, part.transform);
    }

    #[test]
    fn portal_surface_goo_and_catapult_rope_keep_authored_transform_magnitudes() {
        // Numeric metadata from the retained PC prefabs; no game pixels needed.
        let surface = node(
            "Surface",
            [0.0; 3],
            [1.0, 1.5, 1.0],
            [0.0, 0.0, 0.0, 1.0],
            vec![],
            true,
        );
        let goo = node(
            "Goo Spin",
            [0.0; 3],
            [-1.3, 1.1, 1.0],
            [0.0, 0.0, 0.0, 1.0],
            vec![],
            true,
        );
        let rope = node(
            "Rope",
            [0.0; 3],
            [6.659998, 1.0, 1.0],
            [0.0, 0.0, 0.36361992, 0.9315474],
            vec![],
            true,
        );
        let p = extract(&node(
            "Root",
            [0.0; 3],
            [1.0; 3],
            [0.0, 0.0, 0.0, 1.0],
            vec![surface, goo, rope],
            false,
        ));
        assert_eq!(p.parts[0].transform.basis, [1.0, 0.0, 0.0, 1.5]);
        assert_eq!(p.parts[1].transform.basis, [-1.3, 0.0, 0.0, 1.1]);
        let [a, b, c, d] = p.parts[2].transform.basis;
        close((a * a + c * c).sqrt(), 6.659998);
        close((b * b + d * d).sqrt(), 1.0);
        assert!(c > 4.0 && b < -0.6, "rope rotation was discarded");
    }
}

#[cfg(test)]
mod camera_tests {
    use super::*;
    #[test]
    fn camera_export_keeps_authored_hermite_tangents_and_rejects_invalid_curves() {
        let mut fields = serde_json::json!({"lead":1,"maxLeadSpeed":5,
            "shakeDuration":1,"shakeStrength":{"x":0.1875,"y":0.1875},
            "shakeVibrato":20,"shakeRandomness":30,
            "leadOverTime":{"m_Curve":[
                {"time":0,"value":0,"inSlope":0,"outSlope":0},
                {"time":3,"value":0,"inSlope":0,"outSlope":0.6183402},
                {"time":6,"value":2,"inSlope":-0.023091942,"outSlope":-0.023091942}]},
            "seekSpeedByDistance":{"m_Curve":[
                {"time":0,"value":1,"inSlope":0,"outSlope":0},
                {"time":12,"value":30,"inSlope":0,"outSlope":0}]}});
        let camera = camera_def(fields.as_object().unwrap()).unwrap();
        assert!(camera.lead);
        assert_eq!(camera.max_lead_speed, 5.0);
        assert_eq!(camera.shake, game::data::CameraShakeDef::default());
        assert!(
            (game::animation::sample(&camera.lead_over_time, 4.5, 0.0) - 1.240537).abs() < 1e-6
        );
        let mut missing_shake = fields.clone();
        missing_shake
            .as_object_mut()
            .unwrap()
            .remove("shakeStrength");
        assert!(camera_def(missing_shake.as_object().unwrap()).is_err());
        fields["leadOverTime"]["m_Curve"][1]["time"] = (-1).into();
        assert!(camera_def(fields.as_object().unwrap()).is_err());
    }
}
