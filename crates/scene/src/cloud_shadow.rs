//! The PC cloud camera's perspective depth map and two-tap highlight exposure.

use std::ops::Range;

const MAP_W: usize = 512;
const MAP_H: usize = 256;
const NEAR: f32 = 0.3;
const FAR: f32 = 20.0;
const TAN_HALF_FOV: f32 = 0.267_949_2;
const CAMERA_Z: f32 = 18.31;
const BIAS: f32 = 0.05;
const PPU: f32 = crate::top::PPU;

pub const ATLAS_WIDTH: u16 = 512;
pub const ATLAS_HEIGHT: u16 = 1024;
const SLOT_W: u16 = 64;
const SLOT_H: u16 = 32;

/// Receivers use a tile the size of the largest authored cumulus sprite.
pub(crate) fn atlas_slot(index: usize, sprite: &crate::sheet::Sprite) -> Option<[u16; 2]> {
    if sprite.w > SLOT_W || sprite.h > SLOT_H {
        return None;
    }
    let columns = usize::from(ATLAS_WIDTH / SLOT_W);
    let y = index / columns * usize::from(SLOT_H);
    (y < usize::from(ATLAS_HEIGHT)).then_some([(index % columns) as u16 * SLOT_W, y as u16])
}

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    position: [f32; 3],
    // The camera faces −Z. Its right vector is (−1,0,0).
    forward: [f32; 2],
}

impl Camera {
    pub fn new(cumulus_origin: [f32; 3], highest_body_y: f32) -> Self {
        let y = highest_body_y * 0.5;
        let dy = cumulus_origin[1] - y;
        let length = (dy * dy + CAMERA_Z * CAMERA_Z).sqrt();
        Self {
            position: [cumulus_origin[0], y, cumulus_origin[2] + CAMERA_Z],
            forward: [dy / length, -CAMERA_Z / length],
        }
    }

    fn project(self, x: f32, y: f32, z: f32) -> [f32; 3] {
        let dy = y - self.position[1];
        let dz = z - self.position[2];
        let [fy, fz] = self.forward;
        let depth = fy * dy + fz * dz;
        let up = -fz * dy + fy * dz;
        [
            (0.5 - (x - self.position[0]) / (4.0 * TAN_HALF_FOV * depth)) * MAP_W as f32 - 0.5,
            (0.5 + up / (2.0 * TAN_HALF_FOV * depth)) * MAP_H as f32 - 0.5,
            (FAR + NEAR) / (FAR - NEAR) * depth - 2.0 * FAR * NEAR / (FAR - NEAR),
        ]
    }

    /// Intersect a shadow-map row's center ray with the cloud's fixed Z plane.
    fn row(self, row: usize, z: f32) -> (f32, f32) {
        let q = (2.0 * (row as f32 + 0.5) / MAP_H as f32 - 1.0) * TAN_HALF_FOV;
        let [fy, fz] = self.forward;
        let depth = (z - self.position[2]) / (fz + q * fy);
        (self.position[1] + depth * (fy - q * fz), depth)
    }
}

/// Sprite instance in the camera-relative cloud scene.
#[derive(Clone, Copy, Debug)]
pub struct Sprite {
    pub id: u32,
    pub position: [f32; 3],
    pub scale: [f32; 2],
    pub flip: bool,
    pub opacity: f32,
    /// Only on-screen cumulus with a highlight need a mask in the output atlas.
    pub slot: Option<[u16; 2]>,
}

struct Art {
    id: u32,
    width: u16,
    height: u16,
    pivot: [f32; 2],
    /// Ranges of source texels that pass the shadow-caster alpha cutoff.
    rows: Vec<Vec<Range<u16>>>,
    /// Only partial-alpha texels can receive an additive highlight.
    rim: Vec<Vec<u16>>,
}

impl Art {
    fn bounds(&self, sprite: Sprite) -> [f32; 4] {
        let px = if sprite.flip {
            f32::from(self.width) - self.pivot[0]
        } else {
            self.pivot[0]
        };
        let [x, y, _] = sprite.position;
        let sx = sprite.scale[0] / PPU;
        let sy = sprite.scale[1] / PPU;
        let left = x - px * sx;
        let top = y + self.pivot[1] * sy;
        [
            left,
            top,
            left + f32::from(self.width) * sx,
            top - f32::from(self.height) * sy,
        ]
    }
}

pub struct Atlas<'a> {
    pub pixels: &'a [u8],
    pub height: u16,
}

/// Retained sprite masks and scratch buffers, shared by both render backends.
#[derive(Default)]
pub struct Pass {
    art: Vec<Art>,
    depth: Vec<f32>,
    atlas: Vec<u8>,
}

impl Pass {
    pub fn insert(
        &mut self,
        id: u32,
        sprite: &crate::sheet::Sprite,
        alpha: impl Iterator<Item = u8>,
    ) {
        if sprite.w == 0 || sprite.h == 0 || self.art.iter().any(|a| a.id == id) {
            return;
        }
        let bytes: Vec<_> = alpha.collect();
        if bytes.len() != usize::from(sprite.w) * usize::from(sprite.h) {
            return;
        }
        let mut art = Art {
            id,
            width: sprite.w,
            height: sprite.h,
            pivot: [sprite.px, sprite.py],
            rows: Vec::new(),
            rim: Vec::new(),
        };
        for line in bytes.chunks_exact(usize::from(sprite.w)) {
            let mut ranges: Vec<Range<u16>> = Vec::new();
            let mut rim = Vec::new();
            for (x, &a) in line.iter().enumerate() {
                let x = x as u16;
                // Source alpha >= .1 is byte >= 26, or baked coverage >= 141.
                if a >= 141 {
                    match ranges.last_mut() {
                        Some(run) if run.end == x => run.end += 1,
                        _ => ranges.push(x..x + 1),
                    }
                }
                if a > 0 && a < 255 {
                    rim.push(x);
                }
            }
            art.rows.push(ranges);
            art.rim.push(rim);
        }
        self.art.push(art);
    }

    pub fn prepare(&mut self, camera: Camera, clouds: &[Sprite]) -> Option<Atlas<'_>> {
        let height = clouds
            .iter()
            .filter_map(|s| s.slot)
            .map(|slot| slot[1] + SLOT_H)
            .max()?;
        self.depth.resize(MAP_W * MAP_H, 1.0);
        self.depth.fill(1.0);
        self.atlas
            .resize(usize::from(ATLAS_WIDTH) * usize::from(height), 255);
        self.atlas.fill(255);
        for sprite in clouds.iter().filter(|s| s.opacity >= 0.5) {
            if let Some(art) = self.art.iter().find(|art| art.id == sprite.id) {
                cast(&mut self.depth, camera, art, *sprite);
            }
        }
        for sprite in clouds.iter().filter(|s| s.slot.is_some()) {
            if let Some(art) = self.art.iter().find(|art| art.id == sprite.id) {
                receive(&self.depth, &mut self.atlas, camera, art, *sprite);
            }
        }
        Some(Atlas {
            pixels: &self.atlas,
            height,
        })
    }
}

fn encoded_depth(depth: f32) -> f32 {
    FAR / (FAR - NEAR) * (1.0 - NEAR / depth)
}

fn cast(map: &mut [f32], camera: Camera, art: &Art, sprite: Sprite) {
    let [left, top, right, bottom] = art.bounds(sprite);
    let z = sprite.position[2];
    let row_start = camera.project(left, bottom, z)[1]
        .ceil()
        .clamp(0.0, MAP_H as f32) as usize;
    let row_end = (camera.project(left, top, z)[1].floor() + 1.0).clamp(0.0, MAP_H as f32) as usize;
    for row in row_start..row_end {
        let (y, depth) = camera.row(row, z);
        if !(NEAR..=FAR).contains(&depth) || y > top || y <= bottom {
            continue;
        }
        let src_row = ((top - y) * PPU / sprite.scale[1]) as usize;
        let Some(runs) = art.rows.get(src_row) else {
            continue;
        };
        // Within a row, Z and projected scale are constant. Reverse the
        // horizontal span because the shadow camera looks back toward −Z.
        let px = MAP_W as f32 / (4.0 * TAN_HALF_FOV * depth);
        let origin = (MAP_W as f32 - 1.0) * 0.5 + (camera.position[0] - right) * px;
        let step = sprite.scale[0] / PPU * px;
        let sample = encoded_depth(depth);
        for run in runs {
            let span = if sprite.flip {
                run.clone()
            } else {
                art.width - run.end..art.width - run.start
            };
            let a = (origin + f32::from(span.start) * step)
                .ceil()
                .clamp(0.0, MAP_W as f32) as usize;
            let b = (origin + f32::from(span.end) * step)
                .ceil()
                .clamp(0.0, MAP_W as f32) as usize;
            for d in &mut map[row * MAP_W + a..row * MAP_W + b] {
                *d = d.min(sample);
            }
        }
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn depth_sample(map: &[f32], x: f32, y: f32) -> f32 {
    let x = x.clamp(0.0, (MAP_W - 1) as f32);
    let y = y.clamp(0.0, (MAP_H - 1) as f32);
    let ix = x as usize;
    let iy = y as usize;
    let nx = (ix + 1).min(MAP_W - 1);
    let ny = (iy + 1).min(MAP_H - 1);
    let tx = x - ix as f32;
    let ty = y - iy as f32;
    let a = lerp(map[iy * MAP_W + ix], map[iy * MAP_W + nx], tx);
    let b = lerp(map[ny * MAP_W + ix], map[ny * MAP_W + nx], tx);
    lerp(a, b, ty)
}

fn exposure(map: &[f32], u: f32, v: f32, threshold: f32) -> u8 {
    // Filtering happens in encoded depth. Comparing there is equivalent to
    // linearizing each filtered tap, because the depth transform is monotonic.
    match u8::from(depth_sample(map, u, v) > threshold)
        + u8::from(depth_sample(map, u, v + 2.0) > threshold)
    {
        0 => 0,
        1 => 128,
        _ => 255,
    }
}

fn receive(map: &[f32], atlas: &mut [u8], camera: Camera, art: &Art, sprite: Sprite) {
    let Some([tile_x, tile_y]) = sprite.slot else {
        return;
    };
    let [left, top, right, bottom] = art.bounds(sprite);
    let z = sprite.position[2];
    let tl = camera.project(left, top, z);
    let tr = camera.project(right, top, z);
    let bl = camera.project(left, bottom, z);
    let br = camera.project(right, bottom, z);
    let inv_w = f32::from(art.width).recip();
    let inv_h = f32::from(art.height).recip();
    for (y, rim) in art.rim.iter().enumerate() {
        let t = (y as f32 + 0.5) * inv_h;
        let v = lerp(tl[1], bl[1], t);
        let depth = lerp(tl[2], bl[2], t) - BIAS;
        if depth < NEAR {
            continue;
        }
        let threshold = encoded_depth(depth);
        let row = (usize::from(tile_y) + y) * usize::from(ATLAS_WIDTH) + usize::from(tile_x);
        let upper = tl[0] + (br[0] - tr[0]) * t;
        let lower = lerp(tl[0], bl[0], t);
        for &x in rim {
            let local_x = if sprite.flip { art.width - 1 - x } else { x };
            let s = (f32::from(local_x) + 0.5) * inv_w;
            // The PC divides the shadow projection by W in the vertex shader.
            // The main camera then interpolates those values across each triangle.
            let u = if s >= t {
                upper + (tr[0] - tl[0]) * s
            } else {
                lower + (br[0] - bl[0]) * s
            };
            atlas[row + usize::from(x)] = exposure(map, u, v, threshold);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGIN: [f32; 3] = [0.0, 5.5, 42.0];

    fn sprite(id: u32, z: f32, slot: Option<[u16; 2]>) -> Sprite {
        Sprite {
            id,
            position: [0.0, 5.5, z],
            scale: [1.0, 1.0],
            flip: false,
            opacity: 1.0,
            slot,
        }
    }

    fn art(w: u16, h: u16) -> crate::sheet::Sprite {
        crate::sheet::Sprite {
            page: 0,
            x: 0,
            y: 0,
            w,
            h,
            px: f32::from(w) * 0.5,
            py: f32::from(h) * 0.5,
            masks: [None, None],
        }
    }

    fn at(atlas: &Atlas<'_>, x: usize, y: usize) -> u8 {
        atlas.pixels[y * usize::from(ATLAS_WIDTH) + x]
    }

    #[test]
    fn perspective_projection_tracks_the_higher_body_and_reverses_x() {
        let camera = Camera::new(ORIGIN, 11.0);
        assert_eq!(camera.position[..2], [0.0, 5.5]);
        assert!((camera.position[2] - 60.31).abs() < 1e-5);
        let [x, y, z] = camera.project(1.0, 5.5, 50.31);
        assert!((x - 207.73).abs() < 0.02);
        assert_eq!(y, 127.5);
        assert!((z - 9.695_432).abs() < 1e-5);
        let tilted = Camera::new(ORIGIN, 4.0);
        assert_eq!(tilted.position[1], 2.0);
        let [_, y, _] = tilted.project(0.0, ORIGIN[1], ORIGIN[2]);
        assert!((y - 127.5).abs() < 0.0001);
        let (row_y, depth) = tilted.row(95, 49.0);
        let [_, projected_y, clip_z] = tilted.project(0.0, row_y, 49.0);
        assert!((projected_y - 95.0).abs() < 0.0001);
        assert!(
            clip_z < depth,
            "the receiver compares clip Z rather than view depth"
        );
    }

    #[test]
    fn shader_taps_filter_encoded_depth_before_comparison() {
        let mut map = vec![1.0; MAP_W * MAP_H];
        let occluder = encoded_depth(5.0);
        let threshold = encoded_depth(11.0);
        map[0] = occluder;
        map[MAP_W] = occluder;
        assert_eq!(exposure(&map, 0.0, 0.0, threshold), 128);
        // Bilinear interpolation in linear distance would give 12.5 and light
        // the first tap too. The PC filters the encoded 5/20 depth first.
        assert_eq!(exposure(&map, 0.5, 0.5, threshold), 128);
        map[2 * MAP_W] = occluder;
        assert_eq!(exposure(&map, -10.0, 0.0, threshold), 0);
        assert_eq!(exposure(&map, 1.0, 0.0, threshold), 255);
        assert_eq!(exposure(&map, 1.0, 0.0, 1.0), 0);
    }

    #[test]
    fn casters_gate_at_half_opacity_and_ten_percent_source_alpha() {
        let mut pass = Pass::default();
        let source = art(64, 32);
        // Left half hits the baked form of the PC cutoff, right half misses by one byte.
        pass.insert(
            1,
            &source,
            (0..64 * 32).map(|i| if i % 64 < 32 { 141 } else { 140 }),
        );
        pass.insert(2, &source, std::iter::repeat_n(191, 64 * 32));
        let camera = Camera::new(ORIGIN, 11.0);
        let mut caster = sprite(1, 52.31, None);
        let receiver = sprite(2, 44.31, Some([0, 0]));
        caster.opacity = 0.499;
        let atlas = pass.prepare(camera, &[caster, receiver]).unwrap();
        assert_eq!((at(&atlas, 12, 16), at(&atlas, 52, 16)), (255, 255));

        caster.opacity = 0.5;
        let atlas = pass.prepare(camera, &[caster, receiver]).unwrap();
        assert_eq!((at(&atlas, 12, 16), at(&atlas, 52, 16)), (0, 255));
        caster.flip = true;
        let atlas = pass.prepare(camera, &[caster, receiver]).unwrap();
        assert_eq!((at(&atlas, 12, 16), at(&atlas, 52, 16)), (255, 0));
        let mut mirrored = receiver;
        mirrored.flip = true;
        let atlas = pass.prepare(camera, &[caster, mirrored]).unwrap();
        assert_eq!((at(&atlas, 12, 16), at(&atlas, 52, 16)), (0, 255));
        let atlas = pass.prepare(camera, &[receiver]).unwrap();
        assert_eq!(at(&atlas, 12, 16), 255, "no stale mask or self-shadowing");
    }

    #[test]
    fn nearest_caster_wins_and_only_rim_texels_use_a_mask() {
        let mut pass = Pass::default();
        let source = art(64, 32);
        pass.insert(1, &source, std::iter::repeat_n(255, 64 * 32));
        pass.insert(
            2,
            &source,
            (0..64 * 32).map(|i| if i % 2 == 0 { 191 } else { 255 }),
        );
        let camera = Camera::new(ORIGIN, 11.0);
        let near = sprite(1, 52.31, None);
        let far = sprite(1, 42.31, None);
        let recv = sprite(2, 44.31, Some([0, 0]));
        let a = pass.prepare(camera, &[near, far, recv]).unwrap();
        assert_eq!((at(&a, 30, 16), at(&a, 31, 16)), (0, 255));
        let b = pass.prepare(camera, &[far, near, recv]).unwrap();
        assert_eq!(at(&b, 30, 16), 0);
        let c = pass.prepare(camera, &[far, recv]).unwrap();
        assert_eq!(at(&c, 30, 16), 255);
    }
}
