//! Island generation from level blocks.
//!
//! orig: `LevelLayout` (block choice and order), `Level.GenerateInternal()`
//! (placement), `LevelBlock.CloneInto()`, `Level.AddScatteredObjects()`,
//! `Level.ApplyParallaxCorrection()`, `FixedTransform.Fix()`,
//! `RandomizeSprite.Randomize()`.

use alloc::vec;
use alloc::vec::Vec;

use crate::data::{BlockDef, Kind, Layer, LevelConfigDef, ProtoId, Side, SpriteId, WorldData};
use crate::rng::Rng;

/// orig: `ScreenManager.PixelsPerUnit`, for `Dejiggler.RoundToPixels`.
const PPU: f32 = 32.0;
/// orig: `Level.ScatterAttempts`.
const SCATTER_ATTEMPTS: u32 = 10;

/// orig: `Mathf.Clamp`, which (unlike `f32::clamp`) tolerates min > max.
fn mathf_clamp(v: f32, min: f32, max: f32) -> f32 {
    if v < min {
        min
    } else if v > max {
        max
    } else {
        v
    }
}

fn round_px(v: f32) -> f32 {
    libm::roundf(v * PPU) / PPU
}

/// Runtime state of one prototype part after randomization.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PartState {
    pub sprite: Option<SpriteId>,
    /// Runtime tint override (e.g. `RandomizeTree.backColor`); `None` keeps the prefab color.
    pub color: Option<[u8; 4]>,
    pub flip: bool,
    pub visible: bool,
    /// Extra x offset from `RandomizeSprite.shiftX`.
    pub shift: f32,
}

/// One object in the generated island.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct Object {
    /// Stable id, used as the payable id. Assigned by [`Level::add`].
    pub id: u32,
    pub proto: ProtoId,
    pub layer: Layer,
    /// Layer-local position. For parallax layers this is already scaled by
    /// the layer ratio; the screen position is `x - ratio * camera_x`.
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub flip: bool,
    pub parts: Vec<PartState>,
    /// Build points so far while under construction. orig: `WorkableBuilding._currentBuildPoints`.
    pub build: Option<f32>,
    /// orig: `PayableUpgrade.cooldown`, counting down.
    pub cooldown: f32,
    /// Current price; starts at the prototype's and grows by `priceIncrease`.
    pub price: u8,
    /// Marked for chopping. orig: `WorkableTree._marked`.
    pub marked: bool,
    /// Chops left before a tree falls. orig: `WorkableTree.strength`.
    pub chops_left: i32,
    /// Walls and portals. orig: `Damageable.hitPoints`.
    pub hp: i32,
    /// A damaged wall listed for repair. orig: `Kingdom.AddWork(wall)`.
    pub repair: bool,
    /// Ballista: winding progress and aim (degrees). orig: `Ballista._currentWork` / `_currentAngle`.
    pub reload: i32,
    pub aim: f32,
    /// Catapult state (`siege::mode`).
    pub mode: u8,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct Level {
    /// Everything the simulation looks at, in id order.
    pub objects: Vec<Object>,
    /// Pure decoration (most trees, bushes and rocks), only drawn, and faded
    /// at forest edges. Kept apart so the per-tick scans over `objects` stay
    /// short (they're the bulk of a tick on an Old 3DS). In id order.
    pub scenery: Vec<Object>,
    /// World x of the left edge of the ground.
    pub left: f32,
    /// Total ground width. orig: the Ground collider spans `left..left+width`.
    pub width: f32,
    /// Placed blocks in order, with their world x offset (debugging).
    pub blocks: Vec<(usize, f32)>,
    next_id: u32,
}

impl Level {
    /// A level with ground but no objects.
    pub fn empty(left: f32, width: f32) -> Self {
        Level {
            objects: Vec::new(),
            scenery: Vec::new(),
            left,
            width,
            blocks: Vec::new(),
            next_id: 0,
        }
    }

    /// Adds an object, giving it a fresh id.
    pub fn add(&mut self, mut o: Object) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        o.id = id;
        self.objects.push(o);
        id
    }

    /// Objects stay in id order: [`Level::add`] hands out rising ids and
    /// objects are only ever removed in place, so this is a binary search.
    pub fn index_of(&self, id: u32) -> Option<usize> {
        self.objects.binary_search_by_key(&id, |o| o.id).ok()
    }

    /// Moves pure scenery out of `objects` (see [`Level::scenery`]).
    pub fn split_scenery(&mut self, world: &WorldData) {
        let all = core::mem::take(&mut self.objects);
        for o in all {
            if is_scenery(&world.protos[o.proto as usize]) {
                self.scenery.push(o);
            } else {
                self.objects.push(o);
            }
        }
    }

    /// Removes object `id` from whichever list holds it.
    pub fn remove_any(&mut self, id: u32) -> bool {
        if let Some(i) = self.index_of(id) {
            self.objects.remove(i);
            return true;
        }
        match self.scenery.binary_search_by_key(&id, |o| o.id) {
            Ok(i) => {
                self.scenery.remove(i);
                true
            }
            Err(_) => false,
        }
    }

    /// Both lists (for forest edges and drawing).
    pub fn all(&self) -> impl Iterator<Item = &Object> {
        self.objects.iter().chain(self.scenery.iter())
    }
}

/// Decoration nothing in the simulation looks for.
fn is_scenery(p: &crate::data::Proto) -> bool {
    !p.gameplay()
        && p.gaps.is_none()
        && !p.grass
        && !p.clear_grass
        && !matches!(p.kind, Kind::Dormant { .. } | Kind::Placeholder { .. })
        && p.tag != "Campfire"
        && p.tag != "Player"
}

impl Level {}

/// Which unlockables are available. orig: `UnlockManager.UnlockedUnlockables()`.
pub enum Unlocks<'a> {
    All,
    /// Only these unlock ids; `force` is the recent unlock every island
    /// includes for a while. orig: `AlwaysIncludeUnlock()` → `forcePlacement`.
    Only {
        ids: &'a [i32],
        force: Option<i32>,
    },
}

struct Placement {
    block: usize,
    position: f32,
}

/// orig: `LevelLayout`.
struct Layout<'a> {
    blocks: &'a [BlockDef],
    unlocked: Vec<bool>,
    /// orig: `LevelBlock.forcePlacement`.
    forced: Vec<bool>,
    counts: Vec<i32>,
    placed: Vec<Placement>,
}

impl<'a> Layout<'a> {
    fn group_range(config: &LevelConfigDef, group: u8) -> (i32, i32) {
        config
            .group_counts
            .get(group as usize)
            .copied()
            .unwrap_or((0, 0))
    }

    /// orig: `LevelBlock.CanPlace()`.
    fn can_place(&self, i: usize, config: &LevelConfigDef) -> bool {
        let b = &self.blocks[i];
        if !self.unlocked[i] || b.groups[0] == 0 {
            return false;
        }
        !b.groups
            .iter()
            .any(|&g| g != 0 && self.counts[g as usize] >= Self::group_range(config, g).1)
    }

    /// orig: `LevelBlock.RequiresPlace()`.
    fn requires_place(&self, i: usize, config: &LevelConfigDef) -> i32 {
        if !self.can_place(i, config) {
            return 0;
        }
        let under = self.blocks[i]
            .groups
            .iter()
            .filter(|&&g| g != 0 && self.counts[g as usize] < Self::group_range(config, g).0)
            .count() as i32;
        under + if self.forced[i] { 5 } else { 0 }
    }

    /// orig: `LevelLayout.FindRequiredBlocks()`.
    fn required(&self, config: &LevelConfigDef) -> Vec<usize> {
        let mut best = 1;
        let mut out = Vec::new();
        for i in 0..self.blocks.len() {
            let n = self.requires_place(i, config);
            if n > best {
                out.clear();
                best = n;
            }
            if n == best {
                out.push(i);
            }
        }
        out
    }

    fn total_width(&self) -> i32 {
        self.placed.iter().map(|p| self.blocks[p.block].width).sum()
    }

    /// orig: `LevelLayout.AddBlock()`.
    fn add(&mut self, i: usize, rng: &mut Rng) {
        let b = &self.blocks[i];
        let dir = match b.side {
            Side::Left => -1.0,
            Side::Right => 1.0,
            Side::Any => {
                if rng.value() < 0.5 {
                    -1.0
                } else {
                    1.0
                }
            }
            Side::Distribute => {
                let left = self
                    .placed
                    .iter()
                    .filter(|p| p.block == i && p.position < 0.0)
                    .count();
                let right = self
                    .placed
                    .iter()
                    .filter(|p| p.block == i && p.position > 0.0)
                    .count();
                if left < right || (left == right && rng.value() < 0.5) {
                    -1.0
                } else {
                    1.0
                }
            }
        };
        let mut position = rng.range(b.min_distance, b.max_distance) * dir;
        if b.maximize_mutual_distance {
            let same: Vec<f32> = self
                .placed
                .iter()
                .filter(|p| p.block == i)
                .map(|p| p.position)
                .collect();
            if !same.is_empty() {
                let mut best = f32::NEG_INFINITY;
                for _ in 0..10 {
                    let candidate = rng.range(b.min_distance, b.max_distance) * dir;
                    let nearest = same
                        .iter()
                        .map(|p| (p - candidate).abs())
                        .fold(f32::MAX, f32::min);
                    if nearest > best {
                        position = candidate;
                        best = nearest;
                    }
                }
            }
        }
        for g in b.groups {
            self.counts[g as usize] += 1;
        }
        self.placed.push(Placement { block: i, position });
    }
}

/// orig: `LevelLayout.Generate()`. Returns block indices left to right.
pub fn layout(
    world: &WorldData,
    config: &LevelConfigDef,
    unlocks: &Unlocks,
    rng: &mut Rng,
) -> Vec<usize> {
    let groups = config.group_counts.len().max(64);
    let mut l = Layout {
        blocks: &world.blocks,
        unlocked: world
            .blocks
            .iter()
            .map(|b| {
                !b.needs_unlock
                    || match unlocks {
                        Unlocks::All => true,
                        Unlocks::Only { ids, .. } => ids.contains(&b.unlock_id),
                    }
            })
            .collect(),
        forced: world
            .blocks
            .iter()
            .map(|b| {
                b.needs_unlock
                    && matches!(unlocks, Unlocks::Only { force: Some(f), .. } if *f == b.unlock_id)
            })
            .collect(),
        counts: vec![0; groups],
        placed: Vec::new(),
    };
    loop {
        let req = l.required(config);
        if req.is_empty() {
            break;
        }
        let pick = req[rng.range_i32(0, req.len() as i32) as usize];
        l.add(pick, rng);
    }
    while l.total_width() < config.min_level_width {
        let options: Vec<usize> = (0..world.blocks.len())
            .filter(|&i| l.can_place(i, config))
            .collect();
        if options.is_empty() {
            break;
        }
        let pick = options[rng.range_i32(0, options.len() as i32) as usize];
        l.add(pick, rng);
    }
    l.placed.sort_by(|a, b| a.position.total_cmp(&b.position));
    l.placed.into_iter().map(|p| p.block).collect()
}

/// A new object from a prototype, before generation-time fixes and randomization.
pub fn instantiate(
    world: &WorldData,
    proto: ProtoId,
    layer: Layer,
    x: f32,
    y: f32,
    z: f32,
    scale_x: f32,
) -> Object {
    let p = &world.protos[proto as usize];
    let mut o = Object {
        id: 0,
        marked: false,
        chops_left: p.tree_work.map_or(0, |t| t.strength),
        hp: p
            .wall_def
            .map(|w| w.hit_points)
            .or(p.portal.map(|q| q.hit_points))
            .or(p.catapult.map(|c| c.hit_points))
            .unwrap_or(0),
        build: None,
        repair: false,
        reload: 0,
        aim: 0.0,
        mode: 0,
        cooldown: p.upgrade.map_or(0.0, |u| u.cooldown),
        price: p.payable.map_or(0, |d| d.price),
        proto,
        layer,
        x,
        y,
        z,
        flip: scale_x < 0.0,
        parts: p
            .parts
            .iter()
            .map(|part| PartState {
                sprite: part.sprite,
                color: None,
                flip: part.flip,
                visible: part.sprite.is_some(),
                shift: 0.0,
            })
            .collect(),
    };
    // orig: WorkableTree.Awake hides the chop mark until the tree is marked.
    if let Some(m) = p.tree_work.and_then(|t| t.chop_mark) {
        if let Some(part) = o.parts.get_mut(m as usize) {
            part.visible = false;
        }
    }
    o
}

/// orig: `FixedTransform.Fix()`. Symmetric objects face away from the campfire.
fn fix(world: &WorldData, o: &mut Object, campfire_x: f32, rng: &mut Rng) {
    let Some(f) = world.protos[o.proto as usize].fixed else {
        return;
    };
    if f.symmetric {
        let mut dir = if o.x > campfire_x { 1.0 } else { -1.0 };
        if f.flipped {
            dir = -dir;
        }
        o.flip = dir < 0.0;
    }
    if let Some(z) = f.fix_z {
        o.z = z + rng.value() * f.random_offset_range;
    }
    if let Some(y) = f.fix_y {
        o.y = y;
    }
}

/// orig: `RandomizeSprite.Randomize()` for every part of every object, and
/// `RandomizeTree.Randomize()` for trees.
pub fn randomize(world: &WorldData, o: &mut Object, rng: &mut Rng) {
    let proto = &world.protos[o.proto as usize];
    if let Some(t) = proto.tree {
        let front = rng.value() < 0.5;
        o.z = if front { t.z_front } else { t.z_back };
        let tint = if front { [255; 4] } else { t.back_color };
        for p in &mut o.parts {
            p.color = Some(tint);
        }
        o.z += rng.value() * t.random_offset_range;
    }
    let mut picks: Vec<Option<(usize, bool)>> = vec![None; proto.parts.len()];
    for (i, part) in proto.parts.iter().enumerate() {
        let Some(r) = &part.random else { continue };
        let inherited = if r.match_parent {
            let mut p = part.parent;
            let mut found = None;
            while let Some(pi) = p {
                if let Some(pick) = picks[pi as usize] {
                    found = Some(pick);
                    break;
                }
                p = proto.parts[pi as usize].parent;
            }
            found
        } else {
            None
        };
        let (pick, flip) = match inherited {
            Some(v) => v,
            None => {
                let flip = r.allow_flip && rng.value() < 0.5;
                (rng.range_i32(0, r.sprites.len() as i32) as usize, flip)
            }
        };
        picks[i] = Some((pick, flip));
        let st = &mut o.parts[i];
        if let Some(&s) = r.sprites.get(pick) {
            st.sprite = Some(s);
        }
        if rng.value() < r.deactivation_probability {
            st.visible = false;
        }
        st.flip = flip;
        if r.shift_x > 0.0 {
            st.shift = round_px(rng.range(-r.shift_x, r.shift_x));
        }
    }
}

/// Overlap rect for scatter avoidance. orig: `Level.GetOverlapRegion()`.
fn overlap_rect(world: &WorldData, o: &Object) -> (f32, f32, f32, f32) {
    match world.protos[o.proto as usize].pay_region {
        // Payables: from the pay point, playerPayDistance*2 wide, 100 tall
        // (Unity's Rect takes x,y as the min corner).
        Some((off, half)) => (o.x + off, o.y, half * 2.0, 100.0),
        None => (o.x - 0.5, o.y - 0.5, 1.0, 1.0),
    }
}

fn overlaps(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> bool {
    a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
}

struct Region {
    min: f32,
    max: f32,
}

/// orig: `Level.ScatterObjectAround()`.
#[allow(clippy::too_many_arguments)]
fn scatter_around(
    world: &WorldData,
    objects: &mut Vec<Object>,
    proto: ProtoId,
    region: &Region,
    x: &mut f32,
    campfire_x: f32,
    rng: &mut Rng,
) -> bool {
    let p = &world.protos[proto as usize];
    let Some(s) = &p.scatter else { return false };
    for _ in 0..SCATTER_ATTEMPTS {
        let half = p.half_width;
        let room = (region.max - region.min) / 2.0 - half;
        let offset = mathf_clamp(
            s.spacing * rng.range(-s.irregularity, s.irregularity),
            -room,
            room,
        );
        let nx = *x + offset;
        if nx - half < region.min + s.dist_from_region_edge
            || nx + half > region.max - s.dist_from_region_edge
        {
            continue;
        }
        let mut o = instantiate(world, proto, s.layer, nx, s.y, s.z, 1.0);
        fix(world, &mut o, campfire_x, rng);
        let me = overlap_rect(world, &o);
        let blocked = s.avoid.iter().any(|tag| {
            objects
                .iter()
                .filter(|other| world.protos[other.proto as usize].tag == *tag)
                .any(|other| overlaps(me, overlap_rect(world, other)))
        });
        if !blocked {
            objects.push(o);
            *x = nx;
            return true;
        }
    }
    false
}

/// orig: `Level.ScatterObjectsInRegion()`.
#[allow(clippy::too_many_arguments)]
fn scatter_run(
    world: &WorldData,
    objects: &mut Vec<Object>,
    proto: ProtoId,
    region: &Region,
    start: f32,
    end: f32,
    campfire_x: f32,
    rng: &mut Rng,
) {
    let Some(s) = world.protos[proto as usize].scatter.as_ref() else {
        return;
    };
    let dir = if end > start { 1.0 } else { -1.0 };
    let mut step = s.spacing;
    if s.adjacent {
        let ratio = world.layer(s.layer).ratio;
        if ratio != 0.0 {
            step /= ratio;
        }
    }
    if !step.is_finite() || step <= 0.0 {
        return;
    }
    let mut x = start;
    while dir * x < dir * end {
        let next = x + step * dir;
        if next == x {
            break;
        }
        x = next;
        scatter_around(world, objects, proto, region, &mut x, campfire_x, rng);
    }
}

/// Builds island `level_index` (0 = Land One).
pub fn generate(world: &WorldData, level_index: usize, unlocks: &Unlocks, rng: &mut Rng) -> Level {
    let config = &world.levels[level_index];
    let order = layout(world, config, unlocks, rng);

    // orig: Level.GenerateInternal: the absoluteCenter block is centered on x = 0.
    let mut left = 0.0;
    let mut total = 0;
    for &i in &order {
        let b = &world.blocks[i];
        if b.absolute_center {
            left = (-total - b.width / 2) as f32;
        }
        total += b.width;
    }
    let campfire_x = 0.0;

    let mut objects = Vec::new();
    let mut blocks = Vec::new();
    let mut cursor = left;
    for &i in &order {
        let b = &world.blocks[i];
        let pos_x = cursor - b.left;
        blocks.push((i, pos_x));
        // orig: LevelBlock.CloneInto
        for item in &b.items {
            let mut proto = item.proto;
            let mut layer = item.layer;
            if let Kind::Placeholder {
                proto: target,
                spawn_in_game_layer,
            } = world.protos[proto as usize].kind
            {
                proto = target;
                if spawn_in_game_layer {
                    layer = Layer::Game;
                }
            }
            let mut o = instantiate(
                world,
                proto,
                layer,
                round_px(item.x + pos_x),
                round_px(item.y),
                item.z,
                item.scale_x,
            );
            fix(world, &mut o, campfire_x, rng);
            objects.push(o);
        }
        cursor += b.width as f32;
    }

    // orig: Level.AddScatteredObjects: tiles sorted by x, scatterables by spacing (largest first).
    let mut tiles: Vec<(f32, f32, u32)> = objects
        .iter()
        .filter_map(|o| match world.protos[o.proto as usize].kind {
            Kind::Tile { width, scatter, .. } if o.layer == Layer::Game => {
                Some((o.x, o.x + width, scatter))
            }
            _ => None,
        })
        .collect();
    tiles.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut scatterables = world.scatterables.clone();
    scatterables.sort_by(|a, b| {
        let sa = world.protos[a.0 as usize]
            .scatter
            .as_ref()
            .map_or(0.0, |s| s.spacing);
        let sb = world.protos[b.0 as usize]
            .scatter
            .as_ref()
            .map_or(0.0, |s| s.spacing);
        sb.total_cmp(&sa)
    });
    for (proto, bit) in scatterables {
        // orig: Level.GetContiguousRegions
        let mut regions: Vec<Region> = Vec::new();
        let mut inside = false;
        for &(l, r, flags) in &tiles {
            let on = flags & (1 << bit) != 0;
            if on && !inside {
                regions.push(Region { min: l, max: l });
            }
            if on {
                if let Some(last) = regions.last_mut() {
                    last.max = r;
                }
            }
            inside = on;
        }
        for region in &regions {
            // orig: Level.ScatterObjects: center first, then outwards both ways.
            let mut x = (region.min + region.max) / 2.0;
            scatter_around(world, &mut objects, proto, region, &mut x, campfire_x, rng);
            scatter_run(
                world,
                &mut objects,
                proto,
                region,
                x,
                region.min,
                campfire_x,
                rng,
            );
            scatter_run(
                world,
                &mut objects,
                proto,
                region,
                x,
                region.max,
                campfire_x,
                rng,
            );
        }
    }

    // orig: Level.ApplyParallaxCorrection
    for o in &mut objects {
        let ratio = world.layer(o.layer).ratio;
        if ratio != 1.0 {
            o.x = round_px(o.x * ratio);
        }
    }

    // orig: Randomizer.RandomizeChildren(level) (and Tile.ApplyNoGround)
    for o in &mut objects {
        randomize(world, o, rng);
        // orig: Tile.ApplyNoGround, PrefabPlaceholder.Start (doNotSwap), hidden hints.
        if matches!(
            world.protos[o.proto as usize].kind,
            Kind::Tile {
                no_ground: true,
                ..
            } | Kind::Dormant { .. }
                | Kind::Hint
        ) {
            for p in &mut o.parts {
                p.visible = false;
            }
        }
    }

    let mut level = Level {
        objects: Vec::new(),
        scenery: Vec::new(),
        left,
        width: total as f32,
        blocks,
        next_id: 0,
    };
    for o in objects {
        level.add(o);
    }
    level
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::*;
    use alloc::string::ToString;

    fn tile_proto() -> Proto {
        Proto {
            name: "Tile".to_string(),
            tag: "Untagged".to_string(),
            kind: Kind::Tile {
                width: 4.0,
                no_ground: false,
                scatter: 1, // scatterTree
            },
            parts: vec![Part {
                reflection_only: false,
                sprite: Some(0),
                x: 0.0,
                y: 0.0,
                z: 0.0,
                flip: false,
                flip_y: false,
                transform: Affine2::IDENTITY,
                node: 0,
                color: [255; 4],
                sort_layer: 0,
                sort_order: 0,
                random: None,
                parent: None,
                recolor: false,
                lit: true,
                scaffold: false,
            }],
            half_width: 2.0,
            ..Default::default()
        }
    }

    fn block(name: &str, groups: [u8; 3], side: Side, center: bool, tiles: usize) -> BlockDef {
        BlockDef {
            name: name.to_string(),
            needs_unlock: false,
            unlock_id: 0,
            min_distance: 0.0,
            max_distance: 1.0,
            maximize_mutual_distance: false,
            side,
            absolute_center: center,
            groups,
            left: 0.0,
            width: 4 * tiles as i32,
            items: (0..tiles)
                .map(|i| Placed {
                    proto: 0,
                    layer: Layer::Game,
                    x: 4.0 * i as f32,
                    y: 0.0,
                    z: 0.0,
                    scale_x: 1.0,
                })
                .collect(),
        }
    }

    fn world() -> WorldData {
        let mut tree = tile_proto();
        tree.name = "Tree".to_string();
        tree.kind = Kind::Scenery;
        tree.half_width = 0.5;
        tree.scatter = Some(Scatter {
            layer: Layer::Game,
            adjacent: false,
            spacing: 2.0,
            irregularity: 0.5,
            dist_from_region_edge: 0.0,
            avoid: Vec::new(),
            y: 0.0,
            z: 0.0,
        });
        let mut groups = vec![(0, 0); 8];
        groups[1] = (1, 1); // StartingArea
        groups[3] = (2, 4); // Forest
        WorldData {
            sprites: vec!["tile".to_string()],
            protos: vec![tile_proto(), tree],
            blocks: vec![
                block("StartingArea", [1, 0, 0], Side::Any, true, 3),
                block("Forest", [3, 0, 0], Side::Distribute, false, 2),
            ],
            levels: vec![LevelConfigDef {
                name: "Test".to_string(),
                autumn_start: (0, 0),
                winter_start: (0, 0),
                days_before_game_over: -1,
                starting_coins: 0,
                min_level_width: 30,
                group_counts: groups,
                waves: Default::default(),
                wave_timed_multiplier: 1.0,
                wave_multiplier_timeframe: (0, -1),
                season_change_days: 3.0,
                income_multiplier: 1.0,
                free_boat_parts: 0,
                wave_boat_multiplier: 1.0,
                force_all_unlocks: false,
                defence_waves: [None; 3],
                retaliation_waves: [None; 3],
            }],
            scatterables: vec![(1, 0)],
            layers: vec![LayerDef { ratio: 1.0, z: 0.0 }; 7],
            ..Default::default()
        }
    }

    #[test]
    fn required_blocks_then_fill_to_min_width() {
        let w = world();
        let mut rng = Rng::new(7);
        let order = layout(&w, &w.levels[0], &Unlocks::All, &mut rng);
        let starts = order.iter().filter(|&&i| i == 0).count();
        let forests = order.iter().filter(|&&i| i == 1).count();
        assert_eq!(starts, 1);
        assert!((2..=4).contains(&forests), "{forests}");
        let width: i32 = order.iter().map(|&i| w.blocks[i].width).sum();
        assert!(width >= 28);
    }

    #[test]
    fn starting_area_is_centered_and_ground_is_contiguous() {
        let w = world();
        let lvl = generate(&w, 0, &Unlocks::All, &mut Rng::new(3));
        let (start_idx, start_x) = lvl.blocks.iter().find(|(i, _)| *i == 0).copied().unwrap();
        assert_eq!(start_idx, 0);
        // 12 units wide → spans -6..6.
        assert_eq!(start_x, -6.0);
        let mut tiles: Vec<f32> = lvl
            .objects
            .iter()
            .filter(|o| o.proto == 0)
            .map(|o| o.x)
            .collect();
        tiles.sort_by(f32::total_cmp);
        for pair in tiles.windows(2) {
            assert_eq!(pair[1] - pair[0], 4.0);
        }
        assert_eq!(tiles[0], lvl.left);
    }

    #[test]
    fn trees_are_scattered_inside_flagged_ground() {
        let w = world();
        let lvl = generate(&w, 0, &Unlocks::All, &mut Rng::new(11));
        let trees: Vec<f32> = lvl
            .objects
            .iter()
            .filter(|o| o.proto == 1)
            .map(|o| o.x)
            .collect();
        assert!(trees.len() > 5, "{}", trees.len());
        for x in trees {
            assert!(x - 0.5 >= lvl.left && x + 0.5 <= lvl.left + lvl.width);
        }
    }
}
