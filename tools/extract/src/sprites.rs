//! Cuts each Unity Sprite out of its source atlas using the sprite's mesh.
//!
//! Unity's tight packing places sprites by their polygon outline, so a
//! sprite's bounding rect can include pixels of its neighbours. We rasterize
//! the mesh triangles and copy only the pixels inside them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::yaml::{field, guid_of, hex_bytes, inline_map};

/// RGBA8 image, rows top to bottom.
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

impl Image {
    pub fn load_png(path: &Path) -> Result<Image, String> {
        let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut dec = png::Decoder::new(std::io::BufReader::new(file));
        dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = dec
            .read_info()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let mut buf = vec![0; reader.output_buffer_size().ok_or("png too large")?];
        let info = reader
            .next_frame(&mut buf)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        buf.truncate(info.buffer_size());
        let px = match info.color_type {
            png::ColorType::Rgba => buf,
            png::ColorType::Rgb => buf
                .chunks(3)
                .flat_map(|c| [c[0], c[1], c[2], 255])
                .collect(),
            png::ColorType::GrayscaleAlpha => buf
                .chunks(2)
                .flat_map(|c| [c[0], c[0], c[0], c[1]])
                .collect(),
            png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
            other => {
                return Err(format!(
                    "{}: unsupported color type {other:?}",
                    path.display()
                ))
            }
        };
        Ok(Image {
            w: info.width,
            h: info.height,
            px,
        })
    }
}

pub struct Sprite {
    pub name: String,
    pub img: Image,
    /// Pivot in pixels from the image's top-left corner.
    pub pivot: (f32, f32),
    /// Pixels per world unit. orig: `Sprite.pixelsPerUnit`.
    pub ppu: f32,
}

pub struct Sheet {
    pub sprites: Vec<Sprite>,
    /// Sprite asset GUID → final (deduplicated) sprite name, for clips.
    pub names_by_guid: HashMap<String, String>,
    /// Sprite asset path under Assets/ (e.g. `Sprite/coin.asset`) → final name.
    pub names_by_path: HashMap<String, String>,
    pub skipped: usize,
}

/// ShadowReceiver discards source alpha below .01 and uses half of the
/// remaining alpha range for coverage. Its inverse supplies the light rim.
/// RGB remains the authored cloud art.
pub fn bake_cloud_coverage(image: &mut Image) {
    for pixel in image.px.as_chunks_mut::<4>().0 {
        let alpha = u16::from(pixel[3]);
        pixel[3] = if alpha < 3 {
            0
        } else {
            ((alpha + 256) / 2) as u8
        };
    }
}

#[cfg(test)]
#[test]
fn cloud_coverage_matches_discard_and_half_alpha_without_touching_rgb() {
    let mut image = Image {
        w: 7,
        h: 1,
        px: [0, 1, 2, 3, 127, 254, 255]
            .into_iter()
            .flat_map(|alpha| [13, 29, 97, alpha])
            .collect(),
    };
    bake_cloud_coverage(&mut image);
    let expected: Vec<_> = [0, 0, 0, 129, 191, 255, 255]
        .into_iter()
        .flat_map(|alpha| [13, 29, 97, alpha])
        .collect();
    assert_eq!(image.px, expected);
}

struct Mesh {
    /// Vertex positions in source-atlas pixels, y up from the bottom.
    verts: Vec<(f32, f32)>,
    tris: Vec<[usize; 3]>,
}

fn parse_mesh(text: &str) -> Result<(String, String, f32, Mesh), String> {
    // Only the m_RD block; m_AtlasRD follows it and is empty in this build.
    let rd = &text[..text.find("m_AtlasRD:").unwrap_or(text.len())];
    let name = field(text, "m_Name").ok_or("no m_Name")?.to_owned();
    let tex = guid_of(field(rd, "texture").ok_or("no texture")?)
        .ok_or("bad texture ref")?
        .to_owned();
    let ppu: f32 = field(text, "m_PixelsToUnits")
        .and_then(|v| v.parse().ok())
        .ok_or("no ppu")?;
    // uvTransform = (ppu, pivot x in atlas px, ppu, pivot y in atlas px).
    let uvt = inline_map(
        field(rd, "uvTransform").ok_or("no uvTransform")?,
        &["x", "y", "z", "w"],
    )
    .ok_or("bad uvTransform")?;
    let (pivot_x, pivot_y) = (uvt[1], uvt[3]);
    let count: usize = field(rd, "m_VertexCount")
        .and_then(|v| v.parse().ok())
        .ok_or("no vertex count")?;
    let data =
        hex_bytes(field(rd, "_typelessdata").ok_or("no vertex data")?).ok_or("bad vertex hex")?;
    let idx =
        hex_bytes(field(rd, "m_IndexBuffer").ok_or("no index buffer")?).ok_or("bad index hex")?;
    if count == 0 || data.len() % count != 0 {
        return Err("vertex data size mismatch".into());
    }
    let stride = data.len() / count;
    let f = |o: usize| f32::from_le_bytes(data[o..o + 4].try_into().unwrap());
    let verts = (0..count)
        .map(|i| {
            (
                pivot_x + f(i * stride) * uvt[0],
                pivot_y + f(i * stride + 4) * uvt[2],
            )
        })
        .collect();
    let ids: Vec<usize> = idx
        .chunks(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]) as usize)
        .collect();
    if !ids.len().is_multiple_of(3) || ids.iter().any(|&i| i >= count) {
        return Err("bad index buffer".into());
    }
    let tris = ids.chunks(3).map(|t| [t[0], t[1], t[2]]).collect();
    Ok((name, tex, ppu, Mesh { verts, tris }))
}

fn inside(p: (f32, f32), a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> bool {
    let cross = |o: (f32, f32), u: (f32, f32), v: (f32, f32)| {
        (u.0 - o.0) * (v.1 - o.1) - (u.1 - o.1) * (v.0 - o.0)
    };
    let (d1, d2, d3) = (cross(a, b, p), cross(b, c, p), cross(c, a, p));
    let eps = 1e-3;
    let neg = d1 < -eps || d2 < -eps || d3 < -eps;
    let pos = d1 > eps || d2 > eps || d3 > eps;
    !(neg && pos)
}

fn cut(mesh: &Mesh, atlas: &Image) -> (Image, (f32, f32), (f32, f32)) {
    let min_x = mesh
        .verts
        .iter()
        .map(|v| v.0)
        .fold(f32::MAX, f32::min)
        .floor();
    let max_x = mesh
        .verts
        .iter()
        .map(|v| v.0)
        .fold(f32::MIN, f32::max)
        .ceil();
    let min_y = mesh
        .verts
        .iter()
        .map(|v| v.1)
        .fold(f32::MAX, f32::min)
        .floor();
    let max_y = mesh
        .verts
        .iter()
        .map(|v| v.1)
        .fold(f32::MIN, f32::max)
        .ceil();
    let (w, h) = ((max_x - min_x) as u32, (max_y - min_y) as u32);
    let mut px = vec![0u8; (w * h * 4) as usize];
    for row in 0..h {
        // Output row 0 is the top of the sprite.
        let ay = max_y - row as f32 - 0.5;
        for col in 0..w {
            let ax = min_x + col as f32 + 0.5;
            let hit = mesh.tris.iter().any(|t| {
                inside(
                    (ax, ay),
                    mesh.verts[t[0]],
                    mesh.verts[t[1]],
                    mesh.verts[t[2]],
                )
            });
            if !hit {
                continue;
            }
            let (sx, sy) = (ax.floor() as i64, atlas.h as i64 - 1 - ay.floor() as i64);
            if sx < 0 || sy < 0 || sx >= atlas.w as i64 || sy >= atlas.h as i64 {
                continue;
            }
            let s = ((sy as u32 * atlas.w + sx as u32) * 4) as usize;
            let d = ((row * w + col) * 4) as usize;
            px[d..d + 4].copy_from_slice(&atlas.px[s..s + 4]);
        }
    }
    (Image { w, h, px }, (min_x, max_y), (max_x, min_y))
}

pub fn extract_all(dir: &Path, guids: &HashMap<String, PathBuf>) -> Result<Sheet, String> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "asset"))
        .collect();
    files.sort();

    let mut atlases: HashMap<String, Image> = HashMap::new();
    let mut sheet = Sheet {
        sprites: Vec::new(),
        names_by_guid: HashMap::new(),
        names_by_path: HashMap::new(),
        skipped: 0,
    };
    let mut used: HashMap<String, u32> = HashMap::new();

    for path in files {
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let (name, tex, ppu, mesh) = match parse_mesh(&text) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("skip {}: {e}", path.display());
                sheet.skipped += 1;
                continue;
            }
        };
        if !atlases.contains_key(&tex) {
            let Some(tex_path) = guids.get(&tex) else {
                eprintln!("skip {name}: texture {tex} not in export");
                sheet.skipped += 1;
                continue;
            };
            atlases.insert(tex.clone(), Image::load_png(tex_path)?);
        }
        let (img, (left, top), _) = cut(&mesh, &atlases[&tex]);
        if img.w == 0 || img.h == 0 {
            sheet.skipped += 1;
            continue;
        }

        let meta = path.with_extension("asset.meta");
        let guid = std::fs::read_to_string(&meta).ok().and_then(|m| {
            m.lines()
                .find_map(|l| l.strip_prefix("guid: ").map(|g| g.trim().to_owned()))
        });
        let n = used.entry(name.clone()).or_insert(0);
        *n += 1;
        let final_name = if *n == 1 { name } else { format!("{name}#{n}") };
        if let Some(g) = guid {
            sheet.names_by_guid.insert(g, final_name.clone());
        }
        if let Some(file) = path.file_name() {
            sheet.names_by_path.insert(
                format!("Sprite/{}", file.to_string_lossy()),
                final_name.clone(),
            );
        }

        // Pivot relative to the cut image's top-left, from uvTransform.
        let uvt_pivot = mesh_pivot(&text);
        sheet.sprites.push(Sprite {
            name: final_name,
            pivot: (uvt_pivot.0 - left, top - uvt_pivot.1),
            img,
            ppu,
        });
    }
    Ok(sheet)
}

fn mesh_pivot(text: &str) -> (f32, f32) {
    let rd = &text[..text.find("m_AtlasRD:").unwrap_or(text.len())];
    let v = field(rd, "uvTransform")
        .and_then(|s| inline_map(s, &["y", "w"]))
        .unwrap_or_else(|| vec![0.0, 0.0]);
    (v[0], v[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_in_triangle() {
        let (a, b, c) = ((0.0, 0.0), (4.0, 0.0), (0.0, 4.0));
        assert!(inside((1.0, 1.0), a, b, c));
        assert!(!inside((3.0, 3.0), a, b, c));
        // Winding doesn't matter.
        assert!(inside((1.0, 1.0), a, c, b));
    }
}
