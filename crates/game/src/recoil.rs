//! Bounded three-body catapult physics: chassis, two free wheel hinges and a
//! flat static ground edge. Positions are centers of mass relative to the
//! object's current origin. The chassis polygon is a trigger, so only wheels
//! contact the ground. No wheel pivot is pinned and no recoil spring is used.
//!
//! This is a specialized, altered Rust implementation of the sequential impulse
//! equations in Box2D 2.3.1 (revolute joints, single-point contacts and islands),
//! validated against that engine, not a claim of bitwise Unity solver parity.
//!
//! Original Box2D notice:
//! ```text
//! Copyright (c) 2006-2013 Erin Catto http://www.gphysics.com
//!
//! This software is provided 'as-is', without any express or implied warranty.
//! In no event will the authors be held liable for any damages arising from
//! the use of this software.
//!
//! Permission is granted to anyone to use this software for any purpose,
//! including commercial applications, and to alter it and redistribute it
//! freely, subject to the following restrictions:
//! 1. The origin of this software must not be misrepresented; you must not
//! claim that you wrote the original software. If you use this software in a
//! product, an acknowledgment in the product documentation would be appreciated
//! but is not required.
//! 2. Altered source versions must be plainly marked as such, and must not be
//! misrepresented as being the original software.
//! 3. This notice may not be removed or altered from any source distribution.
//! ```

use crate::data::Affine2;

// Box2D's linear convergence slop. Unity does not serialize this solver value.
const LINEAR_SLOP: f32 = 0.005;
const MAX_CATCHUP_SECONDS: f32 = 60.0;

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Definition {
    pub body_mass: f32,
    /// Derived polygon inertia about the selected physical wheel COM.
    pub body_inertia: f32,
    pub rear_body_inertia: f32,
    pub wheel_mass: f32,
    pub wheel_radius: f32,
    pub front_anchor: [f32; 2],
    pub rear_anchor: [f32; 2],
    pub gravity: f32,
    pub ground_height: f32,
    pub fixed_step: f32,
    pub max_angular_speed: f32,
    pub max_linear_speed: f32,
    pub body_linear_drag: f32,
    pub body_angular_drag: f32,
    pub wheel_linear_drag: f32,
    pub wheel_angular_drag: f32,
    pub friction: f32,
    pub restitution: f32,
    pub velocity_iterations: u8,
    pub position_iterations: u8,
    pub velocity_threshold: f32,
    pub max_linear_correction: f32,
    pub max_angular_correction: f32,
    pub baumgarte: f32,
    /// Circle/edge skin in this specialization, using the authored Unity contact
    /// offset. Exact Unity native manifold generation remains unmeasured.
    pub contact_offset: f32,
    pub sleep_time: f32,
    pub linear_sleep_tolerance: f32,
    pub angular_sleep_tolerance: f32,
}

impl Default for Definition {
    /// PC reference parameters for tests/manual fixtures. The asset extractor
    /// supplies production values explicitly instead of defaulting missing data.
    fn default() -> Self {
        Self {
            body_mass: 5.0,
            body_inertia: 0.8364514,
            rear_body_inertia: 0.8364514,
            wheel_mass: 1.0,
            wheel_radius: 0.171875,
            front_anchor: [0.421875, 0.171875],
            rear_anchor: [-0.421875, 0.171875],
            gravity: 15.0,
            ground_height: 0.8675,
            fixed_step: 0.02,
            max_angular_speed: core::f32::consts::TAU,
            max_linear_speed: 100.0,
            body_linear_drag: 1.0,
            body_angular_drag: 0.0,
            wheel_linear_drag: 0.0,
            wheel_angular_drag: 0.0,
            friction: 0.4,
            restitution: 0.0,
            velocity_iterations: 8,
            position_iterations: 3,
            velocity_threshold: 1.0,
            max_linear_correction: 0.2,
            max_angular_correction: 8.0 * core::f32::consts::PI / 180.0,
            baumgarte: 0.2,
            contact_offset: 0.01,
            sleep_time: 0.5,
            linear_sleep_tolerance: 0.01,
            angular_sleep_tolerance: 2.0 * core::f32::consts::PI / 180.0,
        }
    }
}

impl Definition {
    pub fn valid(&self) -> bool {
        [
            self.body_mass,
            self.body_inertia,
            self.rear_body_inertia,
            self.wheel_mass,
            self.wheel_radius,
            self.gravity,
            self.max_angular_speed,
            self.max_linear_speed,
            self.max_linear_correction,
            self.max_angular_correction,
            self.sleep_time,
            self.linear_sleep_tolerance,
            self.angular_sleep_tolerance,
        ]
        .iter()
        .all(|v| v.is_finite() && *v > 0.0)
            && [
                self.body_linear_drag,
                self.body_angular_drag,
                self.wheel_linear_drag,
                self.wheel_angular_drag,
                self.friction,
                self.restitution,
                self.velocity_threshold,
                self.contact_offset,
            ]
            .iter()
            .all(|v| v.is_finite() && *v >= 0.0)
            && self.ground_height.is_finite()
            && self.fixed_step.is_finite()
            && (0.001..=0.1).contains(&self.fixed_step)
            && self.baumgarte.is_finite()
            && (0.0..=1.0).contains(&self.baumgarte)
            && (1..=32).contains(&self.velocity_iterations)
            && (1..=16).contains(&self.position_iterations)
            && self.restitution <= 1.0
            && self
                .front_anchor
                .iter()
                .chain(&self.rear_anchor)
                .all(|v| v.is_finite())
            && self.front_anchor[0] > self.rear_anchor[0]
    }
    pub fn center(&self, side: i8) -> [f32; 2] {
        if side > 0 {
            self.front_anchor
        } else {
            self.rear_anchor
        }
    }
    pub fn inertia(&self, side: i8) -> f32 {
        if side > 0 {
            self.body_inertia
        } else {
            self.rear_body_inertia
        }
    }
    fn inverse_masses(&self, side: i8) -> ([f32; 3], [f32; 3]) {
        let wheel_i = 0.5 * self.wheel_mass * self.wheel_radius * self.wheel_radius;
        (
            [
                1.0 / self.body_mass,
                1.0 / self.wheel_mass,
                1.0 / self.wheel_mass,
            ],
            [1.0 / self.inertia(side), 1.0 / wheel_i, 1.0 / wheel_i],
        )
    }
}

/// Uniform polygon inertia about an explicit center, independent of winding.
/// Unity's direction setter moves the chassis center of mass to this center.
pub fn polygon_inertia(vertices: &[[f32; 2]], mass: f32, center: [f32; 2]) -> Option<f32> {
    if vertices.len() < 3 || !mass.is_finite() || mass <= 0.0 {
        return None;
    }
    let mut twice_area = 0.0;
    let mut weighted = 0.0;
    for i in 0..vertices.len() {
        let a = [vertices[i][0] - center[0], vertices[i][1] - center[1]];
        let b = [
            vertices[(i + 1) % vertices.len()][0] - center[0],
            vertices[(i + 1) % vertices.len()][1] - center[1],
        ];
        let cross = a[0] * b[1] - a[1] * b[0];
        twice_area += cross;
        weighted += cross
            * (a[0] * a[0] + a[1] * a[1] + a[0] * b[0] + a[1] * b[1] + b[0] * b[0] + b[1] * b[1]);
    }
    let inertia = mass * weighted / (6.0 * twice_area);
    (inertia.is_finite() && inertia > 0.0).then_some(inertia)
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Body {
    pub position: [f32; 2],
    pub velocity: [f32; 2],
    pub angle: f32,
    pub angular_velocity: f32,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct State {
    pub object: u32,
    pub side: i8,
    /// Chassis, physical front wheel, physical rear wheel.
    pub bodies: [Body; 3],
    pub pending: f32,
    pub ground_y: f32,
    /// Impulses applied to each wheel by its hinge, world coordinates.
    pub joint_impulses: [[f32; 2]; 2],
    /// Ground impulses [normal, tangent] for each wheel.
    pub contact_impulses: [[f32; 2]; 2],
    pub sleep_time: f32,
    pub awake: bool,
}

impl State {
    pub fn new(object: u32, side: i8, d: &Definition) -> Self {
        Self {
            object,
            side,
            bodies: [
                Body {
                    position: d.center(side),
                    ..Body::default()
                },
                Body {
                    position: d.front_anchor,
                    ..Body::default()
                },
                Body {
                    position: d.rear_anchor,
                    ..Body::default()
                },
            ],
            pending: 0.0,
            ground_y: 0.0,
            joint_impulses: [[0.0; 2]; 2],
            contact_impulses: [[0.0; 2]; 2],
            sleep_time: 0.0,
            awake: true,
        }
    }
    pub fn valid(&self, d: &Definition) -> bool {
        matches!(self.side, -1 | 1)
            && d.valid()
            && self.bodies.iter().all(|b| {
                // Saves are untrusted. These generous local/world limits prevent
                // finite-but-overflowing inputs from poisoning constraint math.
                b.position.iter().all(|v| bounded(*v, 1_000_000.0))
                    && b.velocity.iter().all(|v| bounded(*v, 1_000.0))
                    && bounded(b.angle, 1_000_000.0)
                    && bounded(b.angular_velocity, 1_000.0)
            })
            && self.pending.is_finite()
            && self.pending >= 0.0
            && self.pending < d.fixed_step
            && bounded(self.ground_y, 1_000_000.0)
            && self.sleep_time.is_finite()
            && (0.0..=d.sleep_time + d.fixed_step + 1e-5).contains(&self.sleep_time)
            && (self.awake
                || (self.sleep_time >= d.sleep_time
                    && self
                        .bodies
                        .iter()
                        .all(|b| b.velocity == [0.0; 2] && b.angular_velocity == 0.0)))
            && self
                .joint_impulses
                .iter()
                .flatten()
                .chain(self.contact_impulses.iter().flatten())
                .all(|v| bounded(*v, 1_000_000.0))
            && self.contact_impulses.iter().all(|p| p[0] >= 0.0)
    }
    pub fn wake(&mut self) {
        self.awake = true;
        self.sleep_time = 0.0;
    }
    pub fn impact(&mut self, d: &Definition) {
        self.wake();
        self.bodies[0].angular_velocity +=
            -0.5 * self.side as f32 * d.body_mass / d.inertia(self.side);
    }
    /// PC Moving.Update writes only chassis linear velocity, leaving wheel spin
    /// and all angular velocities dynamic.
    pub fn drive(&mut self, vx: f32) {
        if !vx.is_finite() {
            return;
        }
        if vx != 0.0 {
            self.wake();
        }
        self.bodies[0].velocity = [vx, 0.0];
    }
    /// PC entry to Reloading clears chassis linear velocity once.
    pub fn stop(&mut self) {
        self.bodies[0].velocity = [0.0; 2];
    }
    pub fn shift_origin(&mut self, delta: [f32; 2]) {
        for body in &mut self.bodies {
            body.position = sub(body.position, delta);
        }
        self.ground_y -= delta[1];
    }
    pub fn pose(&self, d: &Definition) -> Affine2 {
        body_pose(self.bodies[0], d.center(self.side))
    }
    pub fn wheel_pose(&self, index: usize, d: &Definition) -> Affine2 {
        let i = index.min(1);
        body_pose(
            self.bodies[i + 1],
            if i == 0 {
                d.front_anchor
            } else {
                d.rear_anchor
            },
        )
    }
    /// Simulation calls are small; reject nonfinite/unbounded external catch-up
    /// rather than letting float subtraction produce an endless loop.
    pub fn advance(&mut self, dt: f32, d: &Definition) {
        if !dt.is_finite() || !(0.0..=MAX_CATCHUP_SECONDS).contains(&dt) || !d.valid() {
            return;
        }
        self.pending += dt;
        let steps = libm::floorf(self.pending / d.fixed_step) as u32;
        self.pending -= steps as f32 * d.fixed_step;
        // A rounding ulp at the boundary is not a negative clock.
        self.pending = self
            .pending
            .max(0.0)
            .min(d.fixed_step - f32::EPSILON * d.fixed_step);
        for _ in 0..steps {
            if !self.awake {
                break;
            }
            self.step(d);
        }
    }
    fn step(&mut self, d: &Definition) {
        let h = d.fixed_step;
        let (im, ii) = d.inverse_masses(self.side);
        let center = d.center(self.side);
        let anchors = [d.front_anchor, d.rear_anchor];
        // Like Box2D discrete contact generation, use the pre-step geometry.
        let contact = [
            self.bodies[1].position[1] - self.ground_y <= d.wheel_radius + d.contact_offset,
            self.bodies[2].position[1] - self.ground_y <= d.wheel_radius + d.contact_offset,
        ];
        let mut lever = [0.0; 2];
        let mut bounce = [0.0; 2];
        for (i, b) in self.bodies.iter_mut().enumerate() {
            let linear = if i == 0 {
                d.body_linear_drag
            } else {
                d.wheel_linear_drag
            };
            let angular = if i == 0 {
                d.body_angular_drag
            } else {
                d.wheel_angular_drag
            };
            b.velocity[1] -= h * d.gravity;
            b.velocity = mul(b.velocity, 1.0 / (1.0 + h * linear));
            b.angular_velocity /= 1.0 + h * angular;
        }
        for j in 0..2 {
            let i = j + 1;
            if !contact[j] {
                self.contact_impulses[j] = [0.0; 2];
                continue;
            }
            lever[j] = 0.5
                * (self.ground_y + d.contact_offset - self.bodies[i].position[1] - d.wheel_radius);
            if self.bodies[i].velocity[1] < -d.velocity_threshold {
                bounce[j] = -d.restitution * self.bodies[i].velocity[1];
            }
            let [normal, tangent] = self.contact_impulses[j];
            self.bodies[i].velocity = add(self.bodies[i].velocity, mul([tangent, normal], im[i]));
            self.bodies[i].angular_velocity -= ii[i] * lever[j] * tangent;
        }
        let r = anchors.map(|a| rot(self.bodies[0].angle, sub(a, center)));
        let k = r.map(|r| joint_matrix(im[0] + im[1], ii[0], r));
        // Physical rear then front matches the reference island's joint order.
        for j in [1, 0] {
            self.joint_velocity_impulse(j, r[j], self.joint_impulses[j], &im, &ii);
        }
        for _ in 0..d.velocity_iterations {
            for j in [1, 0] {
                let rel = sub(
                    self.bodies[j + 1].velocity,
                    add(
                        self.bodies[0].velocity,
                        cross_sv(self.bodies[0].angular_velocity, r[j]),
                    ),
                );
                let impulse = solve(k[j], mul(rel, -1.0));
                self.joint_impulses[j] = add(self.joint_impulses[j], impulse);
                self.joint_velocity_impulse(j, r[j], impulse, &im, &ii);
            }
            for j in [1, 0] {
                if !contact[j] {
                    continue;
                }
                let i = j + 1;
                let tangent_speed =
                    self.bodies[i].velocity[0] - self.bodies[i].angular_velocity * lever[j];
                let tangent_mass = 1.0 / (im[i] + ii[i] * lever[j] * lever[j]);
                let old = self.contact_impulses[j][1];
                let cap = d.friction * self.contact_impulses[j][0];
                let new = (old - tangent_mass * tangent_speed).clamp(-cap, cap);
                let delta = new - old;
                self.contact_impulses[j][1] = new;
                self.bodies[i].velocity[0] += im[i] * delta;
                self.bodies[i].angular_velocity -= ii[i] * lever[j] * delta;
                let old = self.contact_impulses[j][0];
                let new = (old - (self.bodies[i].velocity[1] - bounce[j]) / im[i]).max(0.0);
                self.contact_impulses[j][0] = new;
                self.bodies[i].velocity[1] += im[i] * (new - old);
            }
        }
        for b in &mut self.bodies {
            let speed = length(b.velocity);
            if speed > d.max_linear_speed {
                b.velocity = mul(b.velocity, d.max_linear_speed / speed);
            }
            b.angular_velocity = b
                .angular_velocity
                .clamp(-d.max_angular_speed, d.max_angular_speed);
            b.position = add(b.position, mul(b.velocity, h));
            b.angle += h * b.angular_velocity;
        }
        let mut solved = false;
        for _ in 0..d.position_iterations {
            let mut min_separation = 0.0_f32;
            for j in [1, 0] {
                if !contact[j] {
                    continue;
                }
                let i = j + 1;
                let separation =
                    self.bodies[i].position[1] - self.ground_y - d.contact_offset - d.wheel_radius;
                min_separation = min_separation.min(separation);
                let correction =
                    (d.baumgarte * (separation + LINEAR_SLOP)).clamp(-d.max_linear_correction, 0.0);
                self.bodies[i].position[1] -= correction;
            }
            let mut joint_error = 0.0_f32;
            for j in [1, 0] {
                let r = rot(self.bodies[0].angle, sub(anchors[j], center));
                let error = sub(self.bodies[j + 1].position, add(self.bodies[0].position, r));
                joint_error = joint_error.max(length(error));
                let impulse = solve(joint_matrix(im[0] + im[j + 1], ii[0], r), mul(error, -1.0));
                self.bodies[0].position = sub(self.bodies[0].position, mul(impulse, im[0]));
                self.bodies[0].angle -= ii[0] * cross(r, impulse);
                self.bodies[j + 1].position =
                    add(self.bodies[j + 1].position, mul(impulse, im[j + 1]));
            }
            if min_separation >= -3.0 * LINEAR_SLOP && joint_error <= LINEAR_SLOP {
                solved = true;
                break;
            }
        }
        let quiet = self.bodies.iter().all(|b| {
            dot(b.velocity, b.velocity) <= d.linear_sleep_tolerance * d.linear_sleep_tolerance
                && b.angular_velocity.abs() <= d.angular_sleep_tolerance
        });
        if quiet {
            self.sleep_time += h;
        } else {
            self.sleep_time = 0.0;
        }
        if solved && self.sleep_time >= d.sleep_time {
            self.awake = false;
            for b in &mut self.bodies {
                b.velocity = [0.0; 2];
                b.angular_velocity = 0.0;
            }
        }
    }
    fn joint_velocity_impulse(
        &mut self,
        j: usize,
        r: [f32; 2],
        p: [f32; 2],
        im: &[f32; 3],
        ii: &[f32; 3],
    ) {
        self.bodies[0].velocity = sub(self.bodies[0].velocity, mul(p, im[0]));
        self.bodies[0].angular_velocity -= ii[0] * cross(r, p);
        self.bodies[j + 1].velocity = add(self.bodies[j + 1].velocity, mul(p, im[j + 1]));
    }
}

fn bounded(value: f32, limit: f32) -> bool {
    value.is_finite() && value.abs() <= limit
}
fn body_pose(b: Body, local_center: [f32; 2]) -> Affine2 {
    let (s, c) = (libm::sinf(b.angle), libm::cosf(b.angle));
    Affine2 {
        basis: [c, -s, s, c],
        translation: sub(b.position, rot(b.angle, local_center)),
    }
}
fn add(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] + b[0], a[1] + b[1]]
}
fn sub(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] - b[0], a[1] - b[1]]
}
fn mul(a: [f32; 2], b: f32) -> [f32; 2] {
    [a[0] * b, a[1] * b]
}
fn dot(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}
fn cross(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[1] - a[1] * b[0]
}
fn cross_sv(s: f32, v: [f32; 2]) -> [f32; 2] {
    [-s * v[1], s * v[0]]
}
fn length(a: [f32; 2]) -> f32 {
    libm::sqrtf(dot(a, a))
}
fn rot(a: f32, v: [f32; 2]) -> [f32; 2] {
    let (s, c) = (libm::sinf(a), libm::cosf(a));
    [c * v[0] - s * v[1], s * v[0] + c * v[1]]
}
fn joint_matrix(m: f32, i: f32, r: [f32; 2]) -> [f32; 3] {
    [m + i * r[1] * r[1], -i * r[0] * r[1], m + i * r[0] * r[0]]
}
fn solve(k: [f32; 3], b: [f32; 2]) -> [f32; 2] {
    let inv = 1.0 / (k[0] * k[2] - k[1] * k[1]);
    [
        (k[2] * b[0] - k[1] * b[1]) * inv,
        (k[0] * b[1] - k[1] * b[0]) * inv,
    ]
}

#[cfg(test)]
pub(crate) fn fixture() -> Definition {
    Definition::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inertia_integral_uses_real_shape_and_center_not_sprite_rectangle() {
        let square = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
        assert!((polygon_inertia(&square, 3.0, [0.0; 2]).unwrap() - 2.0).abs() < 1e-6);
        assert!((polygon_inertia(&square, 3.0, [1.0, 0.0]).unwrap() - 5.0).abs() < 1e-6);
        assert!(fixture().valid());
    }
    #[test]
    fn torque_changes_chassis_momentum_without_a_pinned_wheel_or_invented_spring() {
        let d = fixture();
        let mut s = State::new(1, 1, &d);
        s.ground_y = -10.0; // no contact: torque must not invent a restoring spring
        s.impact(&d);
        assert!((s.bodies[0].angular_velocity * d.body_inertia + 2.5).abs() < 1e-6);
        s.advance(0.2, &d);
        assert!(s.bodies[1].position[0] != d.front_anchor[0]);
        assert!(s.bodies[0].angle < 0.0);
        let com = s
            .bodies
            .iter()
            .zip([d.body_mass, d.wheel_mass, d.wheel_mass])
            .fold([0.0; 2], |a, (b, m)| add(a, mul(b.position, m)));
        assert!(com.iter().all(|v| v.is_finite()));
        assert!(
            com[1] / (d.body_mass + 2.0 * d.wheel_mass) < 0.0,
            "the free assembly COM must fall; the opposite wheel may rise under torque"
        );
    }
    #[test]
    fn wheels_contact_roll_sleep_and_wake_under_drive_and_recoil() {
        let d = fixture();
        let mut s = State::new(1, 1, &d);
        s.ground_y = d.ground_height - 1.0;
        s.advance(2.0, &d);
        assert!(!s.awake, "settled body must sleep");
        for w in &s.bodies[1..] {
            assert!(w.position[1] >= s.ground_y + d.wheel_radius + d.contact_offset - 0.016);
        }
        let before = s.bodies[0].position;
        for _ in 0..50 {
            s.drive(0.3);
            s.advance(0.02, &d);
        }
        assert!(s.bodies[0].position[0] > before[0] + 0.15);
        assert!(s.bodies[1].angle.abs() > 0.1 && s.bodies[2].angle.abs() > 0.1);
        s.stop();
        s.advance(3.0, &d);
        s.impact(&d);
        s.advance(0.1, &d);
        assert!(s.awake && s.bodies[0].angle < 0.0);
    }
    #[test]
    fn rebase_pause_and_saved_warmstart_resume_preserve_the_trajectory() {
        let d = fixture();
        let mut a = State::new(1, 1, &d);
        a.advance(0.14, &d);
        a.impact(&d);
        a.advance(0.073, &d);
        let paused = a;
        a.advance(0.0, &d);
        assert_eq!(a, paused);
        let old = a.pose(&d);
        let mut b = a;
        b.shift_origin([3.0, -2.0]);
        let moved = b.pose(&d);
        assert!((old.translation[0] - moved.translation[0] - 3.0).abs() < 1e-6);
        a.advance(0.1, &d);
        b.advance(0.1, &d);
        for i in 0..3 {
            assert!((a.bodies[i].position[0] - b.bodies[i].position[0] - 3.0).abs() < 1e-5);
        }
        #[cfg(feature = "save")]
        {
            let mut saved: State =
                postcard::from_bytes(&postcard::to_allocvec(&a).unwrap()).unwrap();
            a.advance(0.23, &d);
            saved.advance(0.23, &d);
            assert_eq!(a, saved);
        }
    }
    /// Independent upstream C++ Box2D 2.3.1, revision
    /// 7e633c4fb86a68bf072fb8ae67ea2c060114750e. Harness and comparison receipt:
    /// review/2026-11-08/catapult-three-body-oracle.{cpp,json}.
    /// Full source geometry/mass data, discrete collision, 8/3 iterations;
    /// initial fall, sleep, impact at step 101, drive 201..250, stop at 251.
    /// Each row records all three COMs, angles, linear/angular velocities.
    /// This constrains a Box2D-equivalent specialization, not Unity native parity.
    #[test]
    fn independent_box2d_oracle_matches_both_directions_drop_recoil_drive_and_coast() {
        // Retain the independent C++ dump's round-trip decimal representation.
        #[allow(clippy::excessive_precision)]
        #[rustfmt::skip]
        const EXPECTED: &[(i8, usize, bool, [f32; 18])] = &[
            (1, 10, true, [0.421559423, 0.022093486, -0.0773347169, -0.00269427896, 0.258482307, -0.385988474, 0.421559423, 0.022093486, 0.0016266543, -0.0025376149, 8.94069672e-08, 0.016387267, -0.419668108, 0.0865005851, 0.00162854407, 0.0200177636, 0.635057867, 0.0203567948]),
            (1, 50, false, [0.421886116, 0.0412320197, -0.00435564248, 0.0, 0.0, 0.0, 0.421886116, 0.0412320197, -0.000509172503, 0.0, 0.0, 0.0, -0.421855867, 0.0447971784, 0.00613104366, 0.0, 0.0, 0.0]),
            (1, 101, true, [0.421819568, 0.0400608815, -0.0341498293, -0.000732264889, -0.0746330246, -1.49993336, 0.421819568, 0.0400608815, -0.000421595527, -0.000734786154, 8.94069672e-08, 0.00437884871, -0.421448857, 0.0685092509, 0.00613104366, 0.00478154793, 1.17599714, -1.82880733e-09]),
            (1, 110, true, [0.420722336, 0.0419500172, -0.147579402, 0.000864037196, 0.000231662911, 0.012347213, 0.420722336, 0.0419500172, 0.00540094962, 0.000864266651, 3.92610673e-08, -0.00514070224, -0.413855821, 0.165934473, 0.00613104366, -0.000670968962, -0.0100262733, -1.82880733e-09]),
            (1, 120, true, [0.421968549, 0.0444693416, 0.00395085057, -0.000299580861, 0.103859186, 0.186985195, 0.421968549, 0.0444693416, -0.00308218691, -0.000303334789, 7.4505806e-09, 0.00180059997, -0.42177543, 0.0414465182, 0.00614640722, -0.000129069915, -9.31322575e-08, 0.000768186117]),
            (1, 201, true, [0.42550537, 0.043932084, -0.000529939309, 0.183104575, 8.39041786e-06, 1.47286109e-05, 0.42550537, 0.043932084, -0.0239919424, 0.183079839, 4.57675924e-08, -1.08237553, -0.418244421, 0.0443638153, -0.0146898441, 0.183155492, 5.32736522e-09, -1.08142817]),
            (1, 250, true, [0.712396204, 0.0442779064, -0.000336627854, 0.294118136, -4.96686567e-08, -5.92099596e-08, 0.712396204, 0.0442779064, -1.71896625, 0.294118136, -4.96686567e-08, -1.7370044, -0.131353721, 0.04455854, -1.70795774, 0.294118136, 2.89743035e-10, -1.73555934]),
            (1, 400, true, [0.865376532, 0.044374086, -0.000282300898, 0.0177916531, 4.98408887e-08, 5.21518331e-08, 0.865376532, 0.044374086, -2.62226033, 0.0177916531, 4.98408887e-08, -0.105043262, 0.021626588, 0.0446122438, -2.6105783, 0.0177916531, 5.83777648e-09, -0.104969457]),
            (-1, 10, true, [-0.421562225, 0.0215555355, 0.0772004724, 0.00295505393, 0.263062298, 0.411556363, 0.419674754, 0.08662875, -0.00162855815, -0.021269545, 0.60946697, -0.0203569718, -0.421557873, 0.0216334015, -0.0017150928, 0.00253804633, 8.94069672e-08, -0.0164163727]),
            (-1, 50, false, [-0.421884805, 0.0411827154, 0.00426541967, 0.0, 0.0, 0.0, 0.421857566, 0.0447816513, -0.00614013802, 0.0, 0.0, 0.0, -0.421884745, 0.0411935635, 0.000409527915, 0.0, 0.0, 0.0]),
            (-1, 101, true, [-0.421808183, 0.0397596769, 0.0340606123, 0.000715649803, -0.0895596445, 1.49993336, 0.421452522, 0.0684927627, -0.00614013802, -0.00468251575, 1.17599761, -1.929928e-09, -0.42187047, 0.0397979096, 0.000323195389, 0.00072426215, 4.47034836e-08, -0.00431662705]),
            (-1, 110, true, [-0.420722306, 0.0418910198, 0.147507697, -0.000863679801, 0.000277961139, -0.0123575898, 0.41386497, 0.165899783, -0.00614013802, 0.00067174877, -0.0100350818, -1.929928e-09, -0.420721442, 0.0418995097, -0.00549199991, -0.000864471192, 5.11063263e-08, 0.00514272787]),
            (-1, 120, true, [-0.421966761, 0.0448000431, -0.00401837705, 0.000299088424, 0.1246325, -0.186987475, 0.421776682, 0.0414095446, -0.0061542308, 0.00011837644, -5.21540642e-08, -0.000704627251, -0.421968907, 0.044647377, 0.00298421062, 0.000313247496, -1.49011612e-08, -0.00185965409]),
            (-1, 201, true, [-0.425504327, 0.0439418741, 0.000492023188, -0.183104828, 9.76648789e-06, -1.46167304e-05, 0.418245524, 0.0443570167, 0.0146725178, -0.183080405, -3.37695383e-09, 1.08100736, -0.425504446, 0.0439427793, 0.0239069108, -0.183155343, -4.2661668e-08, 1.08278775]),
            (-1, 250, true, [-0.712395191, 0.0442800894, 0.000310984557, -0.294118077, -4.97139396e-08, 5.92288956e-08, 0.131354734, 0.044542484, 1.708009, -0.294118077, 2.6043781e-10, 1.73564065, -0.712395191, 0.0442804135, 1.71884274, -0.294118077, -4.97139396e-08, 1.73699081]),
            (-1, 400, true, [-0.865377128, 0.0443741046, 0.000259702676, -0.0177919827, 3.50069875e-08, -4.3179444e-08, -0.0216271952, 0.0445932299, 2.61068892, -0.0177919827, -1.42566448e-09, 0.104977287, -0.865377128, 0.0443741083, 2.62214565, -0.0177919827, 3.50069875e-08, 0.105045214]),
        ];
        let d = fixture();
        for side in [1, -1] {
            let mut s = State::new(1, side, &d);
            s.ground_y = -0.1325;
            for step in 1..=400 {
                if step == 101 {
                    s.impact(&d);
                }
                if (201..=250).contains(&step) {
                    s.drive(0.3 * side as f32);
                }
                if step == 251 {
                    s.stop();
                }
                s.advance(d.fixed_step, &d);
                assert!(s.valid(&d));
                if let Some((_, _, awake, expected)) =
                    EXPECTED.iter().find(|r| r.0 == side && r.1 == step)
                {
                    assert_eq!(s.awake, *awake, "side {side}, step {step}: sleep");
                    for (i, b) in s.bodies.iter().enumerate() {
                        let actual = [
                            b.position[0],
                            b.position[1],
                            b.angle,
                            b.velocity[0],
                            b.velocity[1],
                            b.angular_velocity,
                        ];
                        for j in 0..6 {
                            assert!(
                                (actual[j] - expected[6 * i + j]).abs() < 1e-5,
                                "side {side}, step {step}, body {i}, field {j}: {} vs {}",
                                actual[j],
                                expected[6 * i + j]
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn corrupt_finite_saves_sleep_mismatch_and_unbounded_clock_are_rejected() {
        let d = fixture();
        let s = State::new(1, 1, &d);
        for field in 0..8 {
            let mut bad = s;
            match field {
                0 => bad.bodies[0].position[0] = f32::MAX,
                1 => bad.bodies[1].velocity[1] = f32::MAX,
                2 => bad.bodies[2].angle = f32::MAX,
                3 => bad.bodies[0].angular_velocity = f32::MAX,
                4 => bad.joint_impulses[1][0] = f32::MAX,
                5 => bad.contact_impulses[0][0] = -1.0,
                6 => bad.ground_y = f32::MAX,
                _ => bad.awake = false,
            }
            assert!(!bad.valid(&d), "field {field}");
        }
        let mut settled = s;
        settled.advance(2.0, &d);
        assert!(!settled.awake && settled.valid(&d));
        settled.bodies[1].angular_velocity = 0.01;
        assert!(!settled.valid(&d));
        for dt in [f32::NAN, f32::INFINITY, -1.0, MAX_CATCHUP_SECONDS + 1.0] {
            let mut unchanged = s;
            unchanged.advance(dt, &d);
            assert_eq!(unchanged, s);
        }
    }

    #[test]
    fn physical_wheels_have_their_own_unmirrored_render_transform() {
        let d = fixture();
        let mut s = State::new(1, -1, &d);
        s.bodies[1].angle = 0.6;
        s.bodies[1].position = [2.0, 3.0];
        let t = s.wheel_pose(0, &d);
        let anchor = d.front_anchor;
        let center = [
            t.basis[0] * anchor[0] + t.basis[1] * anchor[1] + t.translation[0],
            t.basis[2] * anchor[0] + t.basis[3] * anchor[1] + t.translation[1],
        ];
        assert!((center[0] - 2.0).abs() < 1e-6 && (center[1] - 3.0).abs() < 1e-6);
        assert!(
            t.basis[2] > 0.0,
            "left facing must not mirror physical wheel spin"
        );
    }
    #[test]
    fn fixed_clock_is_independent_of_bounded_frame_partitioning() {
        let d = fixture();
        let mut whole = State::new(1, 1, &d);
        whole.impact(&d);
        let mut split = whole;
        whole.advance(1.0, &d);
        for _ in 0..50 {
            split.advance(d.fixed_step, &d);
        }
        assert_eq!(whole.bodies, split.bodies);
        assert_eq!(whole.joint_impulses, split.joint_impulses);
        assert_eq!(whole.contact_impulses, split.contact_impulses);
        assert_eq!(whole.awake, split.awake);
        assert!((whole.pending - split.pending).abs() < 1e-6);
    }
}
