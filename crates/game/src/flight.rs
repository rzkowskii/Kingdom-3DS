//! Squid flight driven by the original FlyUp coroutine and a bounded 2D body.
//! The source applies lift*mass as Force, after WaitForFixedUpdate, for the
//! authored duration. Gravity and Rigidbody2D damping act at the fixed step;
//! the Update predicate predicts altitude using y + 0.5*vy.
//!
//! Translation uses Box2D's semi-implicit/Pade equations. Static circle/ground
//! and circle/polygon-boundary contacts cover the authored wall geometry. This
//! is a specialization, not a Unity native manifold/timing parity claim. Dead
//! body rotation and Unity's concave-polygon decomposition are not reproduced.

use crate::build::WALK_Y;

const LINEAR_SLOP: f32 = 0.005;
const MAX_CONTACTS: usize = 16;
const MAX_CATCHUP: f32 = 60.0;

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Definition {
    pub mass: f32,
    pub gravity: f32,
    pub gravity_scale: f32,
    pub linear_drag: f32,
    pub angular_drag: f32,
    pub fixed_step: f32,
    pub max_linear_speed: f32,
    pub max_acceleration_per_frame: f32,
    pub fly_lift: f32,
    pub fly_duration: f32,
    pub fly_cooldown: f32,
    /// Absolute PC world Y; callers convert to WALK_Y-relative actor coordinates.
    pub base_altitude: f32,
    pub move_speed: f32,
    pub scan_range: f32,
    pub attack_cooldown: f32,
    pub physical_radius: f32,
    pub physical_offset: [f32; 2],
    pub dead_radius: f32,
    pub grab_bounds: [f32; 4],
    pub carry_capacity: u8,
    pub spawn_overlay_color: [f32; 4],
    pub ground_height: f32,
    pub friction: f32,
    pub restitution: f32,
    /// Authored contact offset mapped to edge/polygon skin in this specialization.
    pub contact_offset: f32,
    pub velocity_threshold: f32,
    pub max_linear_correction: f32,
    pub baumgarte: f32,
    pub velocity_iterations: u8,
    pub position_iterations: u8,
}
impl Default for Definition {
    /// Reference fixture; production extraction supplies all authored fields.
    fn default() -> Self {
        Self {
            mass: 1.0,
            gravity: 15.0,
            gravity_scale: 0.3,
            linear_drag: 0.5,
            angular_drag: 0.1,
            fixed_step: 0.02,
            max_linear_speed: 100.0,
            max_acceleration_per_frame: 0.0,
            fly_lift: 8.0,
            fly_duration: 0.6,
            fly_cooldown: 0.4,
            base_altitude: 2.2,
            move_speed: 1.3,
            scan_range: 5.0,
            attack_cooldown: 4.0,
            physical_radius: 0.58,
            physical_offset: [0.0, 0.875],
            dead_radius: 0.22,
            grab_bounds: [-0.2, 0.3, 1.0, 0.6],
            carry_capacity: 2,
            spawn_overlay_color: [0.5686275, 0.0627451, 0.3921569, 1.0],
            ground_height: 0.8675,
            friction: 0.4,
            restitution: 0.0,
            contact_offset: 0.01,
            velocity_threshold: 1.0,
            max_linear_correction: 0.2,
            baumgarte: 0.2,
            velocity_iterations: 8,
            position_iterations: 3,
        }
    }
}
impl Definition {
    pub fn valid(&self) -> bool {
        [
            self.mass,
            self.gravity,
            self.max_linear_speed,
            self.fly_lift,
            self.fly_duration,
            self.physical_radius,
            self.dead_radius,
            self.move_speed,
            self.scan_range,
            self.max_linear_correction,
        ]
        .iter()
        .all(|v| v.is_finite() && *v > 0.0 && *v <= 10000.0)
            && [
                self.gravity_scale,
                self.linear_drag,
                self.angular_drag,
                self.max_acceleration_per_frame,
                self.fly_cooldown,
                self.attack_cooldown,
                self.friction,
                self.restitution,
                self.contact_offset,
                self.velocity_threshold,
            ]
            .iter()
            .all(|v| v.is_finite() && *v >= 0.0 && *v <= 10000.0)
            && self.fixed_step.is_finite()
            && (0.001..=0.1).contains(&self.fixed_step)
            && self.baumgarte.is_finite()
            && (0.0..=1.0).contains(&self.baumgarte)
            && self.restitution <= 1.0
            && bounded(self.base_altitude, 10000.0)
            && bounded(self.ground_height, 10000.0)
            && self
                .physical_offset
                .iter()
                .chain(&self.grab_bounds)
                .all(|v| bounded(*v, 10000.0))
            && self.grab_bounds[2] > 0.0
            && self.grab_bounds[3] > 0.0
            && self
                .spawn_overlay_color
                .iter()
                .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
            && (1..=32).contains(&self.velocity_iterations)
            && (1..=16).contains(&self.position_iterations)
            && self.carry_capacity > 0
    }
}

/// GreedCtx snapshots contain WALK_Y-relative WORLD vertices. `origin` is
/// collider transform metadata for scanners and must not be added to points.
pub type Obstacle = crate::data::ObstacleDef;

/// These remain authoritative in the legacy Troll/Mover save prefix.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
}
impl Motion {
    /// Validate canonical actor motion when loading the separately saved clocks.
    pub fn valid(&self) -> bool {
        bounded(self.x, 1_000_000.0)
            && bounded(self.y, 1_000_000.0)
            && bounded(self.vx, 1000.0)
            && bounded(self.vy, 1000.0)
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Input {
    /// WALK_Y-relative desired altitude, like Motion.y.
    pub altitude: f32,
    /// Mover's desired signed speed; None while paused, zero when stopped.
    pub move_velocity: Option<f32>,
    pub alive: bool,
    pub collide_with_obstacles: bool,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct State {
    pub enemy: u32,
    pub pending: f32,
    pub fly_timeout: f32,
    /// None after the coroutine has completed; pending force may remain for
    /// the next physics step because WaitForFixedUpdate runs after physics.
    pub flap_elapsed: Option<f32>,
    pub force_pending: bool,
}
impl State {
    pub fn new(enemy: u32) -> Self {
        Self {
            enemy,
            pending: 0.0,
            fly_timeout: 0.0,
            flap_elapsed: None,
            force_pending: false,
        }
    }
    pub fn valid(&self, d: &Definition) -> bool {
        d.valid()
            && self.pending.is_finite()
            && (0.0..d.fixed_step).contains(&self.pending)
            && self.fly_timeout.is_finite()
            && (0.0..=d.fly_duration + d.fly_cooldown + 1e-5).contains(&self.fly_timeout)
            && self
                .flap_elapsed
                .is_none_or(|t| t.is_finite() && (0.0..d.fly_duration).contains(&t))
    }
    /// A render-frame call: catch up fixed physics and waiting coroutines, then
    /// run the Squid.Update flap predicate and Mover.Update velocity assignment.
    /// No position easing or altitude clamp is applied.
    pub fn advance(
        &mut self,
        dt: f32,
        m: &mut Motion,
        input: Input,
        d: &Definition,
        obstacles: &[Obstacle],
    ) {
        if !dt.is_finite()
            || dt <= 0.0
            || dt > MAX_CATCHUP
            || !self.valid(d)
            || !m.valid()
            || !bounded(input.altitude, 1_000_000.0)
            || input.move_velocity.is_some_and(|v| !bounded(v, 1000.0))
        {
            return;
        }
        self.pending += dt;
        let steps = libm::floorf(self.pending / d.fixed_step) as u32;
        self.pending = (self.pending - steps as f32 * d.fixed_step)
            .max(0.0)
            .min(d.fixed_step - f32::EPSILON * d.fixed_step);
        for _ in 0..steps {
            self.physics_step(m, input, d, obstacles);
            if let Some(elapsed) = self.flap_elapsed {
                let next = elapsed + d.fixed_step;
                self.force_pending = true;
                self.flap_elapsed = (next < d.fly_duration).then_some(next);
            }
        }
        self.fly_timeout = (self.fly_timeout - dt).max(0.0);
        if input.alive && self.fly_timeout <= 0.0 && m.y + 0.5 * m.vy < input.altitude {
            self.fly_timeout = d.fly_duration + d.fly_cooldown;
            self.flap_elapsed = Some(0.0);
        }
        if let Some(target) = input.move_velocity {
            if (target > 0.0 && target > m.vx) || (target < 0.0 && target < m.vx) {
                let acceleration = d.max_acceleration_per_frame * dt / d.fixed_step;
                m.vx = if acceleration <= 0.0 {
                    target
                } else if target > 0.0 {
                    target.min(m.vx + acceleration)
                } else {
                    target.max(m.vx - acceleration)
                };
            }
        }
    }
    fn physics_step(
        &mut self,
        m: &mut Motion,
        input: Input,
        d: &Definition,
        obstacles: &[Obstacle],
    ) {
        let h = d.fixed_step;
        let radius = if input.alive {
            d.physical_radius
        } else {
            d.dead_radius
        };
        let center = [m.x + d.physical_offset[0], m.y + d.physical_offset[1]];
        let mut contacts = [Contact::default(); MAX_CONTACTS];
        let mut count = 0;
        if let Some(c) = ground_contact(center, radius, d) {
            contacts[count] = c;
            count += 1;
        }
        if input.collide_with_obstacles {
            for (index, obstacle) in obstacles.iter().enumerate() {
                if let Some(mut c) = wall_contact(center, radius, d.contact_offset, obstacle) {
                    c.wall = Some(index);
                    if count < MAX_CONTACTS {
                        contacts[count] = c;
                        count += 1;
                    }
                }
            }
        }
        let lift = if self.force_pending { d.fly_lift } else { 0.0 };
        self.force_pending = false;
        let damping = 1.0 / (1.0 + h * if input.alive { d.linear_drag } else { 0.0 });
        let mut v = [
            m.vx * damping,
            (m.vy + h * (lift - d.gravity * d.gravity_scale)) * damping,
        ];
        for c in &mut contacts[..count] {
            let normal_speed = dot(v, c.normal);
            if normal_speed < -d.velocity_threshold {
                c.bias = -d.restitution * normal_speed;
            }
        }
        // Upright body: the contact impulse changes translation only. Every
        // wall uses its authored friction0; the ground mixes default .4/.4.
        for _ in 0..d.velocity_iterations {
            for c in &mut contacts[..count] {
                let tangent = [c.normal[1], -c.normal[0]];
                let old = c.tangent_impulse;
                let cap = c.friction * c.normal_impulse;
                c.tangent_impulse = (old - dot(v, tangent) * d.mass).clamp(-cap, cap);
                v = add(v, mul(tangent, (c.tangent_impulse - old) / d.mass));
                let old = c.normal_impulse;
                c.normal_impulse = (old - (dot(v, c.normal) - c.bias) * d.mass).max(0.0);
                v = add(v, mul(c.normal, (c.normal_impulse - old) / d.mass));
            }
        }
        let speed = libm::sqrtf(dot(v, v));
        if speed > d.max_linear_speed {
            v = mul(v, d.max_linear_speed / speed);
        }
        m.vx = v[0];
        m.vy = v[1];
        m.x += h * m.vx;
        m.y += h * m.vy;
        for _ in 0..d.position_iterations {
            let mut min_separation = 0.0_f32;
            for previous in &contacts[..count] {
                let center = [m.x + d.physical_offset[0], m.y + d.physical_offset[1]];
                let next = match previous.wall {
                    Some(i) => wall_contact(center, radius, d.contact_offset, &obstacles[i]),
                    None => ground_contact(center, radius, d),
                };
                if let Some(c) = next {
                    min_separation = min_separation.min(c.separation);
                    let correction = (d.baumgarte * (c.separation + LINEAR_SLOP))
                        .clamp(-d.max_linear_correction, 0.0);
                    m.x -= correction * c.normal[0];
                    m.y -= correction * c.normal[1];
                }
            }
            if min_separation >= -3.0 * LINEAR_SLOP {
                break;
            }
        }
    }
}
#[derive(Clone, Copy, Debug, Default)]
struct Contact {
    normal: [f32; 2],
    separation: f32,
    friction: f32,
    wall: Option<usize>,
    normal_impulse: f32,
    tangent_impulse: f32,
    bias: f32,
}
fn ground_contact(center: [f32; 2], radius: f32, d: &Definition) -> Option<Contact> {
    let separation = center[1] - (d.ground_height - WALK_Y) - radius - d.contact_offset;
    (separation <= 0.0).then_some(Contact {
        normal: [0.0, 1.0],
        separation,
        friction: d.friction,
        ..Contact::default()
    })
}
/// Closest boundary of the filled authored polygon, including concave outlines.
/// This avoids treating a concavity as its convex hull or colliding with internal
/// triangulation edges. Unity may generate multiple manifolds for decomposition.
fn wall_contact(center: [f32; 2], radius: f32, skin: f32, wall: &Obstacle) -> Option<Contact> {
    if wall.points.len() < 3 || wall.points.len() > 64 {
        return None;
    }
    let p = center;
    let mut min = [f32::INFINITY; 2];
    let mut max = [f32::NEG_INFINITY; 2];
    for v in &wall.points {
        if !v.iter().all(|x| bounded(*x, 10000.0)) {
            return None;
        }
        for i in 0..2 {
            min[i] = min[i].min(v[i]);
            max[i] = max[i].max(v[i]);
        }
    }
    let reach = radius + skin;
    if p[0] < min[0] - reach
        || p[0] > max[0] + reach
        || p[1] < min[1] - reach
        || p[1] > max[1] + reach
    {
        return None;
    }
    let mut inside = false;
    let mut nearest = [0.0; 2];
    let mut best = f32::INFINITY;
    let mut edge = [0.0; 2];
    let mut area = 0.0;
    for i in 0..wall.points.len() {
        let a = wall.points[i];
        let b = wall.points[(i + 1) % wall.points.len()];
        let ab = sub(b, a);
        area += a[0] * b[1] - a[1] * b[0];
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            inside = !inside;
        }
        let len = dot(ab, ab);
        if len <= f32::EPSILON {
            continue;
        }
        let q = add(a, mul(ab, (dot(sub(p, a), ab) / len).clamp(0.0, 1.0)));
        let sq = dot(sub(p, q), sub(p, q));
        if sq < best {
            best = sq;
            nearest = q;
            edge = ab;
        }
    }
    if !best.is_finite() {
        return None;
    }
    let distance = libm::sqrtf(best);
    let separation = if inside {
        -distance - reach
    } else {
        distance - reach
    };
    if separation > 0.0 {
        return None;
    }
    let normal = if distance > 1e-6 {
        mul(
            sub(p, nearest),
            if inside {
                -1.0 / distance
            } else {
                1.0 / distance
            },
        )
    } else {
        let len = libm::sqrtf(dot(edge, edge));
        mul(
            [edge[1], -edge[0]],
            if area > 0.0 { 1.0 / len } else { -1.0 / len },
        )
    };
    Some(Contact {
        normal,
        separation,
        friction: 0.0,
        ..Contact::default()
    })
}
fn bounded(v: f32, max: f32) -> bool {
    v.is_finite() && v.abs() <= max
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    fn input(altitude: f32) -> Input {
        Input {
            altitude,
            move_velocity: None,
            alive: true,
            collide_with_obstacles: true,
        }
    }
    fn motion(y: f32) -> Motion {
        Motion {
            x: 0.0,
            y,
            vx: 0.0,
            vy: 0.0,
        }
    }

    #[test]
    fn source_lift_is_a_mass_independent_force_after_fixed_wait_with_float_loop_duration() {
        let d = Definition {
            linear_drag: 0.0,
            ..Definition::default()
        };
        let mut s = State::new(1);
        let mut m = motion(10.0);
        s.advance(0.001, &mut m, input(20.0), &d, &[]);
        assert_eq!(m.vy, 0.0);
        assert_eq!(s.flap_elapsed, Some(0.0));
        assert!(!s.force_pending);
        let mut applied = 0;
        for step in 1..=32 {
            applied += usize::from(s.force_pending);
            s.advance(d.fixed_step, &mut m, input(-100.0), &d, &[]);
            if step == 1 {
                assert!(
                    (m.vy + 0.09).abs() < 1e-6,
                    "first fixed wait must not apply force early"
                );
            }
        }
        // .02f accumulated thirty times is just below .6f in the original CIL.
        assert_eq!(applied, 31);
        assert_eq!(s.flap_elapsed, None);
        assert!(!s.force_pending);
        assert!((m.vy - 2.08).abs() < 2e-6);
        let heavy = Definition { mass: 7.0, ..d };
        let mut hs = State::new(2);
        let mut hm = motion(10.0);
        hs.advance(0.001, &mut hm, input(20.0), &heavy, &[]);
        for _ in 0..32 {
            hs.advance(d.fixed_step, &mut hm, input(-100.0), &heavy, &[]);
        }
        assert_eq!(m, hm);
    }
    #[test]
    fn predictive_altitude_cooldown_and_death_keep_the_original_coroutine_rules() {
        let d = Definition::default();
        let mut s = State::new(1);
        let mut m = motion(0.0);
        m.vy = 3.0;
        s.advance(0.001, &mut m, input(1.0), &d, &[]);
        assert_eq!(
            s.flap_elapsed, None,
            "rising fast enough: do not flap despite being below target"
        );
        m.vy = -1.0;
        s.advance(0.001, &mut m, input(1.0), &d, &[]);
        assert_eq!(s.flap_elapsed, Some(0.0));
        let mut dead = input(100.0);
        dead.alive = false;
        s.advance(0.02, &mut m, dead, &d, &[]);
        let before = m.vy;
        s.advance(0.02, &mut m, dead, &d, &[]);
        assert!(
            (m.vy - before - 0.07).abs() < 1e-6,
            "death removes drag, but does not cancel an already-running FlyUp"
        );
        for _ in 0..40 {
            s.advance(0.02, &mut m, dead, &d, &[]);
        }
        assert_eq!(s.flap_elapsed, None);
        assert!(!s.force_pending);
        let before = m.vy;
        s.advance(0.02, &mut m, dead, &d, &[]);
        assert!(
            (m.vy - before + 0.09).abs() < 1e-6,
            "dead bodies receive no new flap"
        );
        let mut cooldown = State::new(3);
        cooldown.fly_timeout = 0.2;
        cooldown.advance(0.1, &mut motion(10.0), input(100.0), &d, &[]);
        assert_eq!(cooldown.flap_elapsed, None);
    }
    #[test]
    fn horizontal_mover_accelerates_but_drag_handles_stop_and_same_direction_slowdown() {
        let d = Definition::default();
        let mut s = State::new(1);
        let mut m = motion(20.0);
        let mut i = input(-100.0);
        i.move_velocity = Some(1.3);
        s.advance(0.01, &mut m, i, &d, &[]);
        assert_eq!(m.vx, 1.3);
        assert_eq!(m.x, 0.0);
        i.move_velocity = Some(0.2);
        s.advance(0.01, &mut m, i, &d, &[]);
        assert!(
            (m.vx - 1.3 / 1.01).abs() < 1e-6,
            "smaller target is not an instantaneous brake"
        );
        let before = m.vx;
        i.move_velocity = Some(0.0);
        s.advance(0.02, &mut m, i, &d, &[]);
        assert!((m.vx - before / 1.01).abs() < 1e-6);
        i.move_velocity = Some(-1.3);
        s.advance(0.001, &mut m, i, &d, &[]);
        assert_eq!(m.vx, -1.3);
    }
    #[test]
    fn physical_circle_uses_world_points_ground_offset_and_retreat_layer_filter() {
        let d = Definition::default();
        let wall = Obstacle {
            origin: [99.0, 88.0],
            points: vec![[1.0, -1.0], [1.5, -1.0], [1.5, 3.0], [1.0, 3.0]],
        };
        let mut s = State::new(1);
        let mut m = motion(0.0);
        let mut i = input(0.0);
        i.move_velocity = Some(1.3);
        for _ in 0..100 {
            s.advance(0.02, &mut m, i, &d, core::slice::from_ref(&wall));
        }
        assert!(
            m.x < 0.43,
            "world points must not get origin added a second time"
        );
        i.collide_with_obstacles = false;
        for _ in 0..100 {
            s.advance(0.02, &mut m, i, &d, core::slice::from_ref(&wall));
        }
        assert!(
            m.x > 2.0,
            "Retreat changes collision layer, so walls no longer stop flight"
        );
        let mut falling = motion(3.0);
        let mut state = State::new(2);
        let mut dead = input(-100.0);
        dead.alive = false;
        for _ in 0..300 {
            state.advance(0.02, &mut falling, dead, &d, &[]);
        }
        let expected = d.ground_height - WALK_Y + d.dead_radius + d.contact_offset
            - d.physical_offset[1]
            - LINEAR_SLOP;
        assert!((falling.y - expected).abs() < 1e-4);
        assert_eq!(falling.vy, 0.0);
    }
    #[test]
    fn concave_boundary_does_not_replace_authored_shape_with_its_hull() {
        let wall = Obstacle {
            origin: [50.0, 50.0],
            points: vec![
                [0.0, 0.0],
                [2.0, 0.0],
                [2.0, 1.0],
                [1.0, 1.0],
                [1.0, 2.0],
                [0.0, 2.0],
            ],
        };
        assert!(
            wall_contact([1.5, 1.5], 0.1, 0.0, &wall).is_none(),
            "empty notch remains empty"
        );
        let c = wall_contact([0.95, 1.5], 0.1, 0.0, &wall).unwrap();
        assert!(
            c.normal[0] > 0.99 && c.separation < 0.0,
            "inside body is pushed toward closest exposed boundary"
        );
        let mut reversed = wall.clone();
        reversed.points.reverse();
        let r = wall_contact([0.95, 1.5], 0.1, 0.0, &reversed).unwrap();
        assert_eq!(c.normal, r.normal);
        assert_eq!(c.separation, r.separation);
    }
    #[test]
    fn pause_and_serialized_coroutine_resume_preserve_force_phase_and_reject_corrupt_clocks() {
        let d = Definition::default();
        let mut s = State::new(1);
        let mut m = motion(1.0);
        s.advance(0.073, &mut m, input(2.0), &d, &[]);
        let before = (s, m);
        for dt in [0.0, f32::NAN, f32::INFINITY, -1.0, MAX_CATCHUP + 1.0] {
            s.advance(dt, &mut m, input(2.0), &d, &[]);
            assert_eq!((s, m), before);
        }
        for field in 0..3 {
            let mut bad = s;
            match field {
                0 => bad.pending = d.fixed_step,
                1 => bad.fly_timeout = f32::MAX,
                _ => bad.flap_elapsed = Some(f32::NAN),
            }
            assert!(!bad.valid(&d));
        }
        #[cfg(feature = "save")]
        {
            let mut loaded: State =
                postcard::from_bytes(&postcard::to_allocvec(&s).unwrap()).unwrap();
            let mut lm = m;
            for _ in 0..75 {
                s.advance(1.0 / 30.0, &mut m, input(2.0), &d, &[]);
                loaded.advance(1.0 / 30.0, &mut lm, input(2.0), &d, &[]);
            }
            assert_eq!(s, loaded);
            assert_eq!(m, lm);
        }
    }
    /// Upstream Box2D 2.3.1 C++ reference, pinned at
    /// 7e633c4fb86a68bf072fb8ae67ea2c060114750e. Source driver is retained in
    /// review/2026-11-08/squid-flight-oracle.cpp with its limitations/receipt.
    /// Discrete fixed-step body, Force-mode lift, one wall with friction0,
    /// authored ground/circle, Rigidbody damping, dead drag0, retreat filtering.
    #[test]
    #[allow(clippy::excessive_precision)] // Preserve the independent C++ float32 trace verbatim.
    fn independent_box2d_body_oracle_matches_flaps_death_drag_and_wall_layer_change() {
        #[rustfmt::skip]
        const EXPECTED:&[(bool,usize,[f32;4],bool)]=&[
            (false, 1, [0.0, 2.99821782, 1.29999995, -0.0891089067], false),
            (false, 10, [0.231683165, 2.90483499, 1.29999995, -0.85241735], false),
            (false, 50, [1.2613858, 1.80398059, 1.29999995, -0.381989777], true),
            (false, 100, [2.28048468, 1.30273938, 0.790450335, 0.16863063], true),
            (false, 150, [1.03475308, 1.27999878, -1.29999995, 0.480001003], true),
            (false, 151, [1.00875306, 1.2909987, -1.29999995, 0.550001025], true),
            (false, 200, [-0.265247256, 1.04900074, -1.29999995, -2.2599988], false),
            (false, 225, [-0.91524756, -0.665998161, -1.29999995, -4.50999784], false),
            (false, 300, [-0.91524756, -0.657532096, 0.0, 0.0], false),
            (true, 1, [0.0, -0.00178217888, 1.29999995, -0.0891089067], true),
            (true, 10, [0.231683165, 0.0162979364, 1.29999995, 0.371851057], true),
            (true, 50, [0.411881089, 0.803554773, 1.29999995, 0.0582234487], false),
            (true, 100, [0.411881089, 0.498323798, 1.29999995, 0.670839369], false),
            (true, 150, [0.411881089, 0.505979657, 1.29999995, 1.12701142], false),
            (true, 151, [0.43762365, 0.52651453, 1.29999995, 1.02674401], false),
            (true, 200, [1.69900882, 0.397360802, 1.29999995, 0.681320846], true),
            (true, 225, [2.34257221, 0.546538472, 1.29999995, -0.683267772], false),
            (true, 300, [4.27326202, 0.31501615, 1.29999995, 0.0424935594], true),
        ];
        let d = Definition::default();
        let walls = [Obstacle {
            origin: [99.0, 88.0],
            points: vec![[1.0, -1.0], [1.5, -1.0], [1.5, 3.0], [1.0, 3.0]],
        }];
        for wall in [false, true] {
            let mut s = State::new(1);
            let mut m = motion(if wall { 0.0 } else { 3.0 });
            for frame in 1..=300 {
                let alive = wall || frame < 151;
                let speed = if !alive {
                    0.0
                } else if wall || frame <= 50 {
                    1.3
                } else if frame <= 100 {
                    0.0
                } else {
                    -1.3
                };
                let input = Input {
                    altitude: if wall { 0.0 } else { 1.325 },
                    move_velocity: Some(speed),
                    alive,
                    collide_with_obstacles: wall && frame <= 150,
                };
                s.advance(0.02, &mut m, input, &d, &walls);
                assert!(s.valid(&d));
                if let Some((_, _, expected, flapping)) =
                    EXPECTED.iter().find(|row| row.0 == wall && row.1 == frame)
                {
                    for (i, actual) in [m.x, m.y, m.vx, m.vy].iter().enumerate() {
                        // Upstream sleeps the settled corpse; this specialized
                        // body continues harmless sub-slop position correction.
                        let tolerance = if i < 2 { 5e-5 } else { 1e-5 };
                        assert!(
                            (actual - expected[i]).abs() < tolerance,
                            "wall={wall} frame={frame} field={i}: {actual} vs {}",
                            expected[i]
                        );
                    }
                    assert_eq!(
                        s.flap_elapsed.is_some(),
                        *flapping,
                        "wall={wall} frame={frame} flap phase"
                    );
                }
            }
        }
    }
}
