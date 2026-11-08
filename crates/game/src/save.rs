//! Saving and loading: the whole island as it stands, and the unlocks kept
//! across runs, as bytes for the backend to store.
//!
//! orig: `PersistentManager` (the `AUTO` slot, `storage_v34_AUTO.dat`) and
//! `UnlockManager` (the `unlocks` file). The original saves each object's
//! `IPersistent` data and rebuilds the scene from prefabs; we snapshot the
//! simulation state instead, which resumes exactly where it left off.

use alloc::vec::Vec;

use crate::progress::Progress;
use crate::state::GameState;

/// Bumped whenever the saved state's layout changes.
pub const VERSION: u16 = 12;
const MAGIC: [u8; 4] = *b"KNL1";
/// v5 already requires every trailing field. Later format bumps must not
/// reinterpret a truncated v5 snapshot as an earlier layout needing defaults.
const COMPLETE_LAYOUT_VERSION: u16 = 5;

fn exact_decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, postcard::Error> {
    let (value, tail) = postcard::take_from_bytes(bytes)?;
    if !tail.is_empty() {
        return Err(postcard::Error::DeserializeBadEncoding);
    }
    Ok(value)
}

/// The slot names the backends store under. orig: `"AUTO"`, `"unlocks"`.
pub const GAME_SLOT: &str = "game";
pub const UNLOCKS_SLOT: &str = "unlocks";
/// Which game slot is in use (one byte).
pub const CURRENT_SLOT: &str = "slot";
/// Game save slots. The first keeps `GAME_SLOT`'s name, so older saves load.
pub const SLOTS: usize = 3;

/// The storage name of game slot `i` (0-based).
pub fn game_slot(i: usize) -> alloc::string::String {
    if i == 0 {
        GAME_SLOT.into()
    } else {
        alloc::format!("{GAME_SLOT}{}", i + 1)
    }
}

/// Why a save couldn't be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// Not a save, or from another version of the game.
    Format,
    /// Made with different game data (`world.bin`); ids wouldn't line up.
    World,
    /// Damaged. orig: "Save file corrupted".
    Corrupt,
}

/// A cheap fingerprint of the game data (FNV-1a over `world.bin`). Saves
/// keep it, since they refer to prototypes and sprites by index.
pub fn fingerprint(world_bin: &[u8]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for &b in world_bin {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

fn header(world: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(10);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&world.to_le_bytes());
    out
}

fn body(bytes: &[u8], world: Option<u32>) -> Result<(u16, &[u8]), LoadError> {
    if bytes.len() < 10 || bytes[..4] != MAGIC {
        return Err(LoadError::Format);
    }
    let stored = u16::from_le_bytes([bytes[4], bytes[5]]);
    if stored == 0 || stored > VERSION {
        return Err(LoadError::Format);
    }
    let w = u32::from_le_bytes([bytes[6], bytes[7], bytes[8], bytes[9]]);
    if world.is_some_and(|x| x != w) {
        return Err(LoadError::World);
    }
    Ok((stored, &bytes[10..]))
}

/// orig: `PersistentManager.Save()`.
pub fn save_game(g: &GameState, world: u32) -> Vec<u8> {
    let mut out = header(world);
    // Serializing plain data into a Vec can't fail.
    out.extend(postcard::to_allocvec(g).expect("plain-data serialization cannot fail"));
    let animation = AnimationSave {
        portals: g.greed.portal_visuals().to_vec(),
        recoils: g.siege.recoils.clone(),
        squids: g.greed.squid_states(),
    };
    out.extend(postcard::to_allocvec(&animation).expect("plain-data serialization cannot fail"));
    out.extend(
        postcard::to_allocvec(&g.siege.boulder_states())
            .expect("plain-data serialization cannot fail"),
    );
    out.extend(
        postcard::to_allocvec(&g.siege.boulder_contact_ages())
            .expect("plain-data serialization cannot fail"),
    );
    out
}

/// Kept outside the legacy GameState prefix: changing a nested Greed/Siege
/// layout would make otherwise valid older saves impossible to decode.
#[derive(serde::Serialize, serde::Deserialize)]
struct AnimationSave {
    portals: Vec<crate::greed::PortalVisual>,
    recoils: Vec<crate::recoil::State>,
    squids: Vec<crate::greed::SquidRuntime>,
}

/// Exact v9 tail, before squid coroutine and flight clocks were persisted.
#[derive(serde::Serialize, serde::Deserialize)]
struct AnimationSaveV9 {
    portals: Vec<crate::greed::PortalVisual>,
    recoils: Vec<crate::recoil::State>,
}

/// Exact v8 tail. Its one-axis recoil cannot be decoded as a three-body state.
#[derive(serde::Serialize, serde::Deserialize)]
struct AnimationSaveV8 {
    portals: Vec<crate::greed::PortalVisual>,
    recoils: Vec<RecoilV8>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct RecoilV8 {
    object: u32,
    side: i8,
    angle: f32,
    angular_velocity: f32,
    pending: f32,
}

impl AnimationSaveV8 {
    fn migrate(
        self,
        world: &crate::data::WorldData,
        level: &crate::level::Level,
    ) -> Result<AnimationSaveV9, LoadError> {
        let mut recoils = Vec::with_capacity(self.recoils.len());
        for old in self.recoils {
            let o = level
                .index_of(old.object)
                .map(|i| &level.objects[i])
                .ok_or(LoadError::Corrupt)?;
            let def = world.protos[o.proto as usize]
                .catapult
                .and_then(|c| c.recoil)
                .ok_or(LoadError::Corrupt)?;
            if !matches!(old.side, -1 | 1)
                || !old.angle.is_finite()
                || !(0.0..core::f32::consts::FRAC_PI_2).contains(&old.angle)
                || !old.angular_velocity.is_finite()
                || old.angular_velocity.abs() > def.max_angular_speed + 1e-5
                || !old.pending.is_finite()
                || !(0.0..def.fixed_step).contains(&old.pending)
            {
                return Err(LoadError::Corrupt);
            }
            let mut r = crate::recoil::State::new(old.object, old.side, &def);
            r.ground_y = def.ground_height - o.y;
            r.pending = old.pending;
            let pivot = if old.side > 0 {
                def.front_anchor
            } else {
                def.rear_anchor
            };
            let angle = -(old.side as f32) * old.angle;
            let speed = -(old.side as f32) * old.angular_velocity;
            let (sin, cos) = (libm::sinf(angle), libm::cosf(angle));
            r.bodies[0].angle = angle;
            r.bodies[0].angular_velocity = speed;
            // Preserve the v8 pose and its tangential velocities. From this
            // instant onward contacts/hinges, rather than a pinned wheel, govern it.
            for (body, anchor) in r.bodies[1..]
                .iter_mut()
                .zip([def.front_anchor, def.rear_anchor])
            {
                let offset = [anchor[0] - pivot[0], anchor[1] - pivot[1]];
                let rotated = [
                    cos * offset[0] - sin * offset[1],
                    sin * offset[0] + cos * offset[1],
                ];
                body.position = [pivot[0] + rotated[0], pivot[1] + rotated[1]];
                body.velocity = [-speed * rotated[1], speed * rotated[0]];
            }
            recoils.push(r);
        }
        Ok(AnimationSaveV9 {
            portals: self.portals,
            recoils,
        })
    }
}

#[cfg(test)]
pub(crate) fn legacy_game(g: &GameState, world: u32, version: u16) -> Vec<u8> {
    assert!(version < 8);
    let mut out = header(world);
    out[4..6].copy_from_slice(&version.to_le_bytes());
    out.extend(postcard::to_allocvec(g).unwrap());
    out
}

/// orig: `PersistentManager.Load()`.
pub fn load_game(
    bytes: &[u8],
    world_id: u32,
    world: &crate::data::WorldData,
) -> Result<GameState, LoadError> {
    let (stored, body) = body(bytes, Some(world_id))?;
    let look = postcard::to_allocvec(&crate::kingdom::Look::default())
        .expect("plain-data serialization cannot fail");
    let tutorial = postcard::to_allocvec(&crate::tutorial::Tutorial::default())
        .expect("plain-data serialization cannot fail");
    // Saved before the last fields were added: the look (a king with the
    // first pattern and emblem), and before that the tutorial (it sets
    // itself up on the first tick).
    let with = |tails: &[&[u8]]| {
        let mut old = body.to_vec();
        for t in tails {
            old.extend_from_slice(t);
        }
        exact_decode(&old)
    };
    let mut animation = None;
    let mut squids = None;
    let mut boulders = None;
    let mut contact_ages = None;
    let mut legacy_animation = None;
    let mut g: GameState = if stored >= COMPLETE_LAYOUT_VERSION {
        let (g, tail) =
            postcard::take_from_bytes::<GameState>(body).map_err(|_| LoadError::Corrupt)?;
        if stored >= 10 {
            let extra = if stored >= 11 {
                let (extra, boulder_tail) = postcard::take_from_bytes::<AnimationSave>(tail)
                    .map_err(|_| LoadError::Corrupt)?;
                if stored >= 12 {
                    let (states, ages) =
                        postcard::take_from_bytes(boulder_tail).map_err(|_| LoadError::Corrupt)?;
                    boulders = Some(states);
                    contact_ages = Some(exact_decode(ages).map_err(|_| LoadError::Corrupt)?);
                } else {
                    boulders = Some(exact_decode(boulder_tail).map_err(|_| LoadError::Corrupt)?);
                }
                extra
            } else {
                exact_decode::<AnimationSave>(tail).map_err(|_| LoadError::Corrupt)?
            };
            squids = Some(extra.squids);
            animation = Some(AnimationSaveV9 {
                portals: extra.portals,
                recoils: extra.recoils,
            });
        } else if stored == 9 {
            animation =
                Some(exact_decode::<AnimationSaveV9>(tail).map_err(|_| LoadError::Corrupt)?);
        } else if stored == 8 {
            legacy_animation =
                Some(exact_decode::<AnimationSaveV8>(tail).map_err(|_| LoadError::Corrupt)?);
        } else if !tail.is_empty() {
            return Err(LoadError::Corrupt);
        }
        Ok(g)
    } else {
        exact_decode(body)
            .or_else(|_| with(&[&look]))
            .or_else(|_| with(&[&tutorial, &look]))
    }
    .map_err(|_| LoadError::Corrupt)?;
    if stored < 7 {
        g.migrate_animation_clocks();
    }
    if !g.ids_valid(world) {
        return Err(LoadError::Corrupt);
    }
    if let Some(old) = legacy_animation {
        animation = Some(old.migrate(world, &g.level)?);
    }
    if let Some(extra) = animation {
        if !g
            .greed
            .restore_portal_visuals(extra.portals, world, &g.level)
            || !g.siege.restore_recoils(extra.recoils, world, &g.level)
        {
            return Err(LoadError::Corrupt);
        }
    } else {
        g.greed.migrate_portal_visuals(world, &g.level);
    }
    if let Some(squids) = squids {
        if !g.greed.restore_squid_states(squids, world) {
            return Err(LoadError::Corrupt);
        }
    } else {
        g.greed.migrate_squid_states(world);
    }
    if let Some(boulders) = boulders {
        if !g.siege.restore_boulder_states(boulders, world) || !boss_boulders_valid(&g, world) {
            return Err(LoadError::Corrupt);
        }
    } else if g
        .greed
        .trolls
        .iter()
        .any(|t| matches!(t.state, crate::greed::TrollState::ThrowBoulder { .. }))
    {
        return Err(LoadError::Corrupt);
    }
    if contact_ages.is_some_and(|ages| !g.siege.restore_boulder_contact_ages(ages)) {
        return Err(LoadError::Corrupt);
    }
    g.blender.rebind_red_moon(world);
    Ok(g)
}

fn boss_boulders_valid(g: &GameState, world: &crate::data::WorldData) -> bool {
    use crate::greed::TrollState;
    let release = crate::greed::boss_boulder_release_time(world);
    let held = |id| {
        g.siege
            .boulders
            .iter()
            .filter(move |b| b.runtime.held_by == Some(id))
            .count()
    };
    g.siege.boulders.iter().all(|b| {
        b.runtime.held_by.is_none_or(|owner| {
            g.greed.trolls.iter().any(|t| {
                t.id == owner
                    && t.kind.is_boss()
                    && t.x.is_finite()
                    && t.y.is_finite()
                    && matches!(t.mover.facing, -1 | 1)
                    && (t.is_dead()
                        || matches!(t.state, TrollState::ThrowBoulder { t } if t < release))
            })
        })
    }) && g.greed.trolls.iter().all(|t| match t.state {
        TrollState::ThrowBoulder { t: elapsed } => {
            t.kind.is_boss()
                && (0.0..2.6).contains(&elapsed)
                && t.bash.is_none()
                && t.puke.is_none()
                && held(t.id) == usize::from(elapsed < release)
        }
        TrollState::Dead { .. } if t.kind.is_boss() => held(t.id) <= 1,
        _ => true,
    })
}

/// orig: `UnlockManager.SaveUnlocks()` (unlocks don't depend on the game data).
pub fn save_progress(p: &Progress) -> Vec<u8> {
    let mut out = header(0);
    out.extend(postcard::to_allocvec(p).expect("plain-data serialization cannot fail"));
    out
}

/// orig: `UnlockManager.LoadUnlocks()`: a damaged file starts over.
pub fn load_progress(bytes: &[u8]) -> Result<Progress, LoadError> {
    let (stored, body) = body(bytes, None)?;
    // Saved before `chosen` (None, a 0 byte), and before that
    // `tutorial_done` (false, another 0).
    let with = |zeros: usize| {
        let mut old = body.to_vec();
        old.resize(old.len() + zeros, 0);
        exact_decode(&old)
    };
    if stored >= COMPLETE_LAYOUT_VERSION {
        exact_decode(body)
    } else {
        exact_decode(body).or_else(|_| with(1)).or_else(|_| with(2))
    }
    .map_err(|_| LoadError::Corrupt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{DayDef, DirectorDef, LevelConfigDef, WorldData};
    use alloc::vec;

    fn save_world() -> WorldData {
        WorldData {
            days: vec![DayDef {
                name: "day".into(),
                seasons: 7,
                weathers: 1,
                tracks: vec![],
            }],
            director: DirectorDef {
                regular_days: vec![0],
                boss_days: vec![0],
                recovery_days: vec![0],
                num_regular_days: (3, 4),
                ..Default::default()
            },
            steeds: vec![Default::default()],
            levels: vec![LevelConfigDef {
                name: "island".into(),
                autumn_start: (40, 40),
                winter_start: (60, 60),
                days_before_game_over: -1,
                starting_coins: 0,
                min_level_width: 0,
                group_counts: vec![],
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
            ..Default::default()
        }
    }

    #[test]
    fn v12_contact_guard_resumes_and_v11_loads_without_an_age_tail() {
        let w = save_world();
        let mut g = GameState::new(&w, 0, 23);
        g.siege
            .boulders
            .push(crate::siege::Boulder::test_lodged(3.0));
        let ages = vec![Some(0.09)];
        assert!(g.siege.restore_boulder_contact_ages(ages.clone()));
        let bytes = save_game(&g, 3);
        let resumed = load_game(&bytes, 3, &w).unwrap();
        assert_eq!(resumed.siege.boulder_contact_ages(), ages);
        assert_eq!(save_game(&resumed, 3), bytes);
        let end_of_v11 = bytes.len() - postcard::to_allocvec(&ages).unwrap().len();
        assert_eq!(
            load_game(&bytes[..end_of_v11], 3, &w).err(),
            Some(LoadError::Corrupt)
        );
        let mut old = bytes[..end_of_v11].to_vec();
        old[4..6].copy_from_slice(&11u16.to_le_bytes());
        assert_eq!(
            load_game(&old, 3, &w).unwrap().siege.boulder_contact_ages(),
            vec![None]
        );
        let mut bad = bytes[..end_of_v11].to_vec();
        bad.extend(postcard::to_allocvec(&vec![Some(f32::NAN)]).unwrap());
        assert_eq!(load_game(&bad, 3, &w).err(), Some(LoadError::Corrupt));
    }

    #[test]
    fn v10_lodged_boulder_migrates_as_an_unheld_friendly_stone() {
        let w = save_world();
        let mut g = GameState::new(&w, 0, 42);
        g.siege
            .boulders
            .push(crate::siege::Boulder::test_lodged(2.0));
        let mut bytes = header(1);
        bytes[4..6].copy_from_slice(&10u16.to_le_bytes());
        bytes.extend(postcard::to_allocvec(&g).unwrap());
        bytes.extend(
            postcard::to_allocvec(&AnimationSave {
                portals: g.greed.portal_visuals().to_vec(),
                recoils: g.siege.recoils.clone(),
                squids: g.greed.squid_states(),
            })
            .unwrap(),
        );
        let migrated = load_game(&bytes, 1, &w).unwrap();
        let boulder = &migrated.siege.boulders[0];
        assert_eq!(boulder.x, 2.0);
        assert!(boulder.stuck.is_some());
        assert!(!boulder.runtime.hostile);
        assert!(boulder.runtime.held_by.is_none());
        assert!(load_game(&save_game(&migrated, 1), 1, &w).is_ok());
    }

    #[test]
    fn v10_squid_flight_and_every_retreat_phase_roundtrip_and_v9_migrates() {
        use crate::greed::{EnemyKind, SquidPhase, TrollState};
        let w = save_world();
        let mut g = GameState::new(&w, 0, 9);
        g.greed
            .spawn_troll(EnemyKind::Squid, 7.0, 1, false, &mut g.rng);
        g.greed.trolls[0].y = 1.25;
        g.greed.trolls[0].vy = -0.4;
        for phase in [
            SquidPhase::Chase,
            SquidPhase::Retreat { wait: 0.37 },
            SquidPhase::NoPortal { wait: 8.7 },
            SquidPhase::Stranded,
            SquidPhase::Despawning { elapsed: 0.61 },
        ] {
            let mut states = g.greed.squid_states();
            let r = &mut states[0];
            r.phase = phase;
            r.initialized = true;
            r.current_speed = 1.18;
            r.altitude = 1.325;
            r.transition_elapsed = 0.72;
            r.chase_wait = 0.24;
            r.animation_elapsed = 0.3;
            r.dive_blend = Some(0.12);
            r.attack_pending = true;
            r.grab_pending = true;
            r.flight.pending = 0.013;
            r.flight.fly_timeout = 0.68;
            r.flight.flap_elapsed = Some(0.32);
            r.flight.force_pending = true;
            assert!(g.greed.restore_squid_states(states, &w));
            let bytes = save_game(&g, 91);
            let resumed = load_game(&bytes, 91, &w).unwrap();
            assert_eq!(resumed.greed.squid_states(), g.greed.squid_states());
            assert_eq!(save_game(&resumed, 91), bytes);
            assert_eq!(resumed.greed.trolls[0].y, 1.25);
            // A pending force is consumed at the same next fixed step after load.
            let mut before = g.greed.squid_states()[0].flight;
            let mut after = resumed.greed.squid_states()[0].flight;
            let mut a = crate::flight::Motion {
                x: 7.,
                y: 1.25,
                vx: 0.3,
                vy: -0.4,
            };
            let mut b = a;
            let input = crate::flight::Input {
                altitude: 1.325,
                move_velocity: Some(0.3),
                alive: true,
                collide_with_obstacles: false,
            };
            before.advance(0.017, &mut a, input, &w.squid, &[]);
            after.advance(0.017, &mut b, input, &w.squid, &[]);
            assert_eq!((before, a), (after, b));
        }
        g.greed.trolls[0].state = TrollState::Retreat { t: 0.0 };
        let mut old = header(91);
        old[4..6].copy_from_slice(&9u16.to_le_bytes());
        old.extend(postcard::to_allocvec(&g).unwrap());
        old.extend(
            postcard::to_allocvec(&AnimationSaveV9 {
                portals: vec![],
                recoils: vec![],
            })
            .unwrap(),
        );
        let migrated = load_game(&old, 91, &w).unwrap();
        assert_eq!(
            migrated.greed.squid_states()[0].phase,
            SquidPhase::Retreat { wait: 0.0 }
        );
        assert_eq!(
            migrated.greed.trolls[0].y, 1.25,
            "saved positions already use ground-relative coordinates"
        );
        assert_eq!(migrated.greed.squid_states()[0].flight.pending, 0.0);
        let mut truncated = save_game(&g, 91);
        truncated.pop();
        assert_eq!(
            load_game(&truncated, 91, &w).err(),
            Some(LoadError::Corrupt)
        );
        let mut missing = header(91);
        missing.extend(postcard::to_allocvec(&g).unwrap());
        missing.extend(
            postcard::to_allocvec(&AnimationSave {
                portals: vec![],
                recoils: vec![],
                squids: vec![],
            })
            .unwrap(),
        );
        assert_eq!(load_game(&missing, 91, &w).err(), Some(LoadError::Corrupt));
        let mut corrupt = g.greed.squid_states();
        corrupt[0].flight.pending = f32::NAN;
        let mut bad = header(91);
        bad.extend(postcard::to_allocvec(&g).unwrap());
        bad.extend(
            postcard::to_allocvec(&AnimationSave {
                portals: vec![],
                recoils: vec![],
                squids: corrupt,
            })
            .unwrap(),
        );
        assert_eq!(load_game(&bad, 91, &w).err(), Some(LoadError::Corrupt));
        g.greed.trolls[0].y = f32::NAN;
        assert_eq!(
            load_game(&save_game(&g, 91), 91, &w).err(),
            Some(LoadError::Corrupt)
        );
        g.greed.trolls[0].y = 1.25;
        g.greed.trolls[0].mover.vx = f32::MAX;
        assert_eq!(
            load_game(&save_game(&g, 91), 91, &w).err(),
            Some(LoadError::Corrupt)
        );
    }

    #[test]
    fn v9_physics_tail_roundtrips_and_older_versions_migrate_without_replaying_effects() {
        use crate::data::{CatapultDef, Layer, PortalDef, Proto};
        use crate::greed::{PortalVisual, PortalVisualState};
        let mut w = save_world();
        w.protos = vec![
            Proto {
                portal: Some(PortalDef {
                    cliff: false,
                    hit_points: 100,
                    by_arrows: true,
                    by_knights: true,
                    hit_box: (-1., 1., 0., 3.),
                    defense_range: 3.,
                    defense_delay: (2., 16.),
                    defense_cooldown: 6.,
                    rebuild_days: 3.,
                    rift: None,
                }),
                ..Default::default()
            },
            Proto {
                catapult: Some(CatapultDef {
                    crank_rate: 0.1,
                    force: (10., 10.),
                    scan: (1., 10.),
                    hit_points: 3,
                    boulder_damage: 1,
                    stuck_probability: 0.,
                    spoon: (0., 1.),
                    recoil: Some(crate::recoil::fixture()),
                }),
                ..Default::default()
            },
            Proto {
                workshop_side: Some((1, 1)),
                ..Default::default()
            },
        ];
        let mut g = GameState::new(&w, 0, 1);
        let portal = g.level.add(crate::level::instantiate(
            &w,
            0,
            Layer::Game,
            8.,
            1.,
            0.,
            1.,
        ));
        g.greed.portal_hit(portal, &mut |_| {});
        assert!(g.greed.restore_portal_visuals(
            vec![PortalVisual {
                object: portal,
                state: PortalVisualState::Open,
                elapsed: 0.4,
                triggers: 0
            }],
            &w,
            &g.level
        ));
        let counter = g.level.add(crate::level::instantiate(
            &w,
            2,
            Layer::Game,
            3.,
            1.,
            0.,
            1.,
        ));
        g.siege.order(&w, &mut g.level, counter);
        let def = crate::recoil::fixture();
        let mut recoil = crate::recoil::State::new(g.siege.catapults[0].object, 1, &def);
        recoil.ground_y = def.ground_height - 1.0;
        recoil.impact(&def);
        recoil.advance(0.073, &def);
        g.siege.recoils.push(recoil);
        let bytes = save_game(&g, 42);
        let mut resumed = load_game(&bytes, 42, &w).unwrap();
        assert_eq!(resumed.greed.portal_animation(portal), Some(("Open", 0.4)));
        assert_eq!(resumed.siege.recoils, g.siege.recoils);
        assert_eq!(save_game(&resumed, 42), bytes);
        g.siege.recoils[0].advance(0.07, &def);
        resumed.siege.recoils[0].advance(0.07, &def);
        assert_eq!(g.siege.recoils, resumed.siege.recoils);
        let old = load_game(&legacy_game(&g, 42, 7), 42, &w).unwrap();
        assert_eq!(old.greed.portal_animation(portal), Some(("Idle", 0.0)));
        assert!(old.siege.recoils.is_empty());
        // Every current suffix is required; never reinterpret damage as an old layout.
        let prefix = 10 + postcard::to_allocvec(&g).unwrap().len();
        for end in prefix..bytes.len() {
            assert_eq!(
                load_game(&bytes[..end], 42, &w).err(),
                Some(LoadError::Corrupt)
            );
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert_eq!(load_game(&trailing, 42, &w).err(), Some(LoadError::Corrupt));
        let mut bad = resumed.clone();
        bad.siege.recoils[0].bodies[0].angular_velocity = f32::NAN;
        assert_eq!(
            load_game(&save_game(&bad, 42), 42, &w).err(),
            Some(LoadError::Corrupt)
        );
        bad = resumed.clone();
        bad.siege.recoils[0].ground_y += 1.;
        assert_eq!(
            load_game(&save_game(&bad, 42), 42, &w).err(),
            Some(LoadError::Corrupt)
        );

        let legacy = |recoil| {
            let mut b = legacy_game(&resumed, 42, 7);
            b[4..6].copy_from_slice(&8_u16.to_le_bytes());
            b.extend(
                postcard::to_allocvec(&AnimationSaveV8 {
                    portals: resumed.greed.portal_visuals().to_vec(),
                    recoils: vec![recoil],
                })
                .unwrap(),
            );
            b
        };
        let old_recoil = |angle| RecoilV8 {
            object: resumed.siege.catapults[0].object,
            side: 1,
            angle,
            angular_velocity: 0.5,
            pending: 0.007,
        };
        let old_bytes = legacy(old_recoil(0.08));
        let migrated = load_game(&old_bytes, 42, &w).unwrap();
        assert_eq!(migrated.greed.portal_animation(portal), Some(("Open", 0.4)));
        let r = &migrated.siege.recoils[0];
        assert_eq!(r.bodies[0].angle, -0.08);
        assert_eq!(r.bodies[0].angular_velocity, -0.5);
        assert_eq!(r.pending, 0.007);
        assert_eq!(r.bodies[0].position, def.front_anchor);
        assert!(r.bodies[2].position[1] > def.rear_anchor[1]);
        assert!(r.bodies[2].velocity[1] > 0.0);
        assert_eq!(r.pose(&def).point(def.front_anchor), def.front_anchor);
        for end in prefix..old_bytes.len() {
            assert_eq!(
                load_game(&old_bytes[..end], 42, &w).err(),
                Some(LoadError::Corrupt)
            );
        }
        assert_eq!(
            load_game(&legacy(old_recoil(f32::NAN)), 42, &w).err(),
            Some(LoadError::Corrupt)
        );
        let mut invalid_id = old_recoil(0.08);
        invalid_id.object = u32::MAX;
        assert_eq!(
            load_game(&legacy(invalid_id), 42, &w).err(),
            Some(LoadError::Corrupt)
        );
    }

    #[test]
    fn trailing_bytes_are_rejected_for_legacy_game_and_progress_versions() {
        let w = save_world();
        let g = GameState::new(&w, 0, 1);
        let mut old = legacy_game(&g, 42, 4);
        assert!(load_game(&old, 42, &w).is_ok());
        old.push(0);
        assert_eq!(load_game(&old, 42, &w).err(), Some(LoadError::Corrupt));
        for version in [4_u16, 5, 7, VERSION] {
            let mut p = save_progress(&Progress::everything());
            p[4..6].copy_from_slice(&version.to_le_bytes());
            assert!(load_progress(&p).is_ok());
            p.push(0);
            assert_eq!(load_progress(&p).err(), Some(LoadError::Corrupt));
        }
    }

    #[test]
    fn v5_and_current_game_saves_require_their_tutorial_and_look_tails() {
        let w = save_world();
        let mut g = GameState::new(&w, 0, 52);
        g.look.queen = true;
        let look_len = postcard::to_allocvec(&g.look).unwrap().len();
        let tutorial_len = postcard::to_allocvec(&g.tutorial).unwrap().len();
        for version in [5_u16, VERSION] {
            let bytes = if version < 8 {
                legacy_game(&g, 123, version)
            } else {
                save_game(&g, 123)
            };
            assert!(load_game(&bytes, 123, &w).unwrap().look.queen);
            for cut in [look_len, look_len + tutorial_len] {
                assert_eq!(
                    load_game(&bytes[..bytes.len() - cut], 123, &w).err(),
                    Some(LoadError::Corrupt),
                    "version {version}, missing {cut} bytes"
                );
            }
        }
    }

    #[test]
    fn v5_and_current_progress_saves_require_their_tutorial_and_chosen_tails() {
        let p = Progress {
            land: 2,
            tutorial_done: true,
            ..Default::default()
        };
        for version in [5_u16, VERSION] {
            let mut bytes = save_progress(&p);
            bytes[4..6].copy_from_slice(&version.to_le_bytes());
            assert_eq!(load_progress(&bytes).unwrap(), p);
            // chosen=None and tutorial_done=true each occupy one mandatory byte.
            for cut in [1, 2] {
                assert_eq!(
                    load_progress(&bytes[..bytes.len() - cut]).err(),
                    Some(LoadError::Corrupt),
                    "version {version}, missing {cut} bytes"
                );
            }
        }
    }
}
