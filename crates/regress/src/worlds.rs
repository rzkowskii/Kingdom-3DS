//! Synthetic worlds built in code, so scenarios run without the game's
//! files (CI has none). They follow the shapes `state.rs`'s unit tests and
//! `tests/perf.rs` already use: real mechanics, hand-made prototypes.

use game::data::*;
use game::level::{instantiate, Object};
use game::state::GameState;

/// Island ground for synthetic runs: x in [`LEFT`, `LEFT + WIDTH`].
pub const LEFT: f32 = -30.0;
pub const WIDTH: f32 = 60.0;

/// Prototype indices in [`world`].
pub mod proto {
    /// Pay 1: upgrades into [`WALL1`].
    pub const WALL0: u16 = 0;
    /// A wall scaffold: 3 build points, only builders finish it.
    pub const WALL1: u16 = 1;
    /// A hammer shop (stocked directly; not paid for).
    pub const SHOP_HAMMER: u16 = 2;
    /// Pay 3: upgrades into [`WALL1`].
    pub const POST3: u16 = 3;
    /// A big site: 60 build points (the perf bench's).
    pub const SITE: u16 = 4;
}

fn named(name: &str) -> Proto {
    Proto {
        name: name.to_string(),
        tag: "Untagged".to_string(),
        root_z: 0.1,
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

fn upgrade(next: Option<u16>, in_region: bool) -> UpgradeDef {
    UpgradeDef {
        next,
        only_in_buildable_region: in_region,
        requires_stone: false,
        cooldown: 0.0,
    }
}

/// The world every synthetic scenario uses: no level blocks, one day kind,
/// a director that only runs the clock and seasons, and a purse.
pub fn world(starting_coins: i32) -> WorldData {
    let mut wall0 = named("Wall0");
    wall0.payable = Some(payable(1));
    wall0.upgrade = Some(upgrade(Some(proto::WALL1), true));

    let mut wall1 = named("Wall1");
    wall1.payable = Some(payable(3));
    wall1.upgrade = Some(upgrade(None, true));
    wall1.building = Some(BuildingDef {
        build_points: 3,
        auto_build_rate: 0.0,
        scaffold: vec![],
    });

    let mut shop = named("ShopHammer");
    shop.shop = Some(ShopDef {
        tool: ToolKind::Hammer,
        item_placement: (-1.05, 0.25 + game::build::WALK_Y),
        item_spacing: 0.2,
        max_items: 4,
    });

    let mut post3 = named("Post3");
    post3.payable = Some(payable(3));
    post3.upgrade = Some(upgrade(Some(proto::WALL1), true));

    let mut site = named("Site");
    site.building = Some(BuildingDef {
        build_points: 60,
        auto_build_rate: 0.0,
        scaffold: vec![
            ScaffoldModuleDef {
                transform: Affine2::default(),
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
    site.payable = Some(payable(3));
    site.upgrade = Some(upgrade(None, false));

    WorldData {
        layers: vec![LayerDef { ratio: 1.0, z: 0.0 }; 7],
        protos: vec![wall0, wall1, shop, post3, site],
        days: vec![DayDef {
            name: "d".to_string(),
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
        levels: vec![LevelConfigDef {
            name: "regress".to_string(),
            autumn_start: (40, 40),
            winter_start: (60, 60),
            days_before_game_over: -1,
            starting_coins,
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
        // The real purse's shape (open top), as in tests/perf.rs.
        bag: BagDef {
            wall: vec![(-1.0, 1.5), (-1.0, 0.0), (1.0, 0.0), (1.0, 1.5)],
            spill: vec![(-3.0, -3.0), (3.0, -3.0), (3.0, -0.05), (-3.0, -0.05)],
            coin_shape: vec![(-0.114, 0.0), (0.114, 0.0), (0.114, 0.094), (-0.114, 0.094)],
            spawn_interval: 0.05,
            gravity_scale: 1.0,
            friction: 0.4,
            bounciness: 0.1,
            ..Default::default()
        },
        steeds: vec![horse_regular()],
        default_steed: 0,
        ..Default::default()
    }
}

/// The default steed, with `SteedStats::HORSE_REGULAR`'s numbers (Horse
/// Regular in docs/systems.md). Saves and sound need at least one steed.
fn horse_regular() -> SteedDef {
    let h = game::monarch::SteedStats::HORSE_REGULAR;
    SteedDef {
        name: "Horse Regular".to_string(),
        walk_speed: h.walk_speed,
        run_speed: h.run_speed,
        forest_speed_multiplier: h.forest_speed_multiplier,
        walk_stamina_rate: h.walk_stamina_rate,
        run_stamina_rate: h.run_stamina_rate,
        stand_stamina_rate: h.stand_stamina_rate,
        eat_delay: h.eat_delay,
        eat_duration: h.eat_duration,
        rearing_cooldown: h.rearing_cooldown,
        rearing_duration: h.rearing_duration,
        well_fed_duration: h.well_fed_duration,
        tired_duration: h.tired_duration,
        reserve_stamina: h.reserve_stamina,
        reserve_probability: h.reserve_probability,
        does_not_need_grass: h.does_not_need_grass,
        ..Default::default()
    }
}

/// A fresh synthetic island: ground [`LEFT`, `LEFT + WIDTH`], the monarch
/// at x = 0, `coins` in the purse.
pub fn island(world: &WorldData, seed: u32, coins: u32) -> GameState {
    let mut g = GameState::new(world, 0, seed);
    g.level.left = LEFT;
    g.level.width = WIDTH;
    g.monarch.x = 0.0;
    g.wallet.set(coins);
    g
}

/// Places `proto` at `x` on the game layer; `build` starts it as a scaffold.
pub fn place(g: &mut GameState, world: &WorldData, proto: u16, x: f32, build: bool) -> u32 {
    let mut o: Object = instantiate(world, proto, Layer::Game, x, 0.0, 0.1, 1.0);
    if build {
        o.build = Some(0.0);
    }
    let id = g.level.add(o);
    g.mark_payables_dirty();
    id
}

/// Spawns `n` citizens of `role` spread from `x0` by `dx`, rolling their
/// looks on the island's own dice (so the seed varies them).
pub fn spawn(g: &mut GameState, role: Role, n: usize, x0: f32, dx: f32) -> Vec<u32> {
    (0..n)
        .map(|i| g.population.spawn(role, x0 + i as f32 * dx, &mut g.rng))
        .collect()
}

/// Ambient brightness of [`lit_world`]'s presets, by part of the day
/// (`game::time::PartOfDay` order: dawn, day, evening, night). Distinct, so
/// the phase the lighting is in can be read off the ambient light.
pub const LIT_AMBIENT: [f32; 4] = [0.5, 1.0, 0.55, 0.12];

/// [`world`] with a lighting preset for each part of the day (clear
/// weather, every season), so the director and blender run their real
/// schedule. The night preset is the starting light, as Land One starts
/// before dawn.
pub fn lit_world() -> WorldData {
    use game::lighting::{color, float};
    let mut w = world(0);
    w.lights = LIT_AMBIENT
        .iter()
        .enumerate()
        .flat_map(|(part, &a)| {
            [1u8, 2, 4].map(|season| {
                let mut colors = [[0.0; 4]; LIGHT_COLORS.len()];
                // Warmer at dawn and dusk, bluer at night.
                let tint = [
                    [1.0, 0.8, 0.7],
                    [1.0, 1.0, 1.0],
                    [1.0, 0.75, 0.6],
                    [0.6, 0.7, 1.0],
                ][part];
                colors[color::AMBIENT] = [a * tint[0], a * tint[1], a * tint[2], 1.0];
                colors[color::SKY_TOP] = [0.3 * a, 0.5 * a, a, 1.0];
                colors[color::SKY_BOTTOM] = [a, 0.8 * a, 0.6 * a, 1.0];
                let mut floats = [0.0; LIGHT_FLOATS.len()];
                floats[float::STARS] = if part == 3 { 1.0 } else { 0.0 };
                LightPreset {
                    name: format!("Regress {part} {season}"),
                    category: 1,
                    weather: 1,
                    season,
                    part_of_day: part as u8,
                    test_only: false,
                    enabled: (1 << color::AMBIENT)
                        | (1 << color::SKY_TOP)
                        | (1 << color::SKY_BOTTOM)
                        | (1 << (LIGHT_COLORS.len() + float::STARS)),
                    colors,
                    floats,
                }
            })
        })
        .collect();
    // The summer night preset (part 3, first season).
    w.director.initial_lights = vec![3 * 3];
    w
}
