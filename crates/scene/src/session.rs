//! A play session: the island being played, the unlocks kept across runs,
//! the map between islands, and saving. The backends feed it input and
//! draw it; everything else is the same on desktop and 3DS.

use game::controls::{ControlMapper, RawInput};
use game::data::WorldData;
use game::monarch::Action;
use game::progress::{Ending, Progress, Unveiled};
use game::save;
use game::state::{GameEvent, GameState, TICK};
use game::time;
use platform::Storage;

use crate::touch::MapChoice;

/// PC target routing: TeleporterExit and Boat disable lead; dropped crowns
/// are tracked until pickup/despawn. Target changes preserve the old screen
/// position and seek, except startup/resume which explicitly snaps.
fn camera_target(game: &GameState) -> crate::camera::Target {
    use crate::camera::{Subject, Target};
    let (subject, x, lead) = if let Some(exit) = game.teleports.exit {
        (Subject::Teleporter, exit.x, false)
    } else if let Some(crown) = &game.greed.crown {
        (Subject::Crown, crown.x, false)
    } else if let Some(o) = game
        .harbor
        .boat
        .as_ref()
        .and_then(|b| game.level.index_of(b.object))
        .map(|i| &game.level.objects[i])
        .filter(|o| o.mode == game::boat::mode::SAILING)
    {
        (Subject::Boat(o.id), o.x, false)
    } else if game.game_over {
        (Subject::Inactive, game.monarch.x, false)
    } else {
        (Subject::Monarch, game.monarch.x, true)
    };
    Target {
        subject,
        x,
        direction: game.monarch.facing as f32,
        lead,
    }
}

pub(crate) const STAR_COUNT: usize = 110;

#[derive(Debug)]
struct Stars {
    visible: usize,
    ticks: u16,
}

impl Default for Stars {
    fn default() -> Self {
        // Both authored emitters start enabled and unmasked before the first poll.
        Self {
            visible: STAR_COUNT,
            ticks: 0,
        }
    }
}

impl Stars {
    fn step(&mut self, level: f32) {
        self.ticks += 1;
        // Stars.SlowUpdate waits five seconds on the scaled scene clock.
        if self.ticks == 300 {
            self.ticks = 0;
            self.visible = if level > 0.5 {
                let mask = ((1.0 - level) * 2.0 * STAR_COUNT as f32).round_ties_even();
                STAR_COUNT.saturating_sub(mask as usize)
            } else {
                0
            };
        }
    }
}

pub struct Session {
    pub game: GameState,
    pub world: WorldData,
    pub mapper: ControlMapper,
    pub events: Vec<GameEvent>,
    /// Ticks per frame (dev fast-forward).
    pub speed: u32,
    pub paused: bool,
    pub debug: bool,
    pub camera_x: f32,
    camera: crate::camera::Camera,
    pub boulder_particles: crate::boulder_fx::BoulderParticles,
    pub clouds: crate::clouds::Clouds,
    stars: Stars,
    pub bag_bounce: f32,
    pub day_banner: Option<(u32, f32)>,
    /// Animator state for the monarch: current action and time in its clip.
    pub anim: (Action, f32),
    /// Seconds since start, for looping ambient animations (coin spin).
    pub t: f32,
    /// Unlocks and islands reached. orig: `UnlockManager` (`UnlockStorage`).
    pub progress: Progress,
    /// Seconds since the run ended (boat departure / loss camera delay).
    pub won_for: f32,
    /// Where saves go; `None` turns saving off.
    pub storage: Option<Box<dyn Storage>>,
    /// orig: `world.bin`'s fingerprint, kept in saves.
    pub world_id: u32,
    /// The run ended: what it unlocked. orig: `Game.EndGame()`.
    ended: Option<Unveiled>,
    /// The island map shown on the touch screen after a run.
    pub map: Option<MapChoice>,
    /// An extra line for the debug overlay (the backend's frame timings).
    pub note: String,
    /// The debug overlay's game-state lines (the 3DS shows only `note`:
    /// each letter costs a sprite).
    pub debug_lines: bool,
    /// Testing aid kept across runs: a full purse and an unhurt monarch
    /// (`GameState::god`).
    pub god: bool,
    /// A problem the player has to fix, shown on the touch screen (sound
    /// files missing or out of date).
    pub warning: String,
    /// Scroll of the water's displacement maps in texels (coarse, fine),
    /// and the camera they follow. orig: `Water._scrollSlow` / `_scrollFast`.
    pub water_scroll: (f32, f32),
    water_camera: f32,
    /// Sounds the game asked for since the backend's sound player last ran.
    pub cues: Vec<crate::sound::Cue>,
    pub music_cues: Vec<crate::sound::MusicCue>,
    /// The game slot saves go to (`save::game_slot`).
    pub slot: usize,
    /// What each slot holds, read when the pause menu opens.
    pub slots: [SlotState; save::SLOTS],
    /// A failed load never authorizes autosave, quit-save or run-end deletion.
    save_ready: bool,
    /// An explicit new-game request may replace this slot on its first save.
    replace_authorized: Option<usize>,
    pub pending_new_slot: Option<usize>,
    /// Separate from backend audio warnings, so startup cannot erase save errors.
    pub save_message: String,
    /// The banner editor, open over the pause menu.
    pub editor: Option<crate::touch::Editor>,
    /// The corner notice for saving (see [`SaveNotice`]).
    pub save_notice: Option<SaveNotice>,
}

/// Seconds "Game saved" (or "Save failed") stays up.
pub const SAVE_NOTICE_TIME: f32 = 2.5;

/// Seconds the "Saving" spinner shows at the least (a quick write would
/// only flash it).
const SAVING_MIN_TIME: f32 = 0.6;

/// Saving, as shown in the top screen's corner. An autosave waits a frame
/// in `Saving`, so the spinner is on screen while the card is written (the
/// write holds up that frame), then shows its result for a while.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SaveNotice {
    /// Seconds shown, and whether the write worked once it has run.
    Saving(f32, Option<bool>),
    /// Seconds left.
    Saved(f32),
    Failed(f32),
}

/// A save slot's summary for the pause menu.
#[derive(Clone, Debug, PartialEq)]
pub struct SlotInfo {
    pub island: String,
    pub day: u32,
    pub coins: u32,
}

/// A slot is empty only when both primary and backup are absent.
#[derive(Clone, Debug, PartialEq)]
pub enum SlotState {
    Empty,
    Saved(SlotInfo),
    Recovered(SlotInfo),
    Failed(SlotProblem),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotProblem {
    InvalidFormat,
    IncompatibleWorld,
    Corrupt,
    Unreadable,
}

impl SlotProblem {
    pub fn label(self) -> &'static str {
        match self {
            Self::InvalidFormat => "UNSUPPORTED SAVE",
            Self::IncompatibleWorld => "DIFFERENT GAME DATA",
            Self::Corrupt => "CORRUPT SAVE",
            Self::Unreadable => "READ FAILED",
        }
    }
}

impl From<save::LoadError> for SlotProblem {
    fn from(error: save::LoadError) -> Self {
        match error {
            save::LoadError::Format => Self::InvalidFormat,
            save::LoadError::World => Self::IncompatibleWorld,
            save::LoadError::Corrupt => Self::Corrupt,
        }
    }
}

enum Record<T> {
    Empty,
    Loaded {
        value: T,
        backup: bool,
        writable: bool,
    },
    Failed(SlotProblem),
}

fn read_record<T>(
    st: &dyn Storage,
    slot: &str,
    decode: impl Fn(&[u8]) -> Result<T, save::LoadError>,
) -> Record<T> {
    let problem = match st.read(slot) {
        Ok(Some(bytes)) => match decode(&bytes) {
            Ok(value) => {
                return Record::Loaded {
                    value,
                    backup: false,
                    writable: true,
                }
            }
            Err(e) => Some(e.into()),
        },
        Ok(None) => None,
        Err(_) => Some(SlotProblem::Unreadable),
    };
    match st.read_backup(slot) {
        Ok(Some(bytes)) => match decode(&bytes) {
            Ok(value) => Record::Loaded {
                value,
                backup: true,
                writable: problem != Some(SlotProblem::Unreadable),
            },
            Err(e) => Record::Failed(problem.unwrap_or_else(|| e.into())),
        },
        Ok(None) => problem.map_or(Record::Empty, Record::Failed),
        Err(_) => Record::Failed(problem.unwrap_or(SlotProblem::Unreadable)),
    }
}

impl SaveNotice {
    fn result(ok: bool) -> SaveNotice {
        if ok {
            SaveNotice::Saved(SAVE_NOTICE_TIME)
        } else {
            SaveNotice::Failed(SAVE_NOTICE_TIME)
        }
    }
}

impl Session {
    pub fn new(world: WorldData, island: usize, seed: u32, progress: Progress) -> Self {
        let game = GameState::start(&world, island, seed, &progress, None);
        let camera = crate::camera::Camera::new(camera_target(&game));
        let camera_x = camera.position();
        let mut s = Session {
            game,
            world,
            mapper: ControlMapper::default(),
            events: Vec::new(),
            speed: 1,
            paused: false,
            debug: false,
            camera_x,
            camera,
            boulder_particles: Default::default(),
            clouds: crate::clouds::Clouds::new(seed, camera_x),
            stars: Stars::default(),
            bag_bounce: 0.0,
            day_banner: None,
            anim: (Action::Stand, 0.0),
            t: 0.0,
            progress,
            won_for: 0.0,
            storage: None,
            water_scroll: (0.0, 0.0),
            cues: Vec::new(),
            music_cues: Vec::new(),
            slot: 0,
            slots: [SlotState::Empty, SlotState::Empty, SlotState::Empty],
            save_ready: false,
            replace_authorized: None,
            pending_new_slot: None,
            save_message: String::new(),
            water_camera: camera_x,
            world_id: 0,
            ended: None,
            map: None,
            note: String::new(),
            debug_lines: true,
            warning: String::new(),
            god: false,
            editor: None,
            save_notice: None,
        };
        // orig: IntroSequence.RunSequence plays the intro track on the first island.
        if island == 0 {
            if let Some(t) = s.world.audio.sound("IntroSequence.introSoundTrack") {
                s.music_cues.push(crate::sound::MusicCue::Play(t));
            }
        }
        s
    }

    /// Picks up a saved run. orig: `Loader` → `PersistentManager.Load()`.
    pub fn resume(&mut self, game: GameState) {
        self.game = game;
        // The banner editor's pick holds for saved games too.
        if let Some((coat, look)) = self.progress.chosen {
            (self.game.coat, self.game.look) = (coat, look);
        }
        self.camera = crate::camera::Camera::new(camera_target(&self.game));
        self.camera_x = self.camera.position();
        self.water_camera = self.camera_x;
        self.boulder_particles = Default::default();
        // Loader reloads the scene, including both cloud controllers and wind.
        self.clouds = crate::clouds::Clouds::new(self.game.rng.clone().next_u32(), self.camera_x);
        self.music_cues.clear();
    }

    /// orig: `PersistentManager.Save("AUTO")`. Returns false if it failed.
    pub fn save(&mut self) -> bool {
        let written = self.write_save();
        self.save_notice = written.map(SaveNotice::result);
        written.unwrap_or(true)
    }

    /// Writes the run: whether it worked, or `None` with nothing to save
    /// (saving off, or the run over).
    fn write_save(&mut self) -> Option<bool> {
        self.storage.as_ref()?;
        if self.game.game_over || self.game.won {
            return None;
        }
        if !self.save_ready {
            return Some(false);
        }
        let preserve = if self.replace_authorized == Some(self.slot) {
            false
        } else {
            match self.read_game(self.slot) {
                Record::Empty
                | Record::Loaded {
                    backup: false,
                    writable: true,
                    ..
                } => false,
                Record::Loaded {
                    backup: true,
                    writable: true,
                    ..
                } => true,
                _ => {
                    self.save_message = "Save protected. Load or choose another slot.".into();
                    return Some(false);
                }
            }
        };
        let bytes = save::save_game(&self.game, self.world_id);
        let st = self.storage.as_mut()?;
        let ok = if preserve {
            st.write_preserving_backup(&save::game_slot(self.slot), &bytes)
        } else {
            st.write(&save::game_slot(self.slot), &bytes)
        };
        if ok {
            self.replace_authorized = None;
        }
        Some(ok && st.write(save::CURRENT_SLOT, &[self.slot as u8]))
    }

    /// Runs a save put off to this frame, and counts the notice down.
    fn update_save_notice(&mut self, dt: f32) {
        self.save_notice = match self.save_notice {
            Some(SaveNotice::Saving(t, None)) => self
                .write_save()
                .map(|ok| SaveNotice::Saving(t + dt, Some(ok))),
            Some(SaveNotice::Saving(t, Some(ok))) if t + dt < SAVING_MIN_TIME => {
                Some(SaveNotice::Saving(t + dt, Some(ok)))
            }
            Some(SaveNotice::Saving(_, Some(ok))) => Some(SaveNotice::result(ok)),
            Some(SaveNotice::Saved(t)) if t > dt => Some(SaveNotice::Saved(t - dt)),
            Some(SaveNotice::Failed(t)) if t > dt => Some(SaveNotice::Failed(t - dt)),
            _ => None,
        };
    }

    /// The slot the last session used (stored with each save).
    pub fn last_slot(st: &dyn platform::Storage) -> usize {
        st.read(save::CURRENT_SLOT)
            .ok()
            .flatten()
            .and_then(|b| b.first().copied())
            .map_or(0, |b| (b as usize).min(save::SLOTS - 1))
    }

    /// Pauses or resumes; pausing reads the slots for the menu.
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        if paused {
            self.read_slots();
        } else {
            self.pending_new_slot = None;
            self.close_editor();
        }
    }

    /// Shows the coat of arms and monarch `look` on this island, and keeps
    /// them for runs to come.
    pub fn choose(&mut self, coat: game::kingdom::CoatOfArms, look: game::kingdom::Look) {
        (self.game.coat, self.game.look) = (coat, look);
        self.progress.chosen = Some((coat, look));
    }

    /// Closes the banner editor, keeping what was picked with the unlocks.
    pub fn close_editor(&mut self) {
        if self.editor.take().is_some() {
            self.write_progress();
        }
    }

    pub fn saved_progress(st: &dyn Storage) -> Progress {
        match read_record(st, save::UNLOCKS_SLOT, save::load_progress) {
            Record::Loaded { value, .. } => value,
            _ => Progress::default(),
        }
    }

    fn write_progress(&mut self) {
        if !self.save_ready
            || (self.replace_authorized != Some(self.slot)
                && matches!(
                    self.read_game(self.slot),
                    Record::Failed(_)
                        | Record::Loaded {
                            writable: false,
                            ..
                        }
                ))
        {
            return;
        }
        if let Some(st) = &mut self.storage {
            let bytes = save::save_progress(&self.progress);
            match read_record(st.as_ref(), save::UNLOCKS_SLOT, save::load_progress) {
                Record::Empty
                | Record::Loaded {
                    backup: false,
                    writable: true,
                    ..
                } => {
                    st.write(save::UNLOCKS_SLOT, &bytes);
                }
                Record::Loaded {
                    backup: true,
                    writable: true,
                    ..
                } => {
                    st.write_preserving_backup(save::UNLOCKS_SLOT, &bytes);
                }
                _ => {
                    self.save_message = "Progress save protected. Existing file kept.".into();
                }
            }
        }
    }

    fn read_game(&self, i: usize) -> Record<GameState> {
        self.storage.as_ref().map_or(Record::Empty, |st| {
            read_record(st.as_ref(), &save::game_slot(i), |bytes| {
                save::load_game(bytes, self.world_id, &self.world)
            })
        })
    }

    /// Read-only loading for development captures that must never save.
    pub fn saved_game(
        st: &dyn Storage,
        i: usize,
        world_id: u32,
        world: &WorldData,
    ) -> Result<Option<GameState>, SlotProblem> {
        match read_record(st, &save::game_slot(i), |bytes| {
            save::load_game(bytes, world_id, world)
        }) {
            Record::Empty => Ok(None),
            Record::Loaded { value, .. } => Ok(Some(value)),
            Record::Failed(e) => Err(e),
        }
    }

    fn read_slots(&mut self) {
        for i in 0..save::SLOTS {
            self.slots[i] = match self.read_game(i) {
                Record::Empty => SlotState::Empty,
                Record::Failed(e) => SlotState::Failed(e),
                Record::Loaded {
                    value: g, backup, ..
                } => {
                    let info = SlotInfo {
                        island: self
                            .world
                            .levels
                            .get(g.level_index)
                            .map_or(String::new(), |l| l.name.clone()),
                        day: g.days.day,
                        coins: g.wallet.coins(),
                    };
                    if backup {
                        SlotState::Recovered(info)
                    } else {
                        SlotState::Saved(info)
                    }
                }
            };
        }
    }

    /// Explicitly saves the current run into an empty or readable slot. An
    /// invalid target requires the separate new-game confirmation, never Save.
    pub fn save_to(&mut self, i: usize) -> bool {
        let i = i.min(save::SLOTS - 1);
        match self.read_game(i) {
            Record::Failed(_)
            | Record::Loaded {
                writable: false, ..
            } => {
                self.save_message = "Slot protected. Retry LOAD or choose NEW.".into();
                self.save_notice = Some(SaveNotice::result(false));
                self.read_slots();
                return false;
            }
            _ => {}
        }
        self.slot = i;
        self.save_ready = true;
        self.replace_authorized = None;
        let ok = self.save();
        self.read_slots();
        ok
    }

    /// Startup and menu loading share this path. Failure preserves the active
    /// run and every existing file; a placeholder run cannot autosave on exit.
    pub fn load_from(&mut self, i: usize) -> bool {
        let i = i.min(save::SLOTS - 1);
        let (game, empty, writable, backup) = match self.read_game(i) {
            Record::Failed(e) => {
                self.save_message = format!("Slot {}: {}. File kept.", i + 1, e.label());
                self.set_paused(true);
                return false;
            }
            Record::Loaded {
                value,
                backup,
                writable,
            } => (value, false, writable, backup),
            Record::Empty => {
                let seed = self.game.rng.next_u32();
                (
                    GameState::start(&self.world, 0, seed, &self.progress, None),
                    true,
                    true,
                    false,
                )
            }
        };
        self.slot = i;
        self.save_ready = writable;
        self.replace_authorized = None;
        self.pending_new_slot = None;
        self.map = None;
        self.ended = None;
        self.won_for = 0.0;
        self.day_banner = None;
        self.save_notice = None;
        self.resume(game);
        self.save_message = if backup {
            if writable {
                "Backup recovered. Original archived on save.".into()
            } else {
                "Backup loaded. Main unreadable; save elsewhere.".into()
            }
        } else {
            String::new()
        };
        if empty {
            self.intro_track();
            self.save();
        } else if writable {
            if let Some(st) = &mut self.storage {
                st.write(save::CURRENT_SLOT, &[i as u8]);
            }
        }
        self.read_slots();
        self.paused = false;
        true
    }

    fn intro_track(&mut self) {
        if let Some(t) = self.world.audio.sound("IntroSequence.introSoundTrack") {
            self.music_cues.push(crate::sound::MusicCue::Play(t));
        }
    }

    pub fn request_new_game(&mut self, i: usize) {
        self.pending_new_slot = Some(i.min(save::SLOTS - 1));
        self.paused = true;
    }

    pub fn cancel_new_game(&mut self) {
        self.pending_new_slot = None;
    }

    /// CLI --new/--island is already an explicit fresh-run request.
    pub fn authorize_new_game(&mut self) {
        self.save_ready = true;
        self.replace_authorized = Some(self.slot);
    }

    /// Called only by the separate Replace confirmation button.
    pub fn confirm_new_game(&mut self) -> bool {
        let Some(i) = self.pending_new_slot.take() else {
            return false;
        };
        let seed = self.game.rng.next_u32();
        let game = GameState::start(&self.world, 0, seed, &self.progress, None);
        self.slot = i;
        self.map = None;
        self.ended = None;
        self.won_for = 0.0;
        self.day_banner = None;
        self.resume(game);
        self.stars = Stars::default();
        self.intro_track();
        self.authorize_new_game();
        let ok = self.save();
        self.save_message = if ok {
            String::new()
        } else {
            "New game could not be saved. File kept.".into()
        };
        self.read_slots();
        self.paused = !ok;
        ok
    }

    /// orig: `Game.EndGame()`: unlocks are settled and stored; a lost run's
    /// save is deleted (`DeleteSavedGame`).
    fn end_run(&mut self) {
        if self.ended.is_some() {
            return;
        }
        let g = &self.game;
        let out = self.progress.finish(Ending {
            land: g.level_index,
            won: g.won,
            unlock: g.queued_unlock,
        });
        self.ended = Some(out);
        let (options, selected) = self.progress.choices(g.level_index, g.won, out.next);
        self.map = Some(MapChoice {
            options,
            selected,
            won: g.won,
            t: 0.0,
        });
        self.write_progress();
        if self.save_ready
            && self.game.game_over
            && matches!(
                self.read_game(self.slot),
                Record::Loaded {
                    backup: false,
                    writable: true,
                    ..
                }
            )
        {
            if let Some(st) = &mut self.storage {
                st.delete(&save::game_slot(self.slot));
            }
        }
    }

    /// orig: `Game.EndGame()` → `UnlockManager.UnlockAndQueueNextLand()`, then
    /// the island picked on the map loads (sailed to after a win).
    pub fn next_island(&mut self) {
        self.end_run();
        self.ended = None;
        // orig: the map's action button: sail to (or start on) the island picked.
        let land = self
            .map
            .take()
            .map_or(self.game.level_index, |m| m.selected);
        let seed = self.game.rng.next_u32();
        let voyage = self
            .game
            .voyage
            .clone()
            .filter(|_| self.game.won)
            .map(|mut v| {
                v.land = land;
                v
            });
        let look = self.game.look;
        self.game = GameState::start(&self.world, land, seed, &self.progress, voyage.as_ref());
        // orig: NextLandData keeps the player model (and the coat, in the voyage).
        if voyage.is_some() {
            self.game.look = look;
        }
        self.camera = crate::camera::Camera::new(camera_target(&self.game));
        self.camera_x = self.camera.position();
        self.boulder_particles = Default::default();
        self.clouds = crate::clouds::Clouds::new(seed, self.camera_x);
        self.stars = Stars::default();
        self.won_for = 0.0;
        // orig: UnlockManager.Restore → AutoSave.
        if voyage.is_some() {
            self.save();
        }
    }

    /// The map after a run is up (the boat gets a few seconds to sail first).
    pub fn map_shown(&self) -> bool {
        self.map.is_some() && (self.game.game_over || (self.game.won && self.won_for > 4.0))
    }

    /// One frame: `speed` ticks of the simulation with `raw` input.
    pub fn step(&mut self, raw: RawInput) {
        self.step_ticks(raw, self.speed);
    }

    pub fn camera_shake_offset(&self) -> [f32; 2] {
        self.camera.shake_offset()
    }

    pub fn visible_stars(&self) -> usize {
        self.stars.visible
    }

    /// One frame that advances the simulation `ticks` times (a backend
    /// running slower than 60 fps catches up this way).
    pub fn step_ticks(&mut self, raw: RawInput, ticks: u32) {
        self.update_save_notice(TICK * ticks.max(1) as f32);
        if self.paused {
            return;
        }
        let (mut saves, mut ended, mut tutorial) = (false, false, false);
        let mut raw = raw;
        self.game.god = self.god;
        for _ in 0..ticks {
            let controls = self.mapper.map(TICK, raw);
            raw.bag_tap = false;
            self.events.clear();
            self.game.tick(&self.world, &controls, &mut self.events);
            self.boulder_particles.advance(TICK);
            self.stars
                .step(self.game.blender.state.floats[game::lighting::float::STARS]);
            for e in &self.events {
                crate::sound::on_event(
                    &self.world,
                    &self.game,
                    e,
                    &mut self.cues,
                    &mut self.music_cues,
                );
                match *e {
                    GameEvent::BoulderImpact {
                        x,
                        y,
                        shake_y,
                        side,
                        ..
                    } => {
                        self.boulder_particles.spawn(
                            &self.world,
                            crate::boulder_fx::BoulderParticleKind::Dust,
                            x,
                            y,
                            side,
                        );
                        self.camera
                            .shake(&self.world.camera, [x, shake_y + game::build::WALK_Y]);
                    }
                    GameEvent::BoulderFragments { x, y, side } => {
                        self.boulder_particles.spawn(
                            &self.world,
                            crate::boulder_fx::BoulderParticleKind::Fragments,
                            x,
                            y,
                            side,
                        );
                    }
                    GameEvent::CameraShake { x, y } => {
                        self.camera
                            .shake(&self.world.camera, [x, y + game::build::WALK_Y]);
                    }
                    GameEvent::Day(time::DayEvent::Started {
                        day,
                        show_counter: true,
                    }) => {
                        self.day_banner = Some((day, 3.0));
                    }
                    GameEvent::Coin(game::coins::CoinEvent::PickedUp(
                        game::coins::Who::Monarch,
                    )) => self.bag_bounce = 0.15,
                    GameEvent::AutoSave => saves = true,
                    GameEvent::TutorialDone => tutorial = true,
                    GameEvent::GameOver | GameEvent::Won => ended = true,
                    _ => {}
                }
            }
            let action = self.game.monarch.action();
            self.anim = if action == self.anim.0 {
                (action, self.anim.1 + TICK)
            } else {
                (action, 0.0)
            };
            self.t += TICK;
            if self.game.won || self.game.game_over {
                self.won_for += TICK;
            }
            let mut target = camera_target(&self.game);
            if self.game.game_over && !self.game.won && self.won_for >= 4.0 {
                // Game.EndGameSequence seeks the monarch after four seconds,
                // without anticipation. Menu timing is independent.
                target.subject = crate::camera::Subject::Monarch;
                target.x = self.game.monarch.x;
                target.lead = false;
            }
            self.camera_x = self.camera.step(&self.world.camera, target, TICK);
            let camera_x = self.camera_x + self.camera_shake_offset()[0];
            self.clouds
                .step(&self.world, &self.game.blender.state, TICK, camera_x);
        }
        // orig: Water.Update: the maps drift and keep still against the ground
        // as the camera pans (a quarter of a map per unit, 4 maps per band).
        // The fine map follows the camera. Wind currently drives sky clouds.
        let w = &self.world.water;
        let per_unit = 0.25 * w.size.0 as f32;
        let pan = (self.camera_x - self.water_camera) * per_unit;
        self.water_camera = self.camera_x;
        self.water_scroll.0 += TICK * ticks as f32 * 0.08 * w.size.0 as f32 + pan;
        self.water_scroll.1 += pan;
        if saves && self.storage.is_some() {
            self.save_notice = Some(SaveNotice::Saving(0.0, None));
        }
        // orig: TutorialManager.CompleteTutorial → PrefsStore TutorialCompleted.
        if tutorial && !self.progress.tutorial_done {
            self.progress.tutorial_done = true;
            self.write_progress();
        }
        if ended {
            self.end_run();
        }
        let dt = TICK * ticks as f32;
        if let Some(m) = &mut self.map {
            m.t += dt;
        }
        self.bag_bounce = (self.bag_bounce - dt).max(0.0);
        if let Some((_, t)) = &mut self.day_banner {
            *t -= dt;
            if *t <= 0.0 {
                self.day_banner = None;
            }
        }
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    use game::data::{DayDef, DirectorDef, LevelConfigDef};
    use std::{
        cell::RefCell,
        collections::{BTreeMap, BTreeSet},
        rc::Rc,
    };

    #[derive(Clone, Default, Debug, PartialEq)]
    struct Files {
        main: BTreeMap<String, Vec<u8>>,
        backup: BTreeMap<String, Vec<u8>>,
        rejected: BTreeMap<String, Vec<Vec<u8>>>,
        unreadable: BTreeSet<String>,
        backup_unreadable: BTreeSet<String>,
        writes: Vec<String>,
        deletes: Vec<String>,
    }
    #[derive(Clone, Default)]
    struct Memory(Rc<RefCell<Files>>);
    impl Storage for Memory {
        fn read(&self, slot: &str) -> Result<Option<Vec<u8>>, platform::ReadError> {
            let f = self.0.borrow();
            if f.unreadable.contains(slot) {
                Err(platform::ReadError)
            } else {
                Ok(f.main.get(slot).cloned())
            }
        }
        fn read_backup(&self, slot: &str) -> Result<Option<Vec<u8>>, platform::ReadError> {
            let f = self.0.borrow();
            if f.backup_unreadable.contains(slot) {
                Err(platform::ReadError)
            } else {
                Ok(f.backup.get(slot).cloned())
            }
        }
        fn write(&mut self, slot: &str, data: &[u8]) -> bool {
            let mut f = self.0.borrow_mut();
            if let Some(old) = f.main.insert(slot.into(), data.to_vec()) {
                f.backup.insert(slot.into(), old);
            }
            f.writes.push(slot.into());
            true
        }
        fn write_preserving_backup(&mut self, slot: &str, data: &[u8]) -> bool {
            let mut f = self.0.borrow_mut();
            if let Some(old) = f.main.insert(slot.into(), data.to_vec()) {
                f.rejected.entry(slot.into()).or_default().push(old);
            }
            f.writes.push(slot.into());
            true
        }
        fn delete(&mut self, slot: &str) {
            let mut f = self.0.borrow_mut();
            f.main.remove(slot);
            f.backup.remove(slot);
            f.deletes.push(slot.into());
        }
    }
    fn world() -> WorldData {
        WorldData {
            days: vec![DayDef {
                name: "day".into(),
                seasons: 7,
                weathers: 1,
                tracks: vec![],
            }],
            director: DirectorDef {
                regular_days: vec![0],
                boss_days: vec![0],
                recovery_days: vec![0],
                num_regular_days: (3, 4),
                ..Default::default()
            },
            levels: vec![LevelConfigDef {
                name: "island".into(),
                autumn_start: (40, 40),
                winter_start: (60, 60),
                days_before_game_over: -1,
                starting_coins: 5,
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
            steeds: vec![Default::default()],
            ..Default::default()
        }
    }
    fn session(files: &Memory) -> Session {
        let mut s = Session::new(world(), 0, 17, Session::saved_progress(files));
        s.world_id = 42;
        s.storage = Some(Box::new(files.clone()));
        s
    }
    fn good_save(coins: u32) -> Vec<u8> {
        let mut s = session(&Memory::default());
        s.game.wallet.set(coins);
        let b = save::save_game(&s.game, 42);
        assert!(save::load_game(&b, 42, &s.world).is_ok());
        b
    }

    #[test]
    fn pause_freezes_authored_clocks_and_resume_crosses_the_marker_once() {
        use game::greed::{EnemyKind, GreedEvent, TrollState};
        use game::state::GameEvent;
        let mut s = session(&Memory::default());
        s.storage = None;
        s.game
            .population
            .spawn(game::data::Role::Worker, -5.0, &mut s.game.rng);
        s.game.population.units[0].anim_t = 0.4;
        s.game
            .greed
            .spawn_troll(EnemyKind::Squid, 8.0, 0, false, &mut s.game.rng);
        s.game.greed.trolls[0].state = TrollState::GrabCitizen { t: 0.49 };
        s.set_paused(true);
        let before = save::save_game(&s.game, s.world_id);
        let time = s.t;
        s.step_ticks(RawInput::default(), 120);
        assert_eq!(save::save_game(&s.game, s.world_id), before);
        assert_eq!(s.t, time);
        assert!(s.events.is_empty());
        s.set_paused(false);
        s.step_ticks(RawInput::default(), 1);
        assert!(s.t > time);
        assert_eq!(
            s.events
                .iter()
                .filter(|e| matches!(e, GameEvent::Greed(GreedEvent::GrabComplete(_))))
                .count(),
            1
        );
        s.step_ticks(RawInput::default(), 1);
        assert!(!s
            .events
            .iter()
            .any(|e| matches!(e, GameEvent::Greed(GreedEvent::GrabComplete(_)))));
    }

    #[test]
    fn invalid_startup_slots_survive_load_autosave_quit_and_run_end() {
        let good = good_save(19);
        let mut wrong_world = good.clone();
        wrong_world[6..10].copy_from_slice(&43u32.to_le_bytes());
        let mut future = good.clone();
        future[4..6].copy_from_slice(&(save::VERSION + 1).to_le_bytes());
        let mut corrupt = good.clone();
        corrupt.truncate(12);
        for (bytes, problem) in [
            (b"not a save".to_vec(), SlotProblem::InvalidFormat),
            (future, SlotProblem::InvalidFormat),
            (wrong_world, SlotProblem::IncompatibleWorld),
            (corrupt, SlotProblem::Corrupt),
        ] {
            let files = Memory::default();
            files.0.borrow_mut().main.insert("game".into(), bytes);
            files
                .0
                .borrow_mut()
                .backup
                .insert("game".into(), b"also invalid".to_vec());
            files
                .0
                .borrow_mut()
                .main
                .insert("unlocks".into(), b"keep progress".to_vec());
            let original = files.0.borrow().clone();
            let mut s = session(&files);
            assert!(!s.load_from(0));
            assert!(s.paused);
            assert_eq!(s.slots[0], SlotState::Failed(problem));
            assert!(!s.save()); // F5 and backend quit use this exact path.
            assert!(!s.save_to(0));
            s.save_notice = Some(SaveNotice::Saving(0.0, None));
            s.step_ticks(RawInput::default(), 1); // actual deferred autosave path
            s.progress.tutorial_done = true;
            s.write_progress();
            s.game.game_over = true;
            s.end_run(); // invalid placeholder must never delete a run
            assert_eq!(*files.0.borrow(), original);
        }
    }

    #[test]
    fn corrupt_main_recovers_backup_without_replacing_it_on_next_save() {
        let files = Memory::default();
        let backup = good_save(29);
        files
            .0
            .borrow_mut()
            .main
            .insert("game".into(), b"corrupt".to_vec());
        files
            .0
            .borrow_mut()
            .backup
            .insert("game".into(), backup.clone());
        let mut s = session(&files);
        assert!(s.load_from(0));
        assert_eq!(s.game.wallet.coins(), 29);
        assert!(matches!(s.slots[0], SlotState::Recovered(_)));
        assert_eq!(files.0.borrow().main["game"], b"corrupt");
        assert_eq!(files.0.borrow().backup["game"], backup);
        s.game.wallet.set(31);
        assert!(s.save());
        assert_eq!(files.0.borrow().backup["game"], backup);
        assert_eq!(files.0.borrow().rejected["game"], [b"corrupt".to_vec()]);
        assert_eq!(
            save::load_game(&files.0.borrow().main["game"], 42, &s.world)
                .unwrap()
                .wallet
                .coins(),
            31
        );
    }

    #[test]
    fn incompatible_primary_is_retained_through_repeated_recovery_saves() {
        let files = Memory::default();
        let backup = good_save(29);
        files
            .0
            .borrow_mut()
            .backup
            .insert("game".into(), backup.clone());
        let mut originals = Vec::new();
        for world_id in [43u32, 44] {
            let mut incompatible = good_save(41);
            incompatible[6..10].copy_from_slice(&world_id.to_le_bytes());
            files
                .0
                .borrow_mut()
                .main
                .insert("game".into(), incompatible.clone());
            originals.push(incompatible);
            let mut s = session(&files);
            assert!(s.load_from(0));
            assert_eq!(s.game.wallet.coins(), 29);
            assert!(s.save());
            assert_eq!(files.0.borrow().backup["game"], backup);
            assert_eq!(files.0.borrow().rejected["game"], originals);
        }
    }

    #[test]
    fn recovered_run_ending_before_first_save_retains_both_original_files() {
        let files = Memory::default();
        let mut incompatible = good_save(41);
        incompatible[6..10].copy_from_slice(&43u32.to_le_bytes());
        files
            .0
            .borrow_mut()
            .main
            .insert("game".into(), incompatible.clone());
        let backup = good_save(29);
        files
            .0
            .borrow_mut()
            .backup
            .insert("game".into(), backup.clone());
        let mut s = session(&files);
        assert!(s.load_from(0));
        s.game.game_over = true;
        s.end_run();
        assert_eq!(files.0.borrow().main["game"], incompatible);
        assert_eq!(files.0.borrow().backup["game"], backup);
        assert!(files.0.borrow().deletes.is_empty());
    }

    #[test]
    fn slot_becoming_unreadable_blocks_autosave_progress_and_end_run_deletion() {
        let files = Memory::default();
        files
            .0
            .borrow_mut()
            .main
            .insert("game".into(), good_save(23));
        let mut s = session(&files);
        assert!(s.load_from(0));
        files.0.borrow_mut().unreadable.insert("game".into());
        let before = files.0.borrow().clone();
        assert!(!s.save());
        s.progress.tutorial_done = true;
        s.write_progress();
        s.save_notice = Some(SaveNotice::Saving(0.0, None));
        s.step_ticks(RawInput::default(), 1);
        s.game.game_over = true;
        s.end_run();
        assert_eq!(*files.0.borrow(), before);
    }

    #[test]
    fn missing_main_uses_backup_and_missing_with_bad_backup_is_not_empty() {
        let files = Memory::default();
        let backup = good_save(21);
        files
            .0
            .borrow_mut()
            .backup
            .insert("game".into(), backup.clone());
        let mut s = session(&files);
        assert!(s.load_from(0));
        assert_eq!(s.game.wallet.coins(), 21);
        assert!(!files.0.borrow().main.contains_key("game"));
        assert!(s.save());
        assert_eq!(files.0.borrow().backup["game"], backup);
        files
            .0
            .borrow_mut()
            .backup
            .insert("game2".into(), b"invalid".to_vec());
        let before = files.0.borrow().clone();
        assert!(!s.load_from(1));
        assert_eq!(s.slot, 0);
        assert_eq!(s.game.wallet.coins(), 21);
        assert_eq!(*files.0.borrow(), before);
    }

    #[test]
    fn read_failures_preserve_files_and_valid_backup_is_read_only() {
        let files = Memory::default();
        files
            .0
            .borrow_mut()
            .main
            .insert("game".into(), good_save(26));
        files.0.borrow_mut().unreadable.insert("game".into());
        let mut s = session(&files);
        let before = files.0.borrow().clone();
        assert!(!s.load_from(0));
        assert_eq!(s.slots[0], SlotState::Failed(SlotProblem::Unreadable));
        assert!(!s.save());
        assert_eq!(*files.0.borrow(), before);
        files
            .0
            .borrow_mut()
            .backup
            .insert("game".into(), good_save(17));
        assert!(s.load_from(0));
        assert_eq!(s.game.wallet.coins(), 17);
        let before = files.0.borrow().clone();
        assert!(!s.save());
        assert!(!s.save_to(0));
        assert_eq!(*files.0.borrow(), before);
        assert!(s.save_to(1)); // a deliberate save elsewhere is allowed
        assert_eq!(files.0.borrow().main["game"], before.main["game"]);
        let other = Memory::default();
        other.0.borrow_mut().backup_unreadable.insert("game".into());
        assert!(!session(&other).load_from(0));
        assert!(other.0.borrow().writes.is_empty());
    }

    #[test]
    fn only_confirmed_new_game_can_replace_a_protected_slot() {
        let files = Memory::default();
        files
            .0
            .borrow_mut()
            .main
            .insert("game2".into(), b"invalid".to_vec());
        files
            .0
            .borrow_mut()
            .main
            .insert("game".into(), good_save(23));
        let mut s = session(&files);
        assert!(s.load_from(0));
        let before = files.0.borrow().clone();
        s.request_new_game(1);
        assert_eq!(s.pending_new_slot, Some(1));
        s.cancel_new_game();
        assert!(!s.confirm_new_game());
        assert_eq!(*files.0.borrow(), before);
        assert_eq!(s.slot, 0);
        assert_eq!(s.game.wallet.coins(), 23);
        s.request_new_game(1);
        assert!(s.confirm_new_game());
        assert_eq!(s.slot, 1);
        assert!(!s.paused);
        assert_eq!(s.pending_new_slot, None);
        assert!(save::load_game(&files.0.borrow().main["game2"], 42, &s.world).is_ok());
        assert_eq!(files.0.borrow().main["game"], before.main["game"]);
    }

    #[test]
    fn menu_replacement_requires_a_separate_confirmation_tap() {
        let files = Memory::default();
        files
            .0
            .borrow_mut()
            .main
            .insert("game".into(), good_save(23));
        files
            .0
            .borrow_mut()
            .main
            .insert("game2".into(), b"invalid".to_vec());
        let mut s = session(&files);
        assert!(s.load_from(0));
        s.set_paused(true);
        let before = files.0.borrow().clone();
        let sheet = crate::sheet::Sheet::default();
        // Slot 2's NEW action, then the confirmation's CANCEL.
        crate::touch::tap(&mut s, &sheet, (220.0, 114.0));
        assert_eq!(s.pending_new_slot, Some(1));
        assert_eq!(*files.0.borrow(), before);
        crate::touch::tap(&mut s, &sheet, (50.0, 165.0));
        assert_eq!(s.pending_new_slot, None);
        assert_eq!(s.slot, 0);
        assert_eq!(s.game.wallet.coins(), 23);
        assert_eq!(*files.0.borrow(), before);
        crate::touch::tap(&mut s, &sheet, (220.0, 114.0));
        crate::touch::tap(&mut s, &sheet, (200.0, 165.0));
        assert_eq!(s.slot, 1);
        assert!(save::load_game(&files.0.borrow().main["game2"], 42, &s.world).is_ok());
        assert_eq!(files.0.borrow().main["game"], before.main["game"]);
    }

    #[test]
    fn genuinely_empty_slot_starts_and_invalid_progress_is_never_overwritten() {
        let files = Memory::default();
        files
            .0
            .borrow_mut()
            .main
            .insert("unlocks".into(), b"invalid progress".to_vec());
        let mut s = session(&files);
        assert!(s.load_from(0));
        assert!(files.0.borrow().main.contains_key("game"));
        s.progress.tutorial_done = true;
        s.write_progress();
        assert_eq!(files.0.borrow().main["unlocks"], b"invalid progress");
        let valid = save::save_progress(&Progress::everything());
        files
            .0
            .borrow_mut()
            .backup
            .insert("unlocks".into(), valid.clone());
        assert_eq!(Session::saved_progress(&files), Progress::everything());
        s.write_progress();
        assert_eq!(files.0.borrow().backup["unlocks"], valid);
    }

    #[test]
    fn stars_sample_the_blend_every_five_active_seconds_and_keep_phase_on_load() {
        let mut s = Session::new(world(), 0, 1, Progress::default());
        let input = RawInput::default();
        let star = game::lighting::float::STARS;
        s.game.blender.state.floats[star] = 0.75;
        s.step_ticks(input, 299);
        assert_eq!(s.visible_stars(), STAR_COUNT);
        s.paused = true;
        s.step_ticks(input, 300);
        assert_eq!(s.visible_stars(), STAR_COUNT);
        s.paused = false;
        s.step_ticks(input, 1);
        assert_eq!(s.visible_stars(), STAR_COUNT / 2);
        s.game.blender.state.floats[star] = 0.5;
        s.step_ticks(input, 299);
        s.resume(s.game.clone());
        assert_eq!(s.visible_stars(), STAR_COUNT / 2);
        s.step_ticks(input, 1);
        assert_eq!(s.visible_stars(), 0);
    }

    #[test]
    fn camera_catchup_pause_and_crown_target_use_the_same_tick_clock() {
        let mut a = Session::new(world(), 0, 1, Progress::default());
        let mut b = Session::new(world(), 0, 1, Progress::default());
        let input = RawInput {
            direction: 1,
            ..Default::default()
        };
        a.step_ticks(input, 120);
        for _ in 0..120 {
            b.step_ticks(input, 1);
        }
        assert_eq!(a.camera_x, b.camera_x);
        let before = a.camera_x;
        a.paused = true;
        a.step_ticks(input, 120);
        assert_eq!(a.camera_x, before);
        a.paused = false;
        a.game.greed.drop_crown(a.game.monarch.x + 12., 0., 0.);
        a.step_ticks(RawInput::default(), 1);
        let target = camera_target(&a.game);
        assert_eq!(target.subject, crate::camera::Subject::Crown);
        assert!(!target.lead);
        assert!(a.camera_x < target.x);
        let loaded =
            game::save::load_game(&game::save::save_game(&a.game, 0), 0, &a.world).unwrap();
        a.resume(loaded);
        assert!((a.camera_x - target.x).abs() <= 1. / 64.);
    }

    #[test]
    fn boss_bash_marker_shakes_world_projection_and_pause_freezes_the_offset() {
        use game::animation::{Clip, Event, EventKind};
        let mut w = world();
        w.animations.clips.push((
            "boss_bash_right".into(),
            Clip {
                length: 1.2,
                events: vec![Event {
                    time: 0.65,
                    kind: EventKind::BashImpact,
                    float_parameter: 0.0,
                    int_parameter: 0,
                    string_parameter: String::new(),
                }],
                ..Default::default()
            },
        ));
        let mut s = Session::new(w, 0, 1, Progress::everything());
        s.game.monarch.x = 0.0;
        s.resume(s.game.clone());
        s.game
            .greed
            .spawn_troll(game::greed::EnemyKind::Boss, 2.0, 1, true, &mut s.game.rng);
        s.game.greed.trolls[0].bash = Some((0.59, false));
        let input = RawInput::default();
        let before = crate::top::layer_to_screen(&s, 0.0, 1.0, 1.0);
        s.step_ticks(input, 3);
        assert_eq!(
            s.camera_shake_offset(),
            [0.0; 2],
            "wait for the authored marker"
        );
        s.step_ticks(input, 1);
        assert!(s
            .events
            .iter()
            .any(|e| matches!(e, GameEvent::CameraShake { x: 2.0, y: 0.0 })));
        let [dx, dy] = s.camera_shake_offset();
        assert_ne!([dx, dy], [0.0; 2]);
        assert_eq!(s.camera_x, 0.0);
        assert_eq!(
            crate::top::layer_to_screen(&s, 0.0, 1.0, 1.0),
            (before.0 - dx * 32.0, before.1 + dy * 32.0)
        );
        assert_eq!(
            crate::top::layer_to_screen(&s, 0.0, 1.0, 0.0),
            (before.0, before.1 + dy * 32.0)
        );
        s.paused = true;
        s.step_ticks(input, 120);
        assert_eq!(s.camera_shake_offset(), [dx, dy]);
        s.resume(s.game.clone());
        assert_eq!(s.camera_shake_offset(), [0.0; 2]);
    }

    #[test]
    fn loss_camera_waits_four_active_seconds_then_returns_without_lead() {
        use crate::camera::{Camera, Subject, Target};
        use game::animation::{Key, Tangent};

        let mut s = Session::new(world(), 0, 1, Progress::default());
        let constant = |value| {
            vec![Key {
                time: 0.0,
                value,
                in_tangent: Tangent::Slope(0.0),
                out_tangent: Tangent::Slope(0.0),
            }]
        };
        s.world.camera = game::data::CameraDef {
            shake: Default::default(),
            lead: true,
            max_lead_speed: 5.0,
            lead_over_time: constant(2.0),
            seek_speed_by_distance: constant(6.0),
        };
        s.game.monarch.x = 0.0;
        s.game.monarch.facing = 1;
        s.game.game_over = true;
        s.game.has_crown = false;
        // The stolen crown has gone inactive while the camera is at its last
        // position. EndGameSequence retains that view for four active seconds.
        s.camera = Camera::new(Target {
            subject: Subject::Crown,
            x: 12.0,
            direction: 1.0,
            lead: false,
        });
        s.camera_x = s.camera.position();
        assert_eq!(camera_target(&s.game).subject, Subject::Inactive);
        s.step_ticks(RawInput::default(), 239);
        assert!(s.won_for < 4.0);
        assert_eq!(s.camera_x, 12.0);

        let before = s.won_for;
        s.paused = true;
        s.step_ticks(RawInput::default(), 600);
        assert_eq!(s.won_for, before);
        assert_eq!(s.camera_x, 12.0);
        s.paused = false;
        for _ in 0..3 {
            if s.won_for >= 4.0 {
                break;
            }
            s.step_ticks(RawInput::default(), 1);
        }
        assert!(s.won_for >= 4.0);
        assert!(s.camera_x > 11.0 && s.camera_x < 12.0);
        s.step_ticks(RawInput::default(), 600);
        assert_eq!(s.camera_x, s.game.monarch.x);
    }
}
