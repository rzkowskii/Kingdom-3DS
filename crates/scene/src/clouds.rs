//! Camera-relative CloudController decks and World's wind, on the scene clock.
use game::data::{CloudLayerDef, SpriteId, WorldData};
use game::lighting::{color, float, Light};
use game::rng::Rng;

use crate::render::Color;

const APPEAR_COOLDOWN: f32 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloudKind {
    Stratus,
    Cumulus,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CloudSample {
    pub kind: CloudKind,
    pub sprite: SpriteId,
    /// Camera-relative X, world Y and Z.
    pub position: [f32; 3],
    pub scale: [f32; 2],
    pub flip: bool,
    /// Base sprite color times the authored ambient.
    pub tint: Color,
    pub light: Color,
}

#[derive(Clone, Debug)]
struct Cloud {
    sprite: SpriteId,
    flip: bool,
    x: f32,
    y: f32,
    z: f32,
    scale: [f32; 2],
    parallax: f32,
    age: f32,
    alpha: f32,
    active: bool,
    alive: bool,
}

fn snap(value: f32) -> f32 {
    (value * 32.0).round_ties_even() / 32.0
}

impl Cloud {
    fn new(def: &CloudLayerDef, rng: &mut Rng) -> Self {
        // RandomizeSprite.Awake: choose the flip before the sprite. The
        // zero-probability deactivate check still consumes a random draw.
        let flip = def.allow_flip && rng.value() < 0.5;
        let sprite = def.sprites[rng.range_i32(0, def.sprites.len() as i32) as usize];
        rng.value();
        let mut cloud = Self {
            sprite,
            flip,
            x: 0.0,
            y: 0.0,
            z: 0.0,
            scale: [1.0; 2],
            parallax: 0.0,
            age: 0.0,
            alpha: 0.0,
            active: def.prefab_active,
            alive: false,
        };
        cloud.place(def, rng);
        cloud
    }

    fn place(&mut self, def: &CloudLayerDef, rng: &mut Rng) {
        let depth_fraction = rng.value();
        self.x = snap(rng.range(-def.width, def.width));
        self.y = snap(def.y_curve.eval(depth_fraction));
        self.z = def.depth * depth_fraction;
        self.scale = [def.scale_curve.eval(depth_fraction), 1.0];
        self.parallax = def.parallax_curve.eval(self.z / def.depth);
    }

    fn fade_in(&mut self, def: &CloudLayerDef, rng: &mut Rng) {
        // Repositioning reuses the prefab's chosen sprite and renderer flip.
        self.place(def, rng);
        self.age = 0.0;
        self.active = true;
        self.alive = true;
    }

    fn fade_out(&mut self, def: &CloudLayerDef) {
        self.age = self.age.max(def.lifetime - def.fade_time);
        self.alive = false;
    }

    fn step(&mut self, def: &CloudLayerDef, dt: f32, wind: f32, camera_delta: f32) {
        self.age += dt;
        self.alpha = if self.age < def.fade_time {
            self.age / def.fade_time
        } else if self.age < def.lifetime - def.fade_time {
            1.0
        } else if self.age < def.lifetime {
            self.alpha.min((def.lifetime - self.age) / def.fade_time)
        } else {
            self.active = false;
            0.0
        };
        self.y += (0.5 - self.age / def.lifetime) * def.vertical_motion * dt;
        self.x += def.wind_multiplier * wind * dt * self.parallax * 0.75;
        self.x -= self.parallax * camera_delta;
        if self.x < -def.width {
            self.x += 2.0 * def.width;
        } else if self.x > def.width {
            self.x -= 2.0 * def.width;
        }
        let scale = def.scale_curve.eval(self.z / def.depth);
        self.scale = [scale; 2];
    }
}

#[derive(Clone, Debug, Default)]
struct Deck {
    clouds: Vec<Cloud>,
    alive: usize,
    timeout: f32,
}

impl Deck {
    fn step(
        &mut self,
        def: &CloudLayerDef,
        density: f32,
        dt: f32,
        wind: f32,
        camera_delta: f32,
        rng: &mut Rng,
    ) {
        let target = (f32::from(def.max_clouds) * density.clamp(0.0, 1.0)) as i32;
        let mut surplus = self.alive as i32 - target;
        self.timeout -= dt;
        self.alive = 0;
        for cloud in &mut self.clouds {
            if cloud.active {
                if cloud.alive {
                    if self.timeout < 0.0 && surplus > 0 {
                        self.timeout = APPEAR_COOLDOWN;
                        cloud.fade_out(def);
                        surplus -= 1;
                    } else {
                        self.alive += 1;
                    }
                }
                cloud.step(def, dt, wind, camera_delta);
            }
            if !cloud.alive && self.timeout < 0.0 && surplus < 0 {
                self.timeout = APPEAR_COOLDOWN;
                cloud.fade_in(def, rng);
                self.alive += 1;
                surplus += 1;
            }
        }
        if self.timeout < 0.0 && surplus < 0 {
            self.timeout = APPEAR_COOLDOWN;
            if let Some(cloud) = self.clouds.iter_mut().find(|cloud| !cloud.active) {
                cloud.fade_in(def, rng);
                self.alive += 1;
            } else {
                // CreateCloud only positions a new prefab. A later eligible
                // update calls FadeIn, so creation doesn't count as alive.
                self.clouds.push(Cloud::new(def, rng));
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
struct Wind {
    value: f32,
    target: f32,
    timer: f32,
}

impl Wind {
    fn step(&mut self, dt: f32, cooldown: f32, rng: &mut Rng) {
        let change = 0.05 * dt;
        self.value = self.target.clamp(self.value - change, self.value + change);
        if self.timer <= 0.0 {
            let range = if rng.value() < 0.8 { 1.0 } else { 0.2 };
            self.target = rng.range(-range, range);
            self.timer = cooldown * rng.range(1.0, 1.5);
        } else {
            self.timer -= dt;
        }
    }
}

#[derive(Clone, Debug)]
pub struct Clouds {
    stratus: Deck,
    cumulus: Deck,
    wind: Wind,
    rng: Rng,
    last_camera_x: f32,
}

impl Clouds {
    pub fn new(seed: u32, camera_x: f32) -> Self {
        Self {
            stratus: Deck::default(),
            cumulus: Deck::default(),
            wind: Wind::default(),
            rng: Rng::new(seed ^ 0xe91b_2a4d),
            last_camera_x: snap(camera_x),
        }
    }

    pub(crate) fn step(&mut self, world: &WorldData, light: &Light, dt: f32, camera_x: f32) {
        let Some(def) = &world.clouds else { return };
        if !dt.is_finite() || dt <= 0.0 || !camera_x.is_finite() {
            return;
        }
        self.wind.step(dt, def.wind_change_cooldown, &mut self.rng);
        let camera_x = snap(camera_x);
        let delta = camera_x - self.last_camera_x;
        self.last_camera_x = camera_x;
        self.stratus.step(
            &def.stratus,
            light.floats[float::CLOUD_STRATUS_DENSITY],
            dt,
            self.wind.value,
            delta,
            &mut self.rng,
        );
        self.cumulus.step(
            &def.cumulus,
            light.floats[float::CLOUD_DENSITY],
            dt,
            self.wind.value,
            delta,
            &mut self.rng,
        );
    }

    pub fn samples<'a>(
        &'a self,
        world: &'a WorldData,
        state: &'a Light,
    ) -> impl Iterator<Item = CloudSample> + 'a {
        world
            .clouds
            .iter()
            .flat_map(move |def| {
                [
                    (
                        &self.stratus,
                        &def.stratus,
                        CloudKind::Stratus,
                        state.colors[color::CLOUD_STRATUS],
                        // Stratus has no ShadowCamera to bind the depth comparison.
                        [0.0; 4],
                    ),
                    (
                        &self.cumulus,
                        &def.cumulus,
                        CloudKind::Cumulus,
                        state.colors[color::CLOUD_BASE],
                        state.colors[color::CLOUD_HIGHLIGHT],
                    ),
                ]
            })
            .flat_map(move |(deck, def, kind, base, light)| {
                let ambient = state.colors[color::AMBIENT];
                deck.clouds
                    .iter()
                    .filter(move |cloud| cloud.active && cloud.alpha > 0.0 && base[3] > 0.0)
                    .map(move |cloud| CloudSample {
                        kind,
                        sprite: cloud.sprite,
                        position: [
                            snap(def.origin[0] + cloud.x),
                            snap(def.origin[1] + cloud.y),
                            def.origin[2] + cloud.z,
                        ],
                        scale: cloud.scale,
                        flip: cloud.flip,
                        tint: Color::new(
                            base[0] * ambient[0],
                            base[1] * ambient[1],
                            base[2] * ambient[2],
                            base[3] * cloud.alpha,
                        ),
                        light: Color::rgba(light),
                    })
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use game::data::{CloudsDef, Curve};

    fn constant(value: f32) -> Curve {
        Curve {
            keys: vec![[0.0, value, 0.0, 0.0]],
        }
    }

    fn layer() -> CloudLayerDef {
        CloudLayerDef {
            origin: [0.0, 3.5, 50.0],
            depth: 2.0,
            width: 16.0,
            wind_multiplier: 1.0,
            lifetime: 3600.0,
            fade_time: 8.0,
            vertical_motion: 0.0,
            max_clouds: 10,
            sprites: vec![1, 2],
            prefab_active: true,
            allow_flip: true,
            y_curve: constant(-0.75),
            scale_curve: constant(1.25),
            parallax_curve: constant(0.25),
        }
    }

    #[test]
    fn density_uses_truncated_counts_and_strict_half_second_cooldown() {
        let def = layer();
        let mut deck = Deck::default();
        let mut rng = Rng::new(23);
        let mut step = |deck: &mut Deck, density, dt| {
            deck.step(&def, density, dt, 0.0, 0.0, &mut rng);
        };
        step(&mut deck, 0.09, 0.25);
        assert!(deck.clouds.is_empty());
        step(&mut deck, 0.19, 0.25);
        assert_eq!((deck.clouds.len(), deck.alive), (1, 0));
        let variant = (deck.clouds[0].sprite, deck.clouds[0].flip);
        step(&mut deck, 0.19, 0.5);
        assert_eq!(deck.alive, 0);
        step(&mut deck, 0.19, 0.25);
        assert_eq!((deck.clouds.len(), deck.alive), (1, 1));
        assert_eq!(deck.clouds[0].age, 0.0);
        step(&mut deck, 0.0, 0.75);
        assert_eq!(deck.alive, 0);
        assert_eq!(deck.clouds[0].age, 3592.75);
        step(&mut deck, 0.19, 0.75);
        assert_eq!((deck.clouds.len(), deck.alive), (1, 1));
        assert_eq!((deck.clouds[0].sprite, deck.clouds[0].flip), variant);
        assert_eq!(deck.clouds[0].age, 0.0);
    }

    #[test]
    fn lifetime_fades_and_preserves_a_partly_visible_cloud_on_early_removal() {
        let def = layer();
        let mut cloud = Cloud::new(&def, &mut Rng::new(8));
        assert_eq!(cloud.scale, [1.25, 1.0]);
        cloud.step(&def, 2.0, 0.0, 0.0);
        assert_eq!(cloud.alpha, 0.25);
        assert_eq!(cloud.scale, [1.25; 2]);
        cloud.step(&def, 6.0, 0.0, 0.0);
        assert_eq!(cloud.alpha, 1.0);
        cloud.fade_out(&def);
        cloud.step(&def, 4.0, 0.0, 0.0);
        assert_eq!(cloud.alpha, 0.5);
        cloud.step(&def, 4.0, 0.0, 0.0);
        assert!(!cloud.active);
        assert_eq!(cloud.alpha, 0.0);

        cloud.fade_in(&def, &mut Rng::new(9));
        cloud.step(&def, 2.0, 0.0, 0.0);
        cloud.fade_out(&def);
        cloud.step(&def, 1.0, 0.0, 0.0);
        assert_eq!(cloud.alpha, 0.25);
    }

    #[test]
    fn movement_applies_wind_and_camera_parallax_then_wraps_strictly() {
        let def = layer();
        let mut cloud = Cloud::new(&def, &mut Rng::new(9));
        cloud.x = 16.0;
        cloud.step(&def, 1.0, 0.0, 0.0);
        assert_eq!(cloud.x, 16.0);
        cloud.step(&def, 1.0, 1.0, -0.25);
        assert_eq!(cloud.x, -15.75);
        cloud.step(&def, 1.0, -1.0, 0.5);
        assert_eq!(cloud.x, 15.9375);
        assert_eq!(cloud.y, -0.75);

        let mut wind = Wind::default();
        let mut rng = Rng::new(15);
        wind.step(1.0, 60.0, &mut rng);
        assert_eq!(wind.value, 0.0);
        assert!((60.0..=90.0).contains(&wind.timer));
        let target = wind.target;
        assert!((-1.0..=1.0).contains(&target));
        wind.step(1.0, 60.0, &mut rng);
        assert_eq!(wind.target, target);
        assert_eq!(wind.value, target.clamp(-0.05, 0.05));
    }

    #[test]
    fn samples_keep_geometry_while_applying_separate_layer_tints() {
        let stratus = layer();
        let cumulus = CloudLayerDef {
            origin: [0.0, 5.5, 42.0],
            sprites: vec![3],
            ..stratus.clone()
        };
        let world = WorldData {
            clouds: Some(CloudsDef {
                stratus,
                cumulus,
                wind_change_cooldown: 60.0,
            }),
            ..Default::default()
        };
        let mut light = Light::default();
        light.colors[color::AMBIENT] = [0.5, 0.25, 0.125, 1.0];
        light.colors[color::CLOUD_BASE] = [0.5, 0.5, 0.5, 1.0];
        light.colors[color::CLOUD_STRATUS] = [0.25, 0.5, 1.0, 0.5];
        light.colors[color::CLOUD_HIGHLIGHT] = [0.5, 1.0, 0.5, 0.25];
        light.floats[float::CLOUD_STRATUS_DENSITY] = 0.1;
        light.floats[float::CLOUD_DENSITY] = 0.1;
        let mut clouds = Clouds::new(10, 8.0);
        for _ in 0..3 {
            clouds.step(&world, &light, 1.0, 8.0);
        }
        let before: Vec<_> = clouds.samples(&world, &light).collect();
        assert_eq!(before.len(), 2);
        assert_eq!(before[0].kind, CloudKind::Stratus);
        assert_eq!(before[1].kind, CloudKind::Cumulus);
        assert_eq!(before[0].position[1], 2.75);
        assert_eq!(before[1].position[1], 4.75);
        assert_eq!(before[0].tint, Color::new(0.125, 0.125, 0.125, 0.0625));
        assert_eq!(before[1].tint.a, 0.125);

        light.colors[color::CLOUD_STRATUS] = [0.75, 0.25, 0.25, 1.0];
        light.colors[color::CLOUD_HIGHLIGHT] = [0.375, 0.125, 0.125, 0.5];
        let red: Vec<_> = clouds.samples(&world, &light).collect();
        assert_eq!(red[0].tint, Color::new(0.375, 0.0625, 0.03125, 0.125));
        assert_eq!(red[0].light, Color::rgba([0.0; 4]));
        assert_eq!(red[1].tint, before[1].tint);
        assert_eq!(red[1].light, Color::new(0.375, 0.125, 0.125, 0.5));
        for (a, b) in red.iter().zip(before) {
            assert_eq!(
                (a.sprite, a.position, a.scale, a.flip),
                (b.sprite, b.position, b.scale, b.flip)
            );
        }
    }
}
