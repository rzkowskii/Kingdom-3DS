//! MainCamera.Update/SetTarget/Shake: anticipation, target seek and pixel snap.
//! This is a presentation clock, advanced once per admitted simulation tick.
use game::animation::sample;
use game::data::{CameraDef, CameraShakeDef};
use game::rng::Rng;

/// Authored world Y of Main Camera in main.unity.
pub const CAMERA_Y: f32 = 1.875;
const SHAKE_RADIUS: f32 = 6.0;
const MAX_SHAKE_SEGMENTS: usize = 64;
const SHAKE_SEED: u32 = 0x4d41_494e;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subject {
    Monarch,
    Teleporter,
    Boat(u32),
    Crown,
    /// Unity retains the target/seek coordinates when its object is inactive.
    Inactive,
    None,
}

#[derive(Clone, Copy, Debug)]
pub struct Target {
    pub subject: Subject,
    pub x: f32,
    pub direction: f32,
    pub lead: bool,
}

#[derive(Clone, Debug)]
pub struct Camera {
    subject: Subject,
    use_lead: bool,
    target_x: f32,
    position: f32,
    previous_direction: f32,
    lead_time: f32,
    lead_offset: f32,
    seek_offset: f32,
    pan: Option<(f32, f32)>,
    shake: Option<Shake>,
    snapped_shake: [f32; 2],
    /// A presentation event cannot perturb the save's gameplay RNG stream.
    shake_rng: Rng,
}

impl Camera {
    pub fn new(target: Target) -> Self {
        Self {
            subject: target.subject,
            use_lead: target.lead,
            target_x: target.x,
            position: snap(target.x),
            previous_direction: 0.0,
            lead_time: 0.0,
            lead_offset: 0.0,
            seek_offset: 0.0,
            pan: None,
            shake: None,
            snapped_shake: [0.0; 2],
            shake_rng: Rng::new(SHAKE_SEED),
        }
    }

    pub fn position(&self) -> f32 {
        self.position
    }

    /// MainCamera.Shake tests the already published camera transform. Events
    /// supply absolute world coordinates, including the walk plane's Y offset.
    pub fn shake(&mut self, def: &CameraDef, origin: [f32; 2]) -> bool {
        let [x, y] = self.shake_offset();
        let dx = origin[0] - (self.position + x);
        let dy = origin[1] - (CAMERA_Y + y);
        let distance = (dx * dx + dy * dy).sqrt();
        if !distance.is_finite() || distance >= SHAKE_RADIUS || !def.shake.valid() {
            return false;
        }
        // Kill(false) leaves the child transform at its raw offset. DOTween
        // captures it as the next shake's start before adding the waypoints.
        let start = self.shake.as_ref().map_or([0.0; 2], Shake::offset);
        self.shake = Some(Shake::new(&def.shake, start, &mut self.shake_rng));
        true
    }

    /// Snap the shake independently from the follow position as MainCamera
    /// does, leaving seek, lead, audio pan and gameplay on the stable base.
    pub fn shake_offset(&self) -> [f32; 2] {
        self.snapped_shake
    }

    /// MainCamera.PanWithDuration; the game may explicitly take camera control.
    pub fn pan(&mut self, from: f32, to: f32, seconds: f32) {
        self.target_x = from;
        if seconds > 0.0 && seconds.is_finite() && from.is_finite() && to.is_finite() {
            self.pan = Some((to, (to - from) / seconds));
        } else {
            self.target_x = to;
            self.pan = None;
        }
    }

    pub fn step(&mut self, def: &CameraDef, target: Target, dt: f32) -> f32 {
        if dt <= 0.0 || !dt.is_finite() {
            return self.position;
        }
        let previous_view_x = self.position + self.snapped_shake[0];
        if let Some(shake) = &mut self.shake {
            shake.elapsed += f64::from(dt);
            if shake.elapsed >= f64::from(shake.duration) {
                self.shake = None; // MainCamera's OnComplete resets localPosition.
                self.snapped_shake = [0.0; 2];
            } else {
                // Sample once per tick rather than at every world projection.
                self.snapped_shake = shake.offset().map(snap);
            }
        }
        if (target.subject != self.subject || target.lead != self.use_lead)
            && target.subject != Subject::Inactive
        {
            self.subject = target.subject;
            self.use_lead = target.lead;
            self.pan = None;
            self.lead_time = 0.0;
            self.lead_offset = 0.0;
            // SetTarget(null) keeps the previous target coordinate and clears seek.
            self.seek_offset = if target.subject == Subject::None {
                0.0
            } else {
                // SetTarget reads the already published transform, including
                // shake, when the monarch loses the crown during an impact.
                previous_view_x - target.x
            };
        }
        if let Some((to, speed)) = self.pan {
            let before = self.target_x;
            self.target_x += dt * speed;
            if (to - before) * (to - self.target_x) <= 0.0 {
                self.target_x = to;
                self.pan = None;
            }
        } else if !matches!(target.subject, Subject::None | Subject::Inactive) {
            self.target_x = target.x;
            if def.lead && target.lead {
                if target.direction != self.previous_direction {
                    self.lead_time = 0.0;
                    self.previous_direction = target.direction;
                } else {
                    self.lead_time += dt;
                }
                let desired = sample(&def.lead_over_time, self.lead_time, 0.0) * target.direction;
                let max = dt * def.max_lead_speed.max(0.0);
                self.lead_offset += (desired - self.lead_offset).clamp(-max, max);
            }
        }
        if self.seek_offset != 0.0 {
            let speed = sample(&def.seek_speed_by_distance, self.seek_offset.abs(), 1.0).max(0.0);
            self.seek_offset -= self.seek_offset.clamp(-speed * dt, speed * dt);
            self.lead_time = 0.0;
        }
        self.position = snap(self.target_x + self.seek_offset + self.lead_offset);
        self.position
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct ShakeSegment {
    duration: f32,
    target: [f32; 2],
}

#[derive(Clone, Debug)]
struct Shake {
    elapsed: f64,
    duration: f32,
    start: [f32; 2],
    count: usize,
    segments: [ShakeSegment; MAX_SHAKE_SEGMENTS],
}

impl Shake {
    /// Vector-strength DOShakePosition with fadeOut=true. Shipped DOTween
    /// 0x06000030 grows the segment lengths and reduces the magnitude at each
    /// waypoint. SetShake (0x06000203) uses linear easing within a segment.
    fn new(def: &CameraShakeDef, start: [f32; 2], rng: &mut Rng) -> Self {
        let count = (f32::from(def.vibrato) * def.duration) as usize;
        let count = count.clamp(2, MAX_SHAKE_SEGMENTS);
        let mut segments = [ShakeSegment::default(); MAX_SHAKE_SEGMENTS];
        let segments_in_use = &mut segments[..count];
        let mut total = 0.0;
        for (i, segment) in segments_in_use.iter_mut().enumerate() {
            segment.duration = def.duration * (i + 1) as f32 / count as f32;
            total += segment.duration;
        }
        let time_scale = def.duration / total;

        let [x, y] = def.strength;
        let mut strength = [x, y, 0.0];
        let mut magnitude = (x * x + y * y).sqrt();
        let fade_step = magnitude / count as f32;
        let mut angle = rng.range(0.0, 360.0);
        for (i, segment) in segments_in_use.iter_mut().enumerate() {
            segment.duration *= time_scale;
            segment.target = start;
            if i + 1 == count {
                break; // Last relative waypoint is zero, then OnComplete resets.
            }
            if i != 0 {
                angle = angle - 180.0 + rng.range(-def.randomness, def.randomness);
            }
            let yaw = rng.range(-def.randomness, def.randomness).to_radians();
            let (sin, cos) = angle.to_radians().sin_cos();
            let x = cos * magnitude;
            let mut point = [x * yaw.cos(), sin * magnitude, -x * yaw.sin()];
            // The PC assigns the clamped components in order. The X clamp
            // changes the vector magnitude used for the subsequent Y clamp.
            point[0] = clamp_magnitude(point, strength[0])[0];
            point[1] = clamp_magnitude(point, strength[1])[1];
            segment.target = [start[0] + point[0], start[1] + point[1]];
            magnitude = (magnitude - fade_step).max(0.0);
            strength = clamp_magnitude(strength, magnitude);
        }
        Self {
            elapsed: 0.0,
            duration: def.duration,
            start,
            count,
            segments,
        }
    }

    fn offset(&self) -> [f32; 2] {
        let mut from = self.start;
        let mut remaining = self.elapsed as f32;
        for segment in &self.segments[..self.count] {
            if segment.duration > 0.0 && remaining <= segment.duration {
                let t = remaining / segment.duration;
                return [
                    from[0] + (segment.target[0] - from[0]) * t,
                    from[1] + (segment.target[1] - from[1]) * t,
                ];
            }
            remaining -= segment.duration;
            from = segment.target;
        }
        from
    }
}

fn clamp_magnitude(v: [f32; 3], max: f32) -> [f32; 3] {
    let len_sq = v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
    if len_sq > max * max {
        let len = len_sq.sqrt();
        v.map(|component| component / len * max)
    } else {
        v
    }
}

fn snap(x: f32) -> f32 {
    // Mathf.Round uses ties-to-even, at the PC's 32 pixels per world unit.
    (x * 32.0).round_ties_even() / 32.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use game::animation::{Key, Tangent};
    fn key(time: f32, value: f32, input: f32, out: f32) -> Key {
        Key {
            time,
            value,
            in_tangent: Tangent::Slope(input),
            out_tangent: Tangent::Slope(out),
        }
    }
    pub(super) fn definition() -> CameraDef {
        CameraDef {
            lead: true,
            max_lead_speed: 5.0,
            lead_over_time: vec![
                key(0., 0., 0., 0.),
                key(3., 0., 0., 0.6183402),
                key(6., 2., -0.023091942, -0.023091942),
            ],
            seek_speed_by_distance: vec![
                key(0., 1., 0., 0.),
                key(1.2, 2.5, 0., 0.),
                key(5., 2.5, 0., 0.),
                key(12., 30., 0., 0.),
            ],
            shake: CameraShakeDef::default(),
        }
    }
    fn target(x: f32, direction: f32) -> Target {
        Target {
            subject: Subject::Monarch,
            x,
            direction,
            lead: true,
        }
    }

    #[test]
    fn shake_uses_strict_xy_range_from_the_published_camera() {
        let def = definition();
        let t = target(0.0, 1.0);
        let mut c = Camera::new(t);
        assert!(!c.shake(&def, [6.0, CAMERA_Y]));
        assert!(!c.shake(&def, [5.9, CAMERA_Y + 1.5]));
        assert!(c.shake(&def, [5.8, CAMERA_Y + 1.5]));
        c.step(&def, t, 1.0 / 60.0);
        let [x, y] = c.shake_offset();
        assert_eq!([x, y], [-2.0 / 32.0, 4.0 / 32.0]);
        assert!(!c.shake(&def, [x + 6.0, CAMERA_Y + y]));
        assert!(c.shake(&def, [x + 6.0 - 1.0 / 32.0, CAMERA_Y + y]));
    }

    #[test]
    fn shake_follows_shipped_waypoint_timing_and_leaves_tracking_stable() {
        let def = definition();
        let t = target(0.0, 1.0);
        let mut c = Camera::new(t);
        let mut stable = c.clone();
        assert!(c.shake(&def, [0.0, CAMERA_Y]));
        // Independent translation of the shipped DOTween waypoint and linear
        // segment math, seeded with the port's presentation RNG.
        let samples = [
            (0.005, [5, -4]),
            (0.010, [1, 1]),
            (0.025, [2, -2]),
            (0.100, [1, 4]),
            (0.250, [1, 1]),
            (0.500, [2, 2]),
            (0.750, [0, -1]),
            (0.950, [0, 0]),
            (1.001, [0, 0]),
        ];
        let mut before = 0.0;
        for (time, pixels) in samples {
            let moving = target(time * 3.25, -1.0);
            assert_eq!(
                c.step(&def, moving, time - before),
                stable.step(&def, moving, time - before)
            );
            assert_eq!(c.shake_offset(), pixels.map(|v| v as f32 / 32.0), "{time}");
            before = time;
        }
        assert!(c.shake.is_none());
    }

    #[test]
    fn shake_restart_captures_raw_offset_and_waits_for_admitted_ticks() {
        let def = definition();
        let t = target(0.0, 1.0);
        let mut c = Camera::new(t);
        c.shake(&def, [0.0, CAMERA_Y]);
        c.step(&def, t, 0.1);
        let raw = c.shake.as_ref().unwrap().offset();
        let snapped = c.shake_offset();
        assert_ne!(raw, snapped);
        assert!(c.shake(&def, [0.0, CAMERA_Y]));
        let next = c.shake.as_ref().unwrap();
        assert_eq!(next.start, raw);
        assert_eq!(next.elapsed, 0.0);
        assert_eq!(next.segments[next.count - 1].target, raw);
        c.step(&def, target(100.0, 1.0), 0.0);
        assert_eq!(c.shake_offset(), snapped);
        assert_eq!(c.position(), 0.0);
        c.step(&def, t, 0.9375);
        assert!(c.shake.is_some(), "replacement gets a full duration");
        c.step(&def, t, 0.0625);
        assert_eq!(c.shake_offset(), [0.0; 2]);
    }

    #[test]
    fn crown_target_switch_seeks_from_the_last_shaken_view() {
        let mut def = definition();
        def.seek_speed_by_distance = vec![key(0.0, 0.0, 0.0, 0.0)];
        let t = target(0.0, 1.0);
        let mut c = Camera::new(t);
        assert!(c.shake(&def, [0.0, CAMERA_Y]));
        c.step(&def, t, 0.005);
        let previous_view_x = c.position() + c.shake_offset()[0];
        assert_ne!(previous_view_x, 0.0);
        let crown = Target {
            subject: Subject::Crown,
            x: 10.0,
            lead: false,
            ..t
        };
        assert_eq!(c.step(&def, crown, 1.0 / 60.0), previous_view_x);
    }

    #[test]
    fn authored_camera_waits_then_leads_even_when_stopped_and_reverses_at_rate_limit() {
        let def = definition();
        let mut c = Camera::new(target(0., 1.));
        // PC's lead depends on facing duration, not velocity: walk and gallop
        // translate the target without changing camera-to-target displacement.
        for i in 0..180 {
            assert_eq!(
                c.step(&def, target(i as f32 / 32., 1.), 1. / 60.),
                i as f32 / 32.
            );
        }
        for _ in 0..181 {
            c.step(&def, target(10., 1.), 1. / 60.);
        }
        assert!((c.position() - 12.).abs() < 0.04);
        let before = c.position();
        c.step(&def, target(10., -1.), 1. / 60.);
        assert!(before - c.position() > 0.0 && before - c.position() <= 3. / 32.);
        assert_eq!(c.step(&def, target(-100., -1.), 0.), c.position());
    }
    #[test]
    fn same_target_lead_change_resets_anticipation_and_seeks_from_current_view() {
        let def = definition();
        let mut c = Camera::new(target(0., 1.));
        for _ in 0..362 {
            c.step(&def, target(0., 1.), 1. / 60.);
        }
        assert_eq!(c.position(), 2.);
        let no_lead = Target {
            lead: false,
            ..target(0., 1.)
        };
        let first = c.step(&def, no_lead, 1. / 60.);
        assert!(first > 1.9 && first < 2.0);
        for _ in 0..180 {
            c.step(&def, no_lead, 1. / 60.);
        }
        assert_eq!(c.position(), 0.);
    }

    #[test]
    fn target_switch_seek_pan_and_crown_follow_use_pc_rules() {
        let def = definition();
        let mut c = Camera::new(target(0., 1.));
        let exit = Target {
            subject: Subject::Teleporter,
            x: 12.,
            direction: 1.,
            lead: false,
        };
        assert_eq!(c.step(&def, exit, 1. / 60.), 0.5); // seek curve(12)=30
        let first = c.position();
        assert!(first < exit.x);
        let inactive = Target {
            subject: Subject::Inactive,
            ..exit
        };
        let next = c.step(&def, inactive, 1. / 60.);
        assert!(
            next > first && next < 2.0,
            "despawn retains the ongoing seek"
        );
        for _ in 0..600 {
            c.step(&def, exit, 1. / 60.);
        }
        assert_eq!(c.position(), 12.);
        let crown = Target {
            subject: Subject::Crown,
            x: 13.,
            direction: 1.,
            lead: false,
        };
        assert!(c.step(&def, crown, 1. / 60.) < 13.);
        for _ in 0..120 {
            c.step(&def, crown, 1. / 60.);
        }
        assert_eq!(c.position(), 13.);
        c.pan(13., 15., 1.);
        for _ in 0..62 {
            c.step(&def, crown, 1. / 60.);
            if c.pan.is_none() {
                break;
            }
        }
        // End the test at the crossing boundary, before follow resumes.
        assert!((c.position() - 15.).abs() < 0.04);
        assert_eq!(c.step(&def, crown, 1. / 60.), 13.);
        assert_eq!(snap(0.5 / 32.), 0.);
        assert_eq!(snap(1.5 / 32.), 2. / 32.);
    }
}
