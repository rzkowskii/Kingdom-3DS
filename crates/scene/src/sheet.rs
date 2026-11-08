//! Sprites and animation clips as the backends draw them: every sprite has an
//! index, a page, its rectangle there and its pivot. Clips refer to sprites
//! by index. Desktop reads `tools/extract`'s JSON; the 3DS reads the packed
//! `sheet.bin` that `pack3ds` writes (with recolour masks split out).

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use serde::{Deserialize, Serialize};

/// FNV-1a: name lookups happen hundreds of times a frame, and the default
/// SipHash is slow on the 3DS's ARM11.
#[derive(Default)]
pub struct Fnv(u64);

impl Hasher for Fnv {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        let mut h = if self.0 == 0 {
            0xcbf2_9ce4_8422_2325
        } else {
            self.0
        };
        for &b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        self.0 = h;
    }
}

type Map<K, V> = HashMap<K, V, BuildHasherDefault<Fnv>>;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct Sprite {
    pub page: u16,
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    /// Pivot in pixels from the sprite's top-left.
    pub px: f32,
    pub py: f32,
    /// The parts a recolour tints (`_RecolorA` / `_RecolorB` pixels), as
    /// separate sprites, where the GPU can't key on alpha (3DS).
    pub masks: [Option<u32>; 2],
}

/// Keys of a child's position: (time, x, y, stepped).
pub type OffsetKeys = Vec<(f32, f32, f32, bool)>;
/// Explicit null keyframes hide the renderer.
pub type SpriteKeys = Vec<(f32, Option<u32>)>;

/// Sprite keys plus authored numeric curves/markers shared with simulation.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Clip {
    pub length: f32,
    pub looping: bool,
    /// Per child path ("" = root): (time, sprite).
    pub sprites: Vec<(String, SpriteKeys)>,
    /// Per child path: (time, x, y, stepped).
    pub offsets: Vec<(String, OffsetKeys)>,
    pub animation: game::animation::Clip,
}

impl Clip {
    fn wrap(&self, t: f32) -> f32 {
        if self.looping && self.length > 0.0 {
            t.rem_euclid(self.length)
        } else {
            t.min(self.length)
        }
    }

    /// Sprite shown on `path` at time `t`. orig: Animator sampling of m_Sprite curves.
    pub fn sprite(&self, path: &str, t: f32) -> Option<u32> {
        self.sprite_frame(path, t).flatten()
    }

    /// No track is None; an explicit null sprite key is Some(None).
    pub fn sprite_frame(&self, path: &str, t: f32) -> Option<Option<u32>> {
        let keys = &self.sprites.iter().find(|k| k.0 == path)?.1;
        let t = self.wrap(t);
        keys.iter()
            .rev()
            .find(|k| k.0 <= t)
            .or(keys.first())
            .map(|k| k.1)
    }

    pub fn pose(&self, path: &str, t: f32, rest: game::animation::Pose) -> game::animation::Pose {
        self.animation.pose(path, t, rest)
    }

    pub fn has_offsets(&self, path: &str) -> bool {
        self.animation
            .tracks
            .iter()
            .any(|k| k.path == path && k.position.iter().any(|k| !k.is_empty()))
            || self.offsets.iter().any(|k| k.0 == path)
    }

    /// Local position of child `path` at time `t`, in units.
    pub fn offset(&self, path: &str, t: f32) -> (f32, f32) {
        if self
            .animation
            .tracks
            .iter()
            .any(|k| k.path == path && k.position.iter().any(|k| !k.is_empty()))
        {
            let p = self
                .animation
                .pose(path, t, game::animation::Pose::default());
            return (p.position[0], p.position[1]);
        }
        let Some(keys) = self.offsets.iter().find(|k| k.0 == path).map(|k| &k.1) else {
            return (0.0, 0.0);
        };
        if keys.is_empty() {
            return (0.0, 0.0);
        }
        let t = self.wrap(t);
        let i = keys.iter().rposition(|k| k.0 <= t).unwrap_or(0);
        let a = keys[i];
        match keys.get(i + 1) {
            Some(b) if !a.3 && b.0 > a.0 => {
                let k = (t - a.0) / (b.0 - a.0);
                (a.1 + (b.1 - a.1) * k, a.2 + (b.2 - a.2) * k)
            }
            _ => (a.1, a.2),
        }
    }
}

/// How a page is stored on the GPU (3DS only; desktop pages are PNGs).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PageFormat {
    Rgba8,
    Rgba5551,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Page {
    /// Path relative to the assets folder.
    pub file: String,
    pub w: u16,
    pub h: u16,
    pub format: PageFormat,
}

/// The game's pixel font. orig: `Resources/fonts/Kingdom`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Font {
    /// Native pixel size (drawn at whole multiples of it).
    pub size: f32,
    pub line: f32,
    /// (character, sprite, advance).
    pub glyphs: Vec<(char, Option<u32>, f32)>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Sheet {
    pub pages: Vec<Page>,
    pub font: Font,
    pub sprites: Vec<Sprite>,
    pub names: Vec<String>,
    pub clips: Vec<(String, Clip)>,
    #[serde(skip)]
    index: Map<String, u32>,
    #[serde(skip)]
    clip_index: Map<String, usize>,
    /// `WorldData::sprites` id → sprite here.
    #[serde(skip)]
    world: Vec<Option<u32>>,
    #[serde(skip)]
    glyph_index: Map<char, (Option<u32>, f32)>,
}

impl Sheet {
    /// Checks references in JSON and packed sheets before rendering.
    pub fn validate(&self) -> Result<(), String> {
        let valid = |id: u32| (id as usize) < self.sprites.len();
        if self.names.len() != self.sprites.len()
            || self.sprites.iter().any(|s| {
                s.page as usize >= self.pages.len()
                    || s.masks.iter().flatten().any(|&id| !valid(id))
            })
            || self.clips.iter().any(|(_, c)| {
                c.sprites
                    .iter()
                    .any(|(_, keys)| keys.iter().any(|&(_, id)| id.is_some_and(|id| !valid(id))))
                    || !c.animation.validate()
            })
            || self
                .font
                .glyphs
                .iter()
                .any(|g| g.1.is_some_and(|id| !valid(id)))
        {
            return Err("sprite sheet contains invalid references; regenerate assets".into());
        }
        Ok(())
    }

    /// Builds the name lookups (after loading) and maps the world's sprites.
    pub fn link(&mut self, world_sprites: &[String]) {
        self.index = self
            .names
            .iter()
            .enumerate()
            .map(|(i, n)| (n.clone(), i as u32))
            .collect();
        self.clip_index = self
            .clips
            .iter()
            .enumerate()
            .map(|(i, c)| (c.0.clone(), i))
            .collect();
        self.world = world_sprites
            .iter()
            .map(|n| self.index.get(n).copied())
            .collect();
        self.glyph_index = self.font.glyphs.iter().map(|g| (g.0, (g.1, g.2))).collect();
    }

    /// A character's sprite (none for spaces) and advance.
    pub fn glyph(&self, c: char) -> Option<(Option<u32>, f32)> {
        self.glyph_index.get(&c).copied()
    }

    pub fn id(&self, name: &str) -> Option<u32> {
        self.index.get(name).copied()
    }

    /// The sprite for a `WorldData` sprite id.
    pub fn world(&self, id: u16) -> Option<u32> {
        self.world.get(id as usize).copied().flatten()
    }

    pub fn clip(&self, name: &str) -> Option<&Clip> {
        self.clip_index.get(name).map(|&i| &self.clips[i].1)
    }

    pub fn get(&self, id: u32) -> &Sprite {
        &self.sprites[id as usize]
    }
}

#[cfg(feature = "json")]
mod json {
    use std::collections::HashMap;

    use serde::Deserialize;

    use super::*;

    #[derive(Deserialize)]
    struct SpriteJson {
        page: u16,
        x: u16,
        y: u16,
        w: u16,
        h: u16,
        px: f32,
        py: f32,
    }

    #[derive(Deserialize)]
    struct SheetJson {
        pages: Vec<String>,
        sprites: HashMap<String, SpriteJson>,
    }

    #[derive(Deserialize)]
    struct FontJson {
        size: f32,
        line: f32,
        glyphs: Vec<(char, String, f32)>,
    }

    #[derive(Deserialize)]
    struct ClipJson {
        length: f32,
        looping: bool,
        sprites: HashMap<String, Vec<(f32, Option<String>)>>,
        offsets: HashMap<String, Vec<(f32, f32, f32, bool)>>,
        animation: game::animation::Clip,
    }

    #[derive(Deserialize)]
    struct ClipFile {
        version: u16,
        clips: HashMap<String, ClipJson>,
    }

    impl Sheet {
        /// From `tools/extract`'s `sprites.json`, `clips.json` and `font.json` (pages 1024²).
        pub fn from_json(sprites: &str, clips: &str, font: &str) -> Result<Sheet, String> {
            let f: FontJson = serde_json::from_str(font).map_err(|e| e.to_string())?;
            let s: SheetJson = serde_json::from_str(sprites).map_err(|e| e.to_string())?;
            let file: ClipFile = serde_json::from_str(clips)
                .map_err(|e| format!("animation data: {e}; regenerate assets"))?;
            if file.version != 1 {
                return Err("unsupported animation data version; regenerate assets".into());
            }
            let c = file.clips;
            let mut sheet = Sheet {
                pages: s
                    .pages
                    .into_iter()
                    .map(|file| Page {
                        file,
                        w: 1024,
                        h: 1024,
                        format: PageFormat::Rgba8,
                    })
                    .collect(),
                ..Default::default()
            };
            let mut names: Vec<(String, SpriteJson)> = s.sprites.into_iter().collect();
            names.sort_by(|a, b| a.0.cmp(&b.0));
            for (name, sp) in names {
                if sp.page as usize >= sheet.pages.len() {
                    return Err(format!("sprite {name} references missing page {}", sp.page));
                }
                sheet.names.push(name);
                sheet.sprites.push(Sprite {
                    page: sp.page,
                    x: sp.x,
                    y: sp.y,
                    w: sp.w,
                    h: sp.h,
                    px: sp.px,
                    py: sp.py,
                    masks: [None, None],
                });
            }
            sheet.link(&[]);
            sheet.font = Font {
                size: f.size,
                line: f.line,
                glyphs: f
                    .glyphs
                    .into_iter()
                    .map(|(c, n, adv)| (c, sheet.id(&n), adv))
                    .collect(),
            };
            let mut clips: Vec<(String, ClipJson)> = c.into_iter().collect();
            clips.sort_by(|a, b| a.0.cmp(&b.0));
            for (name, cj) in clips {
                let mut sprites = Vec::new();
                for (path, keys) in cj.sprites {
                    let keys = keys
                        .into_iter()
                        .map(|(t, n)| {
                            let id = n
                                .map(|n| {
                                    sheet
                                        .id(&n)
                                        .ok_or_else(|| format!("clip {name}: unknown sprite {n}"))
                                })
                                .transpose()?;
                            Ok((t, id))
                        })
                        .collect::<Result<Vec<_>, String>>()?;
                    sprites.push((path, keys));
                }
                sprites.sort_by(|a, b| a.0.cmp(&b.0));
                let mut offsets: Vec<_> = cj.offsets.into_iter().collect();
                offsets.sort_by(|a, b| a.0.cmp(&b.0));
                sheet.clips.push((
                    name,
                    Clip {
                        length: cj.length,
                        looping: cj.looping,
                        sprites,
                        offsets,
                        animation: cj.animation,
                    },
                ));
            }
            Ok(sheet)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_offset_track_is_stationary() {
        let c = Clip {
            offsets: vec![("X".into(), vec![])],
            ..Default::default()
        };
        assert_eq!(c.offset("X", 1.0), (0.0, 0.0));
    }
}
