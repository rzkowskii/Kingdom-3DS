//! The touch screen: the coin bag (the original's purse, coins piling up
//! inside) with the count under it, the day, and — once a run is over — the
//! island map to tap where to sail.

use game::time::{self, Celestial};

use game::kingdom::{self, CoatOfArms, Look};
use game::monarch::Action;
use game::rng::Rng;

use crate::heraldry;
use crate::render::{Color, Draw, Rect, Renderer};
use crate::session::{Session, SlotState};
use crate::sheet::Sheet;
use crate::text;
use crate::top;

pub const W: f32 = 320.0;
pub const H: f32 = 240.0;

/// The bag's pivot on screen and its scale (the sprites are 32 px per unit).
const BAG_AT: (f32, f32) = (160.0, 112.0);
const BAG_SCALE: f32 = 3.0;
/// Bag units to screen pixels.
const BAG_PX: f32 = 32.0 * BAG_SCALE;
/// Where tapping drops a coin.
pub const BAG_TAP: Rect = Rect::new(112.0, 40.0, 96.0, 140.0);
pub const PAUSE: Rect = Rect::new(8.0, 8.0, 24.0, 24.0);
/// The map's middle on screen; map positions are pixels from it, y up.
const MAP_AT: (f32, f32) = (160.0, 120.0);
/// Map buttons: previous, next, and the action (sail / start).
pub const PREV: Rect = Rect::new(196.0, 194.0, 20.0, 18.0);
pub const NEXT: Rect = Rect::new(218.0, 194.0, 20.0, 18.0);
pub const ACTION: Rect = Rect::new(240.0, 194.0, 62.0, 18.0);
/// The pause menu: a row per save slot with Save and Load, and Resume.
const SLOT_ROW: Rect = Rect::new(10.0, 46.0, 300.0, 42.0);
const SLOT_GAP: f32 = 48.0;
pub const RESUME: Rect = Rect::new(110.0, 196.0, 100.0, 28.0);
const REPLACE: Rect = Rect::new(166.0, 152.0, 130.0, 28.0);
const CANCEL: Rect = Rect::new(24.0, 152.0, 130.0, 28.0);
/// Opens the banner editor.
const BANNER: Rect = Rect::new(10.0, 196.0, 92.0, 28.0);

/// The banner editor: previews on the left, choices on the right, the
/// color picker along the bottom.
const PREVIEW_BANNER: (f32, f32) = (46.0, 40.0);
const PREVIEW_BANNER_SCALE: f32 = 3.0;
const PREVIEW_RIDER: (f32, f32) = (106.0, 160.0);
const PREVIEW_RIDER_SCALE: f32 = 1.5;
const KING: Rect = Rect::new(140.0, 10.0, 84.0, 22.0);
const QUEEN: Rect = Rect::new(228.0, 10.0, 84.0, 22.0);
/// Rows with a label and ‹ › to step through: pattern, emblem.
const STEP_ROWS: [f32; 2] = [40.0, 70.0];
const STEP_PREV_X: f32 = 232.0;
const STEP_NEXT_X: f32 = 290.0;
const STEP_SIZE: f32 = 22.0;
/// The three colors being picked for: main, trim, emblem.
const SWATCH_Y: f32 = 100.0;
const SWATCH_X: [f32; 3] = [140.0, 200.0, 260.0];
const SWATCH_W: f32 = 52.0;
const SWATCH_H: f32 = 22.0;
const RANDOM: Rect = Rect::new(140.0, 140.0, 84.0, 22.0);
const DONE: Rect = Rect::new(228.0, 140.0, 84.0, 22.0);
const PALETTE: Rect = Rect::new(8.0, 172.0, 304.0, 60.0);

fn slot_row(i: usize) -> Rect {
    Rect::new(
        SLOT_ROW.x,
        SLOT_ROW.y + i as f32 * SLOT_GAP,
        SLOT_ROW.w,
        SLOT_ROW.h,
    )
}

fn slot_save(i: usize) -> Rect {
    let r = slot_row(i);
    Rect::new(r.x + r.w - 116.0, r.y + 9.0, 52.0, 24.0)
}

fn slot_load(i: usize) -> Rect {
    let r = slot_row(i);
    Rect::new(r.x + r.w - 58.0, r.y + 9.0, 52.0, 24.0)
}

const BACKGROUND: Color = Color::new(28.0 / 255.0, 24.0 / 255.0, 22.0 / 255.0, 1.0);
const GOLD: Color = Color::new(1.0, 0.8, 0.0, 1.0);

/// The island map after a run. orig: `Map` (`Mode.Continue` after a win,
/// `Mode.Restart` after a loss).
pub struct MapChoice {
    pub options: Vec<usize>,
    pub selected: usize,
    pub won: bool,
    /// Seconds open (for the boat and pointer animations).
    pub t: f32,
}

impl MapChoice {
    /// orig: `Map.SelectNextLand()`: the next selectable island in `cycleOrder`.
    pub fn cycle(&mut self, dir: i32) {
        let n = self.options.len() as i32;
        if let Some(i) = self.options.iter().position(|&l| l == self.selected) {
            self.selected = self.options[(i as i32 + dir).rem_euclid(n) as usize];
        } else if let Some(&l) = self.options.first() {
            self.selected = l;
        }
    }
}

/// The banner editor (not in the original, which takes a typed blazon:
/// `Menu.OnButtonSetBlazon`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Editor {
    /// Which color the picker sets: 0 main, 1 trim, 2 emblem.
    pub target: usize,
}

fn step_prev(row: usize) -> Rect {
    Rect::new(STEP_PREV_X, STEP_ROWS[row], STEP_SIZE, STEP_SIZE)
}

fn step_next(row: usize) -> Rect {
    Rect::new(STEP_NEXT_X, STEP_ROWS[row], STEP_SIZE, STEP_SIZE)
}

fn swatch(i: usize) -> Rect {
    Rect::new(SWATCH_X[i], SWATCH_Y, SWATCH_W, SWATCH_H)
}

fn palette_cell(i: usize) -> Rect {
    let (cw, ch) = (
        PALETTE.w / heraldry::PALETTE_COLS as f32,
        PALETTE.h / heraldry::PALETTE_ROWS as f32,
    );
    let (row, col) = (i / heraldry::PALETTE_COLS, i % heraldry::PALETTE_COLS);
    Rect::new(
        PALETTE.x + col as f32 * cw,
        PALETTE.y + row as f32 * ch,
        cw - 1.0,
        ch - 1.0,
    )
}

fn coat_color(coat: &mut CoatOfArms, i: usize) -> &mut [f32; 3] {
    match i {
        0 => &mut coat.primary,
        1 => &mut coat.secondary,
        _ => &mut coat.emblem,
    }
}

/// A tap on the banner editor.
fn tap_editor(s: &mut Session, p: (f32, f32)) {
    let Some(mut ed) = s.editor else { return };
    let (mut coat, mut look) = (s.game.coat, s.game.look);
    let step = |v: u8, by: i32, n: u8| (v as i32 + by).rem_euclid(n as i32) as u8;
    if KING.contains(p) {
        look.queen = false;
    } else if QUEEN.contains(p) {
        look.queen = true;
    } else if step_prev(0).contains(p) {
        look.pattern = step(look.pattern, -1, kingdom::PATTERNS);
    } else if step_next(0).contains(p) {
        look.pattern = step(look.pattern, 1, kingdom::PATTERNS);
    } else if step_prev(1).contains(p) {
        look.emblem = step(look.emblem, -1, kingdom::EMBLEMS);
    } else if step_next(1).contains(p) {
        look.emblem = step(look.emblem, 1, kingdom::EMBLEMS);
    } else if RANDOM.contains(p) {
        let mut rng = Rng::new((s.t * 1000.0) as u32 ^ 0xba22_e125);
        coat = CoatOfArms::random(&mut rng);
        look = Look::random(&mut rng);
    } else if DONE.contains(p) {
        s.close_editor();
        return;
    } else if let Some(i) = (0..3).find(|&i| swatch(i).contains(p)) {
        ed.target = i;
    } else if let Some(i) =
        (0..heraldry::PALETTE_COLS * heraldry::PALETTE_ROWS).find(|&i| palette_cell(i).contains(p))
    {
        *coat_color(&mut coat, ed.target) = heraldry::palette(i);
    } else {
        return;
    }
    s.editor = Some(ed);
    if (coat, look) != (s.game.coat, s.game.look) {
        s.choose(coat, look);
    }
}

/// What a tap on the touch screen did.
pub enum Tap {
    Nothing,
    Bag,
    Pause,
    /// The map's action: sail to (or start on) the selected island.
    Go,
}

fn map_pos(p: (f32, f32)) -> (f32, f32) {
    (MAP_AT.0 + p.0, MAP_AT.1 - p.1)
}

fn plus(a: (f32, f32), x: f32, y: f32) -> (f32, f32) {
    (a.0 + x, a.1 - y)
}

/// Handles a tap at touch-screen pixel `p`.
pub fn tap(s: &mut Session, sheet: &Sheet, p: (f32, f32)) -> Tap {
    if s.map_shown() {
        let world = &s.world;
        let Some(m) = &mut s.map else {
            return Tap::Nothing;
        };
        if ACTION.contains(p) {
            return Tap::Go;
        }
        if PREV.contains(p) {
            m.cycle(-1);
            return Tap::Nothing;
        }
        if NEXT.contains(p) {
            m.cycle(1);
            return Tap::Nothing;
        }
        // orig: MapLand is a Button: tap an island to pick it; tapping the
        // picked one again sets off.
        for (l, land) in world.map.lands.iter().enumerate() {
            let Some(sp) = land.sprite.and_then(|id| sheet.world(id)) else {
                continue;
            };
            let sp = sheet.get(sp);
            let c = map_pos(land.at);
            let (w, h) = (sp.w as f32, sp.h as f32);
            let r = Rect::new(c.0 - sp.px, c.1 - sp.py, w, h);
            if r.contains(p) && m.options.contains(&l) {
                if m.selected == l {
                    return Tap::Go;
                }
                m.selected = l;
                return Tap::Nothing;
            }
        }
        return Tap::Nothing;
    }
    if s.pending_new_slot.is_some() {
        if REPLACE.contains(p) {
            s.confirm_new_game();
        } else if CANCEL.contains(p) || PAUSE.contains(p) {
            s.cancel_new_game();
        }
        return Tap::Nothing;
    }
    if PAUSE.contains(p) {
        return Tap::Pause;
    }
    if s.editor.is_some() {
        tap_editor(s, p);
        return Tap::Nothing;
    }
    if s.paused {
        if BANNER.contains(p) {
            s.editor = Some(Editor::default());
            return Tap::Nothing;
        }
        for i in 0..game::save::SLOTS {
            if slot_save(i).contains(p) {
                if matches!(s.slots[i], SlotState::Failed(_)) {
                    s.request_new_game(i);
                } else {
                    s.save_to(i);
                }
            } else if slot_load(i).contains(p) {
                s.load_from(i);
            }
        }
        if RESUME.contains(p) {
            return Tap::Pause;
        }
        return Tap::Nothing;
    }
    if BAG_TAP.contains(p) {
        return Tap::Bag;
    }
    Tap::Nothing
}

pub fn draw(s: &Session, sheet: &Sheet, r: &mut impl Renderer) {
    r.clear(BACKGROUND);
    if s.map_shown() {
        draw_map(s, sheet, r);
        return;
    }
    if let Some(slot) = s.pending_new_slot {
        text::draw(
            r,
            sheet,
            &format!("REPLACE SLOT {}?", slot + 1),
            36.0,
            60.0,
            24.0,
            Color::WHITE,
        );
        text::draw(
            r,
            sheet,
            "START A NEW GAME IN THIS SLOT.",
            24.0,
            96.0,
            14.0,
            Color::WHITE,
        );
        text::draw(
            r,
            sheet,
            "THE EXISTING SAVE WILL BE REPLACED.",
            24.0,
            116.0,
            14.0,
            Color::WHITE,
        );
        button(r, sheet, CANCEL, "CANCEL", Color::WHITE);
        button(r, sheet, REPLACE, "REPLACE", GOLD);
    } else if let Some(ed) = s.editor {
        draw_editor(s, ed, sheet, r);
    } else if s.paused {
        draw_pause(s, sheet, r);
    } else {
        draw_bag(s, sheet, r);
        draw_day(s, sheet, r);
    }
    if s.god {
        let l = "GOD MODE";
        let (w, _) = text::measure(sheet, l, 12.0);
        text::draw(
            r,
            sheet,
            l,
            ((W - w) / 2.0).round(),
            6.0,
            12.0,
            Color::new(1.0, 0.85, 0.3, 1.0),
        );
    }
    let warnings = [s.warning.as_str(), s.save_message.as_str()];
    let lines = warnings.iter().flat_map(|w| w.lines()).count() as f32;
    for (i, l) in warnings.iter().flat_map(|w| w.lines()).enumerate() {
        let (w, _) = text::measure(sheet, l, 12.0);
        let y = H - 11.0 * (lines - i as f32);
        text::draw(
            r,
            sheet,
            l,
            ((W - w) / 2.0).round(),
            y,
            12.0,
            Color::new(1.0, 0.6, 0.4, 1.0),
        );
    }
    // Pause button.
    r.rect_lines(PAUSE, 2.0, Color::WHITE);
    r.rect(
        Rect::new(PAUSE.x + 7.0, PAUSE.y + 6.0, 3.0, 12.0),
        Color::WHITE,
    );
    r.rect(
        Rect::new(PAUSE.x + 14.0, PAUSE.y + 6.0, 3.0, 12.0),
        Color::WHITE,
    );
}

/// A crisp outline `t` pixels thick (as filled strips: line drawing can
/// lose an edge at 1 px).
fn outline(r: &mut impl Renderer, b: Rect, t: f32, c: Color) {
    r.rect(Rect::new(b.x, b.y, b.w, t), c);
    r.rect(Rect::new(b.x, b.y + b.h - t, b.w, t), c);
    r.rect(Rect::new(b.x, b.y, t, b.h), c);
    r.rect(Rect::new(b.x + b.w - t, b.y, t, b.h), c);
}

fn button(r: &mut impl Renderer, sheet: &Sheet, b: Rect, label: &str, color: Color) {
    outline(r, b, 1.0, color);
    let (w, _) = text::measure(sheet, label, 16.0);
    text::draw(
        r,
        sheet,
        label,
        (b.x + (b.w - w) / 2.0).round(),
        b.y + b.h / 2.0 + 5.0,
        16.0,
        color,
    );
}

/// The pause menu: three save slots (the current one in gold) and Resume.
fn draw_pause(s: &Session, sheet: &Sheet, r: &mut impl Renderer) {
    let title = "PAUSED";
    let (w, _) = text::measure(sheet, title, 24.0);
    text::draw(
        r,
        sheet,
        title,
        ((W - w) / 2.0).round(),
        30.0,
        24.0,
        Color::WHITE,
    );
    let dim = Color::bytes([150, 140, 130, 255]);
    for i in 0..game::save::SLOTS {
        let row = slot_row(i);
        let color = if i == s.slot { GOLD } else { Color::WHITE };
        outline(r, row, if i == s.slot { 2.0 } else { 1.0 }, color);
        text::draw(
            r,
            sheet,
            &format!("SLOT {}", i + 1),
            row.x + 8.0,
            row.y + 17.0,
            16.0,
            color,
        );
        let info = match &s.slots[i] {
            SlotState::Saved(info) | SlotState::Recovered(info) => format!(
                "{} - DAY {} - {} COINS",
                info.island.to_uppercase(),
                info.day,
                info.coins
            ),
            SlotState::Empty => "EMPTY".to_owned(),
            SlotState::Failed(problem) => problem.label().to_owned(),
        };
        text::draw(r, sheet, &info, row.x + 8.0, row.y + 34.0, 14.0, dim);
        let save = if matches!(s.slots[i], SlotState::Failed(_)) {
            "NEW"
        } else {
            "SAVE"
        };
        button(r, sheet, slot_save(i), save, Color::WHITE);
        let load = match s.slots[i] {
            SlotState::Empty => "NEW",
            SlotState::Failed(_) => "RETRY",
            _ => "LOAD",
        };
        button(r, sheet, slot_load(i), load, Color::WHITE);
    }
    button(r, sheet, BANNER, "BANNER", Color::WHITE);
    button(r, sheet, RESUME, "RESUME", GOLD);
}

/// The banner editor: the banner and the monarch as they'll look, king or
/// queen, the pattern and emblem, and a color for each of the three parts.
fn draw_editor(s: &Session, ed: Editor, sheet: &Sheet, r: &mut impl Renderer) {
    let (coat, look) = (s.game.coat, s.game.look);
    let rgb = Color::rgb;
    // orig: Banner: the pattern in the main and trim colors, the emblem over it.
    let d = Draw::tint(Color::WHITE).scale(PREVIEW_BANNER_SCALE);
    if let Some(id) = heraldry::pattern(sheet, look) {
        r.sprite(
            id,
            PREVIEW_BANNER,
            d.recolor(rgb(coat.primary), rgb(coat.secondary)),
        );
    }
    if let Some(id) = heraldry::emblem(sheet, look) {
        r.sprite(
            id,
            PREVIEW_BANNER,
            Draw::tint(rgb(coat.emblem)).scale(PREVIEW_BANNER_SCALE),
        );
    }
    // The monarch standing on the steed, as on the island.
    let scale = PREVIEW_RIDER_SCALE;
    let steed = s.game.steed;
    let clip = &s.world.steeds[steed as usize].clips;
    if let Some(id) = sheet
        .clip(&top::clip_name(clip, Action::Stand))
        .and_then(|c| c.sprite("", 0.0))
    {
        let (mane, hide) = top::steed_colors(s, steed, true);
        r.sprite(
            id,
            PREVIEW_RIDER,
            Draw::tint(Color::WHITE).scale(scale).recolor(mane, hide),
        );
    }
    let who = heraldry::rider(look);
    if let Some(id) = sheet.id(who.default) {
        let d = Draw::tint(Color::WHITE)
            .scale(scale)
            .recolor(rgb(coat.primary), top::SKIN);
        r.sprite(id, PREVIEW_RIDER, d);
    }
    if let Some(id) = sheet.id(who.crown) {
        let (cx, cy) = top::CROWN_DEFAULT_POS;
        let px = 32.0 * scale;
        let at = (
            (PREVIEW_RIDER.0 + cx * px).round(),
            (PREVIEW_RIDER.1 - cy * px).round(),
        );
        r.sprite(id, at, Draw::tint(Color::WHITE).scale(scale));
    }

    let pick = |on: bool| if on { GOLD } else { Color::WHITE };
    button(r, sheet, KING, "KING", pick(!look.queen));
    button(r, sheet, QUEEN, "QUEEN", pick(look.queen));
    for (i, (label, v)) in [("PATTERN", look.pattern), ("EMBLEM", look.emblem)]
        .into_iter()
        .enumerate()
    {
        let y = STEP_ROWS[i] + STEP_SIZE / 2.0 + 5.0;
        text::draw(r, sheet, label, KING.x, y, 16.0, Color::WHITE);
        button(r, sheet, step_prev(i), "<", Color::WHITE);
        button(r, sheet, step_next(i), ">", Color::WHITE);
        let num = format!("{}", v + 1);
        let (w, _) = text::measure(sheet, &num, 16.0);
        let mid = (STEP_PREV_X + STEP_SIZE + STEP_NEXT_X) / 2.0;
        text::draw(
            r,
            sheet,
            &num,
            (mid - w / 2.0).round(),
            y,
            16.0,
            Color::WHITE,
        );
    }
    let mut coat = coat;
    for (i, label) in ["MAIN", "TRIM", "EMBLEM"].into_iter().enumerate() {
        let b = swatch(i);
        r.rect(b, rgb(*coat_color(&mut coat, i)));
        let on = ed.target == i;
        outline(r, b, if on { 2.0 } else { 1.0 }, pick(on));
        let (w, _) = text::measure(sheet, label, 12.0);
        text::draw(
            r,
            sheet,
            label,
            (b.x + (b.w - w) / 2.0).round(),
            b.y + b.h + 12.0,
            12.0,
            pick(on),
        );
    }
    button(r, sheet, RANDOM, "RANDOM", Color::WHITE);
    button(r, sheet, DONE, "DONE", GOLD);
    for i in 0..heraldry::PALETTE_COLS * heraldry::PALETTE_ROWS {
        r.rect(palette_cell(i), rgb(heraldry::palette(i)));
    }
}

fn draw_day(s: &Session, sheet: &Sheet, r: &mut impl Renderer) {
    let g = &s.game;
    let day = format!("DAY {}", g.days.day);
    let (w, _) = text::measure(sheet, &day, 20.0);
    text::draw(r, sheet, &day, W - w - 10.0, 24.0, 20.0, Color::WHITE);
    let dl = time::daylight(g.clock.time);
    let (bx, by, bw, bh) = (W - 90.0, 34.0, 80.0, 8.0);
    let (icon, fill) = match dl.phase {
        Celestial::Sun => (GOLD, Color::bytes([240, 200, 90, 255])),
        Celestial::Moon => (
            Color::bytes([200, 210, 240, 255]),
            Color::bytes([120, 130, 190, 255]),
        ),
    };
    r.circle(bx - 10.0, by + bh / 2.0, 4.0, icon);
    r.rect_lines(Rect::new(bx, by, bw, bh), 1.0, Color::WHITE);
    r.rect(
        Rect::new(bx + 1.0, by + 1.0, (bw - 2.0) * dl.remaining, bh - 2.0),
        fill,
    );

    // The mount's stamina (the original only shows it by the steed panting).
    // orig: Steed.stamina, tiredTimer, wellFedTimer.
    let m = &g.monarch;
    let by = by + 14.0;
    let fill = if m.tired_timer > 0.0 {
        Color::bytes([220, 90, 60, 255])
    } else if m.well_fed_timer > 0.0 {
        GOLD
    } else {
        Color::bytes([120, 200, 110, 255])
    };
    r.rect(Rect::new(bx - 14.0, by + 1.0, 8.0, bh - 2.0), fill);
    r.rect_lines(Rect::new(bx, by, bw, bh), 1.0, Color::WHITE);
    r.rect(
        Rect::new(
            bx + 1.0,
            by + 1.0,
            (bw - 2.0) * m.stamina.clamp(0.0, 1.0),
            bh - 2.0,
        ),
        fill,
    );

    draw_compass(s, sheet, r, (bx - 14.0, by + 14.0));
}

/// Within this many units of the campfire the compass says you're home.
const AT_CAMP: f32 = 6.0;

/// Which way the campfire is, and how far (not in the original: for
/// resuming a game deep in the woods). `at` is the row's top left.
fn draw_compass(s: &Session, sheet: &Sheet, r: &mut impl Renderer, at: (f32, f32)) {
    let dx = s.game.campfire - s.game.monarch.x;
    let flame = Color::bytes([255, 150, 50, 255]);
    let (x, y) = at;
    let mid = y + 10.0;
    if dx.abs() < AT_CAMP {
        r.circle(x + 4.0, mid, 4.0, flame);
        text::draw(r, sheet, "CAMP", x + 14.0, mid + 5.0, 14.0, Color::WHITE);
        return;
    }
    let dir = dx.signum();
    // A pixel arrow pointing at a flame: a shaft and a head of narrowing columns.
    let aw = 20.0;
    let (ax, fire) = if dir > 0.0 {
        (x, x + 28.0)
    } else {
        (x + 12.0, x + 4.0)
    };
    let tip = if dir > 0.0 { ax + aw } else { ax };
    for i in 0..6 {
        let h = 2.0 * (i + 1) as f32;
        let cx = tip - dir * (i as f32 + 1.0);
        r.rect(Rect::new(cx.min(cx + 1.0), mid - h / 2.0, 1.0, h), GOLD);
    }
    let shaft = Rect::new(
        if dir > 0.0 { ax } else { ax + 6.0 },
        mid - 1.5,
        aw - 6.0,
        3.0,
    );
    r.rect(shaft, GOLD);
    r.circle(fire, mid, 4.0, flame);
    let far = format!("{}", dx.abs().round() as i32);
    text::draw(r, sheet, &far, x + 40.0, mid + 5.0, 14.0, Color::WHITE);
}

/// orig: `CoinBag`: the back of the bag, the coins (back pile darker),
/// the bag's front lip; the coin count (`Coin Text`) underneath.
fn draw_bag(s: &Session, sheet: &Sheet, r: &mut impl Renderer) {
    let bag = &s.world.bag;
    let sprite = |id: Option<u16>| id.and_then(|i| sheet.world(i));
    let bounce = (s.bag_bounce / 0.15 * std::f32::consts::PI).sin() * 4.0;
    let at = (BAG_AT.0, BAG_AT.1 - bounce);
    if let Some(b) = sprite(bag.back) {
        r.sprite(b, at, Draw::tint(Color::WHITE).scale(BAG_SCALE));
    }
    if let Some(coin) = sprite(bag.coin) {
        // orig: z order: loose coins behind the back pile, the front pile in front.
        for pile in [
            game::bag::Pile::Loose,
            game::bag::Pile::Back,
            game::bag::Pile::Front,
        ] {
            for c in s.game.bag.coins.iter().filter(|c| c.pile == pile) {
                let p = ((at.0 + c.x * BAG_PX).round(), (at.1 - c.y * BAG_PX).round());
                let rgb = bag
                    .coin_colors
                    .get(c.color as usize)
                    .copied()
                    .unwrap_or([1.0, 0.8, 0.3]);
                let tint = Color::new(rgb[0] * c.shade, rgb[1] * c.shade, rgb[2] * c.shade, 1.0);
                let angle = -c.angle.to_radians();
                r.sprite(coin, p, Draw::tint(tint).scale(BAG_SCALE).angle(angle));
                // orig: BagCoin.Blink: the white overlay ramps up over 0.2 s.
                if c.blink > 0.0 {
                    let a = (c.blink / 0.2).min(1.0);
                    r.sprite(
                        coin,
                        p,
                        Draw::tint(Color::new(a, a, a, 1.0))
                            .scale(BAG_SCALE)
                            .angle(angle)
                            .additive(),
                    );
                }
            }
        }
    }
    if let Some(f) = sprite(bag.front) {
        r.sprite(f, at, Draw::tint(Color::WHITE).scale(BAG_SCALE));
    }
    let count = format!("{}", s.game.wallet.coins());
    let (w, _) = text::measure(sheet, &count, 24.0);
    text::draw(
        r,
        sheet,
        &count,
        (BAG_AT.0 - w / 2.0).round(),
        BAG_AT.1 + 22.0 * BAG_SCALE + 10.0,
        24.0,
        Color::WHITE,
    );
}

/// A looping 4-frame map animation (1 s per loop).
fn frame(frames: &[u16], t: f32) -> Option<u16> {
    if frames.is_empty() {
        return None;
    }
    Some(frames[(t * frames.len() as f32) as usize % frames.len()])
}

/// orig: `Map`, `MapLand`, `MapUnlock`, `MapCloud`.
fn draw_map(s: &Session, sheet: &Sheet, r: &mut impl Renderer) {
    let world = &s.world;
    let map = &world.map;
    let Some(choice) = &s.map else { return };
    let spr = |r: &mut _, id: u16, at: (f32, f32), d: Draw| {
        if let Some(i) = sheet.world(id) {
            Renderer::sprite(r, i, at, d);
        }
    };
    let plain = Draw::tint(Color::WHITE);
    if let Some(b) = map.back.and_then(|i| sheet.world(i)) {
        let (w, h) = map.back_size;
        let rect = Rect::new(MAP_AT.0 - w / 2.0, MAP_AT.1 - h / 2.0, w, h);
        r.sliced(b, rect, map.back_border, Color::WHITE);
    }
    for &(id, p) in &map.details {
        spr(r, id, map_pos(p), plain);
    }
    let progress = &s.progress;
    for (l, land) in map.lands.iter().enumerate() {
        let at = map_pos(land.at);
        let selected = choice.selected == l;
        if let Some(id) = land.sprite {
            // orig: MapLand.SetSelected: the picked island is tinted 0.8.
            let tint = if selected {
                Color::new(0.8, 0.8, 0.8, 1.0)
            } else {
                Color::WHITE
            };
            spr(r, id, at, Draw::tint(tint));
        }
        if progress.revealed(l, choice.won) {
            // orig: MapUnlock: a figurine on a patch of light once unlocked.
            for u in &land.unlocks {
                if !progress.unlocked.contains(&u.unlock) {
                    continue;
                }
                let p = plus(at, u.at.0, u.at.1);
                if let Some(id) = map.light_patch {
                    spr(r, id, p, plain);
                }
                if let Some(id) = u.icon {
                    spr(r, id, plus(p, u.icon_at.0, u.icon_at.1), plain);
                }
            }
        } else {
            for &(id, p) in &land.clouds {
                spr(r, id, plus(at, p.0, p.1), plain);
            }
        }
        // orig: MapLand.SetCurrent: the boat by the island just left,
        // wrecked unless it was won.
        if l == s.game.level_index {
            let frames = if choice.won { &map.boat } else { &map.wreck };
            if let Some(id) = frame(frames, choice.t) {
                let ((x, y), flip) = land.boat;
                spr(r, id, plus(at, x, y), plain.flip(flip));
            }
        }
        if selected {
            if let Some(id) = frame(&map.pointer, choice.t) {
                let ((x, y), flip) = land.pointer;
                spr(r, id, plus(at, x, y), plain.flip(flip));
            }
        }
    }
    // Buttons. orig: Previous / Next Land and the Action button.
    let label = if choice.won { "Sail" } else { "Start" };
    for (rect, t) in [(PREV, "<"), (NEXT, ">"), (ACTION, label)] {
        if let Some(b) = map.button.and_then(|i| sheet.world(i)) {
            r.sliced(b, rect, map.button_border, Color::WHITE);
        }
        let (w, h) = text::measure(sheet, t, 16.0);
        text::draw(
            r,
            sheet,
            t,
            (rect.x + (rect.w - w) / 2.0).round(),
            (rect.y + rect.h / 2.0 + h / 2.0).round(),
            16.0,
            Color::bytes([60, 40, 30, 255]),
        );
    }
    let title = &world.levels[choice.selected].name;
    let (w, _) = text::measure(sheet, title, 18.0);
    text::draw(
        r,
        sheet,
        title,
        (MAP_AT.0 - w / 2.0).round(),
        16.0,
        18.0,
        Color::WHITE,
    );
}
