//! Authored animation data shared by extraction, simulation and presentation.
//! Event intervals use the simulation clock; drawing never dispatches gameplay.
use alloc::{string::String, vec::Vec};

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tangent {
    Slope(f32),
    /// Unity serializes stepped tangents as infinity. Keep them JSON-safe.
    Step,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    pub time: f32,
    pub value: f32,
    pub in_tangent: Tangent,
    pub out_tangent: Tangent,
}

/// Cubic Hermite interpolation with tangents in value/second, independently
/// for each component. Missing curves leave the authored rest value alone.
pub fn sample(keys: &[Key], time: f32, rest: f32) -> f32 {
    let Some(first) = keys.first() else {
        return rest;
    };
    if !time.is_finite() || time <= first.time {
        return first.value;
    }
    let i = keys.iter().rposition(|k| k.time <= time).unwrap_or(0);
    let a = keys[i];
    let Some(b) = keys.get(i + 1) else {
        return a.value;
    };
    let (Tangent::Slope(out), Tangent::Slope(input)) = (a.out_tangent, b.in_tangent) else {
        return a.value;
    };
    let duration = b.time - a.time;
    if duration <= 0.0 {
        return b.value;
    }
    let t = ((time - a.time) / duration).clamp(0.0, 1.0);
    let t2 = t * t;
    let t3 = t2 * t;
    (2.0 * t3 - 3.0 * t2 + 1.0) * a.value
        + (t3 - 2.0 * t2 + t) * duration * out
        + (-2.0 * t3 + 3.0 * t2) * b.value
        + (t3 - t2) * duration * input
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    pub color: [f32; 4],
    pub visible: bool,
}
impl Default for Pose {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
            color: [1.0; 4],
            visible: true,
        }
    }
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TransformTrack {
    pub path: String,
    pub position: [Vec<Key>; 3],
    pub rotation: [Vec<Key>; 4],
    pub scale: [Vec<Key>; 3],
    /// Euler Z in degrees. Other Euler axes are diagnosed by extraction;
    /// the retained source game only animates Z for its 2D sprites.
    pub euler_z: Vec<Key>,
    pub color: [Vec<Key>; 4],
    pub visible: Vec<Key>,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    TriggerAbility,
    HammerHit,
    ProjectileRelease,
    ArmImpact,
    FiringDone,
    GrabComplete,
    BashImpact,
    BoulderRelease,
    StandStart,
    WalkStart,
    GallopStart,
    BackFootImpact,
    FrontFootImpact,
    StopLeaves,
    FadeOutTree,
    Bounce,
    ResetAll,
}

impl EventKind {
    /// Explicit allow-list: serialized function names are never executed.
    pub fn from_function(name: &str) -> Option<Self> {
        Some(match name {
            "OnAnimTriggerAbility" => Self::TriggerAbility,
            "OnAnimHammerHit" => Self::HammerHit,
            "ReleaseProjectileFromAnimation" => Self::ProjectileRelease,
            "ArmImpactFromAnimation" => Self::ArmImpact,
            "FiringDoneFromAnimation" => Self::FiringDone,
            "OnAnimGrabComplete" => Self::GrabComplete,
            "OnAnimBashImpact" => Self::BashImpact,
            "OnAnimBoulderRelease" => Self::BoulderRelease,
            "OnAnimStandStart" => Self::StandStart,
            "OnAnimWalkStart" => Self::WalkStart,
            "OnAnimGallopStart" => Self::GallopStart,
            "OnAnimBackFootImpact" => Self::BackFootImpact,
            "OnAnimFrontFootImpact" => Self::FrontFootImpact,
            "StopLeaves" => Self::StopLeaves,
            "FadeOutTree" => Self::FadeOutTree,
            "OnAnimBounce" => Self::Bounce,
            "ResetAll" => Self::ResetAll,
            _ => return None,
        })
    }
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub time: f32,
    pub kind: EventKind,
    pub float_parameter: f32,
    pub int_parameter: i32,
    pub string_parameter: String,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Clip {
    pub length: f32,
    pub looping: bool,
    pub tracks: Vec<TransformTrack>,
    pub events: Vec<Event>,
}

impl Clip {
    pub fn wrap(&self, time: f32) -> f32 {
        if !time.is_finite() {
            return 0.0;
        }
        if self.looping && self.length > 0.0 {
            time - libm::floorf(time / self.length) * self.length
        } else {
            time.clamp(0.0, self.length.max(0.0))
        }
    }

    pub fn pose(&self, path: &str, time: f32, mut rest: Pose) -> Pose {
        let Some(track) = self.tracks.iter().find(|p| p.path == path) else {
            return rest;
        };
        let time = self.wrap(time);
        for i in 0..3 {
            rest.position[i] = sample(&track.position[i], time, rest.position[i]);
            rest.scale[i] = sample(&track.scale[i], time, rest.scale[i]);
        }
        if !track.euler_z.is_empty() {
            let half = sample(&track.euler_z, time, 0.0) * core::f32::consts::PI / 360.0;
            rest.rotation = [0.0, 0.0, libm::sinf(half), libm::cosf(half)];
        } else if track.rotation.iter().any(|keys| !keys.is_empty()) {
            for i in 0..4 {
                rest.rotation[i] = sample(&track.rotation[i], time, rest.rotation[i]);
            }
            let length = libm::sqrtf(rest.rotation.iter().map(|v| v * v).sum());
            if length > 0.0 {
                for v in &mut rest.rotation {
                    *v /= length;
                }
            } else {
                rest.rotation = Pose::default().rotation;
            }
        }
        for i in 0..4 {
            rest.color[i] = sample(&track.color[i], time, rest.color[i]);
        }
        rest.visible = sample(&track.visible, time, if rest.visible { 1.0 } else { 0.0 }) >= 0.5;
        rest
    }

    /// Number of occurrences in `(from, to]` using absolute clip seconds.
    /// Set `include_start` only upon entering/restarting the state, making the
    /// interval `[from, to]` and admitting an authored event at time zero once.
    /// Keeping elapsed time in saved state means resume needs no event replay.
    pub fn event_count(&self, kind: EventKind, from: f32, to: f32, include_start: bool) -> u32 {
        if !from.is_finite() || !to.is_finite() || from < 0.0 || to < from {
            return 0;
        }
        let mut total = 0_u32;
        for event in self.events.iter().filter(|e| e.kind == kind) {
            if !event.time.is_finite() || event.time < 0.0 || event.time > self.length {
                continue;
            }
            let n = event_occurrences(
                event.time,
                self.length,
                self.looping,
                from,
                to,
                include_start,
            );
            total = total.saturating_add(n);
        }
        total
    }

    pub fn first_event_time(&self, kind: EventKind) -> Option<f32> {
        self.events
            .iter()
            .filter(|e| e.kind == kind)
            .map(|e| e.time)
            .min_by(f32::total_cmp)
    }

    pub fn validate(&self) -> bool {
        fn keys_ok(keys: &[Key]) -> bool {
            keys.iter().all(|k| {
                k.time.is_finite()
                    && k.time >= 0.0
                    && k.value.is_finite()
                    && [k.in_tangent, k.out_tangent].iter().all(|t| match t {
                        Tangent::Step => true,
                        Tangent::Slope(v) => v.is_finite(),
                    })
            }) && keys.windows(2).all(|p| p[0].time <= p[1].time)
        }
        self.length.is_finite()
            && self.length >= 0.0
            && self.events.iter().all(|e| {
                e.time.is_finite()
                    && e.time >= 0.0
                    && e.time <= self.length
                    && e.float_parameter.is_finite()
            })
            && self.tracks.iter().all(|t| {
                t.position
                    .iter()
                    .chain(&t.rotation)
                    .chain(&t.scale)
                    .chain(&t.color)
                    .chain(core::iter::once(&t.euler_z))
                    .chain(core::iter::once(&t.visible))
                    .all(|k| keys_ok(k))
            })
    }
}

/// Shared occurrence arithmetic for extracted markers and canonical fixture fallbacks.
pub fn event_occurrences(
    at: f32,
    length: f32,
    looping: bool,
    from: f32,
    to: f32,
    include_start: bool,
) -> u32 {
    if ![at, length, from, to].iter().all(|v| v.is_finite())
        || from < 0.0
        || to < from
        || at < 0.0
        || at > length
    {
        return 0;
    }
    if looping && length > 0.0 {
        let (from, to, at, length) = (from as f64, to as f64, at as f64, length as f64);
        let first = if include_start {
            libm::ceil((from - at) / length)
        } else {
            libm::floor((from - at) / length) + 1.0
        };
        let last = libm::floor((to - at) / length);
        (last - first.max(0.0) + 1.0).max(0.0) as u32
    } else {
        u32::from(at <= to && (at > from || (include_start && at == from)))
    }
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnimatorState {
    pub controller: String,
    pub name: String,
    pub clip: Option<String>,
    pub speed: f32,
    pub speed_parameter: Option<String>,
    pub cycle_offset: f32,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct Condition {
    pub parameter: String,
    pub mode: i32,
    pub threshold: f32,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct AnimatorTransition {
    pub controller: String,
    pub from: String,
    pub to: Option<String>,
    pub conditions: Vec<Condition>,
    pub exit_time: Option<f32>,
    pub duration: f32,
    pub fixed_duration: bool,
    pub offset: f32,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Controller {
    pub name: String,
    pub default_state: Option<String>,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Library {
    pub clips: Vec<(String, Clip)>,
    pub states: Vec<AnimatorState>,
    pub transitions: Vec<AnimatorTransition>,
    pub controllers: Vec<Controller>,
}

fn controller_key(name: &str) -> &str {
    name.rsplit('/')
        .next()
        .unwrap_or(name)
        .split('.')
        .next()
        .unwrap_or(name)
}
impl Library {
    pub fn clip(&self, name: &str) -> Option<&Clip> {
        self.clips.iter().find(|c| c.0 == name).map(|c| &c.1)
    }
    pub fn state(&self, controller: &str, name: &str) -> Option<&AnimatorState> {
        self.states
            .iter()
            .find(|s| s.controller == controller_key(controller) && s.name == name)
    }
    pub fn state_speed(&self, controller: &str, name: &str) -> Option<f32> {
        self.state(controller, name).map(|s| s.speed)
    }
    pub fn default_state(&self, controller: &str) -> Option<&AnimatorState> {
        let name = self
            .controllers
            .iter()
            .find(|c| c.name == controller_key(controller))?
            .default_state
            .as_deref()?;
        self.state(controller, name)
    }
    pub fn state_clip(&self, controller: &str, name: &str) -> Option<&Clip> {
        self.clip(self.state(controller, name)?.clip.as_deref()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    fn key(time: f32, value: f32, input: f32, out: f32) -> Key {
        Key {
            time,
            value,
            in_tangent: Tangent::Slope(input),
            out_tangent: Tangent::Slope(out),
        }
    }
    fn event(time: f32, kind: EventKind) -> Event {
        Event {
            time,
            kind,
            float_parameter: 0.0,
            int_parameter: 0,
            string_parameter: String::new(),
        }
    }
    #[test]
    fn hermite_uses_tangent_seconds_and_independent_stepped_axes() {
        let mut t = TransformTrack {
            path: "Child".into(),
            ..Default::default()
        };
        t.position[0] = vec![key(0.0, 0.0, 0.0, 8.0), key(2.0, 4.0, 0.0, 0.0)];
        t.position[1] = vec![
            Key {
                out_tangent: Tangent::Step,
                ..key(0.0, 3.0, 0.0, 0.0)
            },
            key(2.0, 7.0, 0.0, 0.0),
        ];
        let c = Clip {
            length: 2.0,
            tracks: vec![t],
            ..Default::default()
        };
        let p = c.pose(
            "Child",
            1.0,
            Pose {
                scale: [2.0; 3],
                ..Default::default()
            },
        );
        assert_eq!(p.position, [4.0, 3.0, 0.0]); // linear X would incorrectly be 2
        assert_eq!(p.scale, [2.0; 3]);
        assert_eq!(
            c.pose("Child", 2.0, Pose::default()).position,
            [4.0, 7.0, 0.0]
        );
        assert_eq!(
            c.pose("Child", -1.0, Pose::default()).position,
            [0.0, 3.0, 0.0]
        );
    }
    #[test]
    fn quaternion_scale_color_and_euler_curves_preserve_local_pose() {
        let mut t = TransformTrack {
            path: "Arm".into(),
            ..Default::default()
        };
        t.rotation[2] = vec![key(0.0, 0.0, 0.0, 0.0), key(1.0, 1.0, 0.0, 0.0)];
        t.rotation[3] = vec![key(0.0, 1.0, 0.0, 0.0), key(1.0, 0.0, 0.0, 0.0)];
        t.scale[0] = vec![key(0.0, -1.3, 0.0, 0.0)];
        t.color[3] = vec![key(0.0, 0.25, 0.0, 0.0)];
        let mut c = Clip {
            length: 1.0,
            tracks: vec![t],
            ..Default::default()
        };
        let p = c.pose("Arm", 0.5, Pose::default());
        assert!((p.rotation[2] - core::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        assert_eq!(p.scale[0], -1.3);
        assert_eq!(p.color[3], 0.25);
        c.tracks[0].euler_z = vec![key(0.0, -180.0, 0.0, 0.0)];
        let p = c.pose("Arm", 0.0, Pose::default());
        assert!((p.rotation[2] + 1.0).abs() < 1e-6);
    }
    #[test]
    fn marker_intervals_survive_loop_catchup_pause_interruption_and_resume() {
        let c = Clip {
            length: 1.0,
            looping: true,
            events: vec![
                event(0.0, EventKind::GallopStart),
                event(0.2, EventKind::TriggerAbility),
            ],
            ..Default::default()
        };
        assert_eq!(c.event_count(EventKind::GallopStart, 0.0, 0.1, true), 1);
        assert_eq!(c.event_count(EventKind::TriggerAbility, 0.0, 0.2, true), 1);
        assert_eq!(c.event_count(EventKind::TriggerAbility, 0.2, 0.2, false), 0); // paused
        assert_eq!(c.event_count(EventKind::TriggerAbility, 0.2, 2.3, false), 2);
        assert_eq!(c.event_count(EventKind::GallopStart, 0.1, 2.3, false), 2);
        assert_eq!(c.event_count(EventKind::TriggerAbility, 0.0, 0.1, true), 0); // interrupted before contact
        assert_eq!(c.event_count(EventKind::TriggerAbility, 0.2, 0.9, false), 0); // resumed after saved contact
        assert_eq!(
            c.event_count(EventKind::TriggerAbility, f32::NAN, 4.0, false),
            0
        );
        let mut once = c.clone();
        once.looping = false;
        assert_eq!(
            once.event_count(EventKind::TriggerAbility, 0.0, 100.0, true),
            1
        );
        assert_eq!(
            once.event_count(EventKind::TriggerAbility, 1.0, 100.0, false),
            0
        );
    }
}
