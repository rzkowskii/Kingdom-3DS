//! Packs the extracted assets for the 3DS romfs:
//!
//! - Sprites are cut back out of the desktop pages and repacked: sprites with
//!   only opaque and clear pixels go on RGBA5551 pages (half the memory),
//!   the few with soft edges on RGBA8 pages. Pages shrink to the smallest
//!   power of two that fits.
//! - The PICA200 has no fragment shaders, so the alpha-keyed recolour can't
//!   run on the GPU: the keyed pixels of each sprite become two extra mask
//!   sprites that the renderer tints separately.
//! - Pages go through `tex3ds` into `.t3x`; the sheet (sprites, clips, font)
//!   becomes `sheet.bin`; `world.bin` is copied as is.
//! - The sound clips (`audio/*.adp`, ~100 MB) go on the SD card instead, so
//!   the `.3dsx` stays small: into `sdcard/3ds/kingdom-3ds/audio` beside the
//!   romfs folder, to copy onto the card's root once.
//!
//! `cargo run --release -p extract --bin pack3ds -- assets crates/n3ds/romfs`

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use scene::sheet::{Page, PageFormat, Sheet, Sprite};

/// orig: the recolour keys (see the desktop's `RECOLOR_FS`): alpha 208 is
/// tinted by `recolorA`, 176 by `recolorB`.
const KEY_A: u8 = 208;
const KEY_B: u8 = 176;
const PAGE: u32 = 1024;
const PAD: u32 = 1;

struct Img {
    w: u32,
    h: u32,
    px: Vec<u8>,
}

fn split_recolor(base: &mut Img, cloud: bool) -> [Option<Img>; 2] {
    // Cloud alpha is shader coverage, even when it equals a recolour key.
    if cloud {
        return [None, None];
    }
    [KEY_A, KEY_B].map(|key| {
        let mut mask = Img {
            w: base.w,
            h: base.h,
            px: vec![0; base.px.len()],
        };
        let mut any = false;
        for (pixel, out) in base
            .px
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(mask.px.as_chunks_mut::<4>().0.iter_mut())
        {
            if pixel[3] == key {
                let shade = pixel[0];
                *out = [shade, shade, shade, 255];
                pixel[3] = 0;
                any = true;
            }
        }
        any.then_some(mask)
    })
}

#[cfg(test)]
#[test]
fn cloud_coverage_at_recolour_keys_survives_3ds_mask_split() {
    let pixels = [17, 28, 32, KEY_A, 33, 29, 23, KEY_B];
    let mut base = Img {
        w: 2,
        h: 1,
        px: pixels.to_vec(),
    };
    assert!(split_recolor(&mut base, true)
        .into_iter()
        .all(|m| m.is_none()));
    assert_eq!(base.px, pixels);
    let [a, b] = split_recolor(&mut base, false);
    assert_eq!(a.unwrap().px, [17, 17, 17, 255, 0, 0, 0, 0]);
    assert_eq!(b.unwrap().px, [0, 0, 0, 0, 33, 33, 33, 255]);
    assert_eq!(base.px, [17, 28, 32, 0, 33, 29, 23, 0]);
}

fn load_png(path: &Path) -> Result<Img, String> {
    let dec = png::Decoder::new(std::io::BufReader::new(
        std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?,
    ));
    let mut r = dec.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; r.output_buffer_size().unwrap_or(0)];
    let info = r.next_frame(&mut buf).map_err(|e| e.to_string())?;
    if info.color_type != png::ColorType::Rgba {
        return Err(format!("{}: expected RGBA", path.display()));
    }
    Ok(Img {
        w: info.width,
        h: info.height,
        px: buf,
    })
}

fn save_png(path: &Path, img: &Img) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), img.w, img.h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    w.write_image_data(&img.px).map_err(|e| e.to_string())
}

/// One sprite image to place, and which sprite it becomes.
struct Piece {
    sprite: usize,
    img: Img,
    soft: bool,
    /// The sprite whose masks these are (itself for a base): a sprite and its
    /// masks go on one page, so drawing a recoloured sprite never switches textures.
    group: usize,
}

/// A sprite and its masks stacked into one image, with each one's offset down it.
struct Block {
    img: Img,
    soft: bool,
    members: Vec<(usize, u32)>,
}

/// A page being filled: its shelves, pixels, and (sprite, x, y) placed.
type Filling = (Shelf, Img, Vec<(usize, u32, u32)>);

struct Shelf {
    w: u32,
    used_h: u32,
    shelf_y: u32,
    shelf_h: u32,
    x: u32,
}

impl Shelf {
    fn place(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if self.x + w > self.w {
            self.shelf_y += self.shelf_h + PAD;
            self.shelf_h = 0;
            self.x = 0;
        }
        if self.shelf_y + h > PAGE || w > self.w {
            return None;
        }
        let at = (self.x, self.shelf_y);
        self.x += w + PAD;
        self.shelf_h = self.shelf_h.max(h);
        self.used_h = self.used_h.max(self.shelf_y + h);
        Some(at)
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (Some(src), Some(out)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: pack3ds <assets dir> <romfs dir>");
        std::process::exit(2);
    };
    if args.len() != 3 {
        eprintln!("usage: pack3ds <assets dir> <romfs dir>");
        std::process::exit(2);
    }
    if let Err(e) = run(Path::new(src), Path::new(out)) {
        eprintln!("pack3ds: {e}");
        std::process::exit(1);
    }
}

fn run(src: &Path, out: &Path) -> Result<(), String> {
    let world_bytes =
        std::fs::read(src.join("world.bin")).map_err(|e| format!("world.bin: {e}"))?;
    let world =
        game::assetfile::decode_world(&world_bytes).map_err(|e| format!("world.bin: {e}"))?;
    let read = |n: &str| {
        std::fs::read_to_string(src.join(n)).map_err(|e| format!("{}: {e}", src.join(n).display()))
    };
    let sheet = Sheet::from_json(
        &read("sprites.json")?,
        &read("clips.json")?,
        &read("font.json")?,
    )?;
    let cloud_sprites: HashSet<_> = world
        .clouds
        .iter()
        .flat_map(game::data::CloudsDef::sprite_ids)
        .map(|id| {
            let name = &world.sprites[usize::from(id)];
            sheet
                .id(name)
                .ok_or_else(|| format!("missing cloud sprite {name}"))
        })
        .collect::<Result<_, _>>()?;
    let pages: Vec<Img> = sheet
        .pages
        .iter()
        .map(|p| load_png(&src.join(&p.file)))
        .collect::<Result<_, _>>()?;

    // Cut every sprite out; split off its recolour masks.
    let mut out_sheet = sheet.clone();
    let mut pieces: Vec<Piece> = Vec::new();
    let mut masks = 0;
    for i in 0..sheet.sprites.len() {
        let s = &mut out_sheet.sprites[i];
        let mut base = {
            let page = &pages[s.page as usize];
            let (w, h) = (s.w as u32, s.h as u32);
            let mut base = Img {
                w,
                h,
                px: vec![0; (w * h * 4) as usize],
            };
            for row in 0..h {
                let from = (((s.y as u32 + row) * page.w + s.x as u32) * 4) as usize;
                let to = (row * w * 4) as usize;
                base.px[to..to + (w * 4) as usize]
                    .copy_from_slice(&page.px[from..from + (w * 4) as usize]);
            }
            base
        };
        let s = *s;
        let cloud = cloud_sprites.contains(&(i as u32));
        for (k, m) in split_recolor(&mut base, cloud).into_iter().enumerate() {
            let Some(m) = m else { continue };
            let id = out_sheet.sprites.len();
            out_sheet.sprites.push(Sprite {
                masks: [None, None],
                ..s
            });
            out_sheet
                .names
                .push(format!("{}#{}", sheet.names[i], ["a", "b"][k]));
            out_sheet.sprites[i].masks[k] = Some(id as u32);
            pieces.push(Piece {
                sprite: id,
                img: m,
                soft: false,
                group: i,
            });
            masks += 1;
        }
        let soft = cloud
            || base
                .px
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[3] != 0 && p[3] != 255);
        pieces.push(Piece {
            sprite: i,
            img: base,
            soft,
            group: i,
        });
    }

    // Trim each piece to its visible pixels (masks especially are mostly
    // empty); the pivot moves with the trim.
    for p in &mut pieces {
        let (w, h) = (p.img.w, p.img.h);
        let alpha = |x: u32, y: u32| p.img.px[((y * w + x) * 4 + 3) as usize] != 0;
        let rows: Vec<u32> = (0..h).filter(|&y| (0..w).any(|x| alpha(x, y))).collect();
        let cols: Vec<u32> = (0..w).filter(|&x| (0..h).any(|y| alpha(x, y))).collect();
        let (Some(&top), Some(&bottom), Some(&left), Some(&right)) =
            (rows.first(), rows.last(), cols.first(), cols.last())
        else {
            continue;
        };
        let (nw, nh) = (right - left + 1, bottom - top + 1);
        if (nw, nh) == (w, h) {
            continue;
        }
        let mut px = vec![0u8; (nw * nh * 4) as usize];
        for row in 0..nh {
            let from = (((top + row) * w + left) * 4) as usize;
            let to = (row * nw * 4) as usize;
            px[to..to + (nw * 4) as usize]
                .copy_from_slice(&p.img.px[from..from + (nw * 4) as usize]);
        }
        p.img = Img { w: nw, h: nh, px };
        let s = &mut out_sheet.sprites[p.sprite];
        s.w = nw as u16;
        s.h = nh as u16;
        s.px -= left as f32;
        s.py -= top as f32;
    }

    // Pack, opaque and soft separately, tallest first.
    std::fs::create_dir_all(out.join("gfx")).map_err(|e| e.to_string())?;
    // Pages from an earlier pack that had more of them.
    for e in std::fs::read_dir(out.join("gfx"))
        .map_err(|e| e.to_string())?
        .flatten()
    {
        if e.path().extension().is_some_and(|x| x == "t3x") {
            std::fs::remove_file(e.path())
                .map_err(|err| format!("{}: {err}", e.path().display()))?;
        }
    }
    out_sheet.pages.clear();
    let mut total_bytes = 0usize;
    let mut blocks: Vec<Block> = Vec::new();
    {
        let mut by_group: std::collections::BTreeMap<usize, Vec<Piece>> = Default::default();
        for p in pieces {
            by_group.entry(p.group).or_default().push(p);
        }
        for (group, members) in by_group {
            let soft = members.iter().any(|p| p.sprite == group && p.soft);
            let w = members.iter().map(|p| p.img.w).max().unwrap_or(0);
            let h = members.iter().map(|p| p.img.h).sum::<u32>() + PAD * (members.len() as u32 - 1);
            let mut img = Img {
                w,
                h,
                px: vec![0; (w * h * 4) as usize],
            };
            let mut y = 0;
            let mut placed = Vec::new();
            for p in &members {
                for row in 0..p.img.h {
                    let to = (((y + row) * w) * 4) as usize;
                    let from = (row * p.img.w * 4) as usize;
                    img.px[to..to + (p.img.w * 4) as usize]
                        .copy_from_slice(&p.img.px[from..from + (p.img.w * 4) as usize]);
                }
                placed.push((p.sprite, y));
                y += p.img.h + PAD;
            }
            blocks.push(Block {
                img,
                soft,
                members: placed,
            });
        }
    }
    for soft in [false, true] {
        let mut group: Vec<&Block> = blocks.iter().filter(|p| p.soft == soft).collect();
        group.sort_by_key(|p| std::cmp::Reverse((p.img.h, p.img.w)));
        let mut shelves: Vec<Filling> = Vec::new();
        for p in group {
            let (w, h) = (p.img.w, p.img.h);
            if w > PAGE || h > PAGE {
                let first = p.members.first().map_or(0, |m| m.0);
                return Err(format!(
                    "{} is larger than a page",
                    sheet.names.get(first).map_or("?", |s| s)
                ));
            }
            let spot = shelves
                .iter_mut()
                .enumerate()
                .find_map(|(n, s)| s.0.place(w, h).map(|xy| (n, xy)));
            let (n, (x, y)) = match spot {
                Some(v) => v,
                None => {
                    let mut s = Shelf {
                        w: PAGE,
                        used_h: 0,
                        shelf_y: 0,
                        shelf_h: 0,
                        x: 0,
                    };
                    let xy = s.place(w, h).expect("fresh page fits");
                    shelves.push((
                        s,
                        Img {
                            w: PAGE,
                            h: PAGE,
                            px: vec![0; (PAGE * PAGE * 4) as usize],
                        },
                        Vec::new(),
                    ));
                    (shelves.len() - 1, xy)
                }
            };
            let page = &mut shelves[n].1;
            for row in 0..h {
                let to = (((y + row) * PAGE + x) * 4) as usize;
                let from = (row * w * 4) as usize;
                page.px[to..to + (w * 4) as usize]
                    .copy_from_slice(&p.img.px[from..from + (w * 4) as usize]);
            }
            for &(sprite, dy) in &p.members {
                shelves[n].2.push((sprite, x, y + dy));
            }
        }
        for (shelf, mut img, placed) in shelves {
            // Smallest power-of-two height that holds it (3DS textures are powers of two).
            let h = shelf.used_h.next_power_of_two().max(8);
            img.px.truncate((PAGE * h * 4) as usize);
            img.h = h;
            let n = out_sheet.pages.len();
            let format = if soft {
                PageFormat::Rgba8
            } else {
                PageFormat::Rgba5551
            };
            let png = out.join(format!("gfx/page_{n}.png"));
            save_png(&png, &img)?;
            let t3x = PathBuf::from(format!("gfx/page_{n}.t3x"));
            tex3ds(&png, &out.join(&t3x), format)?;
            std::fs::remove_file(&png).ok();
            for (sprite, x, y) in placed {
                let s = &mut out_sheet.sprites[sprite];
                s.page = n as u16;
                s.x = x as u16;
                s.y = y as u16;
            }
            total_bytes += (PAGE * h) as usize * if soft { 4 } else { 2 };
            out_sheet.pages.push(Page {
                file: t3x.to_string_lossy().into_owned(),
                w: PAGE as u16,
                h: h as u16,
                format,
            });
        }
    }
    let bin = game::assetfile::encode(game::assetfile::Kind::Sheet, &out_sheet)
        .map_err(|e| e.to_string())?;
    std::fs::write(out.join("sheet.bin"), bin).map_err(|e| e.to_string())?;
    std::fs::write(out.join("world.bin"), &world_bytes).map_err(|e| format!("world.bin: {e}"))?;
    let sd = out
        .parent()
        .unwrap_or(Path::new("."))
        .join("sdcard/3ds/kingdom-3ds/audio");
    let copied = copy_audio(&src.join("audio"), &sd)?;
    println!("audio: {copied} clips copied to {}", sd.display());
    println!(
        "{} sprites ({} recolour masks) on {} pages: {:.1} MiB of texture memory",
        out_sheet.sprites.len(),
        masks,
        out_sheet.pages.len(),
        total_bytes as f32 / (1024.0 * 1024.0)
    );
    for (i, p) in out_sheet.pages.iter().enumerate() {
        println!("  page {i}: 1024×{} {:?}", p.h, p.format);
    }
    Ok(())
}

/// Copies the `.adp` clips that are new or changed. Returns how many.
fn copy_audio(from: &Path, to: &Path) -> Result<usize, String> {
    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
    let mut n = 0;
    for e in std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let p = e.map_err(|e| e.to_string())?.path();
        if p.extension().is_none_or(|x| x != "adp") {
            continue;
        }
        let dst = to.join(p.file_name().unwrap());
        let len = |p: &Path| std::fs::metadata(p).map(|m| m.len()).ok();
        if len(&dst) != len(&p) || std::fs::read(&dst).ok() != std::fs::read(&p).ok() {
            std::fs::copy(&p, &dst).map_err(|e| format!("{}: {e}", p.display()))?;
            n += 1;
        }
    }
    Ok(n)
}

/// orig toolchain: devkitPro's `tex3ds` ($DEVKITPRO/tools/bin).
fn tex3ds(png: &Path, t3x: &Path, format: PageFormat) -> Result<(), String> {
    let tool = std::env::var("DEVKITPRO")
        .map(|d| PathBuf::from(d).join("tools/bin/tex3ds"))
        .unwrap_or_else(|_| PathBuf::from("/opt/devkitpro/tools/bin/tex3ds"));
    let f = match format {
        PageFormat::Rgba8 => "rgba8",
        PageFormat::Rgba5551 => "rgba5551",
    };
    let status = Command::new(&tool)
        .args(["-f", f, "-z", "auto", "-o"])
        .arg(t3x)
        .arg(png)
        .status()
        .map_err(|e| format!("{}: {e} (is devkitPro installed?)", tool.display()))?;
    if !status.success() {
        return Err(format!("tex3ds failed on {}", png.display()));
    }
    Ok(())
}
