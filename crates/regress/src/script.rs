//! Input scripts: what the player's buttons do, tick by tick, as run-length
//! segments of [`RawInput`]. Scenarios build them in code, the desktop
//! records them (`--record`), and both serialize to TOML.

use game::controls::RawInput;
use game::state::TICK;
use serde::{Deserialize, Serialize};

/// Ticks in `seconds` of simulation, rounded to the nearest tick.
pub fn ticks(seconds: f32) -> u32 {
    (seconds / TICK).round() as u32
}

/// Buttons held for a run of ticks. Mirrors [`RawInput`] field for field.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    pub ticks: u32,
    /// -1 left, 0 none, 1 right.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub dir: i8,
    #[serde(default, skip_serializing_if = "is_false")]
    pub sprint: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub pay: bool,
    /// Coin bag tapped. Counts once per frame, like the touch screen.
    #[serde(default, skip_serializing_if = "is_false")]
    pub tap: bool,
}

fn is_zero(v: &i8) -> bool {
    *v == 0
}
fn is_false(v: &bool) -> bool {
    !*v
}

impl Segment {
    pub fn raw(&self) -> RawInput {
        RawInput {
            direction: self.dir,
            sprint: self.sprint,
            pay: self.pay,
            bag_tap: self.tap,
        }
    }

    fn same_input(&self, raw: &RawInput) -> bool {
        (self.dir, self.sprint, self.pay, self.tap)
            == (raw.direction, raw.sprint, raw.pay, raw.bag_tap)
    }
}

/// A whole input sequence.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Script {
    #[serde(default, rename = "segment")]
    pub segments: Vec<Segment>,
}

impl Script {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len_ticks(&self) -> u32 {
        self.segments.iter().map(|s| s.ticks).sum()
    }

    /// Appends one tick of `raw`, merging with the last segment when equal.
    /// This is the recorder.
    pub fn push(&mut self, raw: RawInput, ticks: u32) {
        if ticks == 0 {
            return;
        }
        if let Some(last) = self.segments.last_mut() {
            // A tap is an edge: never merge it into a longer run.
            if last.same_input(&raw) && !raw.bag_tap {
                last.ticks += ticks;
                return;
            }
        }
        self.segments.push(Segment {
            ticks,
            dir: raw.direction,
            sprint: raw.sprint,
            pay: raw.pay,
            tap: raw.bag_tap,
        });
    }

    /// Every tick's input, in order.
    pub fn inputs(&self) -> impl Iterator<Item = RawInput> + '_ {
        self.segments
            .iter()
            .flat_map(|s| std::iter::repeat_n(s.raw(), s.ticks as usize))
    }

    // Builders, in seconds. Durations round to whole ticks.

    pub fn hold(mut self, raw: RawInput, seconds: f32) -> Self {
        self.push(raw, ticks(seconds));
        self
    }

    pub fn idle(self, seconds: f32) -> Self {
        self.hold(RawInput::default(), seconds)
    }

    pub fn walk(self, dir: i8, seconds: f32) -> Self {
        self.hold(
            RawInput {
                direction: dir,
                ..Default::default()
            },
            seconds,
        )
    }

    /// Holds the sprint button while walking: a gallop for as long as the
    /// steed has stamina.
    pub fn gallop(self, dir: i8, seconds: f32) -> Self {
        self.hold(
            RawInput {
                direction: dir,
                sprint: true,
                ..Default::default()
            },
            seconds,
        )
    }

    pub fn hold_pay(self, seconds: f32) -> Self {
        self.hold(
            RawInput {
                pay: true,
                ..Default::default()
            },
            seconds,
        )
    }

    /// Taps the coin bag for one frame of `frame_ticks` ticks (use 6 so the
    /// script lines up with 1, 2 and 3 ticks per frame).
    pub fn tap(mut self, frame_ticks: u32) -> Self {
        self.push(
            RawInput {
                bag_tap: true,
                ..Default::default()
            },
            1,
        );
        self.push(RawInput::default(), frame_ticks.saturating_sub(1));
        self
    }

    pub fn to_toml(&self) -> String {
        toml::to_string(self).expect("script serializes")
    }

    pub fn from_toml(s: &str) -> Result<Self, String> {
        toml::from_str(s).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_merges_and_round_trips() {
        let mut s = Script::new();
        let walk = RawInput {
            direction: 1,
            ..Default::default()
        };
        for _ in 0..30 {
            s.push(walk, 1);
        }
        let tap = RawInput {
            bag_tap: true,
            ..Default::default()
        };
        s.push(tap, 1);
        s.push(tap, 1);
        assert_eq!(s.segments.len(), 3, "{s:?}");
        assert_eq!(s.len_ticks(), 32);
        let back = Script::from_toml(&s.to_toml()).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.inputs().count(), 32);
    }
}
