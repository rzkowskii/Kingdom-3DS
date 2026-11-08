//! Build sites: which objects can be paid for, what paying does, and
//! construction progress. Also the forest regions that block building.
//!
//! orig: `PayableUpgrade` (`CanPay`, `Pay`), `WorkableBuilding`, `Scaffolding`,
//! `Castle` (shop swaps), `World.GenerateForests()` / `Forest` / `ForestItem`.

use alloc::vec::Vec;

use crate::data::{HermitKind, Kind, Layer, ProtoId, WorldData};
use crate::level::{instantiate, Level, Object};
use crate::payable::Payable;
use crate::payment::PayableId;

/// World y where characters stand: the Ground collider's top. The
/// simulation works in ground-relative y. orig: Player prefab y.
pub const WALK_Y: f32 = 0.875;

/// Forest x-intervals, merged. orig: `World.GenerateForests()`; each `Forest`'s
/// `GAPS` region is on the NotBuildable and Forest layers.
pub fn forests(world: &WorldData, level: &Level) -> Vec<(f32, f32)> {
    let mut rects: Vec<(f32, f32)> = level
        .all()
        .filter_map(|o| {
            let f = world.protos[o.proto as usize].forest_item?;
            // orig: ForestItem.UpdateRect: undo the parallax scaling of the x position.
            let ratio = world.layer(o.layer).ratio;
            let x = if ratio != 0.0 { o.x / ratio } else { o.x };
            let w = f.width + 2.0 * f.size_padding;
            Some((x - w / 2.0, x + w / 2.0))
        })
        .collect();
    rects.sort_by(|a, b| ((a.0 + a.1) / 2.0).total_cmp(&((b.0 + b.1) / 2.0)));
    // orig: FindOrCreateForest + MergeForest: overlapping (strictly) regions join.
    let mut merged: Vec<(f32, f32)> = Vec::new();
    for r in rects {
        match merged.iter_mut().find(|m| m.0 < r.1 && m.1 > r.0) {
            Some(m) => {
                m.0 = m.0.min(r.0);
                m.1 = m.1.max(r.1);
            }
            None => merged.push(r),
        }
        // Growing one region can make it overlap another.
        merged.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut i = 0;
        while i + 1 < merged.len() {
            if merged[i].1 > merged[i + 1].0 {
                merged[i].1 = merged[i].1.max(merged[i + 1].1);
                merged.remove(i + 1);
            } else {
                i += 1;
            }
        }
    }
    merged
}

/// Decorations (not trees) within 1 unit of their forest's edge: the
/// original fades these out whenever a forest's region is recomputed, which
/// shrinks the forest to its trees. orig: `Forest.UpdateRegion()` →
/// `ForestItem.FadeAndRemove()`, `ForestItem.DistanceFromForestEdge()`.
pub fn forest_edge_items(world: &WorldData, level: &Level, forests: &[(f32, f32)]) -> Vec<u32> {
    level
        .all()
        .filter_map(|o| {
            let f = world.protos[o.proto as usize].forest_item?;
            if f.controls_forest_size {
                return None;
            }
            let ratio = world.layer(o.layer).ratio;
            let x = if ratio != 0.0 { o.x / ratio } else { o.x };
            let w = f.width + 2.0 * f.size_padding;
            let (a, b) = (x - w / 2.0, x + w / 2.0);
            let &(l, r) = forests.iter().find(|&&(l, r)| l < b && r > a)?;
            let d = (a - l).min(r - b) - f.size_padding;
            (d < 1.0).then_some(o.id)
        })
        .collect()
}

/// Removes edge decorations until the forests settle, as happens in the
/// first seconds of the original. Returns the final forests.
pub fn settle_forests(world: &WorldData, level: &mut Level) -> Vec<(f32, f32)> {
    loop {
        let f = forests(world, level);
        let edge = forest_edge_items(world, level, &f);
        if edge.is_empty() {
            return f;
        }
        level.objects.retain(|o| !edge.contains(&o.id));
        level.scenery.retain(|o| !edge.contains(&o.id));
    }
}

pub fn in_forest(forests: &[(f32, f32)], x: f32) -> bool {
    forests.iter().any(|&(a, b)| a <= x && x <= b)
}

/// Whether the object is a scaffold still being built.
pub fn under_construction(world: &WorldData, o: &Object) -> bool {
    match (&world.protos[o.proto as usize].building, o.build) {
        (Some(b), Some(points)) => points < b.build_points as f32,
        _ => false,
    }
}

/// How many scaffold modules show their filled sprite.
/// orig: `Scaffolding.SetProgress(completed, total)`.
pub fn scaffold_filled(world: &WorldData, o: &Object) -> usize {
    let Some(b) = &world.protos[o.proto as usize].building else {
        return 0;
    };
    let points = o.build.unwrap_or(0.0);
    let progress = if b.build_points > 1 {
        (points / (b.build_points - 1) as f32).clamp(0.0, 1.0)
    } else {
        1.0
    };
    libm::roundf(progress * b.scaffold.len() as f32) as usize
}

/// Kingdom-wide unlocks that gate payables.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tech {
    /// orig: `Kingdom.stoneBuildingUnlocked` (from the mason shrine).
    pub stone: bool,
    /// The hermit riding with the monarch. orig: `Player.passenger`.
    pub passenger: Option<HermitKind>,
}

/// What paying for an upgrade builds: the hermit's tower if one rides along
/// and the site knows it, else the next stage. orig: `PayableUpgrade.GetUpgradePrefab()`.
pub fn upgrade_target(
    world: &WorldData,
    o: &Object,
    passenger: Option<HermitKind>,
) -> Option<(ProtoId, bool)> {
    let p = &world.protos[o.proto as usize];
    if let Some(k) = passenger {
        if let Some(&(_, proto)) = p.passenger_upgrades.iter().find(|u| u.0 == k) {
            return Some((proto, true));
        }
    }
    p.upgrade?.next.map(|n| (n, false))
}

/// The payables the monarch can currently target.
/// orig: `PayableUpgrade.CanPay()`.
pub fn payables(
    world: &WorldData,
    level: &Level,
    forests: &[(f32, f32)],
    tech: Tech,
    other: impl Fn(PayKind, &Object) -> bool,
) -> Vec<Payable> {
    let mut out = Vec::new();
    for o in &level.objects {
        let proto = &world.protos[o.proto as usize];
        let Some(def) = proto.payable else { continue };
        let can_pay = match pay_kind(world, o) {
            // orig: PayableUpgrade.CanPay
            Some(PayKind::Upgrade) => {
                let up = proto.upgrade.expect("upgrade");
                upgrade_target(world, o, tech.passenger).is_some()
                    && o.cooldown <= 0.0
                    && !under_construction(world, o)
                    && (!up.requires_stone || tech.stone)
                    // orig: no NotBuildable collider at transform.position + (0, 0.5).
                    && (!up.only_in_buildable_region || !in_forest(forests, o.x))
            }
            // orig: PayableShop.CanPay: the rack has room (asked of `other`).
            Some(PayKind::Shop) => !under_construction(world, o) && other(PayKind::Shop, o),
            // orig: PayableTree.CanPay: not yet marked and near the forest edge.
            Some(PayKind::MarkTree) => {
                !o.marked && distance_from_forest_edge(world, o, forests) < 4.0
            }
            Some(k) => other(k, o),
            None => continue,
        };
        let dir = if o.flip { -1.0 } else { 1.0 };
        out.push(Payable {
            id: PayableId(o.id),
            x: o.x,
            y: o.y - WALK_Y,
            price: o.price,
            price_increase: def.price_increase,
            can_pay,
            coin_offset: (def.coin_offset.0 * dir, def.coin_offset.1),
            coin_spacing: def.coin_spacing,
            pay_point_offset: def.pay_point_offset * dir,
            pay_distance: def.pay_distance,
        });
    }
    out
}

/// What paying for an object does.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayKind {
    Upgrade,
    Shop,
    MarkTree,
    /// orig: `PayableShield`.
    Shield,
    /// orig: `PayableBorder`: send a knight to charge.
    Border,
    /// orig: `Statue`.
    Statue,
    /// orig: `Steed` (ride it).
    Steed,
    /// orig: `Cabin` (let the hermit out).
    Cabin,
    /// orig: `Hermit` (take it along).
    Hermit,
    /// orig: `Merchant` (send him back for more).
    Merchant,
    /// orig: `PayableWorkshop` (order a catapult).
    Workshop,
    /// orig: `PayableTeleporter`.
    Teleporter,
    /// orig: `Wharf` (stack a boat part, or launch the boat).
    Wharf,
    /// orig: `Beach` (sail away).
    Beach,
    /// orig: `Signpost` (take the map: an unlock after this island).
    Signpost,
}

pub fn pay_kind(world: &WorldData, o: &Object) -> Option<PayKind> {
    let p = &world.protos[o.proto as usize];
    p.payable?;
    if p.upgrade.is_some() {
        Some(PayKind::Upgrade)
    } else if p.shop.is_some() {
        Some(PayKind::Shop)
    } else if p.tree_work.is_some() {
        Some(PayKind::MarkTree)
    } else if p.shield_shop.is_some() {
        Some(PayKind::Shield)
    } else if p.border.is_some() {
        Some(PayKind::Border)
    } else if p.statue.is_some() {
        Some(PayKind::Statue)
    } else if p.steed.is_some() {
        Some(PayKind::Steed)
    } else if p.cabin.is_some() {
        Some(PayKind::Cabin)
    } else if p.hermit.is_some() {
        Some(PayKind::Hermit)
    } else if p.merchant.is_some() {
        Some(PayKind::Merchant)
    } else if p.workshop_side.is_some() {
        Some(PayKind::Workshop)
    } else if p.teleporter.is_some() {
        Some(PayKind::Teleporter)
    } else if p.wharf.is_some() {
        Some(PayKind::Wharf)
    } else if p.beach {
        Some(PayKind::Beach)
    } else if p.signpost.is_some() {
        Some(PayKind::Signpost)
    } else {
        None
    }
}

/// orig: `ForestItem.DistanceFromForestEdge()`.
fn distance_from_forest_edge(world: &WorldData, o: &Object, forests: &[(f32, f32)]) -> f32 {
    let Some(f) = world.protos[o.proto as usize].forest_item else {
        return f32::MAX;
    };
    let w = f.width + 2.0 * f.size_padding;
    let (l, r) = (o.x - w / 2.0, o.x + w / 2.0);
    forests
        .iter()
        .find(|&&(a, b)| a <= l + 1e-3 && r <= b + 1e-3)
        .map_or(f32::MAX, |&(a, b)| (l - a).min(b - r) - f.size_padding)
}

/// Marks a tree for the workers. orig: `WorkableTree.MarkForChopping()`.
pub fn mark_tree(world: &WorldData, level: &mut Level, id: u32) {
    let Some(i) = level.index_of(id) else { return };
    let o = &mut level.objects[i];
    o.marked = true;
    if let Some(m) = world.protos[o.proto as usize]
        .tree_work
        .and_then(|t| t.chop_mark)
    {
        if let Some(p) = o.parts.get_mut(m as usize) {
            p.visible = true;
        }
    }
}

/// What an upgrade did, for the caller's events.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Upgraded {
    pub old: u32,
    pub new: u32,
    /// orig: `Castle.level` of the new object, if it's a castle.
    pub castle_level: Option<u8>,
    /// The hermit riding along was used for it. orig: `Player.OnPassengerUsed()`.
    pub used_passenger: bool,
}

/// Replaces the paid object with its next prefab. orig: `PayableUpgrade.Pay()`
/// and `WorkableBuilding.Start()` (new buildings start as scaffolds).
pub fn upgrade(
    world: &WorldData,
    level: &mut Level,
    id: u32,
    passenger: Option<HermitKind>,
) -> Option<Upgraded> {
    let i = level.index_of(id)?;
    let (next, used_passenger) = upgrade_target(world, &level.objects[i], passenger)?;
    let old = level.objects.remove(i);
    let proto = &world.protos[next as usize];
    let mut o = instantiate(
        world,
        next,
        old.layer,
        old.x,
        old.y,
        proto.root_z,
        if old.flip { -1.0 } else { 1.0 },
    );
    if let Some(b) = &proto.building {
        if b.build_points > 0 {
            o.build = Some(0.0);
        }
    }
    let castle_level = proto.castle_level;
    let new = level.add(o);
    if let Some(lvl) = castle_level {
        castle_effects(world, level, new, lvl);
    }
    Some(Upgraded {
        old: id,
        new,
        castle_level,
        used_passenger,
    })
}

/// orig: `Castle.Start()` when just built: shops appear as the castle grows,
/// nearby walls and towers get a free upgrade, and the last castle sets up
/// shield shops.
fn castle_effects(world: &WorldData, level: &mut Level, castle: u32, castle_level: u8) {
    let tags: &[&str] = match castle_level {
        0 => &["ShopBow", "ShopHammer"],
        2 => &["ShopScythe"],
        3 => &["Workshop"],
        _ => &[],
    };
    for tag in tags {
        swap_dormant(world, level, tag);
    }
    let Some(ci) = level.index_of(castle) else {
        return;
    };
    let cx = level.objects[ci].x;
    match castle_level {
        2 => upgrade_walls_near(world, level, cx, 12.0, 1),
        4 => {
            upgrade_walls_near(world, level, cx, 12.0, 3);
            upgrade_towers_near(world, level, cx, 12.0, 2);
        }
        5 => {
            let spots = world.protos[level.objects[ci].proto as usize]
                .shield_spots
                .clone();
            if let Some(shop) = world.shield_shop {
                let z = world.protos[shop as usize].root_z;
                for dx in spots {
                    // orig: at the placeholder, y = 0.
                    let o = instantiate(world, shop, Layer::Game, cx + dx, 0.0, z, 1.0);
                    level.add(o);
                }
            }
        }
        _ => {}
    }
}

/// orig: `Util.FindClosestGameObjectOnSide()`: nearest on `side` within `range`.
fn closest_on_side(
    level: &Level,
    x: f32,
    side: f32,
    range: f32,
    pick: impl Fn(&Object) -> bool,
) -> Option<u32> {
    level
        .objects
        .iter()
        .filter(|o| pick(o))
        .map(|o| (o.id, side * (o.x - x)))
        .filter(|&(_, d)| d > 0.0 && d < range)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id)
}

/// Replaces an object by a fresh building of `proto` that starts as its scaffold.
fn replace_with(world: &WorldData, level: &mut Level, id: u32, proto: ProtoId) {
    let Some(i) = level.index_of(id) else { return };
    let old = level.objects.remove(i);
    let p = &world.protos[proto as usize];
    let mut o = instantiate(
        world,
        proto,
        old.layer,
        old.x,
        old.y,
        p.root_z,
        if old.flip { -1.0 } else { 1.0 },
    );
    if p.building.as_ref().is_some_and(|b| b.build_points > 0) {
        o.build = Some(0.0);
    }
    level.add(o);
}

/// orig: `Castle.UpgradeWallsOrWrecks()`: the closest foundation, wreck or
/// wall on each side becomes at least `wall_level`.
fn upgrade_walls_near(world: &WorldData, level: &mut Level, x: f32, range: f32, wall_level: u8) {
    for side in [-1.0, 1.0] {
        let found = closest_on_side(level, x, side, range, |o| {
            matches!(
                world.protos[o.proto as usize].tag.as_str(),
                "WallFoundation" | "WallWreck" | "Wall"
            )
        });
        let Some(id) = found else { continue };
        let Some(i) = level.index_of(id) else {
            continue;
        };
        let p = &world.protos[level.objects[i].proto as usize];
        let new = match p.tag.as_str() {
            "WallFoundation" => Some(wall_level),
            "WallWreck" => p
                .upgrade
                .and_then(|u| u.next)
                .and_then(|n| world.protos[n as usize].wall_level)
                .map(|l| l.max(wall_level)),
            // A wall (or its scaffold) below the level.
            _ => p.wall_level.filter(|&l| wall_level > l).map(|_| wall_level),
        };
        if let Some(l) = new {
            if let Some(&proto) = world.wall_prefabs.get(l as usize - 1) {
                replace_with(world, level, id, proto);
            }
        }
    }
}

/// orig: `Castle.UpgradeTowers()` / `UpgradeTowerToLevel()`.
fn upgrade_towers_near(world: &WorldData, level: &mut Level, x: f32, range: f32, tower_level: u8) {
    for side in [-1.0, 1.0] {
        let found = closest_on_side(level, x, side, range, |o| {
            world.protos[o.proto as usize].tag == "Tower"
        });
        let Some(id) = found else { continue };
        let Some(i) = level.index_of(id) else {
            continue;
        };
        let current = world.protos[level.objects[i].proto as usize]
            .tower_level
            .unwrap_or(0);
        if current < tower_level {
            if let Some(&proto) = world.tower_prefabs.get(tower_level as usize - 1) {
                replace_with(world, level, id, proto);
            }
        }
    }
}

/// orig: `PrefabPlaceholder.Swap()` for a `doNotSwap` placeholder with `tag`.
fn swap_dormant(world: &WorldData, level: &mut Level, tag: &str) {
    let Some(i) = level.objects.iter().position(|o| {
        let p = &world.protos[o.proto as usize];
        p.tag == tag && matches!(p.kind, Kind::Dormant { .. })
    }) else {
        return;
    };
    let old = level.objects.remove(i);
    let Kind::Dormant { proto } = world.protos[old.proto as usize].kind else {
        return;
    };
    let mut o = instantiate(
        world,
        proto,
        old.layer,
        old.x,
        old.y,
        old.z,
        if old.flip { -1.0 } else { 1.0 },
    );
    // orig: WorkableBuilding.Start: a new building starts as its scaffold.
    if world.protos[proto as usize]
        .building
        .as_ref()
        .is_some_and(|b| b.build_points > 0)
    {
        o.build = Some(0.0);
    }
    level.add(o);
}

/// Advances self-building scaffolds. orig: `WorkableBuilding.AutoBuild()`.
/// Returns ids of buildings that finished this tick.
pub fn step(world: &WorldData, level: &mut Level, dt: f32) -> Vec<u32> {
    let mut done = Vec::new();
    for o in &mut level.objects {
        o.cooldown = (o.cooldown - dt).max(0.0);
        let Some(b) = &world.protos[o.proto as usize].building else {
            continue;
        };
        let Some(points) = o.build.as_mut() else {
            continue;
        };
        if b.auto_build_rate > 0.0 && *points < b.build_points as f32 {
            // orig: one point every 1/autoBuildRate seconds; continuous here.
            *points = (*points + b.auto_build_rate * dt).min(b.build_points as f32);
            if *points >= b.build_points as f32 {
                done.push(o.id);
            }
        }
    }
    done
}

/// Adds one build point, as a builder's hammer stroke does.
/// orig: `WorkableBuilding.IncrementBuild()`. Returns true when finished.
pub fn add_build_point(world: &WorldData, level: &mut Level, id: u32) -> bool {
    let Some(i) = level.index_of(id) else {
        return false;
    };
    let o = &mut level.objects[i];
    let Some(b) = &world.protos[o.proto as usize].building else {
        return false;
    };
    let Some(points) = o.build.as_mut() else {
        return false;
    };
    *points = (*points + 1.0).min(b.build_points as f32);
    *points >= b.build_points as f32
}

/// Finishes every scaffold at once (debug). orig: `WorkableBuilding.CompleteConstruction()`.
pub fn complete_all(world: &WorldData, level: &mut Level) {
    for o in &mut level.objects {
        if let (Some(b), Some(points)) =
            (&world.protos[o.proto as usize].building, o.build.as_mut())
        {
            *points = b.build_points as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::*;
    use alloc::string::ToString;
    use alloc::vec;

    fn proto(name: &str) -> Proto {
        Proto {
            name: name.to_string(),
            tag: "Untagged".to_string(),
            ..Default::default()
        }
    }

    fn payable(price: u8) -> PayableDef {
        PayableDef {
            price,
            price_increase: 0,
            coin_offset: (0.0, 2.5),
            coin_spacing: 0.4,
            pay_point_offset: 0.0,
            pay_distance: 0.75,
        }
    }

    fn world() -> WorldData {
        let mut wall0 = proto("Wall0");
        wall0.payable = Some(payable(1));
        wall0.upgrade = Some(UpgradeDef {
            next: Some(1),
            only_in_buildable_region: true,
            requires_stone: false,
            cooldown: 0.0,
        });
        let mut wall1 = proto("Wall1");
        wall1.payable = Some(payable(3));
        wall1.upgrade = Some(UpgradeDef {
            next: Some(2),
            only_in_buildable_region: true,
            requires_stone: true,
            cooldown: 0.0,
        });
        wall1.building = Some(BuildingDef {
            build_points: 10,
            auto_build_rate: 0.0,
            scaffold: vec![
                ScaffoldModuleDef {
                    transform: crate::data::Affine2::IDENTITY,
                    flip: false,
                    flip_y: false,
                    sprite: None,
                    fill: None,
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                };
                3
            ],
        });
        let mut tree = proto("Tree");
        tree.forest_item = Some(ForestItemDef {
            controls_forest_size: false,
            size_padding: 0.0,
            width: 2.0,
        });
        WorldData {
            protos: vec![wall0, wall1, proto("Wall2"), tree],
            layers: vec![LayerDef { ratio: 1.0, z: 0.0 }; 7],
            ..Default::default()
        }
    }

    fn level(w: &WorldData, objs: &[(ProtoId, f32)]) -> Level {
        let mut l = Level::empty(-50.0, 100.0);
        for &(p, x) in objs {
            l.add(instantiate(w, p, Layer::Game, x, 0.0, 0.0, 1.0));
        }
        l
    }

    #[test]
    fn forests_merge_overlapping_trees() {
        let w = world();
        let l = level(&w, &[(3, 0.0), (3, 1.5), (3, 10.0)]);
        assert_eq!(forests(&w, &l), vec![(-1.0, 2.5), (9.0, 11.0)]);
    }

    #[test]
    fn walls_in_forests_cannot_be_paid() {
        let w = world();
        let l = level(&w, &[(0, 0.0), (0, 20.0), (3, 0.0)]);
        let f = forests(&w, &l);
        let p = payables(&w, &l, &f, Tech::default(), |_, _| true);
        assert!(!p[0].can_pay);
        assert!(p[1].can_pay);
    }

    #[test]
    fn paying_a_wall_starts_a_scaffold_that_builds_up() {
        let w = world();
        let mut l = level(&w, &[(0, 5.0)]);
        let up = upgrade(&w, &mut l, 0, None).unwrap();
        let o = &l.objects[l.index_of(up.new).unwrap()];
        assert_eq!(o.proto, 1);
        assert!(under_construction(&w, o));
        assert_eq!(scaffold_filled(&w, o), 0);
        // Under construction → can't be upgraded further yet.
        let p = payables(
            &w,
            &l,
            &[],
            Tech {
                stone: true,
                passenger: None,
            },
            |_, _| true,
        );
        assert!(!p[0].can_pay);
        for _ in 0..5 {
            add_build_point(&w, &mut l, up.new);
        }
        let o = &l.objects[l.index_of(up.new).unwrap()];
        // 5 / (10 - 1) of 3 modules → 2.
        assert_eq!(scaffold_filled(&w, o), 2);
        for _ in 0..5 {
            add_build_point(&w, &mut l, up.new);
        }
        let o = &l.objects[l.index_of(up.new).unwrap()];
        assert!(!under_construction(&w, o));
        // Wall1 → Wall2 needs stone.
        assert!(!payables(&w, &l, &[], Tech::default(), |_, _| true)[0].can_pay);
        assert!(
            payables(
                &w,
                &l,
                &[],
                Tech {
                    stone: true,
                    passenger: None
                },
                |_, _| true
            )[0]
            .can_pay
        );
    }
}
