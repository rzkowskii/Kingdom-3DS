//! Day scheduling: which kind of day comes next, its weather and lighting
//! transitions, season changes, and when waves are due.
//!
//! orig: `Director` (`AddCycle`, `AddDay`, `PickDays`, `AddTransitionEvent`,
//! `AddWaveEvents`, `Update`), `DirectorEvent`, `Day.GetRandomWeatherOption()`.

use alloc::vec::Vec;

use crate::data::{SoundId, WorldData};
use crate::lighting::Blender;
use crate::rng::Rng;
use crate::time::{Clock, SECONDS_PER_HOUR, TRANSITIONS};

/// orig: `Season` flags.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Season {
    Summer = 1,
    Autumn = 2,
    Winter = 4,
}

/// Which wave a day gets. orig: `regularWave` / `bossWave` / `recoveryWave` per season.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaveKind {
    Regular,
    Boss,
    Recovery,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EventKind {
    /// orig: `DirectorEvent.Type.PushBlenderState`; duration in game hours.
    Light { preset: u16, duration: f32 },
    /// orig: `DirectorEvent.Type.StartSeason`.
    StartSeason(Season),
    /// A wave is due today. orig: `DirectorEvent.Type.ScheduleWave` (side and
    /// spawn time are worked out by the greed system).
    Wave(WaveKind),
    /// orig: `DirectorEvent.Type.PlayTrack` (queued after the current one).
    Track(SoundId),
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Event {
    pub day: u32,
    pub time: f32,
    pub kind: EventKind,
}

impl Event {
    /// orig: `DirectorEvent.date`.
    pub fn date(&self) -> f32 {
        self.day as f32 * 24.0 + self.time
    }
}

/// Things the director hands to other systems when an event fires.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DirectorOutput {
    Season(Season),
    Wave(WaveKind),
    Track(SoundId),
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug)]
pub struct Director {
    events: Vec<Event>,
    /// orig: `_lastDayAdded`, -1 before the first cycle.
    last_day_added: i32,
    adding_for_season: Season,
    begin_autumn: i32,
    begin_winter: i32,
    /// Weather of each scheduled day (orig: picked in `AddDay`), for debugging and UI.
    pub weather_by_day: Vec<u16>,
    /// Dev and regression tools: every day gets this weather instead of
    /// its random pick (see [`Director::force_weather`]). Not saved.
    #[cfg_attr(feature = "serde", serde(skip))]
    forced_weather: Option<u16>,
}

impl Director {
    pub(crate) fn lights_valid(&self, world: &WorldData) -> bool {
        self.events.iter().all(|e| match e.kind {
            EventKind::Light { preset, .. } => (preset as usize) < world.lights.len(),
            _ => true,
        })
    }
    /// orig: `Director.Init()` + `OnLevelLoaded()`.
    pub fn new(world: &WorldData, level_index: usize, rng: &mut Rng) -> Self {
        let cfg = &world.levels[level_index];
        let range = |(min, max): (i32, i32), rng: &mut Rng| rng.range_i32(min, max + 1);
        Self {
            events: Vec::new(),
            last_day_added: -1,
            adding_for_season: Season::Summer,
            begin_autumn: range(cfg.autumn_start, rng),
            begin_winter: range(cfg.winter_start, rng),
            weather_by_day: Vec::new(),
            forced_weather: None,
        }
    }

    /// Dev and regression tools: from now on every day has `weather` (an
    /// `orig: BlenderState.Weather` flag value), including the lighting
    /// already scheduled. The random pick still happens, so the RNG and
    /// with it gameplay stay as they were; only the lighting changes.
    /// Parts of the day without a preset for that weather keep theirs.
    pub fn force_weather(&mut self, world: &WorldData, weather: u16) {
        self.forced_weather = Some(weather);
        self.weather_by_day.fill(weather);
        for e in &mut self.events {
            if let EventKind::Light { preset, .. } = &mut e.kind {
                let l = &world.lights[*preset as usize];
                if l.category != 1 {
                    continue;
                }
                let season = match l.season {
                    2 => Season::Autumn,
                    4 => Season::Winter,
                    _ => Season::Summer,
                };
                if let Some(p) = Self::weather_preset(world, weather, season, l.part_of_day) {
                    *preset = p;
                }
            }
        }
    }

    /// orig: `Director.GetSeasonFromDay()`.
    pub fn season_of(&self, day: i32) -> Season {
        if day < self.begin_autumn {
            Season::Summer
        } else if day < self.begin_winter {
            Season::Autumn
        } else {
            Season::Winter
        }
    }

    pub fn pending(&self) -> &[Event] {
        &self.events
    }

    fn add_event(&mut self, e: Event) {
        // Keep sorted by date; stable for equal dates like List.Sort is not, but
        // ties only happen between unrelated events.
        let at = self.events.partition_point(|x| x.date() <= e.date());
        self.events.insert(at, e);
    }

    /// orig: `Day.GetRandomWeatherOption()`.
    fn random_weather(options: u16, rng: &mut Rng) -> u16 {
        let bits: Vec<u16> = (0..16)
            .map(|b| 1u16 << b)
            .filter(|b| options & b != 0)
            .collect();
        if bits.is_empty() {
            return 1;
        }
        bits[rng.range_i32(0, bits.len() as i32) as usize]
    }

    /// orig: `Holder.GetBlenderWeatherState()`.
    fn weather_preset(world: &WorldData, weather: u16, season: Season, part: u8) -> Option<u16> {
        world
            .lights
            .iter()
            .position(|l| {
                !l.test_only
                    && l.category == 1
                    && l.weather == weather
                    && l.season == season as u8
                    && l.part_of_day == part
            })
            .map(|i| i as u16)
    }

    /// orig: `Director.AddDay()`.
    fn add_day(&mut self, world: &WorldData, day: u16, wave: Option<WaveKind>, rng: &mut Rng) {
        self.last_day_added += 1;
        let d = self.last_day_added as u32;
        let random = Self::random_weather(world.days[day as usize].weathers, rng);
        let weather = self.forced_weather.unwrap_or(random);
        self.weather_by_day.push(weather);
        for (part, (_, start, end)) in TRANSITIONS.iter().enumerate() {
            // A forced weather without this part of the day keeps the
            // random pick's preset (as `force_weather` does).
            if let Some(preset) =
                Self::weather_preset(world, weather, self.adding_for_season, part as u8).or_else(
                    || Self::weather_preset(world, random, self.adding_for_season, part as u8),
                )
            {
                self.add_event(Event {
                    day: d,
                    time: *start,
                    kind: EventKind::Light {
                        preset,
                        duration: end - start,
                    },
                });
            }
        }
        if let Some(kind) = wave {
            self.add_event(Event {
                day: d,
                time: 0.0,
                kind: EventKind::Wave(kind),
            });
        }
        // orig: Director.AddTrackEvent: start early enough that the track's
        // anchor lands on its hour.
        for t in &world.days[day as usize].tracks {
            let mut start = t.anchor_time - t.anchor_seconds / SECONDS_PER_HOUR;
            if start < 0.0 {
                start += 24.0;
            }
            let days = libm::floorf(start / 24.0) as u32;
            self.add_event(Event {
                day: d + days,
                time: start - days as f32 * 24.0,
                kind: EventKind::Track(t.sound),
            });
        }
    }

    /// orig: `Director.AddFirstDayOfSeason()`.
    fn add_first_day_of_season(&mut self, world: &WorldData, season: Season, rng: &mut Rng) {
        self.add_event(Event {
            day: (self.last_day_added + 1) as u32,
            time: 0.0,
            kind: EventKind::StartSeason(season),
        });
        self.adding_for_season = season;
        let day = match season {
            Season::Autumn => world.director.first_autumn_day,
            Season::Winter => world.director.first_winter_day,
            Season::Summer => return,
        };
        self.add_day(world, day, Some(WaveKind::Regular), rng);
    }

    /// orig: `Director.PickDays()`: draws without replacement, refilling when empty.
    fn pick_days(
        &self,
        world: &WorldData,
        options: &[u16],
        amount: usize,
        exclude: Option<u16>,
        rng: &mut Rng,
    ) -> Vec<u16> {
        let mut out = Vec::with_capacity(amount);
        let mut pool: Vec<u16> = Vec::new();
        for _ in 0..amount {
            if pool.is_empty() {
                pool = options
                    .iter()
                    .copied()
                    .filter(|&d| world.days[d as usize].seasons & self.adding_for_season as u8 != 0)
                    .collect();
                if let Some(ex) = exclude {
                    if pool.len() > 1 {
                        if let Some(i) = pool.iter().position(|&d| d == ex) {
                            pool.remove(i);
                        }
                    }
                }
                if pool.is_empty() {
                    break;
                }
            }
            let i = rng.range_i32(0, pool.len() as i32) as usize;
            out.push(pool.remove(i));
        }
        out
    }

    /// orig: `Director.AddCycle()`: regular days, a boss day, a recovery day.
    fn add_cycle(&mut self, world: &WorldData, rng: &mut Rng) {
        let dir = &world.director;
        let mut exclude = None;
        if self.last_day_added == -1 {
            self.add_day(world, dir.first_day, Some(WaveKind::Regular), rng);
            exclude = Some(dir.first_day);
        }
        let (min, max) = dir.num_regular_days;
        let n = rng.range_i32(min, max + 1).max(0) as usize;
        for d in self.pick_days(world, &dir.regular_days, n, exclude, rng) {
            self.add_day(world, d, Some(WaveKind::Regular), rng);
        }
        if let Some(&d) = self.pick_days(world, &dir.boss_days, 1, None, rng).first() {
            self.add_day(world, d, Some(WaveKind::Boss), rng);
        }
        if let Some(&d) = self
            .pick_days(world, &dir.recovery_days, 1, None, rng)
            .first()
        {
            self.add_day(world, d, Some(WaveKind::Recovery), rng);
        }
        if self.adding_for_season == Season::Summer && self.last_day_added >= self.begin_autumn {
            self.add_first_day_of_season(world, Season::Autumn, rng);
        } else if self.adding_for_season == Season::Autumn
            && self.last_day_added >= self.begin_winter
        {
            self.add_first_day_of_season(world, Season::Winter, rng);
        }
    }

    /// Runs after the clock advanced. Fires at most one due event per tick,
    /// like the original. orig: `Director.Update()`.
    pub fn update(
        &mut self,
        world: &WorldData,
        clock: &Clock,
        blender: &mut Blender,
        rng: &mut Rng,
        mut out: impl FnMut(DirectorOutput),
    ) {
        if clock.day as i32 > self.last_day_added {
            self.add_cycle(world, rng);
        }
        let Some(e) = self.events.first().copied() else {
            return;
        };
        if clock.date() > e.date() {
            self.events.remove(0);
            match e.kind {
                EventKind::Light { preset, duration } => {
                    blender.add(world, preset, duration * SECONDS_PER_HOUR / clock.speed)
                }
                EventKind::StartSeason(s) => out(DirectorOutput::Season(s)),
                EventKind::Wave(k) => out(DirectorOutput::Wave(k)),
                EventKind::Track(t) => out(DirectorOutput::Track(t)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::*;
    use alloc::string::ToString;
    use alloc::vec;

    #[test]
    fn forced_weather_changes_only_the_lighting() {
        let mut w = world();
        // A second weather (2) with its own presets, in summer only.
        let extra: Vec<_> = w
            .lights
            .iter()
            .filter(|l| l.season == 1)
            .map(|l| LightPreset {
                weather: 2,
                ..l.clone()
            })
            .collect();
        w.lights.extend(extra);
        let run = |force: bool| {
            let mut rng = Rng::new(5);
            let mut d = Director::new(&w, 0, &mut rng);
            let mut blender = Blender::new(&w);
            let mut clock = Clock::new();
            let mut pushed = Vec::new();
            for _ in 0..(14 * 240 * 60) {
                clock.advance(1.0 / 60.0);
                if force && d.forced_weather.is_none() && clock.time > 4.5 {
                    // After day 0's lighting was scheduled.
                    d.force_weather(&w, 2);
                }
                let next = d.events.first().copied();
                let before = d.events.len();
                d.update(&w, &clock, &mut blender, &mut rng, |_| {});
                if d.events.len() < before {
                    if let Some(Event {
                        kind: EventKind::Light { preset, .. },
                        day,
                        ..
                    }) = next
                    {
                        let l = &w.lights[preset as usize];
                        pushed.push((day, l.season, l.weather));
                    }
                }
            }
            (rng.next_u32(), pushed)
        };
        let (free_rng, free) = run(false);
        let (forced_rng, forced) = run(true);
        assert_eq!(free_rng, forced_rng, "forcing must not touch the RNG");
        assert!(free.iter().all(|p| p.2 == 1), "{free:?}");
        assert!(forced.len() > 20 && forced.len() == free.len());
        // Summer presets switch; autumn and winter have no weather 2, so
        // they keep the random pick's.
        assert!(
            forced
                .iter()
                .all(|&(_, season, wx)| wx == if season == 1 { 2 } else { 1 }),
            "{forced:?}"
        );
        assert!(
            forced.iter().any(|p| p.1 != 1),
            "reaches autumn: {forced:?}"
        );
    }

    fn world() -> WorldData {
        let day = |name: &str, seasons: u8| DayDef {
            name: name.to_string(),
            seasons,
            weathers: 1,
            tracks: vec![],
        };
        let mut lights = Vec::new();
        for season in [1u8, 2, 4] {
            for part in 0..4u8 {
                lights.push(LightPreset {
                    name: "x".to_string(),
                    category: 1,
                    weather: 1,
                    season,
                    part_of_day: part,
                    test_only: false,
                    enabled: 0,
                    colors: [[0.0; 4]; 17],
                    floats: [0.0; 8],
                });
            }
        }
        WorldData {
            days: vec![
                day("first", 7),
                day("autumn first", 7),
                day("winter first", 7),
                day("r1", 7),
                day("r2", 7),
                day("r3", 7),
                day("boss", 7),
                day("recovery", 7),
            ],
            lights,
            director: DirectorDef {
                first_day: 0,
                first_autumn_day: 1,
                first_winter_day: 2,
                regular_days: vec![0, 3, 4, 5],
                boss_days: vec![6],
                recovery_days: vec![7],
                num_regular_days: (3, 4),
                initial_lights: vec![],
            },
            levels: vec![LevelConfigDef {
                name: "t".to_string(),
                autumn_start: (6, 6),
                winter_start: (12, 12),
                days_before_game_over: -1,
                starting_coins: 0,
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
            ..Default::default()
        }
    }

    #[test]
    fn cycles_are_regular_then_boss_then_recovery() {
        let w = world();
        let mut rng = Rng::new(5);
        let mut d = Director::new(&w, 0, &mut rng);
        d.add_cycle(&w, &mut rng);
        let waves: Vec<WaveKind> = d
            .pending()
            .iter()
            .filter_map(|e| match e.kind {
                EventKind::Wave(k) => Some(k),
                _ => None,
            })
            .collect();
        // First day + 3–4 regular + boss + recovery, then (because autumn is due
        // by day 6 here) autumn's first day, which has a regular wave.
        let boss = waves
            .iter()
            .position(|&k| k == WaveKind::Boss)
            .expect("boss day");
        assert!(boss == 4 || boss == 5, "{waves:?}");
        assert!(waves[..boss].iter().all(|&k| k == WaveKind::Regular));
        assert_eq!(waves[boss + 1], WaveKind::Recovery);
        assert!(waves[boss + 2..].iter().all(|&k| k == WaveKind::Regular));
    }

    #[test]
    fn lighting_fires_on_schedule_and_seasons_change() {
        let w = world();
        let mut rng = Rng::new(9);
        let mut d = Director::new(&w, 0, &mut rng);
        let mut blender = Blender::new(&w);
        let mut clock = Clock::new();
        let mut seasons = Vec::new();
        for _ in 0..(20 * 240 * 60) {
            clock.advance(1.0 / 60.0);
            d.update(&w, &clock, &mut blender, &mut rng, |o| {
                if let DirectorOutput::Season(s) = o {
                    seasons.push((clock.day, s));
                }
            });
        }
        assert_eq!(seasons.first().map(|s| s.1), Some(Season::Autumn));
        assert!(seasons.iter().any(|s| s.1 == Season::Winter));
        // Autumn starts on the first cycle boundary at or after day 6.
        assert!(seasons[0].0 >= 6, "{seasons:?}");
    }
}
