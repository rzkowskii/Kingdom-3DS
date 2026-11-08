//! The two authored one-shot Playground bursts used by the PC Boulder.
//!
//! Game events supply the impact position and yaw. Particle randomness and
//! clocks belong to the scene, independent of gameplay RNG and persistence.

use game::{
    data::{BoulderEmitterDef, BoulderGradientDef, SpriteId, WorldData},
    rng::Rng,
};

const MAX_PARTICLES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoulderParticleKind {
    Dust,
    Fragments,
}

impl BoulderParticleKind {
    fn emitter(self, world: &WorldData) -> Option<&BoulderEmitterDef> {
        let effect = world.boulder_fx.as_ref()?;
        Some(match self {
            Self::Dust => &effect.dust,
            Self::Fragments => &effect.fragments,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoulderParticleSample {
    pub kind: BoulderParticleKind,
    pub sprite: SpriteId,
    pub x: f32,
    pub y: f32,
    /// Painter depth relative to the boulder's game layer.
    pub z: f32,
    /// Scale of the extracted eight-pixel tile at 32 pixels per world unit.
    pub scale: f32,
    /// Screen-space rotation in radians.
    pub angle: f32,
    pub color: [f32; 4],
}

#[derive(Clone, Debug)]
struct Particle {
    kind: BoulderParticleKind,
    position: [f32; 3],
    velocity: [f32; 2],
    gravity: [f32; 2],
    damping: f32,
    lifetime: f32,
    age: f32,
    angle: f32,
    spin: f32,
    row: usize,
    gradient: BoulderGradientDef,
}

#[derive(Clone, Debug)]
pub struct BoulderParticles {
    particles: Vec<Particle>,
    rng: Rng,
}

impl Default for BoulderParticles {
    fn default() -> Self {
        Self {
            particles: Vec::new(),
            rng: Rng::new(0x424f_554c),
        }
    }
}

impl BoulderParticles {
    pub fn spawn(
        &mut self,
        world: &WorldData,
        kind: BoulderParticleKind,
        x: f32,
        y: f32,
        direction: i8,
    ) {
        let Some(emitter) = kind.emitter(world) else {
            return;
        };
        if !x.is_finite()
            || !y.is_finite()
            || emitter.sprites.is_empty()
            || emitter.colors.is_empty()
        {
            return;
        }
        let count = usize::from(emitter.count).min(MAX_PARTICLES);
        let overflow = (self.particles.len() + count).saturating_sub(MAX_PARTICLES);
        self.particles.drain(..overflow);
        let yaw_x = if direction > 0 { 1.0 } else { -1.0 };
        for index in 0..count {
            // PoolSpawner gives the burst world position and identity rotation.
            // HitObject's 0/180-degree Y yaw rotates the local launch velocity.
            // Playground adds source scatter in simulation space afterward.
            let mut position = [x, y, 0.0];
            for (axis, coordinate) in position.iter_mut().enumerate() {
                *coordinate += self
                    .rng
                    .range(emitter.scatter_min[axis], emitter.scatter_max[axis]);
            }
            let velocity = [
                yaw_x
                    * self
                        .rng
                        .range(emitter.velocity_min[0], emitter.velocity_max[0]),
                self.rng
                    .range(emitter.velocity_min[1], emitter.velocity_max[1]),
            ];
            let row = if emitter.animate {
                0
            } else {
                self.rng.range_i32(0, emitter.sprites.len() as i32) as usize
            };
            // Playground increments lifetimeColorId modulo the palette on
            // each birth. The authored 12-particle burst cycles both colors.
            let color = (index + 1) % emitter.colors.len();
            self.particles.push(Particle {
                kind,
                position,
                velocity,
                gravity: emitter.gravity,
                damping: emitter.damping,
                lifetime: self.rng.range(emitter.lifetime[0], emitter.lifetime[1]),
                age: 0.0,
                angle: self
                    .rng
                    .range(emitter.rotation[0], emitter.rotation[1])
                    .to_radians(),
                spin: self
                    .rng
                    .range(emitter.spin[0], emitter.spin[1])
                    .to_radians(),
                row,
                gradient: emitter.colors[color],
            });
        }
    }

    pub fn advance(&mut self, dt: f32) {
        if !dt.is_finite() || dt <= 0.0 {
            return;
        }
        self.particles.retain_mut(|particle| {
            particle.age += dt;
            if particle.age >= particle.lifetime {
                return false;
            }
            // Playground applies gravity, lerps velocity toward zero, and
            // moves by the new velocity. These emitters have collision off.
            let retain = (1.0 - particle.damping * dt).clamp(0.0, 1.0);
            for axis in 0..2 {
                particle.velocity[axis] =
                    (particle.velocity[axis] - particle.gravity[axis] * dt) * retain;
                particle.position[axis] += particle.velocity[axis] * dt;
            }
            particle.angle += particle.spin * dt;
            true
        });
    }

    pub fn samples<'a>(
        &'a self,
        world: &'a WorldData,
    ) -> impl Iterator<Item = BoulderParticleSample> + 'a {
        self.particles.iter().filter_map(|particle| {
            let emitter = particle.kind.emitter(world)?;
            let life = (particle.age / particle.lifetime).clamp(0.0, 1.0);
            let sprite_index = if emitter.animate {
                ((life * emitter.sprites.len() as f32) as usize)
                    .min(emitter.sprites.len().saturating_sub(1))
            } else {
                particle.row
            };
            let mut color = particle.gradient.color;
            let fade = particle.gradient.fade_start;
            if life > fade {
                color[3] *= (1.0 - life) / (1.0 - fade);
            }
            let [x, y, z] = particle.position;
            Some(BoulderParticleSample {
                kind: particle.kind,
                sprite: *emitter.sprites.get(sprite_index)?,
                x,
                y,
                z,
                scale: emitter.size / 0.25,
                angle: particle.angle,
                color,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use game::data::BoulderFxDef;

    use super::*;

    fn fixture() -> WorldData {
        let dust = BoulderEmitterDef {
            count: 12,
            sprites: vec![0, 1, 2, 3],
            animate: true,
            size: 0.25,
            lifetime: [1.0; 2],
            scatter_min: [-0.125, -0.125, -0.1],
            scatter_max: [0.125, 0.125, -0.1],
            velocity_min: [1.0, 2.0],
            velocity_max: [5.0, 4.0],
            gravity: [0.0, 8.0],
            damping: 0.5,
            rotation: [0.0; 2],
            spin: [0.0; 2],
            colors: vec![
                BoulderGradientDef {
                    color: [0.5, 0.4, 0.3, 1.0],
                    fade_start: 0.75,
                },
                BoulderGradientDef {
                    color: [0.6, 0.5, 0.4, 1.0],
                    fade_start: 0.75,
                },
            ],
        };
        let fragments = BoulderEmitterDef {
            count: 4,
            sprites: vec![4, 5],
            animate: false,
            rotation: [100.0; 2],
            spin: [180.0; 2],
            ..dust.clone()
        };
        WorldData {
            boulder_fx: Some(BoulderFxDef { dust, fragments }),
            sprites: (0..6).map(|i| format!("tile_{i}")).collect(),
            ..WorldData::default()
        }
    }

    #[test]
    fn dust_advances_atlas_and_fades_while_fragments_keep_their_row_and_spin() {
        let world = fixture();
        let mut fx = BoulderParticles::default();
        fx.spawn(&world, BoulderParticleKind::Dust, 2.0, 1.0, 1);
        fx.spawn(&world, BoulderParticleKind::Fragments, 2.0, 1.0, 1);
        let at_birth: Vec<_> = fx.samples(&world).collect();
        assert_eq!(at_birth.len(), 16);
        let dust_colors = &world.boulder_fx.as_ref().unwrap().dust.colors;
        assert!(at_birth[..12]
            .iter()
            .enumerate()
            .all(|(i, s)| s.color == dust_colors[(i + 1) % 2].color));
        assert!(at_birth[..12]
            .iter()
            .all(|s| s.kind == BoulderParticleKind::Dust && s.sprite == 0 && s.angle == 0.0));
        assert!(at_birth[12..]
            .iter()
            .all(|s| s.kind == BoulderParticleKind::Fragments && matches!(s.sprite, 4 | 5)));
        fx.advance(0.25);
        let advanced: Vec<_> = fx.samples(&world).collect();
        assert!(advanced[..12]
            .iter()
            .all(|s| s.sprite == 1 && s.color[3] == 1.0));
        for (before, after) in at_birth[12..].iter().zip(&advanced[12..]) {
            assert_eq!(before.sprite, after.sprite);
            assert!((after.angle - before.angle - 45.0_f32.to_radians()).abs() < 1e-6);
        }
        fx.advance(0.625);
        assert!(fx.samples(&world).all(|s| s.color[3] == 0.5));
        fx.advance(0.125);
        assert_eq!(fx.samples(&world).count(), 0);
    }

    #[test]
    fn world_scatter_stays_put_and_yaw_mirrors_only_the_local_launch_velocity() {
        let world = fixture();
        let mut right = BoulderParticles::default();
        let mut left = BoulderParticles::default();
        right.spawn(&world, BoulderParticleKind::Fragments, 0.0, 0.0, 1);
        left.spawn(&world, BoulderParticleKind::Fragments, 0.0, 0.0, -1);
        assert_eq!(
            right.samples(&world).collect::<Vec<_>>(),
            left.samples(&world).collect::<Vec<_>>()
        );
        let before = right.particles[0].clone();
        let dt = game::state::TICK;
        right.advance(dt);
        left.advance(dt);
        let r = right.samples(&world).next().unwrap();
        let l = left.samples(&world).next().unwrap();
        assert!(r.x > before.position[0] && l.x < before.position[0]);
        assert!((r.x + l.x - 2.0 * before.position[0]).abs() < 1e-6);
        assert_eq!(r.y, l.y);
        assert_eq!((r.z, l.z), (-0.1, -0.1));
        assert!(
            (r.y - before.position[1] - (before.velocity[1] - 8.0 * dt) * (1.0 - 0.5 * dt) * dt)
                .abs()
                < 1e-6
        );
    }
}
