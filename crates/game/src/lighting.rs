//! Lighting state blended from presets over time.
//!
//! orig: `Blender`, `BlenderStateFader`, `BlenderState.BlendTo()`.

use alloc::vec::Vec;

use crate::data::{CelestialDef, LightPreset, WorldData, LIGHT_COLORS, LIGHT_FLOATS};

/// Indices into [`Light::colors`] (orig: `BlenderState.ColorField`).
pub mod color {
    pub const AMBIENT: usize = 0;
    pub const ADDITIVE: usize = 1;
    pub const HAZE: usize = 2;
    pub const FILL_TOP: usize = 3;
    pub const FILL_BOTTOM: usize = 4;
    pub const SKY_TOP: usize = 5;
    pub const SKY_BOTTOM: usize = 6;
    pub const WATER: usize = 7;
    pub const SUN_TINT: usize = 8;
    pub const SUN_GLOW: usize = 9;
    pub const MOON_TINT: usize = 10;
    pub const MOON_GLOW: usize = 11;
    pub const CLOUD_BASE: usize = 12;
    pub const CLOUD_HIGHLIGHT: usize = 13;
    pub const CLOUD_STRATUS: usize = 14;
    pub const FOG: usize = 15;
    pub const MIST: usize = 16;
}

/// Indices into [`Light::floats`] (orig: `BlenderState.FloatField`).
pub mod float {
    pub const CLOUD_DENSITY: usize = 0;
    pub const CLOUD_STRATUS_DENSITY: usize = 1;
    pub const FOG_DENSITY: usize = 2;
    pub const MIST_DENSITY: usize = 3;
    pub const STARS: usize = 5;
    pub const RAIN: usize = 6;
    pub const SNOW: usize = 7;
}

/// The current blended lighting. orig: `Blender.state`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Light {
    pub colors: [[f32; 4]; LIGHT_COLORS.len()],
    pub floats: [f32; LIGHT_FLOATS.len()],
}

impl Light {
    /// orig: `BlenderState.BlendTo()`; disabled fields of `p` are left alone.
    fn blend_to(&mut self, p: &LightPreset, w: f32) {
        for (i, c) in self.colors.iter_mut().enumerate() {
            if p.enabled & (1 << i) != 0 {
                for (ch, target) in c.iter_mut().zip(p.colors[i]) {
                    *ch += (target - *ch) * w;
                }
            }
        }
        for (j, f) in self.floats.iter_mut().enumerate() {
            if p.enabled & (1 << (LIGHT_COLORS.len() + j)) != 0 {
                *f += (p.floats[j] - *f) * w;
            }
        }
    }

    /// Perceived brightness of the ambient light. orig: `Color.grayscale`.
    pub fn ambient_grayscale(&self) -> f32 {
        let [r, g, b, _] = self.colors[color::AMBIENT];
        0.299 * r + 0.587 * g + 0.114 * b
    }
}

/// orig: `CelestialBody.CircularInterval()`: progress through [left, right]
/// on a 24-hour circle; 1..2 while outside it.
fn circular_interval(t: f32, left: f32, right: f32, divisor: f32) -> f32 {
    if left < t && t < right {
        (t - left) / (right - left)
    } else {
        (t + divisor - right) % divisor / (left + divisor - right) + 1.0
    }
}

/// Progress along the body's half-ellipse arc as an angle: 0 at rise, π at
/// set, past the ends while below the horizon. orig: `CelestialBody.Update()`.
fn celestial_angle(c: &CelestialDef, time: f32) -> f32 {
    core::f32::consts::PI
        * if c.rises_at < c.sets_at {
            circular_interval(time, c.rises_at, c.sets_at, 24.0)
        } else {
            1.0 + circular_interval(time, c.sets_at, c.rises_at, 24.0)
        }
}

/// Sun/moon position relative to the camera-fixed Heaven layer, in units.
/// Above the horizon between rise and set, below it otherwise.
/// orig: `CelestialBody.Update()`.
pub fn celestial_position(c: &CelestialDef, time: f32) -> (f32, f32) {
    let f = celestial_angle(c, time);
    (
        -libm::cosf(f) * c.orbit_scale.0 + c.orbit_center.0,
        libm::sinf(f) * c.orbit_scale.1 + c.orbit_center.1,
    )
}

/// How far the body has climbed above the horizon: 1 while up, 0 while down,
/// fading over the last [`GLOW_HORIZON_FADE`] of the arc's height. The
/// original's glow shows only while its body is up; without the fade a set
/// sun's wide halo keeps painting the sky above the horizon after dark.
pub fn celestial_up(c: &CelestialDef, time: f32) -> f32 {
    (libm::sinf(celestial_angle(c, time)) / GLOW_HORIZON_FADE).clamp(0.0, 1.0)
}

/// Height of the rise/set fade band of [`celestial_up`], in arc height
/// (`sin(f)`): full glow a little above the horizon, none below it.
const GLOW_HORIZON_FADE: f32 = 0.1;

/// orig: `BlenderStateFader`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug)]
struct Fader {
    preset: u16,
    weight: f32,
    start: f32,
    target: f32,
    duration: f32,
    delay: f32,
    elapsed: f32,
    animating: bool,
    time_to_live: f32,
    remove_now: bool,
}

impl Fader {
    fn new(preset: u16, weight: f32, duration: f32, target: f32) -> Self {
        let mut f = Fader {
            preset,
            weight,
            start: weight,
            target,
            duration: 0.0,
            delay: 0.0,
            elapsed: 0.0,
            animating: false,
            time_to_live: -1.0,
            remove_now: false,
        };
        if duration > 0.0 {
            f.tween_to(target, duration, 0.0, false);
        } else {
            f.weight = target;
        }
        f
    }

    fn tween_to(&mut self, target: f32, duration: f32, delay: f32, remove_after: bool) {
        if duration > 0.0 || delay > 0.0 {
            self.start = self.weight;
            self.target = target;
            self.duration = duration;
            self.delay = delay;
            self.animating = true;
            self.elapsed = 0.0;
            self.remove_now = false;
            self.time_to_live = if remove_after { duration + delay } else { -1.0 };
        } else {
            self.weight = target;
        }
    }

    fn step(&mut self, dt: f32) {
        if self.animating {
            self.elapsed += dt;
            if self.elapsed > self.delay {
                let t = ((self.elapsed - self.delay) / self.duration).clamp(0.0, 1.0);
                self.weight = self.start + (self.target - self.start) * t;
                if self.elapsed > self.delay + self.duration {
                    self.weight = self.target;
                    self.animating = false;
                }
            }
        }
        if !self.remove_now && self.time_to_live > 0.0 {
            self.time_to_live -= dt;
            if self.time_to_live <= 0.0 {
                self.remove_now = true;
            }
        }
    }
}

/// orig: `Blender`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default)]
pub struct Blender {
    pub state: Light,
    stack: Vec<Fader>,
    /// EnemyManager's transient fader reference. Reload looks it up by preset.
    #[cfg_attr(feature = "serde", serde(skip))]
    red_moon_bound: bool,
}

impl Blender {
    pub(crate) fn presets_valid(&self, world: &WorldData) -> bool {
        self.stack
            .iter()
            .all(|f| (f.preset as usize) < world.lights.len())
    }
    /// The presets on the stack and their weights, bottom first (dev tools).
    pub fn presets(&self) -> impl Iterator<Item = (u16, f32)> + '_ {
        self.stack.iter().map(|f| (f.preset, f.weight))
    }

    /// orig: `Blender.Init()`.
    pub fn new(world: &WorldData) -> Self {
        let mut b = Blender::default();
        for &p in &world.director.initial_lights {
            b.add(world, p, 0.0);
        }
        b.update(world, 0.0);
        b.rebind_red_moon(world);
        b
    }

    pub(crate) fn rebind_red_moon(&mut self, world: &WorldData) {
        self.red_moon_bound = world
            .red_moon
            .is_some_and(|preset| self.stack.iter().any(|fader| fader.preset == preset));
    }

    /// orig: EnemyManager.SpawnWave. Restart an active fade from its current weight.
    pub(crate) fn start_red_moon(&mut self, world: &WorldData) {
        let Some(preset) = world.red_moon else { return };
        if self.red_moon_bound {
            if let Some(fader) = self.stack.iter_mut().rfind(|fader| fader.preset == preset) {
                fader.tween_to(1.0, 10.0, 0.0, false);
            }
        } else {
            self.add(world, preset, 10.0);
            self.red_moon_bound = true;
        }
    }

    /// orig: EnemyManager.ClearRedMoon. Release the reference immediately,
    /// while the fader stays on the stack for its ten-second exit.
    pub(crate) fn clear_red_moon(&mut self, world: &WorldData) {
        if core::mem::take(&mut self.red_moon_bound) {
            if let Some(fader) = self
                .stack
                .iter_mut()
                .rfind(|fader| Some(fader.preset) == world.red_moon)
            {
                fader.tween_to(0.0, 10.0, 0.0, true);
            }
        }
    }

    /// Fades `preset` in over `duration` seconds. A preset with a category
    /// replaces the previous one of that category, which fades out over 3 s
    /// once the new one is in. orig: `Blender.AddState()`.
    pub fn add(&mut self, world: &WorldData, preset: u16, duration: f32) {
        let category = world.lights[preset as usize].category;
        let existing = if category != 0 {
            self.stack
                .iter()
                .rposition(|f| world.lights[f.preset as usize].category == category)
        } else {
            None
        };
        match existing {
            None => self.stack.push(Fader::new(preset, 0.0, duration, 1.0)),
            Some(i) if self.stack[i].preset == preset => {
                self.stack[i].tween_to(1.0, duration, 0.0, false)
            }
            Some(i) => {
                self.stack[i].tween_to(0.0, 3.0, duration, true);
                self.stack
                    .insert(i + 1, Fader::new(preset, 0.0, duration, 1.0));
            }
        }
    }

    /// orig: `Blender.Blend()`, run once per frame.
    pub fn update(&mut self, world: &WorldData, dt: f32) {
        for f in &mut self.stack {
            f.step(dt);
        }
        self.stack.retain(|f| !f.remove_now);
        for f in &self.stack {
            self.state
                .blend_to(&world.lights[f.preset as usize], f.weight);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::*;
    use alloc::string::ToString;
    use alloc::vec;

    fn preset(name: &str, ambient: f32) -> LightPreset {
        let mut colors = [[0.0; 4]; 17];
        colors[color::AMBIENT] = [ambient, ambient, ambient, 1.0];
        LightPreset {
            name: name.to_string(),
            category: 1,
            weather: 1,
            season: 1,
            part_of_day: 0,
            test_only: false,
            enabled: 1,
            colors,
            floats: [0.0; 8],
        }
    }

    fn sun() -> CelestialDef {
        CelestialDef {
            sprite: None,
            glows: vec![],
            is_sun: true,
            orbit_scale: (4.0, 3.5),
            orbit_center: (0.0, 0.5),
            rises_at: 6.5,
            sets_at: 21.5,
            z: 60.0,
        }
    }

    #[test]
    fn sun_is_up_at_noon_and_down_at_midnight() {
        let sun = sun();
        let (x, y) = celestial_position(&sun, 14.0);
        assert!(x.abs() < 1e-3 && (y - 4.0).abs() < 1e-3, "{x} {y}");
        assert!(
            celestial_position(&sun, 7.0).0 < 0.0,
            "rises in the east (left)"
        );
        assert!(
            celestial_position(&sun, 2.0).1 < 0.5,
            "below the horizon at night"
        );
    }

    #[test]
    fn the_glow_fades_out_across_the_horizon() {
        let sun = sun();
        assert_eq!(celestial_up(&sun, 14.0), 1.0, "fully up at noon");
        assert_eq!(celestial_up(&sun, 2.0), 0.0, "down at night");
        assert_eq!(celestial_up(&sun, sun.sets_at + 1.0), 0.0, "after set");
        // Part-way through the fade just after rising and just before setting.
        let mid = celestial_up(&sun, sun.rises_at + 0.2);
        assert!(mid > 0.0 && mid < 1.0, "after rise: {mid}");
        let mid = celestial_up(&sun, sun.sets_at - 0.2);
        assert!(mid > 0.0 && mid < 1.0, "before set: {mid}");
        // Symmetric around the horizon.
        let rising = celestial_up(&sun, sun.rises_at + 0.1);
        let setting = celestial_up(&sun, sun.sets_at - 0.1);
        assert!((rising - setting).abs() < 1e-4, "{rising} vs {setting}");
    }

    #[test]
    fn crossfades_between_same_category_presets() {
        let world = WorldData {
            lights: vec![preset("night", 0.2), preset("day", 1.0)],
            director: DirectorDef {
                initial_lights: vec![0],
                ..Default::default()
            },
            ..Default::default()
        };
        let mut b = Blender::new(&world);
        assert!((b.state.colors[color::AMBIENT][0] - 0.2).abs() < 1e-5);
        b.add(&world, 1, 2.0);
        for _ in 0..60 {
            b.update(&world, 1.0 / 60.0);
        }
        let mid = b.state.colors[color::AMBIENT][0];
        assert!(mid > 0.2 && mid < 1.0, "{mid}");
        for _ in 0..(60 * 6) {
            b.update(&world, 1.0 / 60.0);
        }
        assert!((b.state.colors[color::AMBIENT][0] - 1.0).abs() < 1e-3);
        // The old preset is gone 3 s after the new one finished fading in.
        assert_eq!(b.stack.len(), 1);
    }

    #[test]
    fn red_moon_retargets_one_fader_above_weather_and_releases_it_at_dawn() {
        let mut night = preset("night", 0.2);
        night.enabled |= 1 << color::MOON_TINT | 1 << (LIGHT_COLORS.len() + float::STARS);
        night.colors[color::MOON_TINT] = [1.0; 4];
        night.floats[float::STARS] = 1.0;
        let mut red = preset("Red Moon", 0.0);
        red.category = 0;
        red.enabled = 0x0040_6c00;
        red.colors[color::MOON_TINT] = [0.6, 0.2, 0.1, 1.0];
        let mut next = night.clone();
        next.colors[color::AMBIENT] = [0.7, 0.8, 0.9, 1.0];
        next.floats[float::STARS] = 0.8;
        let world = WorldData {
            lights: vec![night, red, next],
            red_moon: Some(1),
            director: DirectorDef {
                initial_lights: vec![0],
                ..Default::default()
            },
            ..Default::default()
        };
        let mut b = Blender::new(&world);
        b.start_red_moon(&world);
        b.update(&world, 4.0);
        assert_eq!(b.presets().collect::<Vec<_>>(), vec![(0, 1.0), (1, 0.4)]);
        b.start_red_moon(&world);
        b.add(&world, 2, 2.0);
        b.update(&world, 5.0);
        assert_eq!(
            b.presets().map(|(id, _)| id).collect::<Vec<_>>(),
            vec![2, 1]
        );
        assert!((b.presets().last().unwrap().1 - 0.7).abs() < 1e-6);
        assert_eq!(
            b.state.colors[color::AMBIENT],
            world.lights[2].colors[color::AMBIENT]
        );
        assert!((b.state.colors[color::MOON_TINT][1] - 0.44).abs() < 1e-6);
        assert!((b.state.floats[float::STARS] - 0.24).abs() < 1e-6);

        b.clear_red_moon(&world);
        b.update(&world, 5.0);
        let fading = b.presets().last().unwrap();
        assert_eq!(fading.0, 1);
        assert!((fading.1 - 0.35).abs() < 1e-6);
        // Another accepted wave during the fade appends a new fader in the
        // live scene, because dawn has released the old reference.
        b.start_red_moon(&world);
        assert_eq!(
            b.presets().collect::<Vec<_>>(),
            vec![(2, 1.0), fading, (1, 0.0)]
        );
        b.update(&world, 5.0);
        assert_eq!(b.presets().collect::<Vec<_>>(), vec![(2, 1.0), (1, 0.5)]);
    }
}
