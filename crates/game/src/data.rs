//! Static game data produced by `tools/extract` from the user's copy:
//! level blocks, object prototypes, island configs. Serialized with postcard
//! (`world.bin`) behind the `serde` feature.

use alloc::string::String;
use alloc::vec::Vec;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Index into [`WorldData::sprites`].
pub type SpriteId = u16;
/// Index into [`WorldData::protos`].
pub type ProtoId = u16;

/// orig: `ContentLayers` (+ the layer objects in the main scene).
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Layer {
    BackdropFar,
    BackdropNear,
    BackgroundFar,
    BackgroundNear,
    Game,
    ForegroundFar,
    ForegroundNear,
}

impl Layer {
    pub const ALL: [Layer; 7] = [
        Layer::BackdropFar,
        Layer::BackdropNear,
        Layer::BackgroundFar,
        Layer::BackgroundNear,
        Layer::Game,
        Layer::ForegroundFar,
        Layer::ForegroundNear,
    ];

    /// orig: `ContentLayers` names, which are also the layer object names.
    pub fn from_name(name: &str) -> Option<Layer> {
        Some(match name {
            "BackdropFarLayer" => Layer::BackdropFar,
            "BackdropNearLayer" => Layer::BackdropNear,
            "BackgroundFarLayer" => Layer::BackgroundFar,
            "BackgroundNearLayer" => Layer::BackgroundNear,
            "GameLayer" => Layer::Game,
            "ForegroundFarLayer" => Layer::ForegroundFar,
            "ForegroundNearLayer" => Layer::ForegroundNear,
            _ => return None,
        })
    }

    /// orig: `ContentLayers` enum order, used by `ScatteredObject.layer`.
    pub fn from_content_index(i: i64) -> Option<Layer> {
        Some(match i {
            0 => Layer::BackgroundFar,
            1 => Layer::BackgroundNear,
            2 => Layer::Game,
            3 => Layer::ForegroundFar,
            4 => Layer::ForegroundNear,
            5 => Layer::BackdropFar,
            6 => Layer::BackdropNear,
            _ => return None,
        })
    }
}

/// A content layer's placement in the main scene.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayerDef {
    /// orig: `Parallax2D.ratio.x`; 1 = moves with the world, 0 = fixed to the camera.
    pub ratio: f32,
    /// Depth of the layer object; larger is farther.
    pub z: f32,
}

/// Picks a random sprite for a part at level generation. orig: `RandomizeSprite`.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct RandomSprite {
    pub sprites: Vec<SpriteId>,
    pub allow_flip: bool,
    /// Use the pick of the nearest randomizing ancestor part instead. orig: `matchParent`.
    pub match_parent: bool,
    pub deactivation_probability: f32,
    /// Random sideways nudge of the part, ± this many units. orig: `shiftX`.
    pub shift_x: f32,
}

/// A 2D affine transform in world coordinates (Y up). Composing matrices
/// preserves the shear produced by rotated, nonuniformly scaled parents.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine2 {
    /// Row-major linear basis: xx, xy, yx, yy.
    pub basis: [f32; 4],
    pub translation: [f32; 2],
}

impl Default for Affine2 {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Affine2 {
    pub const IDENTITY: Self = Self {
        basis: [1.0, 0.0, 0.0, 1.0],
        translation: [0.0; 2],
    };

    pub fn from_pose(p: crate::animation::Pose) -> Self {
        let [x, y, z, w] = p.rotation;
        // Unity rotations are normalized quaternions. Normalize defensively
        // because exported rounded components need not have exact unit length.
        let n = x * x + y * y + z * z + w * w;
        let k = if n > 0.0 { 2.0 / n } else { 0.0 };
        Self {
            basis: [
                (1.0 - k * (y * y + z * z)) * p.scale[0],
                k * (x * y - z * w) * p.scale[1],
                k * (x * y + z * w) * p.scale[0],
                (1.0 - k * (x * x + z * z)) * p.scale[1],
            ],
            translation: [p.position[0], p.position[1]],
        }
    }

    /// Parent × local: a local point first goes through `local`, then `self`.
    pub fn compose(self, local: Self) -> Self {
        let [a, b, c, d] = self.basis;
        let [e, f, g, h] = local.basis;
        Self {
            basis: [a * e + b * g, a * f + b * h, c * e + d * g, c * f + d * h],
            translation: self.point(local.translation),
        }
    }

    pub fn point(self, p: [f32; 2]) -> [f32; 2] {
        let [a, b, c, d] = self.basis;
        [
            self.translation[0] + a * p[0] + b * p[1],
            self.translation[1] + c * p[0] + d * p[1],
        ]
    }

    /// Convert the world Y-up basis to screen Y-down, optionally reflecting
    /// the whole object about its root (not each child about its own pivot).
    pub fn screen_basis(self, mirrored: bool) -> [f32; 4] {
        let [a, b, c, d] = self.basis;
        let f = if mirrored { -1.0 } else { 1.0 };
        [f * a, -f * b, -c, d]
    }
}

/// An authored Transform, including ancestors without a SpriteRenderer.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct TransformNode {
    /// Relative to the prefab root, whose path is empty.
    pub path: String,
    pub parent: Option<u16>,
    pub local: crate::animation::Pose,
    /// Asset path of an enabled Animator's controller, when present.
    pub animator: Option<String>,
}

/// Relative path inside an Animator's subtree; siblings are never animated.
pub fn animation_path<'a>(path: &'a str, animator_root: &str) -> Option<&'a str> {
    if animator_root.is_empty() {
        Some(path)
    } else if path == animator_root {
        Some("")
    } else {
        path.strip_prefix(animator_root)?.strip_prefix('/')
    }
}

/// One SpriteRenderer of a prototype, flattened to the prototype's root.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub sprite: Option<SpriteId>,
    /// Unity layer 29: visible only to the extra water-reflection camera.
    pub reflection_only: bool,
    /// Offset from the prototype root, in units (scale already applied).
    pub x: f32,
    pub y: f32,
    pub z: f32,
    /// The SpriteRenderer flip only; hierarchy mirroring is in `transform`.
    pub flip: bool,
    pub flip_y: bool,
    /// Full authored hierarchy transform, including scale and rotation.
    pub transform: Affine2,
    /// Transform node used to resolve animation paths.
    pub node: u16,
    pub color: [u8; 4],
    /// orig: `SpriteRenderer.sortingLayer` value, then `sortingOrder`.
    pub sort_layer: i16,
    pub sort_order: i16,
    pub random: Option<RandomSprite>,
    /// Index of the parent part (for `match_parent`), if any.
    pub parent: Option<u16>,
    /// orig: `Recolor` material toggle → which colors to use.
    pub recolor: bool,
    /// Lit by the ambient color (the `Pow-*` lit materials); unlit materials
    /// and the default sprite material ignore lighting.
    pub lit: bool,
    /// Inside the building's `Scaffolding`: only shown while under construction.
    pub scaffold: bool,
}

/// orig: `FixedTransform`.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FixedTransform {
    pub fix_z: Option<f32>,
    pub fix_y: Option<f32>,
    pub random_offset_range: f32,
    /// Mirror to face away from the campfire. orig: `symmetric` / `flipped`.
    pub symmetric: bool,
    pub flipped: bool,
}

/// orig: `Payable` fields.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PayableDef {
    pub price: u8,
    pub price_increase: u8,
    /// orig: `floatingCoinPosition`, relative to the object.
    pub coin_offset: (f32, f32),
    pub coin_spacing: f32,
    /// orig: `playerPayPointOffset.x`.
    pub pay_point_offset: f32,
    /// orig: `playerPayDistance`.
    pub pay_distance: f32,
}

/// orig: `PayableUpgrade`.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UpgradeDef {
    pub next: Option<ProtoId>,
    pub only_in_buildable_region: bool,
    pub requires_stone: bool,
    pub cooldown: f32,
}

/// One scaffold piece. orig: `ScaffoldModule` (its sprite, then `fillSprite` once built that far).
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScaffoldModuleDef {
    pub transform: Affine2,
    pub flip: bool,
    pub flip_y: bool,
    pub sprite: Option<SpriteId>,
    pub fill: Option<SpriteId>,
    /// Offset from the building root, in units.
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// orig: `WorkableBuilding` (+ its inactive `Scaffolding` child).
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct BuildingDef {
    pub build_points: i32,
    pub auto_build_rate: f32,
    /// Modules ordered nearest-first, the order they fill in.
    /// orig: `Scaffolding.CompareScaffoldModule`.
    pub scaffold: Vec<ScaffoldModuleDef>,
}

/// orig: `ForestItem`.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ForestItemDef {
    pub controls_forest_size: bool,
    pub size_padding: f32,
    /// orig: root renderer bounds width (0 without a renderer).
    pub width: f32,
}

/// Tools sold in shops; each turns a peasant into a role.
/// orig: `DroppableTool` tags and `Character.Professions`.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolKind {
    Bow,
    Hammer,
    Scythe,
    Shield,
    /// The baker's loaves (only beggars want these). orig: Bread.prefab.
    Bread,
}

impl ToolKind {
    pub fn from_tag(tag: &str) -> Option<ToolKind> {
        Some(match tag {
            "Bow" => ToolKind::Bow,
            "Hammer" => ToolKind::Hammer,
            "Scythe" => ToolKind::Scythe,
            "Shield" => ToolKind::Shield,
            "Bread" => ToolKind::Bread,
            _ => return None,
        })
    }
}

/// Citizen roles. orig: character prefab tags.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    Beggar,
    Peasant,
    Worker,
    Archer,
    Farmer,
    Knight,
}

impl Role {
    pub fn from_tag(tag: &str) -> Option<Role> {
        Some(match tag {
            "Beggar" => Role::Beggar,
            "Peasant" => Role::Peasant,
            "Worker" => Role::Worker,
            "Archer" => Role::Archer,
            "Farmer" => Role::Farmer,
            "Knight" => Role::Knight,
            _ => return None,
        })
    }

    /// orig: `Character.Professions`.
    pub fn for_tool(t: ToolKind) -> Role {
        match t {
            ToolKind::Bow => Role::Archer,
            ToolKind::Hammer => Role::Worker,
            ToolKind::Scythe => Role::Farmer,
            ToolKind::Shield | ToolKind::Bread => Role::Knight,
        }
    }

    /// The tool a role drops when hit. orig: `Character.dropItem`.
    pub fn tool(self) -> Option<ToolKind> {
        match self {
            Role::Archer => Some(ToolKind::Bow),
            Role::Worker => Some(ToolKind::Hammer),
            Role::Farmer => Some(ToolKind::Scythe),
            Role::Knight => Some(ToolKind::Shield),
            Role::Beggar | Role::Peasant => None,
        }
    }
}

/// orig: `PayableShop`.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShopDef {
    pub tool: ToolKind,
    /// orig: `itemPlacement`, relative to the shop.
    pub item_placement: (f32, f32),
    pub item_spacing: f32,
    pub max_items: u8,
}

/// orig: `WorkableTree`.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TreeWorkDef {
    /// Chops needed. orig: `strength`.
    pub strength: i32,
    /// orig: `coinYield` (inclusive).
    pub coin_yield: (i32, i32),
    /// Part index of the chop mark, hidden until the tree is marked. orig: `chopMark`.
    pub chop_mark: Option<u16>,
}

/// orig: `BeggarCamp`.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CampDef {
    pub max_beggars: u8,
    pub spawn_interval: f32,
    pub hunger_interval: f32,
}

/// orig: `RandomizeTree`.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TreeDepth {
    pub z_front: f32,
    pub z_back: f32,
    pub back_color: [u8; 4],
    pub random_offset_range: f32,
}

/// orig: `ScatteredObject`.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct Scatter {
    pub layer: Layer,
    pub adjacent: bool,
    pub spacing: f32,
    pub irregularity: f32,
    pub dist_from_region_edge: f32,
    /// Tags of objects this must not overlap. orig: `avoidOverlapWith`.
    pub avoid: Vec<String>,
    /// The prefab root's own y and z, kept when scattered.
    pub y: f32,
    pub z: f32,
}

/// Bits of `Tile.scatterX` toggles, in [`TILE_SCATTER_FLAGS`] order.
pub type ScatterFlags = u32;

/// orig: the `scatter*` bools of `Tile`, in declaration order.
pub const TILE_SCATTER_FLAGS: [&str; 20] = [
    "scatterTree",
    "scatterTreeBG",
    "scatterShrub",
    "scatterShrubBig",
    "scatterShrubBigBG",
    "scatterShrubBirds",
    "scatterMoss",
    "scatterWaterReed",
    "scatterRoots",
    "scatterFireflies",
    "scatterWall0",
    "scatterFarmhouse0",
    "scatterTower0",
    "scatterHill",
    "scatterRocks",
    "scatterTreeCurtain",
    "scatterBackdropNear",
    "scatterBackdropFar",
    "scatterLightBeamBG",
    "",
];

/// Gameplay role of a prototype. Grows as systems are ported.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Kind {
    #[default]
    Scenery,
    /// A ground tile; blocks measure their width from these. orig: `Tile`.
    Tile {
        width: f32,
        no_ground: bool,
        scatter: ScatterFlags,
    },
    /// orig: `PrefabPlaceholder`: replaced by `proto` at generation.
    Placeholder {
        proto: ProtoId,
        spawn_in_game_layer: bool,
    },
    /// orig: `PrefabPlaceholder` with `doNotSwap`: invisible until something
    /// (e.g. a castle upgrade) swaps in `proto`.
    Dormant { proto: ProtoId },
    /// Tutorial hint glyph. orig: `HintText`; hidden until the tutorial is ported.
    Hint,
}

/// An object template: one block item, prefab, or scatterable.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Proto {
    pub name: String,
    pub tag: String,
    pub kind: Kind,
    pub parts: Vec<Part>,
    pub transforms: Vec<TransformNode>,
    pub fixed: Option<FixedTransform>,
    pub scatter: Option<Scatter>,
    /// Half the object's width, for scattering. orig: `ScatteredObject.GetHalfWidth()`.
    pub half_width: f32,
    /// orig: `RandomizeTree`: half the trees stand in front, half behind and tinted.
    pub tree: Option<TreeDepth>,
    /// Payable pay point region for overlap tests, if this is a payable:
    /// (x offset, half width). orig: `Level.GetOverlapRegion()`.
    pub pay_region: Option<(f32, f32)>,
    /// The prefab root's z, used when the object is spawned by an upgrade.
    pub root_z: f32,
    pub payable: Option<PayableDef>,
    pub upgrade: Option<UpgradeDef>,
    pub building: Option<BuildingDef>,
    pub forest_item: Option<ForestItemDef>,
    /// orig: `Castle.level` (0 = Castle1).
    pub castle_level: Option<u8>,
    pub shop: Option<ShopDef>,
    pub tree_work: Option<TreeWorkDef>,
    pub camp: Option<CampDef>,
    /// A citizen placed in the level (turned into a unit at start).
    pub character: Option<Role>,
    /// Has a `Wall` component (walls define the kingdom's borders).
    pub wall: bool,
    pub wall_def: Option<WallDef>,
    /// Enabled authored colliders on Unity's Obstacles layer, in root-local coordinates.
    pub obstacles: Vec<ObstacleDef>,
    pub portal: Option<PortalDef>,
    pub gaps: Option<Gaps>,
    pub farmhouse: Option<FarmhouseDef>,
    /// A grass seed patch. orig: `Grass`.
    pub grass: bool,
    /// Removes grass under its GAPS region once built. orig: `ClearSpaceOnStart.clearGrass`.
    pub clear_grass: bool,
    /// Where archers stand on it, relative to the (unflipped) root. orig: `GuardSlot` children.
    pub guard_slots: Vec<(f32, f32)>,
    /// orig: `Wall.level`.
    pub wall_level: Option<u8>,
    /// orig: `Tower.level`.
    pub tower_level: Option<u8>,
    /// Castle: x offsets of the shield shops it sets up. orig: `Castle.shieldShopPlaceholders`.
    pub shield_spots: Vec<f32>,
    /// Castle: seconds its keep takes to build (the shield shops wait for it).
    /// orig: the keep's `WorkableBuilding` (`buildPoints` / `autoBuildRate`).
    pub keep_time: f32,
    pub shield_shop: Option<ShieldShopDef>,
    /// Sets up a shield shop where it stands. orig: `TowerKnight`.
    pub tower_knight: bool,
    /// Spawns a banker when built. orig: `Castle.bankerPrefab`.
    pub banker: Option<ProtoId>,
    pub border: Option<BorderDef>,
    pub statue: Option<StatueDef>,
    /// Unlocks stone building once built. orig: `MasonShrine`.
    pub mason_shrine: bool,
    /// A steed standing free (index into `WorldData::steeds`). orig: `Steed` in `Mode.Free`.
    pub steed: Option<u16>,
    /// orig: `SteedSpawn`: `amount` distinct picks from these steeds.
    pub steed_spawn: Option<(u8, Vec<u16>)>,
    /// A hermit's cabin: the hermit it sends out. orig: `Cabin.hermitPrefab`.
    pub cabin: Option<ProtoId>,
    pub hermit: Option<HermitKind>,
    /// What this build site becomes when paid with a hermit riding along.
    /// orig: `PayableUpgrade.passengerUpgrades`.
    pub passenger_upgrades: Vec<(HermitKind, ProtoId)>,
    /// orig: `Baker` (`maxBeggars`, `callBeggarInterval`).
    pub baker: Option<(u8, f32)>,
    /// orig: `MerchantSpawner.merchantPrefab`.
    pub merchant_spawner: Option<ProtoId>,
    /// orig: `Merchant` (`numCoins`, `supplyTimeInHours`).
    pub merchant: Option<(u8, f32)>,
    /// A workshop's two catapult counters (child prototypes) and their x offsets.
    pub workshop: Vec<(ProtoId, f32)>,
    /// One counter: which side it builds for, and the catapult. orig: `PayableWorkshop`.
    pub workshop_side: Option<(i8, ProtoId)>,
    pub catapult: Option<CatapultDef>,
    /// A dog trapped under a fallen tree. orig: `DogSpawn`.
    pub dog_spawn: Option<DogSpawnDef>,
    /// orig: `PayableTeleporter.cooldown`.
    pub teleporter: Option<f32>,
    /// orig: `TeleporterRift` (it can be upgraded into a teleporter).
    pub rift: bool,
    /// orig: `Chest` (`coins`, `spillRange`).
    pub chest: Option<(u8, (f32, f32))>,
    /// orig: `Banker` (spawned by `Castle.bankerPrefab`).
    pub banker_def: Option<BankerDef>,
    pub ballista: Option<BallistaDef>,
    /// orig: `Wharf`.
    pub wharf: Option<WharfDef>,
    /// orig: `Boat`.
    pub boat: Option<BoatDef>,
    /// orig: `Beach` (pay to sail away).
    pub beach: bool,
    /// orig: `Signpost`: the part index of its map.
    pub signpost: Option<u16>,
    /// Torches, campfires and lanterns. orig: `FirePlaceholder` children.
    pub fires: Vec<FireDef>,
}

/// A fire on a prefab. orig: `FirePlaceholder` (swapped for `Holder.firePrefab`).
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FireDef {
    /// From the object's origin (x mirrors with it), z toward the camera negative.
    pub at: (f32, f32, f32),
    pub size: (f32, f32),
    pub night_only: bool,
    pub auto_start: bool,
    /// orig: `forceRealLight`: a point light, not just a glow sprite.
    pub real_light: bool,
}

/// How fires look. orig: `Fire.prefab` (its glow, light and curves) and
/// the `Flame` prefabs it picks from by size.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Default)]
pub struct FireStyleDef {
    pub glow: Option<SpriteId>,
    pub glow_color: [f32; 4],
    pub glow_scale: f32,
    pub light_color: [f32; 3],
    /// orig: `sizeToStrengthCurve` (by size magnitude), `intensityCurve`
    /// and `rangeCurve` (by strength), `ambientIntensity` (by the ambient
    /// light's grayscale).
    pub strength: Vec<(f32, f32)>,
    pub intensity: Vec<(f32, f32)>,
    pub range: Vec<(f32, f32)>,
    pub ambient: Vec<(f32, f32)>,
    /// Flame size and its first sprite (`None`: particles only).
    pub flames: Vec<((f32, f32), Option<SpriteId>)>,
    /// The monarch lights the torch below this ambient grayscale.
    /// orig: `Player.torchBrightnessThreshold`.
    pub torch_threshold: f32,
}

/// orig: `Wharf` and its `BoatPart` children.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Default)]
pub struct WharfDef {
    pub work_per_part: u8,
    /// Boat parts in `GetComponentsInChildren` order: (part index, centroid
    /// x, y relative to the part's pivot). orig: `BoatPart.centroid`.
    pub parts: Vec<(u16, f32, f32)>,
    /// orig: `skeletonBase` (part index, offset from the root).
    pub skeleton: Option<u16>,
    pub skeleton_at: (f32, f32),
    /// orig: `boatPrefab`.
    pub boat: Option<ProtoId>,
}

/// orig: `Boat`.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Default)]
pub struct BoatDef {
    pub water_height: f32,
    pub push_speed: f32,
    pub forest_speed_multiplier: f32,
    pub sail_speed: f32,
    pub max_workers: u8,
    pub max_knights: u8,
    /// x offsets of its `BoatPushPosition`s.
    pub push_positions: Vec<f32>,
    /// The sail's part index (its clip changes when it sets sail).
    pub sail: Option<u16>,
}

/// orig: `CoinBag` and its colliders, `BagCoin.prefab`. Units are the
/// bag's own (32 px each), y up, origin at the bag sprites' pivot.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Default)]
pub struct BagDef {
    pub back: Option<SpriteId>,
    pub front: Option<SpriteId>,
    pub closed: Option<SpriteId>,
    pub coin: Option<SpriteId>,
    /// orig: `BagCoin.colors`, by coin number.
    pub coin_colors: Vec<[f32; 3]>,
    /// The bag's wall (a solid outline). orig: `Collider`.
    pub wall: Vec<(f32, f32)>,
    /// Coins that reach this fell out. orig: the `Ground` trigger.
    pub spill: Vec<(f32, f32)>,
    /// A coin's outline. orig: `BagCoin/Collider`.
    pub coin_shape: Vec<(f32, f32)>,
    /// orig: `coinSpawnInterval`.
    pub spawn_interval: f32,
    /// orig: the coin's `Rigidbody2D` (gravity scale, linear and angular drag)
    /// and the `BagCoins` material.
    pub gravity_scale: f32,
    pub linear_drag: f32,
    pub angular_drag: f32,
    pub friction: f32,
    pub bounciness: f32,
}

/// orig: the loader scene's `Map` (pixel positions from its centre, y up).
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Default)]
pub struct MapDef {
    pub back: Option<SpriteId>,
    /// The background is 9-sliced to this size. orig: its `RectTransform`.
    pub back_size: (f32, f32),
    /// Sliced borders (left, bottom, right, top). orig: the sprites' `m_Border`.
    pub back_border: (f32, f32, f32, f32),
    pub button_border: (f32, f32, f32, f32),
    /// Sea monster, whale, wreck.
    pub details: Vec<(SpriteId, (f32, f32))>,
    /// By `Land`.
    pub lands: Vec<MapLandDef>,
    pub boat: Vec<SpriteId>,
    pub wreck: Vec<SpriteId>,
    pub pointer: Vec<SpriteId>,
    pub light_patch: Option<SpriteId>,
    pub map_icon: Option<SpriteId>,
    pub button: Option<SpriteId>,
}

/// orig: `MapLand` and its children.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Default)]
pub struct MapLandDef {
    pub sprite: Option<SpriteId>,
    pub at: (f32, f32),
    /// The boat beside it (offset, mirrored).
    pub boat: ((f32, f32), bool),
    pub pointer: ((f32, f32), bool),
    /// Covering it until it's unlocked. orig: `MapCloud`.
    pub clouds: Vec<(SpriteId, (f32, f32))>,
    /// orig: `MapUnlock`: unlockable, place, figurine sprite and its offset,
    /// and where the map icon hovers.
    pub unlocks: Vec<MapUnlockDef>,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Default)]
pub struct MapUnlockDef {
    pub unlock: u8,
    pub at: (f32, f32),
    pub icon: Option<SpriteId>,
    pub icon_at: (f32, f32),
    pub map_at: (f32, f32),
}

/// orig: `Unlockable`.
pub const UNLOCKABLES: [&str; 13] = [
    "StatueArcher",
    "StatueWorker",
    "StatueFarmer",
    "StatueKnight",
    "Meadow",
    "Grave",
    "Cave",
    "Waterfall",
    "Sakura",
    "Dog",
    "HermitBallista",
    "HermitBaker",
    "HermitKnight",
];

/// orig: `Catapult` and its `Boulder`.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CatapultDef {
    /// Arm turn per crank stroke, as a fraction of the 68° wind. orig: `crankRate`.
    pub crank_rate: f32,
    pub force: (f32, f32),
    /// Enemies it fires at: this far in front (min, max). orig: `scanRange`.
    pub scan: (f32, f32),
    pub hit_points: i32,
    pub boulder_damage: i32,
    pub stuck_probability: f32,
    /// Where the boulder leaves from (the spoon), relative.
    pub spoon: (f32, f32),
    /// Authored chassis, wheel, hinge and ground-contact physics.
    pub recoil: Option<crate::recoil::Definition>,
}

/// orig: `DogSpawn`.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Default)]
pub struct DogSpawnDef {
    /// orig: `colorOptions`.
    pub colors: Vec<[f32; 3]>,
    /// The trapped dog part, hidden once it's out, and its x offset.
    pub trapped_part: Option<u16>,
    pub trapped_x: f32,
}

/// orig: `Banker` prefab.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BankerDef {
    pub walk: f32,
    pub run: f32,
    pub wander_range: f32,
    pub coin_scan_range: f32,
    pub payout_portion: f32,
    pub daily_interest: f32,
    pub max_interest: i32,
    pub player_max_coins: u32,
    pub payout_interval: f32,
    pub max_coins: u32,
}

/// orig: the hermit prefabs' tags.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HermitKind {
    Baker,
    Ballista,
    Knight,
}

impl HermitKind {
    pub fn from_tag(tag: &str) -> Option<Self> {
        Some(match tag {
            "HermitBaker" => HermitKind::Baker,
            "HermitBallista" => HermitKind::Ballista,
            "HermitKnight" => HermitKind::Knight,
            _ => return None,
        })
    }
}

/// orig: `Ballista` and its `Bolt`.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Default)]
pub struct BallistaDef {
    /// Where bolts leave from, relative to the (unflipped) tower.
    pub bolt_pos: (f32, f32),
    pub range: f32,
    pub min_range: f32,
    pub angle: (f32, f32),
    pub reload_work: i32,
    pub damage: i32,
    pub max_hits: u8,
    /// The body part and its reload frames.
    pub body_part: Option<u16>,
    pub frames: Vec<SpriteId>,
    /// Where its reloader stands, relative. orig: `GetWorkOffset` −0.5, `GetWorkHeight` 2.0625.
    pub work_pos: (f32, f32),
}

/// orig: `Steed` (prefab values).
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Default)]
pub struct SteedDef {
    pub name: String,
    /// Its prototype, for when it stands free.
    pub proto: ProtoId,
    /// Clip name prefix, e.g. `player_horse`. orig: its animator override controller.
    pub clips: String,
    pub walk_speed: f32,
    pub run_speed: f32,
    pub forest_speed_multiplier: f32,
    pub walk_stamina_rate: f32,
    pub run_stamina_rate: f32,
    pub stand_stamina_rate: f32,
    pub eat_delay: f32,
    pub eat_duration: f32,
    pub rearing_cooldown: f32,
    pub rearing_duration: f32,
    pub well_fed_duration: f32,
    pub tired_duration: f32,
    pub reserve_stamina: f32,
    pub reserve_probability: f32,
    pub attacks_while_running: bool,
    /// orig: `hitBox` (x, y, width, height), facing right.
    pub hit_box: (f32, f32, f32, f32),
    pub attack_damage: i32,
    pub attack_stamina_cost: f32,
    pub attracts_deer: bool,
    pub poop_coins: u8,
    pub inspire_citizens: bool,
    pub inspired_duration: f32,
    pub inspired_range: f32,
    pub ability_cooldown: f32,
    pub recolor_to_coat: bool,
    pub disable_switching: bool,
    pub does_not_need_grass: bool,
    pub wander_range: f32,
    pub coat: [f32; 3],
    pub mane: [f32; 3],
    /// The rider's place (the `King` child).
    pub rider: (f32, f32),
    /// The rider's torch. orig: `King/Torch`'s `FirePlaceholder`.
    pub torch: Option<FireDef>,
    /// Where a hermit sits. orig: the `Passenger` child.
    pub passenger: (f32, f32),
}

/// orig: `Statue.Deity`.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Deity {
    Archer,
    Worker,
    Knight,
    Farmer,
    Time,
}

/// orig: `Statue`.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct StatueDef {
    pub deity: Deity,
    pub charge_days: f32,
    pub charge_multiplier: f32,
    pub max_charge: f32,
    /// The root sprite when neglected / while charged.
    pub overgrown: Option<SpriteId>,
    pub clean: Option<SpriteId>,
    /// Parts of the charge indicators, in order. orig: `charges`.
    pub charge_parts: Vec<u16>,
}

/// orig: `PayableShield`.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShieldShopDef {
    /// Where the shield waits, relative. orig: `shieldPosition`.
    pub shield_pos: (f32, f32),
    /// The banner part, shown once a knight carries the shield.
    pub banner_part: Option<u16>,
    pub banner: Option<SpriteId>,
    pub torn: Option<SpriteId>,
}

/// orig: `PayableBorder`.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BorderDef {
    pub distance_from_wall: f32,
    pub min_campfire_distance: f32,
    pub knight_range: f32,
}

/// orig: `Holder.grassSprites` / `grassPrefab` / `Grass.thicketPrefab`.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Default)]
pub struct GrassDef {
    /// 8 stages × 8 variations. orig: `grassSprites[stage * 8 + variation]`.
    pub sprites: Vec<SpriteId>,
    pub spacing: f32,
    pub update_interval: f32,
    pub z: f32,
    pub thicket: Option<ProtoId>,
    /// orig: `World.thicketSpacing`.
    pub thicket_spacing: f32,
}

/// A trigger box projected onto physics layers. orig: `GAPS`.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gaps {
    /// Left edge and width, relative to the (unflipped) object.
    pub x: f32,
    pub width: f32,
    /// Unity layer bits.
    pub layers: u32,
}

impl Gaps {
    pub const NOT_GRASSABLE: u32 = 1 << 24;
    pub const NOT_BUILDABLE: u32 = 1 << 26;
    pub const NOT_FARMABLE: u32 = 1 << 27;
    pub const FOREST: u32 = 1 << 28;

    /// World x-range of the region. orig: `GAPS.GetRegion()` (mirrored with the object).
    pub fn span(&self, x: f32, flip: bool) -> (f32, f32) {
        let left = if flip { -self.x - self.width } else { self.x };
        (x + left, x + left + self.width)
    }
}

/// orig: `Farmhouse`.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FarmhouseDef {
    pub update_interval: f32,
    pub max_farmlands: u8,
    pub statue_extra_farmlands: u8,
    pub left_seed_offset: f32,
    pub right_seed_offset: f32,
    pub level: u8,
}

/// orig: `Farmland` (the `Holder.farmlandPrefab`).
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Default)]
pub struct FarmlandDef {
    pub proto: ProtoId,
    pub farm_points: i32,
    pub coin_yield: i32,
    pub decay_interval: f32,
    pub coin_drop_force: (f32, f32),
    /// Growth frames for the root renderer and the `backgroundRenderer` part.
    pub frames: Vec<SpriteId>,
    pub back_frames: Vec<SpriteId>,
    /// (front, back) when left to rot, and when frozen.
    pub decayed: (Option<SpriteId>, Option<SpriteId>),
    pub frozen: (Option<SpriteId>, Option<SpriteId>),
    pub stages: Curve,
    pub spacing: f32,
    /// Part indices of the root renderer and the background renderer.
    pub front_part: u16,
    pub back_part: u16,
    /// Width of its GAPS region. orig: `CreateOrFindExisting`.
    pub width: f32,
}

/// A prototype placed in a block.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub proto: ProtoId,
    pub layer: Layer,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    /// -1 when the instance is mirrored. orig: `localScale.x`.
    pub scale_x: f32,
}

/// orig: `LevelBlockSide`.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Any,
    Left,
    Right,
    Distribute,
}

/// orig: `LevelBlock`.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct BlockDef {
    pub name: String,
    pub needs_unlock: bool,
    pub unlock_id: i32,
    pub min_distance: f32,
    pub max_distance: f32,
    pub maximize_mutual_distance: bool,
    pub side: Side,
    pub absolute_center: bool,
    /// orig: `groupOne` / `groupTwo` / `groupThree` (`LevelBlockGroup`, 0 = None).
    pub groups: [u8; 3],
    /// Left edge of the leftmost tile, block-local. orig: `LevelBlock.GetLeft()`.
    pub left: f32,
    /// orig: `LevelBlock.GetWidth()` (floored).
    pub width: i32,
    pub items: Vec<Placed>,
}

/// orig: `LevelConfig` (fields used so far).
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct LevelConfigDef {
    pub name: String,
    pub autumn_start: (i32, i32),
    pub winter_start: (i32, i32),
    pub days_before_game_over: i32,
    pub starting_coins: i32,
    pub min_level_width: i32,
    /// (min, max) per `LevelBlockGroup` index.
    pub group_counts: Vec<(i32, i32)>,
    /// Indices into `WorldData::waves`: [regular, boss, recovery] × [summer, autumn, winter].
    pub waves: [[Option<u16>; 3]; 3],
    /// orig: `waveTimedMultiplier` during `waveMultiplierTimeframe` (days).
    pub wave_timed_multiplier: f32,
    pub wave_multiplier_timeframe: (i32, i32),
    /// [summer, autumn, winter]. orig: `portalDefenceWave*`, `retaliationWave*`.
    pub defence_waves: [Option<u16>; 3],
    pub retaliation_waves: [Option<u16>; 3],
    /// orig: `seasonChangeDays` (how long the world takes to turn).
    pub season_change_days: f32,
    /// orig: `incomeMultiplier` (merchant cargo).
    pub income_multiplier: f32,
    /// orig: `freeBoatParts` (already stacked at the wharf).
    pub free_boat_parts: u8,
    /// orig: `waveBoatMultiplier` (once the boat is in the water).
    pub wave_boat_multiplier: f32,
    /// orig: `forceAllUnlocks`.
    pub force_all_unlocks: bool,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Default)]
pub struct WorldData {
    pub sprites: Vec<String>,
    pub protos: Vec<Proto>,
    pub blocks: Vec<BlockDef>,
    /// Islands One..Six. orig: `Holder.levelConfigs`.
    pub levels: Vec<LevelConfigDef>,
    /// orig: `Level.scatterableObjects`, with the `Tile` flag bit each one uses.
    pub scatterables: Vec<(ProtoId, u8)>,
    /// Indexed by `Layer as usize`.
    pub layers: Vec<LayerDef>,
    pub lights: Vec<LightPreset>,
    pub days: Vec<DayDef>,
    pub director: DirectorDef,
    pub celestials: Vec<CelestialDef>,
    /// Sprite of each tool, indexed by `ToolKind as usize`. orig: `Tool*.prefab`.
    pub tool_sprites: Vec<Option<SpriteId>>,
    pub waves: Vec<WaveDef>,
    pub farmland: Option<FarmlandDef>,
    pub grass: Option<GrassDef>,
    /// orig: `Castle.shieldShopPrefab` / `TowerKnight.shieldShopPrefab`.
    pub shield_shop: Option<ProtoId>,
    /// Wall and tower stage by level (1-based). orig: `Holder.wallPrefabs` / `towerPrefabs`.
    pub wall_prefabs: Vec<ProtoId>,
    pub tower_prefabs: Vec<ProtoId>,
    /// orig: `Kingdom.borderPrefab`.
    pub border: Option<ProtoId>,
    pub steeds: Vec<SteedDef>,
    /// orig: Bolt.prefab sprite.
    pub bolt_sprite: Option<SpriteId>,
    /// orig: Boulder.prefab sprite.
    pub boulder_sprite: Option<SpriteId>,
    /// The rift a teleporter's exit leaves behind. orig: `TeleporterExit.teleporterRiftPrefab`.
    pub rift_proto: Option<ProtoId>,
    /// orig: `Player.standardSteedPrefab`.
    pub default_steed: u16,
    /// The purse the monarch's coins pile up in. orig: `CoinBag`, `BagCoin`.
    pub bag: BagDef,
    /// The island map. orig: `Map` in the loader scene.
    pub map: MapDef,
    /// The island each unlockable is found on (`Land` index), by `Unlockable`.
    /// orig: `UnlockManager.unlockableOnLand`.
    pub unlockable_on_land: Vec<u8>,
    /// The water band and its reflection. orig: `Level/Water`.
    pub water: WaterDef,
    pub audio: AudioDef,
    pub fire: FireStyleDef,
    /// Authored animation curves, event markers and controller state metadata.
    pub animations: crate::animation::Library,
    /// MainCamera's authored anticipation and target-seeking curves.
    pub camera: CameraDef,
    /// Squid prefab flight, scanner and physical collider parameters.
    pub squid: crate::flight::Definition,
    /// Boss hand and force, plus the authored projectile used for throwback.
    pub boss_throw: BossThrowDef,
    /// The two Playground particle bursts referenced by Boulder. Synthetic worlds omit these.
    pub boulder_fx: Option<BoulderFxDef>,
    /// Exact `EnemyManager.redMoon` reference. Synthetic worlds may omit it.
    pub red_moon: Option<u16>,
    /// Sky `CloudController` instances. Synthetic worlds may omit these.
    pub clouds: Option<CloudsDef>,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct CloudsDef {
    pub stratus: CloudLayerDef,
    pub cumulus: CloudLayerDef,
    /// orig: `World.windChangeCooldown`.
    pub wind_change_cooldown: f32,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct CloudLayerDef {
    /// Controller origin in Unity world units. Cloud depth adds to Z.
    pub origin: [f32; 3],
    pub depth: f32,
    /// Distance from the camera-following origin to either horizontal wrap edge.
    pub width: f32,
    pub wind_multiplier: f32,
    pub lifetime: f32,
    pub fade_time: f32,
    pub vertical_motion: f32,
    pub max_clouds: u16,
    /// `RandomizeSprite.sprites`, in authored order, with the cloud shader's
    /// coverage baked into alpha. Sample these with nearest filtering.
    pub sprites: Vec<SpriteId>,
    pub prefab_active: bool,
    pub allow_flip: bool,
    pub y_curve: Curve,
    pub scale_curve: Curve,
    pub parallax_curve: Curve,
}

impl CloudsDef {
    pub fn sprite_ids(&self) -> impl Iterator<Item = SpriteId> + '_ {
        self.stratus
            .sprites
            .iter()
            .chain(&self.cumulus.sprites)
            .copied()
    }

    pub fn valid(&self, sprite_count: usize) -> bool {
        self.wind_change_cooldown > 0.0
            && self.wind_change_cooldown <= 3600.0
            && self.stratus.valid(sprite_count)
            && self.cumulus.valid(sprite_count)
    }
}

impl CloudLayerDef {
    pub fn valid(&self, sprite_count: usize) -> bool {
        self.valid_with_parallax(sprite_count, 1.0)
    }

    pub fn valid_with_parallax(&self, sprite_count: usize, max_parallax: f32) -> bool {
        let curve_valid = |curve: &Curve, min, max| {
            (1..=16).contains(&curve.keys.len())
                && curve.keys.iter().all(|&[t, v, incoming, outgoing]| {
                    (0.0..=1.0).contains(&t)
                        && (min..=max).contains(&v)
                        && [incoming, outgoing]
                            .into_iter()
                            .all(|s| s.is_infinite() || s.abs() <= 100.0)
                })
                && curve.keys.windows(2).all(|k| k[0][0] < k[1][0])
        };
        self.origin.iter().all(|v| v.abs() <= 1000.0)
            && (0.01..=100.0).contains(&self.depth)
            && (0.01..=100.0).contains(&self.width)
            && self.wind_multiplier.abs() <= 10.0
            && (0.01..=60.0).contains(&self.fade_time)
            && (2.0 * self.fade_time..=7200.0).contains(&self.lifetime)
            && self.vertical_motion.abs() <= 10.0
            && (1..=256).contains(&self.max_clouds)
            && (1..=16).contains(&self.sprites.len())
            && self
                .sprites
                .iter()
                .all(|&id| usize::from(id) < sprite_count)
            && curve_valid(&self.y_curve, -100.0, 100.0)
            && curve_valid(&self.scale_curve, 0.01, 10.0)
            && curve_valid(&self.parallax_curve, 0.0, max_parallax)
    }
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct BoulderFxDef {
    pub dust: BoulderEmitterDef,
    pub fragments: BoulderEmitterDef,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct BoulderEmitterDef {
    pub count: u8,
    pub sprites: Vec<SpriteId>,
    /// Dust advances through the row, while each fragment selects a row at birth.
    pub animate: bool,
    pub size: f32,
    pub lifetime: [f32; 2],
    pub scatter_min: [f32; 3],
    pub scatter_max: [f32; 3],
    pub velocity_min: [f32; 2],
    pub velocity_max: [f32; 2],
    /// Playground subtracts this vector in each update.
    pub gravity: [f32; 2],
    pub damping: f32,
    pub rotation: [f32; 2],
    pub spin: [f32; 2],
    pub colors: Vec<BoulderGradientDef>,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoulderGradientDef {
    pub color: [f32; 4],
    /// Fraction of the particle lifetime when its alpha begins to fade to zero.
    pub fade_start: f32,
}

impl BoulderFxDef {
    pub fn valid(&self, sprite_count: usize) -> bool {
        [&self.dust, &self.fragments]
            .into_iter()
            .all(|emitter| emitter.valid(sprite_count))
    }
}

impl BoulderEmitterDef {
    fn valid(&self, sprite_count: usize) -> bool {
        let range = |[min, max]: [f32; 2]| {
            min.is_finite() && max.is_finite() && min <= max && min >= -1000.0 && max <= 1000.0
        };
        (1..=32).contains(&self.count)
            && (1..=16).contains(&self.sprites.len())
            && self.sprites.iter().all(|&id| (id as usize) < sprite_count)
            && self.size.is_finite()
            && (0.01..=2.0).contains(&self.size)
            && range(self.lifetime)
            && self.lifetime[0] > 0.0
            && self.lifetime[1] <= 5.0
            && self
                .scatter_min
                .into_iter()
                .zip(self.scatter_max)
                .all(|(min, max)| range([min, max]))
            && self
                .velocity_min
                .into_iter()
                .zip(self.velocity_max)
                .all(|(min, max)| range([min, max]))
            && self
                .gravity
                .into_iter()
                .all(|g| g.is_finite() && g.abs() <= 100.0)
            && (0.0..=10.0).contains(&self.damping)
            && range(self.rotation)
            && range(self.spin)
            && (1..=8).contains(&self.colors.len())
            && self.colors.iter().all(|gradient| {
                gradient.color.iter().all(|v| (0.0..=1.0).contains(v))
                    && (0.0..1.0).contains(&gradient.fade_start)
            })
    }
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BossThrowDef {
    pub force: (f32, f32),
    /// Rest position of the direct Hand child, before facing is applied.
    pub hand: [f32; 3],
    pub boulder_radius: f32,
    pub boulder_ghost: f32,
    pub boulder_damage: i32,
    pub boulder_max_hits: u8,
    pub boulder_linear_drag: f32,
    pub boulder_angular_drag: f32,
    pub boulder_restitution: f32,
    pub boulder_friction: f32,
    pub gravity: f32,
    pub fixed_step: f32,
    /// Absolute PC world height. Projectile state stores ground-relative Y.
    pub ground_y: f32,
}

impl Default for BossThrowDef {
    /// Shipped PC values for synthetic worlds. Extraction reads the assets.
    fn default() -> Self {
        Self {
            force: (4.0, 7.0),
            hand: [0.4, 0.125, 0.0],
            boulder_radius: 0.15625,
            boulder_ghost: 0.3,
            boulder_damage: 15,
            boulder_max_hits: 5,
            boulder_linear_drag: 0.1,
            boulder_angular_drag: 1.5,
            boulder_restitution: 0.5,
            boulder_friction: 1.0,
            gravity: 15.0,
            fixed_step: 0.02,
            ground_y: 0.8675,
        }
    }
}

impl BossThrowDef {
    pub fn valid(&self) -> bool {
        self.force.0 > 0.0
            && self.force.0 <= self.force.1
            && self.force.1 <= 100.0
            && self.hand.iter().all(|x| x.abs() <= 10.0)
            && self.boulder_radius > 0.0
            && self.boulder_radius <= 10.0
            && (0.0..=10.0).contains(&self.boulder_ghost)
            && self.boulder_damage > 0
            && self.boulder_max_hits > 0
            && (0.0..=100.0).contains(&self.boulder_linear_drag)
            && (0.0..=100.0).contains(&self.boulder_angular_drag)
            && (0.0..=1.0).contains(&self.boulder_restitution)
            && (0.0..=10.0).contains(&self.boulder_friction)
            && self.gravity > 0.0
            && self.gravity <= 100.0
            && (0.001..=0.1).contains(&self.fixed_step)
            && self.ground_y.abs() <= 10.0
    }
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ObstacleDef {
    /// Collider Transform origin, used by Squid.Chase when comparing obstacle/citizen X.
    pub origin: [f32; 2],
    /// Polygon vertices after all child transforms, before the placed object's transform.
    pub points: Vec<[f32; 2]>,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Default)]
pub struct CameraDef {
    pub lead: bool,
    pub max_lead_speed: f32,
    pub lead_over_time: Vec<crate::animation::Key>,
    pub seek_speed_by_distance: Vec<crate::animation::Key>,
    pub shake: CameraShakeDef,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraShakeDef {
    pub duration: f32,
    pub strength: [f32; 2],
    pub vibrato: u8,
    pub randomness: f32,
}

impl Default for CameraShakeDef {
    fn default() -> Self {
        Self {
            duration: 1.0,
            strength: [0.1875; 2],
            vibrato: 20,
            randomness: 30.0,
        }
    }
}

impl CameraShakeDef {
    pub fn valid(&self) -> bool {
        (0.0..=5.0).contains(&self.duration)
            && self.duration > 0.0
            && self.strength.into_iter().all(|s| (0.0..=2.0).contains(&s))
            && self.vibrato > 0
            && self.duration * f32::from(self.vibrato) <= 64.0
            && (0.0..=180.0).contains(&self.randomness)
    }
}

impl CameraDef {
    pub fn valid(&self) -> bool {
        self.max_lead_speed.is_finite()
            && self.max_lead_speed >= 0.0
            && self.shake.valid()
            && [&self.lead_over_time, &self.seek_speed_by_distance]
                .into_iter()
                .all(|keys| {
                    keys.windows(2).all(|k| k[0].time <= k[1].time)
                        && keys.iter().all(|k| {
                            k.time.is_finite()
                                && k.time >= 0.0
                                && k.value.is_finite()
                                && [k.in_tangent, k.out_tangent].into_iter().all(|t| match t {
                                    crate::animation::Tangent::Slope(v) => v.is_finite(),
                                    crate::animation::Tangent::Step => true,
                                })
                        })
                })
    }
}

pub type SoundId = u16;
pub type AudioClipId = u16;

/// Sounds and the clips they play. orig: `AudioEmitter` prefabs and the
/// script fields that point at them.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Default)]
pub struct AudioDef {
    pub clips: Vec<AudioClipDef>,
    pub sounds: Vec<SoundDef>,
    /// `"Class.field"` (first seen) and `"Prefab:Class.field"` → sound,
    /// sorted by key. Array fields add `#index`.
    pub refs: Vec<(String, SoundId)>,
}

impl AudioDef {
    /// The sound a script field plays. orig: `public AudioEmitter <field>`.
    pub fn sound(&self, key: &str) -> Option<SoundId> {
        self.refs
            .binary_search_by(|(k, _)| k.as_str().cmp(key))
            .ok()
            .map(|i| self.refs[i].1)
    }

    /// The sound `field` of `class` on prefab `prefab`, else on any prefab.
    pub fn sound_on(&self, prefab: &str, class_field: &str) -> Option<SoundId> {
        let mut key = String::from(prefab);
        key.push(':');
        key.push_str(class_field);
        self.sound(&key).or_else(|| self.sound(class_field))
    }
}

/// A converted clip (`audio/<name>.adp`).
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Default)]
pub struct AudioClipDef {
    pub name: String,
    pub seconds: f32,
    pub channels: u8,
    /// Long clips (the soundtrack) are streamed rather than held in memory.
    pub stream: bool,
}

/// Mixer group. orig: `Main Mixer` groups.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SoundGroup {
    #[default]
    World,
    Ui,
    Ambient,
    Music,
}

/// How a sound fades with distance. orig: `AudioRolloffMode`.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Rolloff {
    Logarithmic,
    #[default]
    Linear,
    /// Gain by distance / max distance.
    Custom(Vec<(f32, f32)>),
}

/// orig: an `AudioEmitter` and its `AudioSource`.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Default)]
pub struct SoundDef {
    pub name: String,
    pub clips: Vec<AudioClipId>,
    /// Pick a clip at random (else the first). orig: `random`.
    pub random: bool,
    pub max_simultaneous: u8,
    pub min_retrigger: f32,
    /// orig: `playInRangeOnly` (beyond `max_distance` it isn't started).
    pub in_range_only: bool,
    pub volume: f32,
    pub looping: bool,
    /// 0 = plain stereo, 1 = placed in the world. orig: `spatialBlend`.
    pub spatial: f32,
    pub min_distance: f32,
    pub max_distance: f32,
    pub rolloff: Rolloff,
    pub group: SoundGroup,
    /// Plays only between these hours in these seasons (flags), fading over
    /// 2 s. orig: `TimeBasedAudio` (the ambience beds).
    pub schedule: Option<(f32, f32, u8)>,
}

impl SoundDef {
    /// Gain at `distance` from the listener. orig: Unity's rolloff modes.
    pub fn attenuation(&self, distance: f32) -> f32 {
        let a = match &self.rolloff {
            Rolloff::Logarithmic => {
                if distance <= self.min_distance {
                    1.0
                } else if distance >= self.max_distance {
                    0.0
                } else {
                    self.min_distance / distance.max(0.001)
                }
            }
            Rolloff::Linear => {
                let span = (self.max_distance - self.min_distance).max(0.001);
                (1.0 - (distance - self.min_distance) / span).clamp(0.0, 1.0)
            }
            Rolloff::Custom(keys) => {
                let t = distance / self.max_distance.max(0.001);
                crate::data::sample_keys(keys, t)
            }
        };
        1.0 + (a - 1.0) * self.spatial
    }
}

/// Piecewise-linear value of `keys` (time, value) at `t`, held at the ends.
pub fn sample_keys(keys: &[(f32, f32)], t: f32) -> f32 {
    match keys {
        [] => 1.0,
        [k] => k.1,
        _ => {
            if t <= keys[0].0 {
                return keys[0].1;
            }
            for w in keys.windows(2) {
                if t <= w[1].0 {
                    let span = (w[1].0 - w[0].0).max(1e-6);
                    return w[0].1 + (w[1].1 - w[0].1) * (t - w[0].0) / span;
                }
            }
            keys[keys.len() - 1].1
        }
    }
}

/// orig: `Water` and its `Water Material` (`Custom/Water`).
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct WaterDef {
    /// World heights of the band's top (the horizon) and bottom.
    pub top: f32,
    pub bottom: f32,
    pub z: f32,
    /// The reflection is squashed to this much of its height. orig: `reflectionScale`.
    pub reflection_scale: f32,
    /// Largest wave offset in pixels. orig: `maxDisplacement`.
    pub max_displacement: (f32, f32),
    /// Displacement maps (red = x, green = y, 0.5 = none), `size` texels
    /// each, repeated `repeat` times across the band. orig:
    /// `_Displacement1Tex` (`water_coarse`) and `_Displacement2Tex` (`water_fine`).
    pub size: (u16, u16),
    pub repeat: (f32, f32),
    pub coarse: Vec<[u8; 2]>,
    pub fine: Vec<[u8; 2]>,
}

impl Default for WaterDef {
    fn default() -> Self {
        WaterDef {
            top: 0.25,
            bottom: -1.75,
            z: -0.5,
            reflection_scale: 0.8,
            max_displacement: (16.0, 16.0),
            size: (0, 0),
            repeat: (4.0, 1.0),
            coarse: Vec::new(),
            fine: Vec::new(),
        }
    }
}

impl WaterDef {
    /// Map `map` at texel (`x`, `y`) wrapped, as offsets in -0.5..0.5.
    pub fn sample(&self, map: &[[u8; 2]], x: f32, y: f32) -> (f32, f32) {
        let (w, h) = (self.size.0 as i32, self.size.1 as i32);
        if map.len() != (w * h) as usize || w == 0 {
            return (0.0, 0.0);
        }
        let tx = (libm::floorf(x) as i32).rem_euclid(w);
        let ty = (libm::floorf(y) as i32).rem_euclid(h);
        let [r, g] = map[(ty * w + tx) as usize];
        (r as f32 / 255.0 - 0.5, g as f32 / 255.0 - 0.5)
    }
}

impl Proto {
    /// Reject malformed hierarchy references and nonfinite geometry before
    /// render or socket sampling. Zero and negative authored scales are valid.
    pub fn transforms_valid(&self) -> bool {
        let affine = |a: Affine2| a.basis.iter().chain(&a.translation).all(|v| v.is_finite());
        if self.transforms.len() > u16::MAX as usize + 1 {
            return false;
        }
        let mut composed = Vec::<Affine2>::with_capacity(self.transforms.len());
        for (i, n) in self.transforms.iter().enumerate() {
            if n.parent.is_some_and(|p| p as usize >= i)
                || !n
                    .local
                    .position
                    .iter()
                    .chain(&n.local.rotation)
                    .chain(&n.local.scale)
                    .chain(&n.local.color)
                    .all(|v| v.is_finite())
            {
                return false;
            }
            let local = Affine2::from_pose(n.local);
            let transform = n
                .parent
                .map_or(local, |p| composed[p as usize].compose(local));
            if !affine(transform) {
                return false;
            }
            composed.push(transform);
        }
        self.parts.iter().all(|p| {
            (self.transforms.is_empty() || (p.node as usize) < self.transforms.len())
                && [p.x, p.y, p.z].iter().all(|v| v.is_finite())
                && affine(p.transform)
        }) && self.building.as_ref().is_none_or(|b| {
            b.scaffold
                .iter()
                .all(|m| [m.x, m.y, m.z].iter().all(|v| v.is_finite()) && affine(m.transform))
        })
    }

    /// Compose a sampled hierarchy with the same ancestor order as Unity.
    /// This is shared by visible poses and authoritative animated sockets.
    pub fn sampled_transform(
        &self,
        node: u16,
        mut sample: impl FnMut(&TransformNode) -> crate::animation::Pose,
    ) -> Option<(Affine2, crate::animation::Pose)> {
        fn visit(
            nodes: &[TransformNode],
            index: usize,
            remaining: usize,
            sample: &mut impl FnMut(&TransformNode) -> crate::animation::Pose,
        ) -> Option<(Affine2, crate::animation::Pose)> {
            if remaining == 0 {
                return None;
            }
            let node = nodes.get(index)?;
            let pose = sample(node);
            let local = Affine2::from_pose(pose);
            let transform = match node.parent {
                Some(parent) => visit(nodes, parent as usize, remaining - 1, sample)?
                    .0
                    .compose(local),
                None => local,
            };
            Some((transform, pose))
        }
        visit(
            &self.transforms,
            node as usize,
            self.transforms.len(),
            &mut sample,
        )
    }

    /// A clip's paths are relative to its Animator Transform, not necessarily
    /// to the prefab root. Missing channels retain authored local defaults.
    pub fn clip_transform(
        &self,
        path: &str,
        animator_root: &str,
        clip: &crate::animation::Clip,
        time: f32,
    ) -> Option<Affine2> {
        let index = self.transforms.iter().position(|n| n.path == path)?;
        self.sampled_transform(index as u16, |node| {
            animation_path(&node.path, animator_root)
                .map_or(node.local, |path| clip.pose(path, time, node.local))
        })
        .map(|p| p.0)
    }

    /// Takes part in the simulation (anything a system looks for). Everything
    /// else is scenery, which per-tick scans can skip.
    pub fn gameplay(&self) -> bool {
        self.payable.is_some()
            || self.upgrade.is_some()
            || self.building.is_some()
            || self.castle_level.is_some()
            || self.shop.is_some()
            || self.tree_work.is_some()
            || self.camp.is_some()
            || self.character.is_some()
            || self.wall
            || self.wall_def.is_some()
            || self.portal.is_some()
            || self.farmhouse.is_some()
            || !self.guard_slots.is_empty()
            || !self.shield_spots.is_empty()
            || self.shield_shop.is_some()
            || self.tower_knight
            || self.banker.is_some()
            || self.border.is_some()
            || self.statue.is_some()
            || self.mason_shrine
            || self.steed.is_some()
            || self.steed_spawn.is_some()
            || self.cabin.is_some()
            || self.hermit.is_some()
            || self.baker.is_some()
            || self.merchant_spawner.is_some()
            || self.merchant.is_some()
            || !self.workshop.is_empty()
            || self.workshop_side.is_some()
            || self.catapult.is_some()
            || self.dog_spawn.is_some()
            || self.teleporter.is_some()
            || self.rift
            || self.chest.is_some()
            || self.banker_def.is_some()
            || self.ballista.is_some()
            || self.wharf.is_some()
            || self.boat.is_some()
            || self.beach
            || self.signpost.is_some()
    }
}

impl WorldData {
    pub fn transforms_valid(&self) -> bool {
        self.protos.iter().all(Proto::transforms_valid)
    }

    pub fn layer(&self, l: Layer) -> LayerDef {
        self.layers
            .get(l as usize)
            .copied()
            .unwrap_or(LayerDef { ratio: 1.0, z: 0.0 })
    }
}

/// Color fields of a lighting preset, in `Light::colors` order.
/// orig: `BlenderState.ColorField` (minus `None`).
pub const LIGHT_COLORS: [&str; 17] = [
    "ambientColor",
    "additiveColor",
    "hazeColor",
    "fillTop",
    "fillBottom",
    "skyTopColor",
    "skyBottomColor",
    "waterColor",
    "sunTint",
    "sunGlowColor",
    "moonTint",
    "moonGlowColor",
    "cloudBaseColor",
    "cloudHighlightColor",
    "cloudStratusColor",
    "fogColor",
    "mistColor",
];
/// Float fields, in `Light::floats` order. orig: `BlenderState.FloatField`.
pub const LIGHT_FLOATS: [&str; 8] = [
    "cloudDensity",
    "cloudStratusDensity",
    "fogDensity",
    "mistDensity",
    "iceFloeDensity",
    "stars",
    "rain",
    "snow",
];

/// orig: `BlenderState` asset.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct LightPreset {
    pub name: String,
    /// orig: `BlenderState.Category` (0 None, 1 Weather, 2 Forest).
    pub category: u8,
    /// orig: `BlenderState.Weather` flag value.
    pub weather: u16,
    /// orig: `Season` flag value (1 summer, 2 autumn, 4 winter).
    pub season: u8,
    /// orig: `BlenderState.PartOfDay`.
    pub part_of_day: u8,
    pub test_only: bool,
    /// Bit i set → `colors[i]` participates in blending; bit 17+j → `floats[j]`.
    pub enabled: u32,
    pub colors: [[f32; 4]; 17],
    pub floats: [f32; 8],
}

impl LightPreset {
    pub fn is_red_moon_overlay(&self) -> bool {
        const COLORS: [usize; 4] = [10, 11, 13, 14];
        const STARS: usize = 5;
        // The overlay owns moon tint, moon glow, cloud highlight, cloud
        // stratus and stars. Weather owns the other channels.
        const ENABLED: u32 = 0x0040_6c00;
        self.category == 0
            && !self.test_only
            && self.enabled == ENABLED
            && COLORS
                .iter()
                .flat_map(|&i| self.colors[i])
                .chain([self.floats[STARS]])
                .all(|v| (0.0..=1.0).contains(&v))
    }
}

/// orig: `Day` asset.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct DayDef {
    pub name: String,
    /// orig: `Day.seasonOptions` flags.
    pub seasons: u8,
    /// orig: `Day.weatherOptions` flags.
    pub weathers: u16,
    /// orig: `Day.tracks`.
    pub tracks: Vec<TrackDef>,
}

/// A soundtrack cue: `sound` is at `anchor_seconds` into its clip at
/// `anchor_time` o'clock. orig: `Day.TrackPlay`.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct TrackDef {
    pub anchor_time: f32,
    pub anchor_seconds: f32,
    pub sound: SoundId,
}

/// orig: the `Director` component's day lists (indices into `WorldData::days`).
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Default)]
pub struct DirectorDef {
    pub first_day: u16,
    pub first_autumn_day: u16,
    pub first_winter_day: u16,
    pub regular_days: Vec<u16>,
    pub boss_days: Vec<u16>,
    pub recovery_days: Vec<u16>,
    /// orig: `Director.numRegularDays`.
    pub num_regular_days: (i32, i32),
    /// orig: `Blender.initialStack` (indices into `WorldData::lights`).
    pub initial_lights: Vec<u16>,
}

/// One glow child of a celestial body: a wide additive halo drawn over it.
/// orig: a `SpriteRenderer` child of a `CelestialBody`.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct CelestialGlow {
    pub sprite: SpriteId,
    /// Depth of the halo, next to the body's [`CelestialDef::z`].
    pub z: f32,
    /// orig: the child transform's scale (a wide halo, 8×).
    pub scale: f32,
    /// orig: the child's `SpriteRenderer.color`, tinting and fading the halo.
    pub color: [f32; 4],
}

/// orig: `CelestialBody` (sun and moon in the main scene's Heaven layer).
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct CelestialDef {
    pub sprite: Option<SpriteId>,
    /// The body's glow children, farthest first as in the scene.
    pub glows: Vec<CelestialGlow>,
    pub is_sun: bool,
    pub orbit_scale: (f32, f32),
    pub orbit_center: (f32, f32),
    pub rises_at: f32,
    pub sets_at: f32,
    /// Depth of the body (Heaven layer).
    pub z: f32,
}

/// orig: `UnityEngine.AnimationCurve` (Hermite keys; clamped outside the keys).
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Curve {
    /// (time, value, inSlope, outSlope); infinite slopes mean a step.
    #[cfg_attr(feature = "serde", serde(serialize_with = "finite_keys"))]
    pub keys: Vec<[f32; 4]>,
}

/// JSON has no infinities: there the step slopes are
/// written as ±1e39, which reads back as infinite. Binary formats keep the
/// floats as they are, so `world.bin` doesn't change.
#[cfg(feature = "serde")]
fn finite_keys<S: serde::Serializer>(keys: &[[f32; 4]], s: S) -> Result<S::Ok, S::Error> {
    if !s.is_human_readable() {
        return keys.serialize(s);
    }
    let wide = |v: f32| match v {
        v if v.is_nan() => 0.0,
        v if v.is_infinite() => 1e39_f64.copysign(v as f64),
        v => v as f64,
    };
    s.collect_seq(keys.iter().map(|k| k.map(wide)))
}

impl Curve {
    /// orig: `AnimationCurve.Evaluate()`.
    pub fn eval(&self, t: f32) -> f32 {
        let k = &self.keys;
        match k.len() {
            0 => return 0.0,
            1 => return k[0][1],
            _ => {}
        }
        if t <= k[0][0] {
            return k[0][1];
        }
        let last = k[k.len() - 1];
        if t >= last[0] {
            return last[1];
        }
        let i = k.iter().rposition(|key| key[0] <= t).unwrap_or(0);
        let (a, b) = (k[i], k[i + 1]);
        let dt = b[0] - a[0];
        if dt <= 0.0 || a[3].is_infinite() || b[2].is_infinite() {
            return a[1];
        }
        let s = (t - a[0]) / dt;
        let (s2, s3) = (s * s, s * s * s);
        let h00 = 2.0 * s3 - 3.0 * s2 + 1.0;
        let h10 = s3 - 2.0 * s2 + s;
        let h01 = -2.0 * s3 + 3.0 * s2;
        let h11 = s3 - s2;
        h00 * a[1] + h10 * a[3] * dt + h01 * b[1] + h11 * b[2] * dt
    }
}

/// orig: `Wave` asset: enemy counts by day.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq, Default)]
pub struct WaveDef {
    pub name: String,
    pub trolls: Curve,
    pub tough_trolls: Curve,
    pub squids: Curve,
    pub bosses: Curve,
    pub killer_bosses: Curve,
    pub both_sides: bool,
    pub boss_wave: bool,
}

/// orig: `Portal` + its `Damageable`.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PortalDef {
    pub cliff: bool,
    pub hit_points: i32,
    /// Hurt by arrows / by knights. orig: `Damageable.damagedBy` (Arrow, Knight).
    pub by_arrows: bool,
    pub by_knights: bool,
    /// Its trigger's bounds relative to the (unflipped) root: (x0, x1, y0, y1).
    pub hit_box: (f32, f32, f32, f32),
    /// orig: `defenseTriggerRange`, `defenseSpawnDelay`, `defenseSpawnCooldown`.
    pub defense_range: f32,
    pub defense_delay: (f32, f32),
    pub defense_cooldown: f32,
    /// Cliff portals rebuild over this many days. orig: `rebuildDays`.
    pub rebuild_days: f32,
    /// What a regular portal leaves behind. orig: `teleporterRiftPrefab`.
    pub rift: Option<ProtoId>,
}

/// orig: `Wall` + its `Damageable`.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WallDef {
    pub hit_points: i32,
    pub repair_rate: i32,
    pub repair_rate_above_max: i32,
    pub bonus_hit_points_multiplier: f32,
    pub minimum_intact: f32,
    /// What's left when it falls. orig: `wreckPrefab`.
    pub wreck: Option<ProtoId>,
}

#[cfg(test)]
mod transform_validation_tests {
    use super::*;
    use alloc::vec;
    #[test]
    fn malformed_transform_graphs_and_nonfinite_geometry_are_rejected() {
        let mut p = Proto {
            transforms: vec![
                TransformNode {
                    path: "".into(),
                    parent: None,
                    local: crate::animation::Pose::default(),
                    animator: None,
                },
                TransformNode {
                    path: "Child".into(),
                    parent: Some(0),
                    local: crate::animation::Pose {
                        scale: [0.0, -1.3, 1.0],
                        ..Default::default()
                    },
                    animator: None,
                },
            ],
            ..Default::default()
        };
        assert!(
            p.transforms_valid(),
            "zero and mirrored scales are authored values"
        );
        p.transforms[0].parent = Some(1);
        assert!(!p.transforms_valid());
        p.transforms[0].parent = None;
        p.transforms[1].parent = Some(1);
        assert!(!p.transforms_valid());
        p.transforms[1].parent = Some(0);
        p.transforms[1].local.rotation[0] = f32::NAN;
        assert!(!p.transforms_valid());
        p.transforms[1].local.rotation[0] = 0.0;
        p.transforms[0].local.scale[1] = f32::MAX;
        p.transforms[1].local.scale[1] = 2.0;
        assert!(
            !p.transforms_valid(),
            "finite inputs must not overflow on composition"
        );
        p.transforms[0].local.scale[1] = 1.0;
        p.transforms[1].local.scale[1] = -1.3;
        p.parts.push(Part {
            reflection_only: false,
            sprite: None,
            x: 0.0,
            y: 0.0,
            z: 0.0,
            flip: false,
            flip_y: false,
            transform: Affine2::IDENTITY,
            node: 2,
            color: [255; 4],
            sort_layer: 0,
            sort_order: 0,
            random: None,
            parent: None,
            recolor: false,
            lit: false,
            scaffold: false,
        });
        assert!(!p.transforms_valid());
        p.parts[0].node = 1;
        assert!(p.transforms_valid());
        p.parts[0].transform.basis[0] = f32::INFINITY;
        assert!(!p.transforms_valid());
        assert!(
            Proto::default().transforms_valid(),
            "empty test fixtures remain legal"
        );
    }
}
