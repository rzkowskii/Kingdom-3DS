//! Time of day: when the lighting changes, how long the lit day and the
//! dark night last, and what each part of the day looks like.
//!
//! Two users share this module:
//!
//! - the synthetic `time_of_day_lighting` scenario (CI), which runs the
//!   real blender over made-up presets and checks that each lighting phase
//!   starts on the director's schedule, every day, and nowhere else;
//! - `regress tod` (local, needs your converted assets), which picks random
//!   times of day from a seed, measures the port's lighting at each one on
//!   the real island, can take screenshots of the port and of the original
//!   (through `tools/orig-harness/`) at the same clock times, and reports how
//!   the two differ.
//!
//! The lighting is keyed to the clock only (`Director` pushes presets at
//! fixed hours), so samples are matched between games by clock time, never
//! by frame.

use std::collections::BTreeMap;
use std::path::Path;

use game::lighting::{color, float};
use game::state::GameState;
use game::time::{PartOfDay, SECONDS_PER_HOUR, TRANSITIONS};

/// The blended lighting at one tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightSample {
    pub tick: u32,
    pub day: u32,
    pub time: f32,
    /// Gameplay daytime (`Kingdom.isDaytime`).
    pub daytime: bool,
    pub ambient: [f32; 4],
    pub sky_top: [f32; 4],
    pub sky_bottom: [f32; 4],
    pub stars: f32,
    pub fog: f32,
}

impl LightSample {
    pub fn of(tick: u32, g: &GameState) -> Self {
        let l = &g.blender.state;
        LightSample {
            tick,
            day: g.clock.day,
            time: g.clock.time,
            daytime: g.days.is_daytime,
            ambient: l.colors[color::AMBIENT],
            sky_top: l.colors[color::SKY_TOP],
            sky_bottom: l.colors[color::SKY_BOTTOM],
            stars: l.floats[float::STARS],
            fog: l.floats[float::FOG_DENSITY],
        }
    }

    /// Hours since 00:00 on day 0.
    pub fn hours(&self) -> f64 {
        self.day as f64 * 24.0 + self.time as f64
    }
}

/// Perceived brightness. orig: `Color.grayscale`.
pub fn luma(c: [f32; 4]) -> f64 {
    0.299 * c[0] as f64 + 0.587 * c[1] as f64 + 0.114 * c[2] as f64
}

/// `"r,g,b,a"` as the harness writes Unity colours (and the port mirrors).
pub fn parse_color(s: &str) -> Option<[f32; 4]> {
    let v: Vec<f32> = s.split(',').filter_map(|p| p.parse().ok()).collect();
    (v.len() == 4).then(|| [v[0], v[1], v[2], v[3]])
}

pub fn format_color(c: [f32; 4]) -> String {
    format!("{},{},{},{}", c[0], c[1], c[2], c[3])
}

pub fn part_name(p: PartOfDay) -> &'static str {
    match p {
        PartOfDay::Dawn => "dawn",
        PartOfDay::Day => "day",
        PartOfDay::Evening => "evening",
        PartOfDay::Night => "night",
    }
}

/// A change smaller than this per sample is no change (f32 noise).
const STILL: f64 = 1e-5;
/// A change that starts at least this long after the previous one is the
/// onset of a new lighting phase.
const QUIET_H: f64 = 0.25;
/// An onset this close to a scheduled transition start belongs to it.
const MATCH_H: f64 = 1.0;

/// Hours where the ambient light starts changing after a quiet stretch.
/// `series` is `(hours since day 0 00:00, ambient luma)`, in time order.
pub fn onsets(series: &[(f64, f64)]) -> Vec<f64> {
    let mut out = Vec::new();
    let mut last_change = f64::NEG_INFINITY;
    for w in series.windows(2) {
        if (w[1].1 - w[0].1).abs() > STILL {
            if w[1].0 - last_change >= QUIET_H {
                // The change began somewhere between the two samples.
                out.push(0.5 * (w[0].0 + w[1].0));
            }
            last_change = w[1].0;
        }
    }
    out
}

/// The scheduled transition an onset at `hours` belongs to, if any.
fn scheduled(hours: f64) -> Option<PartOfDay> {
    let tod = hours.rem_euclid(24.0);
    TRANSITIONS
        .iter()
        .map(|&(p, start, _)| {
            let d = (tod - start as f64).rem_euclid(24.0);
            (p, d.min(24.0 - d))
        })
        .filter(|&(_, d)| d <= MATCH_H)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(p, _)| p)
}

/// Ambient luma at `hours`, linearly interpolated (NaN outside the series).
pub fn luma_at(series: &[(f64, f64)], hours: f64) -> f64 {
    let i = series.partition_point(|s| s.0 < hours);
    match (i.checked_sub(1).and_then(|j| series.get(j)), series.get(i)) {
        (_, Some(b)) if b.0 == hours => b.1,
        (Some(a), Some(b)) => a.1 + (b.1 - a.1) * (hours - a.0) / (b.0 - a.0),
        _ => f64::NAN,
    }
}

/// Lighting metrics over a run that covers at least day 1 (24 h – 48 h) and
/// the start of day 2:
///
/// - `{dawn,day,evening,night}_light_start_h`: the hour of day 1 when that
///   phase's lighting starts to blend in;
/// - `light_cycle_s`: real seconds from day 1's dawn light to day 2's;
/// - `unscheduled_light_changes`: light changes that start away from any
///   scheduled transition (should be none: nothing else moves the lighting
///   of an idle monarch in the open);
/// - `lit_hours` / `dark_hours`: hours of day 1 brighter / darker than
///   halfway between its darkest and brightest ambient light;
/// - `{part}_ambient`: ambient luma once each phase has settled on day 1.
pub fn light_metrics(series: &[(f64, f64)]) -> BTreeMap<String, f64> {
    let mut m = BTreeMap::new();
    let on = onsets(series);
    let unscheduled = on.iter().filter(|&&h| scheduled(h).is_none()).count();
    m.insert("unscheduled_light_changes".into(), unscheduled as f64);
    let start = |day: u32, part: PartOfDay| {
        on.iter()
            .copied()
            .filter(|&h| h >= day as f64 * 24.0 && h < (day + 1) as f64 * 24.0)
            .find(|&h| scheduled(h) == Some(part))
    };
    for (part, ..) in TRANSITIONS {
        let h = start(1, part).map_or(f64::NAN, |h| h - 24.0);
        m.insert(format!("{}_light_start_h", part_name(part)), h);
    }
    m.insert(
        "light_cycle_s".into(),
        match (start(1, PartOfDay::Dawn), start(2, PartOfDay::Dawn)) {
            (Some(a), Some(b)) => (b - a) * SECONDS_PER_HOUR as f64,
            _ => f64::NAN,
        },
    );
    let day1: Vec<f64> = series
        .iter()
        .filter(|s| (24.0..48.0).contains(&s.0))
        .map(|s| s.1)
        .collect();
    if day1.len() > 1 {
        let lo = day1.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = day1.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let mid = 0.5 * (lo + hi);
        let lit = day1.iter().filter(|&&l| l > mid).count() as f64 / day1.len() as f64 * 24.0;
        m.insert("lit_hours".into(), lit);
        m.insert("dark_hours".into(), 24.0 - lit);
    }
    for (part, h) in SETTLED {
        m.insert(
            format!("{}_ambient", part_name(part)),
            luma_at(series, 24.0 + h),
        );
    }
    m
}

/// An hour of day 1 well after each phase has finished blending in.
pub const SETTLED: [(PartOfDay, f64); 4] = [
    (PartOfDay::Dawn, 7.9),
    (PartOfDay::Day, 15.0),
    (PartOfDay::Evening, 21.0),
    (PartOfDay::Night, 23.9),
];

/// Gameplay day and night lengths (`Kingdom.isDaytime` edges), in seconds:
/// `(daytime_s, night_s)` from the first full day/night after the start.
pub fn daytime_lengths(edges: &[(f64, bool)]) -> (f64, f64) {
    let up = edges.iter().position(|e| e.1);
    let Some(i) = up else {
        return (f64::NAN, f64::NAN);
    };
    let at = |j: usize| edges.get(j).map_or(f64::NAN, |e| e.0);
    let s = SECONDS_PER_HOUR as f64;
    ((at(i + 1) - at(i)) * s, (at(i + 2) - at(i + 1)) * s)
}

/// The eight windows sample times are drawn from: each phase's blend and
/// the stable stretch after it (`(name, start, end)` in hours of the day;
/// the night's stable stretch runs past midnight).
pub fn windows() -> [(&'static str, f32, f32); 8] {
    let t = TRANSITIONS;
    [
        ("dawn blend", t[0].1, t[0].2),
        ("dawn", t[0].2, t[1].1),
        ("day blend", t[1].1, t[1].2),
        ("day", t[1].2, t[2].1),
        ("evening blend", t[2].1, t[2].2),
        ("evening", t[2].2, t[3].1),
        ("night blend", t[3].1, t[3].2),
        ("night", t[3].2, t[0].1 + 24.0),
    ]
}

/// A time to sample: `(day, hour, window name)`.
#[derive(Clone, Debug, PartialEq)]
pub struct SampleTime {
    pub day: u32,
    pub hour: f32,
    pub window: &'static str,
}

impl SampleTime {
    pub fn hours(&self) -> f64 {
        self.day as f64 * 24.0 + self.hour as f64
    }
    pub fn label(&self) -> String {
        let h = self.hour.floor();
        let min = ((self.hour - h) * 60.0).floor();
        format!("day {} {:02}:{:02.0}", self.day, h as u32, min)
    }
}

/// `n` random times on days `1..=days` (day 0's "First" lighting is
/// skipped), spread over all eight windows, in time order. The same seed
/// always picks the same times.
pub fn pick_times(seed: u32, n: usize, days: u32) -> Vec<SampleTime> {
    let mut rng = game::rng::Rng::new(seed.max(1));
    let w = windows();
    // Shuffle the window order so a short run doesn't always favour dawn.
    let mut order: Vec<usize> = (0..w.len()).collect();
    for i in (1..order.len()).rev() {
        order.swap(i, rng.range_i32(0, i as i32 + 1) as usize);
    }
    let mut out: Vec<SampleTime> = (0..n)
        .map(|i| {
            let (name, a, b) = w[order[i % w.len()]];
            let day = rng.range_i32(1, days.max(1) as i32 + 1) as u32;
            // Stay a little inside the window so a sample is clearly in it.
            let pad = (b - a) * 0.05;
            let mut hour = rng.range(a + pad, b - pad);
            let mut day = day;
            if hour >= 24.0 {
                hour -= 24.0;
                day += 1;
            }
            SampleTime {
                day,
                hour,
                window: name,
            }
        })
        .collect();
    out.sort_by(|a, b| a.hours().total_cmp(&b.hours()));
    out
}

// ---- Weather ---------------------------------------------------------------

/// Weather names and flag values. orig: `BlenderState.Weather`; the
/// lighting presets are named after them ("Clear Summer Day").
pub const WEATHERS: [(&str, u16); 9] = [
    ("Clear", 0x1),
    ("Cloudy", 0x2),
    ("Foggy", 0x4),
    ("Overcast", 0x8),
    ("Fresh", 0x10),
    ("Precipitation", 0x20),
    ("HeavyPrecipitation", 0x40),
    ("First", 0x80),
    ("Dusty", 0x100),
];

/// The flag value of a weather name (case doesn't matter).
pub fn weather_flag(name: &str) -> Option<u16> {
    WEATHERS
        .iter()
        .find(|w| w.0.eq_ignore_ascii_case(name))
        .map(|w| w.1)
}

/// Weathers with a summer preset for every part of the day in `world`:
/// the ones a forced-weather run can show in full.
pub fn summer_weathers(world: &game::data::WorldData) -> Vec<(&'static str, u16)> {
    WEATHERS
        .iter()
        .copied()
        .filter(|&(_, flag)| {
            (0..4u8).all(|part| {
                world.lights.iter().any(|l| {
                    !l.test_only
                        && l.category == 1
                        && l.weather == flag
                        && l.season == 1
                        && l.part_of_day == part
                })
            })
        })
        .collect()
}

// ---- Images --------------------------------------------------------------

/// An RGBA8 image.
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

impl Image {
    pub fn load(path: &Path) -> Result<Image, String> {
        let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut dec = png::Decoder::new(std::io::BufReader::new(file));
        dec.set_transformations(png::Transformations::normalize_to_color8());
        let mut r = dec
            .read_info()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let size = r
            .output_buffer_size()
            .ok_or_else(|| format!("{}: image too large", path.display()))?;
        let mut buf = vec![0; size];
        let info = r
            .next_frame(&mut buf)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        buf.truncate(info.buffer_size());
        let px = match info.color_type {
            png::ColorType::Rgba => buf,
            png::ColorType::Rgb => buf
                .chunks(3)
                .flat_map(|c| [c[0], c[1], c[2], 255])
                .collect(),
            png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
            png::ColorType::GrayscaleAlpha => buf
                .chunks(2)
                .flat_map(|c| [c[0], c[0], c[0], c[1]])
                .collect(),
            png::ColorType::Indexed => return Err(format!("{}: indexed PNG", path.display())),
        };
        Ok(Image {
            w: info.width,
            h: info.height,
            px,
        })
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), self.w, self.h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()
            .and_then(|mut w| w.write_image_data(&self.px))
            .map_err(|e| format!("{}: {e}", path.display()))
    }

    /// RGB at (x, y), alpha ignored: the desktop's framebuffer carries alpha
    /// below 1 where haze blends, which isn't what's on screen.
    fn rgb(&self, x: u32, y: u32) -> [f64; 3] {
        let i = ((y * self.w + x) * 4) as usize;
        [
            self.px[i] as f64 / 255.0,
            self.px[i + 1] as f64 / 255.0,
            self.px[i + 2] as f64 / 255.0,
        ]
    }

    /// The part `(x, y, w, h)`, alpha forced opaque.
    pub fn crop(&self, (x0, y0, w, h): (u32, u32, u32, u32)) -> Image {
        let mut px = Vec::with_capacity((w * h * 4) as usize);
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                let i = ((y * self.w + x) * 4) as usize;
                px.extend_from_slice(&[self.px[i], self.px[i + 1], self.px[i + 2], 255]);
            }
        }
        Image { w, h, px }
    }

    /// Nearest-neighbour resize to height `h`, keeping the aspect ratio.
    pub fn to_height(&self, h: u32) -> Image {
        let w = (self.w as u64 * h as u64 / self.h.max(1) as u64).max(1) as u32;
        let mut px = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            let sy = y * self.h / h;
            for x in 0..w {
                let sx = x * self.w / w;
                let i = ((sy * self.w + sx) * 4) as usize;
                px.extend_from_slice(&self.px[i..i + 4]);
            }
        }
        Image { w, h, px }
    }

    /// Images side by side with a gap, top-aligned on a dark background.
    pub fn side_by_side(parts: &[&Image]) -> Image {
        let gap = 8;
        let h = parts.iter().map(|p| p.h).max().unwrap_or(1);
        let w = parts.iter().map(|p| p.w).sum::<u32>() + gap * parts.len().saturating_sub(1) as u32;
        let mut px = [24u8, 24, 24, 255].repeat((w * h) as usize);
        let mut x0 = 0;
        for p in parts {
            for y in 0..p.h {
                let src = (y * p.w * 4) as usize;
                let dst = ((y * w + x0) * 4) as usize;
                px[dst..dst + (p.w * 4) as usize]
                    .copy_from_slice(&p.px[src..src + (p.w * 4) as usize]);
            }
            x0 += p.w + gap;
        }
        Image { w, h, px }
    }
}

/// The top screen inside a desktop screenshot (`--screenshot`): the window
/// size varies with the window manager, so it's worked out from the image
/// size the way the desktop lays the screens out.
pub fn desktop_top_screen(w: u32, h: u32) -> (u32, u32, u32, u32) {
    let (tw, th, bh) = (scene::top::W, scene::top::H, scene::touch::H);
    let scale = (w as f32 / tw).min(h as f32 / (th + bh)).floor().max(1.0);
    let (sw, sh) = ((tw * scale) as u32, (th * scale) as u32);
    let x = (w.saturating_sub(sw)) / 2;
    (x, 0, sw.min(w - x), sh.min(h))
}

/// What a screenshot looks like, in numbers. Bands are thirds of the
/// height: the sky (top), the middle (trees, buildings, the monarch) and
/// the bottom (ground and water).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ImageStats {
    pub luma: f64,
    pub sky: f64,
    pub middle: f64,
    pub bottom: f64,
    pub rgb: [f64; 3],
    /// Mean red minus mean blue: warm (fire, sunset) above 0, cold below.
    pub warmth: f64,
    /// Standard deviation of luma.
    pub contrast: f64,
}

impl ImageStats {
    pub fn of(img: &Image) -> ImageStats {
        let mut band = [(0.0, 0u64); 3];
        let mut rgb = [0.0; 3];
        let (mut sum, mut sum2, mut n) = (0.0, 0.0, 0u64);
        // Every other pixel is plenty, and keeps 1080p screenshots quick.
        for y in (0..img.h).step_by(2) {
            let b = ((y * 3) / img.h.max(1)).min(2) as usize;
            for x in (0..img.w).step_by(2) {
                let c = img.rgb(x, y);
                let l = 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2];
                band[b].0 += l;
                band[b].1 += 1;
                for k in 0..3 {
                    rgb[k] += c[k];
                }
                sum += l;
                sum2 += l * l;
                n += 1;
            }
        }
        let n = n.max(1) as f64;
        let mean = |b: (f64, u64)| b.0 / b.1.max(1) as f64;
        let rgb = rgb.map(|v| v / n);
        let luma = sum / n;
        ImageStats {
            luma,
            sky: mean(band[0]),
            middle: mean(band[1]),
            bottom: mean(band[2]),
            rgb,
            warmth: rgb[0] - rgb[2],
            contrast: (sum2 / n - luma * luma).max(0.0).sqrt(),
        }
    }
}

/// How the port's screenshot differs from the original's at the same time
/// of day: empty when they look alike. Islands, camera and scale differ
/// between the two games, so only big differences are reported.
pub fn differences(port: &ImageStats, orig: &ImageStats) -> Vec<String> {
    let mut out = Vec::new();
    let ratio = |a: f64, b: f64| (a + 0.02) / (b + 0.02);
    for (name, p, o) in [
        ("overall", port.luma, orig.luma),
        ("sky", port.sky, orig.sky),
        // Trees, buildings and the monarch: where fog and mist show most.
        ("middle", port.middle, orig.middle),
        ("ground", port.bottom, orig.bottom),
    ] {
        let r = ratio(p, o);
        if r < 0.75 {
            out.push(format!("{name} darker ({p:.2} vs {o:.2})"));
        } else if r > 1.33 {
            out.push(format!("{name} brighter ({p:.2} vs {o:.2})"));
        }
    }
    let dw = port.warmth - orig.warmth;
    if dw > 0.06 {
        out.push(format!(
            "warmer ({:+.2} vs {:+.2})",
            port.warmth, orig.warmth
        ));
    } else if dw < -0.06 {
        out.push(format!(
            "colder ({:+.2} vs {:+.2})",
            port.warmth, orig.warmth
        ));
    }
    if port.contrast < orig.contrast * 0.6 {
        out.push(format!(
            "flatter ({:.2} vs {:.2} contrast)",
            port.contrast, orig.contrast
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn onsets_find_each_new_change_once() {
        // Still, then a ramp from 5.0 h to 6.0 h, then still.
        let s: Vec<(f64, f64)> = (0..800)
            .map(|i| {
                let h = i as f64 * 0.01;
                (h, ((h - 5.0).clamp(0.0, 1.0)) * 0.5)
            })
            .collect();
        let on = onsets(&s);
        assert_eq!(on.len(), 1, "{on:?}");
        assert!((on[0] - 5.005).abs() < 0.01, "{on:?}");
        assert_eq!(scheduled(on[0]), Some(PartOfDay::Dawn));
        assert_eq!(scheduled(11.0), None);
        assert_eq!(scheduled(21.3 + 48.0), Some(PartOfDay::Night));
        assert!((luma_at(&s, 5.5) - 0.25).abs() < 1e-9);
    }

    #[test]
    fn picked_times_cover_every_window_and_repeat_per_seed() {
        let a = pick_times(7, 16, 2);
        assert_eq!(a, pick_times(7, 16, 2));
        assert_ne!(a, pick_times(8, 16, 2));
        for (name, ..) in windows() {
            assert_eq!(a.iter().filter(|t| t.window == name).count(), 2, "{name}");
        }
        assert!(a.windows(2).all(|w| w[0].hours() <= w[1].hours()));
        assert!(a
            .iter()
            .all(|t| t.day >= 1 && (0.0..24.0).contains(&t.hour)));
    }

    #[test]
    fn weathers_by_name() {
        assert_eq!(weather_flag("clear"), Some(1));
        assert_eq!(weather_flag("HeavyPrecipitation"), Some(0x40));
        assert_eq!(weather_flag("Sunny"), None);
        let w = crate::worlds::lit_world();
        assert_eq!(summer_weathers(&w), vec![("Clear", 1)]);
    }

    #[test]
    fn image_stats_and_differences() {
        let flat = |v: u8, w: u32, h: u32| Image {
            w,
            h,
            px: [v, v, v, 128].repeat((w * h) as usize),
        };
        let s = ImageStats::of(&flat(255, 8, 6));
        assert!((s.luma - 1.0).abs() < 1e-6 && s.contrast < 1e-6 && s.warmth.abs() < 1e-6);
        let dark = ImageStats::of(&flat(40, 8, 6));
        assert!(differences(&dark, &s).iter().any(|d| d.contains("darker")));
        assert!(differences(&s, &s).is_empty());
        // A 2x desktop window with both screens stacked.
        assert_eq!(desktop_top_screen(800, 960), (0, 0, 800, 480));
        assert_eq!(desktop_top_screen(1000, 960), (100, 0, 800, 480));
        let sbs = Image::side_by_side(&[&flat(0, 4, 4), &flat(0, 2, 2).to_height(4)]);
        assert_eq!((sbs.w, sbs.h), (4 + 8 + 4, 4));
    }
}
