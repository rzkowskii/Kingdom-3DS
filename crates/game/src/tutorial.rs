//! The first island's guide: the previous monarch's ghost leads the new one
//! to the campfire, then on to the beggars and the wall site, and fades once
//! two citizens are hired and a wall stands. The KINGDOM title in the forest
//! goes once the monarch has ridden past it and it's out of view.
//!
//! orig: `Ghost`, `Title`, `TutorialManager`. The original's hint glyphs and
//! payable gating aren't ported; the ghost picks its goals from the island
//! instead of from the hints.

use crate::build::under_construction;
use crate::data::{Role, WorldData};
use crate::level::{Level, Object};
use crate::units::{Mover, Population};

/// orig: Ghost prefab `moveSpeed`, `playerDist`, `goalDist`.
const SPEED: f32 = 3.0;
const PLAYER_DIST: f32 = 2.5;
const GOAL_DIST: f32 = 1.5;
/// orig: `Ghost.Fade()` → `SpriteFX.FadeOut(5)`.
const FADE: f32 = 5.0;
/// orig: `Title._crumbleScanner` (10 units around the title, here as seen
/// through its layer).
const TITLE_RANGE: f32 = 10.0;
/// How far (in its layer, from the monarch) the title must be before it's
/// out of view: half the 12.5-unit screen, half its ~10-unit width, and
/// room for the camera's lead. orig: `Title.Crumble()` breaks it into
/// debris instead, which looked wrong at 3DS scale.
const TITLE_GONE: f32 = 14.0;
/// Citizens hired before the ghost's job is done.
const HIRES: usize = 2;

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct Ghost {
    /// Its object in `Level::scenery`.
    pub object: u32,
    pub x: f32,
    pub mover: Mover,
    /// 1 while guiding, down to 0 fading out.
    pub alpha: f32,
    /// orig: `State.WaitForPlayer`: still where it was placed.
    waiting: bool,
    /// orig: `_hasReachedCampfire`.
    reached_campfire: bool,
    /// orig: `_lastMoved` (seconds since).
    since_moved: f32,
    pub anim_t: f32,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tutorial {
    /// Set up from the level (saves from before this module start unset).
    started: bool,
    pub ghost: Option<Ghost>,
    /// The title's object, and seconds since the monarch passed it.
    pub title: Option<(u32, Option<f32>)>,
    hired: bool,
    walled: bool,
    /// Seconds to the next look for a finished wall.
    wall_check: f32,
}

/// What the ghost looks at.
pub struct TutorialCtx<'a> {
    pub world: &'a WorldData,
    pub monarch_x: f32,
    pub campfire: f32,
    pub population: &'a Population,
}

impl Tutorial {
    /// Finds the ghost and title on a fresh island. Without `guide` (the
    /// tutorial was finished on an earlier run, or this isn't the first
    /// island) the ghost is taken away.
    pub fn new(world: &WorldData, level: &mut Level, guide: bool) -> Self {
        let find = |level: &Level, name: &str| {
            level
                .all()
                .find(|o| world.protos[o.proto as usize].name == name)
                .map(|o| (o.id, o.x))
        };
        let mut ghost = find(level, "Ghost").map(|(object, x)| Ghost {
            object,
            x,
            mover: Mover::new(),
            alpha: 1.0,
            waiting: true,
            reached_campfire: false,
            since_moved: 0.0,
            anim_t: 0.0,
        });
        if !guide {
            if let Some(g) = ghost.take() {
                level.remove_any(g.object);
            }
        }
        Tutorial {
            started: true,
            ghost,
            title: find(level, "Title").map(|(id, _)| (id, None)),
            ..Default::default()
        }
    }

    /// Whether the guide is still at work (for the backend: it's done when
    /// this turns false).
    pub fn guiding(&self) -> bool {
        self.ghost.as_ref().is_some_and(|g| g.alpha >= 1.0)
    }

    /// One tick. Returns true the tick the ghost's job is done.
    pub fn update(&mut self, dt: f32, ctx: &TutorialCtx, level: &mut Level) -> bool {
        // A save from before this module: that run is under way, so no
        // ghost (and no TutorialDone that would hide it from new runs).
        if !self.started {
            *self = Tutorial::new(ctx.world, level, false);
        }
        self.update_title(dt, ctx, level);
        let Some(mut g) = self.ghost.take() else {
            return false;
        };
        let was_guiding = g.alpha >= 1.0;
        g.anim_t += dt;
        if !self.hired {
            let hired = [
                Role::Peasant,
                Role::Worker,
                Role::Archer,
                Role::Farmer,
                Role::Knight,
            ]
            .iter()
            .map(|&r| ctx.population.count(r))
            .sum::<usize>();
            self.hired = hired >= HIRES;
        }
        self.wall_check -= dt;
        if !self.walled && self.wall_check <= 0.0 {
            self.wall_check = 0.5;
            self.walled = level.objects.iter().any(|o| {
                ctx.world.protos[o.proto as usize]
                    .wall_level
                    .is_some_and(|l| l >= 1)
                    && !under_construction(ctx.world, o)
            });
        }

        // orig: ShouldFade: the campfire was reached and the tutorial's over.
        if self.hired && self.walled {
            g.alpha -= dt / FADE;
            g.mover.stop();
        } else if g.waiting {
            // orig: ShouldGoToPlayer.
            g.waiting = (ctx.monarch_x - g.x).abs() >= PLAYER_DIST;
        } else {
            let goal = self.goal(&g, ctx, level);
            show_goal(&mut g, goal, ctx.monarch_x, dt);
        }
        g.mover.update(&mut g.x, dt);

        let done = was_guiding && g.alpha < 1.0;
        if let Some(o) = scenery_mut(level, g.object) {
            o.x = g.x;
            o.flip = g.mover.facing < 0;
        }
        if g.alpha > 0.0 {
            self.ghost = Some(g);
        } else {
            level.remove_any(g.object);
        }
        done
    }

    /// Where the ghost leads: the campfire first, then the nearest beggar
    /// until two are hired, then the nearest wall site.
    /// orig: `Ghost.ShowGoal()` (`campfirePosition`, then `ghostHint`).
    fn goal(&self, g: &Ghost, ctx: &TutorialCtx, level: &Level) -> f32 {
        if !g.reached_campfire {
            return ctx.campfire;
        }
        let nearest = |xs: &mut dyn Iterator<Item = f32>| {
            xs.min_by(|a, b| (a - g.x).abs().total_cmp(&(b - g.x).abs()))
        };
        let target = if !self.hired {
            nearest(
                &mut ctx
                    .population
                    .units
                    .iter()
                    .filter(|u| u.role == Role::Beggar)
                    .map(|u| u.x),
            )
        } else {
            nearest(
                &mut level
                    .objects
                    .iter()
                    .filter(|o| {
                        let p = &ctx.world.protos[o.proto as usize];
                        p.tag == "WallFoundation"
                            || (p.wall_level.is_some() && under_construction(ctx.world, o))
                    })
                    .map(|o| o.x),
            )
        };
        target.unwrap_or(ctx.monarch_x)
    }

    fn update_title(&mut self, dt: f32, ctx: &TutorialCtx, level: &mut Level) {
        let Some((id, passed)) = &mut self.title else {
            return;
        };
        let Some(o) = level.all().find(|o| o.id == *id) else {
            self.title = None;
            return;
        };
        // Where it shows from the monarch, through its layer's parallax.
        let off = (o.x - ctx.world.layer(o.layer).ratio * ctx.monarch_x).abs();
        match passed {
            None => {
                if off < TITLE_RANGE {
                    *passed = Some(0.0);
                }
            }
            Some(t) => {
                *t += dt;
                if off > TITLE_GONE {
                    level.remove_any(*id);
                    self.title = None;
                }
            }
        }
    }
}

/// orig: `Ghost.ShowGoal()`: keeps close to the monarch, between them and
/// the goal, and faces the goal when it stops.
fn show_goal(g: &mut Ghost, goal: f32, monarch_x: f32, dt: f32) {
    if !g.reached_campfire && (g.x - goal).abs() <= GOAL_DIST + 0.25 {
        g.reached_campfire = true;
    }
    let near_dist = PLAYER_DIST * 3.0;
    let near = (monarch_x - goal).abs() < near_dist;
    let reach = if near { near_dist } else { PLAYER_DIST };
    let side = if monarch_x < goal {
        goal - GOAL_DIST
    } else {
        goal + GOAL_DIST
    };
    let to = side.clamp(monarch_x - reach, monarch_x + reach);
    let d = (to - g.x).abs();
    g.since_moved += dt;
    if d > 0.1 {
        g.mover.set_goal(to, 0.5 + (SPEED - 0.5) * d.min(1.0));
        g.since_moved = 0.0;
    } else if g.since_moved > 0.7 {
        g.mover.stop();
        g.mover.facing = if goal > g.x { 1 } else { -1 };
    }
}

fn scenery_mut(level: &mut Level, id: u32) -> Option<&mut Object> {
    let i = level.scenery.binary_search_by_key(&id, |o| o.id).ok()?;
    level.scenery.get_mut(i)
}
