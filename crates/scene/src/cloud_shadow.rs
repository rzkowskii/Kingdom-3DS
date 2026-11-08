//! The PC cloud camera's perspective depth map and two-tap highlight exposure.

use std::ops::Range;

use crate::{cloud_mesh::CloudMesh, render::Draw};

pub const MAP_W: usize = 512;
pub const MAP_H: usize = 256;
const NEAR: f32 = 0.3;
const FAR: f32 = 20.0;
const TAN_HALF_FOV: f32 = 0.267_949_2;
const CAMERA_Z: f32 = 18.31;
const BIAS: f32 = 0.05;
const PPU: f32 = crate::top::PPU;

pub const ATLAS_WIDTH: u16 = 1024;
pub const ATLAS_HEIGHT: u16 = 1024;

/// Mesh data, or a rectangle for a legacy pack without the optional mesh asset.
pub struct Geometry<'a> {
    mesh: Option<&'a CloudMesh>,
    quad: [[f32; 2]; 4],
}

impl<'a> Geometry<'a> {
    pub fn new(sprite: &crate::sheet::Sprite, mesh: Option<&'a CloudMesh>) -> Self {
        let (x, y) = (-sprite.px, -sprite.py);
        let (r, b) = (x + f32::from(sprite.w), y + f32::from(sprite.h));
        Self {
            mesh,
            quad: [[x, y], [r, y], [r, b], [x, b]],
        }
    }

    pub fn vertices(&self) -> &[[f32; 2]] {
        self.mesh.map_or(&self.quad, |mesh| &mesh.vertices)
    }

    pub fn indices(&self) -> &[u16] {
        self.mesh.map_or(&[0, 1, 2, 0, 2, 3], |mesh| &mesh.indices)
    }
}

/// The flip mirrors local geometry about the authored pivot. The vertex's
/// UV stays attached to it and is still calculated from the unflipped point.
pub fn screen_point([x, y]: [f32; 2], at: (f32, f32), draw: &Draw) -> [f32; 2] {
    let x = if draw.flip { -x } else { x };
    let y = if draw.flip_y { -y } else { y };
    let [a, b, c, d] = draw.affine;
    [
        at.0 + (a * x + b * y) * draw.scale,
        at.1 + (c * x + d * y) * draw.scale,
    ]
}

pub fn world_point([x, y]: [f32; 2], position: [f32; 3], scale: [f32; 2], flip: bool) -> [f32; 3] {
    let x = if flip { -x } else { x };
    [
        position[0] + x * scale[0] / PPU,
        position[1] - y * scale[1] / PPU,
        position[2],
    ]
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

    pub fn project(self, x: f32, y: f32, z: f32) -> [f32; 3] {
        let dy = y - self.position[1];
        let dz = z - self.position[2];
        let [fy, fz] = self.forward;
        let depth = fy * dy + fz * dz;
        let up = -fz * dy + fy * dz;
        [
            (0.5 - (x - self.position[0]) / (4.0 * TAN_HALF_FOV * depth)) * MAP_W as f32 - 0.5,
            (0.5 + up / (2.0 * TAN_HALF_FOV * depth)) * MAP_H as f32 - 0.5,
            clip_z(depth),
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

fn clip_z(depth: f32) -> f32 {
    (FAR + NEAR) / (FAR - NEAR) * depth - 2.0 * FAR * NEAR / (FAR - NEAR)
}

/// Sprite instance in the camera-relative cloud scene.
#[derive(Clone, Copy, Debug)]
pub struct Sprite {
    pub(crate) id: u32,
    pub(crate) position: [f32; 3],
    pub(crate) scale: [f32; 2],
    pub(crate) flip: bool,
    pub(crate) opacity: f32,
    pub(crate) screen: [f32; 2],
    /// The same disparity weight as the cloud's draw command.
    pub(crate) stereo_depth: f32,
    pub(crate) receiver: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Receiver {
    pub index: usize,
    pub position: [f32; 3],
}

#[derive(Clone, Copy, Debug)]
pub struct Mask {
    pub tile: [u16; 2],
    pub origin: [i16; 2],
}

struct Art {
    id: u32,
    sprite: crate::sheet::Sprite,
    mesh: Option<CloudMesh>,
    /// Source rows that pass the caster cutoff and the rows with a visible rim.
    cast_rows: Vec<Vec<Range<u16>>>,
    rim_rows: Vec<Vec<Range<u16>>>,
}

impl Art {
    fn geometry(&self) -> Geometry<'_> {
        Geometry::new(&self.sprite, self.mesh.as_ref())
    }

    fn left_pivot(&self, flip: bool) -> f32 {
        if flip {
            f32::from(self.sprite.w) - self.sprite.px
        } else {
            self.sprite.px
        }
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
    masks: Vec<Option<Mask>>,
    projected: Vec<[f32; 3]>,
}

impl Pass {
    pub fn insert(
        &mut self,
        id: u32,
        sprite: &crate::sheet::Sprite,
        mesh: Option<&CloudMesh>,
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
            sprite: *sprite,
            mesh: mesh.cloned(),
            cast_rows: Vec::new(),
            rim_rows: Vec::new(),
        };
        for line in bytes.chunks_exact(usize::from(sprite.w)) {
            // Source alpha >= .1 is byte >= 26, or baked coverage >= 141.
            art.cast_rows.push(runs(line, |a| a >= 141));
            art.rim_rows.push(runs(line, |a| a > 0 && a < 255));
        }
        self.art.push(art);
    }

    /// The shadow camera renders tight caster meshes into forward encoded Z.
    pub fn prepare_depth(&mut self, camera: Camera, clouds: &[Sprite]) -> Option<&[f32]> {
        if !clouds.iter().any(|sprite| sprite.receiver.is_some()) {
            return None;
        }
        self.depth.resize(MAP_W * MAP_H, 1.0);
        self.depth.fill(1.0);
        for sprite in clouds.iter().filter(|s| s.opacity >= 0.5) {
            if let Some(art) = self.art.iter().find(|art| art.id == sprite.id) {
                cast(&mut self.depth, &mut self.projected, camera, art, *sprite);
            }
        }
        Some(&self.depth)
    }

    /// Build the shared caster depth and receiver layout, then rasterize the
    /// left eye. A zero half-separation draws the ordinary 2D view.
    pub fn prepare_left(
        &mut self,
        camera: Camera,
        clouds: &[Sprite],
        half_separation: f32,
    ) -> Option<Atlas<'_>> {
        self.masks.clear();
        self.prepare_depth(camera, clouds)?;
        let mut shelf = Shelf::default();
        for sprite in clouds {
            let Some(receiver) = sprite.receiver else {
                continue;
            };
            let Some(art) = self.art.iter().find(|art| art.id == sprite.id) else {
                continue;
            };
            let disparity = (half_separation * sprite.stereo_depth).abs();
            let (x, y) = screen_bounds(art, *sprite, disparity);
            self.masks.resize(self.masks.len().max(receiver + 1), None);
            self.masks[receiver] = shelf.place(x, y);
        }
        let height = (shelf.y + shelf.row_height).next_multiple_of(8);
        if height == 0 {
            return None;
        }
        self.atlas
            .resize(usize::from(ATLAS_WIDTH) * usize::from(height), 255);
        Some(self.rasterize_eye(camera, clouds, -half_separation))
    }

    /// Reuse the depth, layout and CPU pixels from this frame's left-eye
    /// preparation. The backend uploads the left atlas before calling this.
    pub fn prepare_right(
        &mut self,
        camera: Camera,
        clouds: &[Sprite],
        half_separation: f32,
    ) -> Atlas<'_> {
        self.rasterize_eye(camera, clouds, half_separation)
    }

    fn rasterize_eye(&mut self, camera: Camera, clouds: &[Sprite], eye: f32) -> Atlas<'_> {
        self.atlas.fill(255);
        for sprite in clouds {
            let Some(mask) = sprite.receiver.and_then(|i| self.mask(i)) else {
                continue;
            };
            if let Some(art) = self.art.iter().find(|art| art.id == sprite.id) {
                // Shift only the receiver's screen-space geometry. Its
                // world-space shadow coordinates and the caster map are shared.
                let mut shifted = *sprite;
                shifted.screen[0] += eye * sprite.stereo_depth;
                receive(
                    &self.depth,
                    &mut self.atlas,
                    &mut self.projected,
                    camera,
                    art,
                    shifted,
                    mask,
                );
            }
        }
        Atlas {
            pixels: &self.atlas,
            height: (self.atlas.len() / usize::from(ATLAS_WIDTH)) as u16,
        }
    }

    pub fn mask(&self, receiver: usize) -> Option<Mask> {
        self.masks.get(receiver).copied().flatten()
    }
}

fn runs(line: &[u8], keep: impl Fn(u8) -> bool) -> Vec<Range<u16>> {
    let mut ranges: Vec<Range<u16>> = Vec::new();
    for (x, &alpha) in line.iter().enumerate() {
        if keep(alpha) {
            let x = x as u16;
            match ranges.last_mut() {
                Some(run) if run.end == x => run.end += 1,
                _ => ranges.push(x..x + 1),
            }
        }
    }
    ranges
}

#[derive(Default)]
struct Shelf {
    x: u16,
    y: u16,
    row_height: u16,
}

impl Shelf {
    fn place(&mut self, x: Range<i32>, y: Range<i32>) -> Option<Mask> {
        if x.is_empty() || y.is_empty() {
            return None;
        }
        let w = (x.end - x.start) as u16;
        let h = (y.end - y.start) as u16;
        if self.x + w > ATLAS_WIDTH {
            self.x = 0;
            self.y += self.row_height;
            self.row_height = 0;
        }
        if self.y + h > ATLAS_HEIGHT {
            return None;
        }
        let tile = [self.x, self.y];
        self.x += w;
        self.row_height = self.row_height.max(h);
        Some(Mask {
            tile,
            origin: [x.start as i16, y.start as i16],
        })
    }
}

fn screen_bounds(art: &Art, sprite: Sprite, disparity: f32) -> (Range<i32>, Range<i32>) {
    let pivot = art.left_pivot(sprite.flip) * sprite.scale[0];
    let left = (sprite.screen[0] - disparity) - pivot;
    let top = sprite.screen[1] - art.sprite.py * sprite.scale[1];
    let right = (sprite.screen[0] + disparity) - pivot + f32::from(art.sprite.w) * sprite.scale[0];
    let bottom = top + f32::from(art.sprite.h) * sprite.scale[1];
    let x = 0..crate::top::W as i32;
    let y = 0..crate::top::H as i32;
    (pixels(left, right, 0.5, x), pixels(top, bottom, 0.5, y))
}

fn pixels(lo: f32, hi: f32, center: f32, clip: Range<i32>) -> Range<i32> {
    let boundary = |v: f32| ((v - center).ceil() as i32).clamp(clip.start, clip.end);
    boundary(lo)..boundary(hi)
}

fn triangle_rows(triangle: [[f32; 2]; 3], center: f32, clip: Range<i32>) -> Range<i32> {
    let [a, b, c] = triangle.map(|v| v[1]);
    pixels(a.min(b).min(c), a.max(b).max(c), center, clip)
}

fn row_span(triangle: [[f32; 2]; 3], y: f32) -> [f32; 2] {
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for i in 0..3 {
        let [ax, ay] = triangle[i];
        let [bx, by] = triangle[(i + 1) % 3];
        if (ay <= y && y < by) || (by <= y && y < ay) {
            let x = lerp(ax, bx, (y - ay) / (by - ay));
            lo = lo.min(x);
            hi = hi.max(x);
        }
    }
    [lo, hi]
}

fn project_sprite(
    vertices: &mut Vec<[f32; 3]>,
    camera: Camera,
    geometry: &Geometry<'_>,
    s: Sprite,
) {
    vertices.clear();
    vertices.extend(geometry.vertices().iter().map(|&point| {
        let [x, y, z] = world_point(point, s.position, s.scale, s.flip);
        camera.project(x, y, z)
    }));
}

fn encoded_depth(depth: f32) -> f32 {
    FAR / (FAR - NEAR) * (1.0 - NEAR / depth)
}

fn cast(map: &mut [f32], projected: &mut Vec<[f32; 3]>, camera: Camera, art: &Art, sprite: Sprite) {
    let geometry = art.geometry();
    project_sprite(projected, camera, &geometry, sprite);
    let [x, y, z] = sprite.position;
    let left = x - art.left_pivot(sprite.flip) * sprite.scale[0] / PPU;
    let top = y + art.sprite.py * sprite.scale[1] / PPU;
    let right = left + f32::from(art.sprite.w) * sprite.scale[0] / PPU;
    let x_clip = 0..MAP_W as i32;
    let width = art.sprite.w;
    for &ids in geometry.indices().as_chunks::<3>().0 {
        let triangle = ids.map(|i| {
            let [u, v, _] = projected[usize::from(i)];
            [u, v]
        });
        for row in triangle_rows(triangle, 0.0, 0..MAP_H as i32) {
            // The source texture sample is perspective-correct. Unproject
            // the shadow pixel's row to the sprite plane before its alpha test.
            let (world_y, depth) = camera.row(row as usize, z);
            if !(NEAR..=FAR).contains(&depth) || world_y > top {
                continue;
            }
            let src_row = ((top - world_y) * PPU / sprite.scale[1]) as usize;
            let Some(runs) = art.cast_rows.get(src_row) else {
                continue;
            };
            let [lo, hi] = row_span(triangle, row as f32);
            let px = MAP_W as f32 / (4.0 * TAN_HALF_FOV * depth);
            let origin = (MAP_W as f32 - 1.0) * 0.5 + (camera.position[0] - right) * px;
            let step = sprite.scale[0] / PPU * px;
            let sample = encoded_depth(depth);
            for run in runs {
                // The shadow camera looks back toward −Z, reversing screen X.
                let span = if sprite.flip {
                    run.clone()
                } else {
                    width - run.end..width - run.start
                };
                let a = lo.max(origin + f32::from(span.start) * step);
                let b = hi.min(origin + f32::from(span.end) * step);
                let row_start = row as usize * MAP_W;
                let span = pixels(a, b, 0.0, x_clip.clone());
                if !span.is_empty() {
                    for d in
                        &mut map[row_start + span.start as usize..row_start + span.end as usize]
                    {
                        *d = d.min(sample);
                    }
                }
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

/// Receiver coordinates have already been divided by shadow-camera W at
/// each vertex. The orthographic main camera interpolates them affinely.
fn uv_gradients(triangle: [[f32; 2]; 3], shadow: [[f32; 3]; 3]) -> Option<[[f32; 2]; 2]> {
    let [a, b, c] = triangle;
    let [bx, by] = [b[0] - a[0], b[1] - a[1]];
    let [cx, cy] = [c[0] - a[0], c[1] - a[1]];
    let det = bx * cy - by * cx;
    if det.abs() <= f32::EPSILON {
        return None;
    }
    Some([0, 1].map(|i| {
        let s = shadow[1][i] - shadow[0][i];
        let t = shadow[2][i] - shadow[0][i];
        [(s * cy - t * by) / det, (t * bx - s * cx) / det]
    }))
}

fn receive(
    map: &[f32],
    atlas: &mut [u8],
    projected: &mut Vec<[f32; 3]>,
    camera: Camera,
    art: &Art,
    sprite: Sprite,
    mask: Mask,
) {
    let geometry = art.geometry();
    project_sprite(projected, camera, &geometry, sprite);
    let (x_clip, y_clip) = screen_bounds(art, sprite, 0.0);
    let [tile_x, tile_y] = mask.tile.map(i32::from);
    let [ox, oy] = mask.origin.map(i32::from);
    let left = sprite.screen[0] - art.left_pivot(sprite.flip) * sprite.scale[0];
    let top = sprite.screen[1] - art.sprite.py * sprite.scale[1];
    let width = art.sprite.w;
    let [fy, fz] = camera.forward;
    let z_depth = fz * (sprite.position[2] - camera.position[2]);
    for &ids in geometry.indices().as_chunks::<3>().0 {
        let ids = ids.map(usize::from);
        let triangle = ids.map(|i| {
            let [x, y] = geometry.vertices()[i];
            let x = if sprite.flip { -x } else { x };
            [
                sprite.screen[0] + x * sprite.scale[0],
                sprite.screen[1] + y * sprite.scale[1],
            ]
        });
        let shadow = ids.map(|i| projected[i]);
        let Some([du, dv]) = uv_gradients(triangle, shadow) else {
            continue;
        };
        for y in triangle_rows(triangle, 0.5, y_clip.clone()) {
            let center_y = y as f32 + 0.5;
            let src_y = ((center_y - top) / sprite.scale[1]) as usize;
            let Some(runs) = art.rim_rows.get(src_y) else {
                continue;
            };
            if runs.is_empty() {
                continue;
            }
            // Clip Z is affine and independent of X on the sprite plane.
            let world_y = sprite.position[1] + (sprite.screen[1] - center_y) / PPU;
            let depth = clip_z(fy * (world_y - camera.position[1]) + z_depth) - BIAS;
            if depth < NEAR {
                continue;
            }
            let threshold = encoded_depth(depth);
            let [lo, hi] = row_span(triangle, center_y);
            let dy = center_y - triangle[0][1];
            let u = shadow[0][0] + du[1] * dy;
            let v = shadow[0][1] + dv[1] * dy;
            let atlas_row = (tile_y + y - oy) as usize * usize::from(ATLAS_WIDTH);
            for run in runs {
                let span = if sprite.flip {
                    width - run.end..width - run.start
                } else {
                    run.clone()
                };
                let a = lo.max(left + f32::from(span.start) * sprite.scale[0]);
                let b = hi.min(left + f32::from(span.end) * sprite.scale[0]);
                for x in pixels(a, b, 0.5, x_clip.clone()) {
                    let dx = x as f32 + 0.5 - triangle[0][0];
                    atlas[atlas_row + (tile_x + x - ox) as usize] =
                        exposure(map, u + du[0] * dx, v + dv[0] * dx, threshold);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGIN: [f32; 3] = [0.0, 5.5, 42.0];

    fn sprite(id: u32, z: f32, receiver: Option<usize>) -> Sprite {
        Sprite {
            id,
            position: [0.0, 5.5, z],
            scale: [1.0, 1.0],
            flip: false,
            opacity: 1.0,
            screen: [200.0, 120.0],
            stereo_depth: 1.0,
            receiver,
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

    fn screen_exposure(pass: &Pass, x: i16, y: i16) -> u8 {
        let mask = pass.mask(0).unwrap();
        let u = (x - mask.origin[0]) as usize + usize::from(mask.tile[0]);
        let v = (y - mask.origin[1]) as usize + usize::from(mask.tile[1]);
        pass.atlas[v * usize::from(ATLAS_WIDTH) + u]
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
            None,
            (0..64 * 32).map(|i| if i % 64 < 32 { 141 } else { 140 }),
        );
        pass.insert(2, &source, None, std::iter::repeat_n(191, 64 * 32));
        let camera = Camera::new(ORIGIN, 11.0);
        let mut caster = sprite(1, 52.31, None);
        let receiver = sprite(2, 44.31, Some(0));
        caster.opacity = 0.499;
        let atlas = pass.prepare_left(camera, &[caster, receiver], 0.0).unwrap();
        assert_eq!((at(&atlas, 12, 16), at(&atlas, 52, 16)), (255, 255));

        caster.opacity = 0.5;
        let atlas = pass.prepare_left(camera, &[caster, receiver], 0.0).unwrap();
        assert_eq!((at(&atlas, 12, 16), at(&atlas, 52, 16)), (0, 255));
        caster.flip = true;
        let atlas = pass.prepare_left(camera, &[caster, receiver], 0.0).unwrap();
        assert_eq!((at(&atlas, 12, 16), at(&atlas, 52, 16)), (255, 0));
        let mut mirrored = receiver;
        mirrored.flip = true;
        let atlas = pass.prepare_left(camera, &[caster, mirrored], 0.0).unwrap();
        assert_eq!((at(&atlas, 12, 16), at(&atlas, 52, 16)), (255, 0));
        let atlas = pass.prepare_left(camera, &[receiver], 0.0).unwrap();
        assert_eq!(at(&atlas, 12, 16), 255, "no stale mask or self-shadowing");
    }

    #[test]
    fn nearest_caster_wins_and_only_rim_texels_use_a_mask() {
        let mut pass = Pass::default();
        let source = art(64, 32);
        pass.insert(1, &source, None, std::iter::repeat_n(255, 64 * 32));
        pass.insert(
            2,
            &source,
            None,
            (0..64 * 32).map(|i| if i % 2 == 0 { 191 } else { 255 }),
        );
        let camera = Camera::new(ORIGIN, 11.0);
        let near = sprite(1, 52.31, None);
        let far = sprite(1, 42.31, None);
        let recv = sprite(2, 44.31, Some(0));
        let a = pass.prepare_left(camera, &[near, far, recv], 0.0).unwrap();
        assert_eq!((at(&a, 30, 16), at(&a, 31, 16)), (0, 255));
        let b = pass.prepare_left(camera, &[far, near, recv], 0.0).unwrap();
        assert_eq!(at(&b, 30, 16), 0);
        let c = pass.prepare_left(camera, &[far, recv], 0.0).unwrap();
        assert_eq!(at(&c, 30, 16), 255);
    }

    #[test]
    fn caster_and_receiver_use_authored_triangle_coverage() {
        let mut pass = Pass::default();
        let source = art(64, 32);
        let triangle = CloudMesh {
            vertices: vec![[-32.0, -16.0], [32.0, -16.0], [-32.0, 16.0]],
            indices: vec![0, 1, 2],
        };
        pass.insert(
            1,
            &source,
            Some(&triangle),
            std::iter::repeat_n(255, 64 * 32),
        );
        pass.insert(2, &source, None, std::iter::repeat_n(191, 64 * 32));
        pass.insert(
            3,
            &source,
            Some(&triangle),
            std::iter::repeat_n(191, 64 * 32),
        );
        let camera = Camera::new(ORIGIN, 11.0);
        let caster = sprite(1, 52.31, None);
        let receiver = sprite(2, 44.31, Some(0));
        let a = pass.prepare_left(camera, &[caster, receiver], 0.0).unwrap();
        assert_eq!((at(&a, 12, 16), at(&a, 52, 16)), (0, 255));

        let solid_caster = sprite(2, 52.31, None);
        let tight_receiver = sprite(3, 44.31, Some(0));
        let b = pass
            .prepare_left(camera, &[solid_caster, tight_receiver], 0.0)
            .unwrap();
        assert_eq!((at(&b, 12, 16), at(&b, 52, 16)), (0, 255));
    }

    #[test]
    fn two_fragments_in_one_enlarged_source_texel_get_different_exposure() {
        let mut pass = Pass::default();
        pass.insert(
            1,
            &art(64, 32),
            None,
            (0..64 * 32).map(|i| if i % 64 < 32 { 255 } else { 0 }),
        );
        pass.insert(2, &art(1, 1), None, [191].into_iter());
        let camera = Camera::new(ORIGIN, 11.0);
        let caster = sprite(1, 52.31, None);
        let mut receiver = sprite(2, 44.31, Some(0));
        receiver.scale = [16.0; 2];
        receiver.opacity = 0.49;
        let atlas = pass.prepare_left(camera, &[caster, receiver], 0.0).unwrap();
        assert_eq!((at(&atlas, 4, 8), at(&atlas, 12, 8)), (0, 255));
    }

    #[test]
    fn fractional_disparity_samples_each_eye_on_its_own_pixel_grid() {
        let mut pass = Pass::default();
        pass.insert(
            1,
            &art(64, 32),
            None,
            (0..64 * 32).map(|i| if i % 64 < 32 { 255 } else { 0 }),
        );
        pass.insert(2, &art(1, 1), None, [191].into_iter());
        let camera = Camera::new(ORIGIN, 11.0);
        let mut caster = sprite(1, 52.31, None);
        caster.position[0] = 0.5 / PPU;
        let mut receiver = sprite(2, 44.31, Some(0));
        receiver.scale = [16.0; 2];
        receiver.opacity = 0.49;
        receiver.stereo_depth = 0.5;
        let clouds = [caster, receiver];

        let left = pass
            .prepare_left(camera, &clouds, 0.5)
            .unwrap()
            .pixels
            .to_vec();
        let depth = pass.depth.clone();
        let layout = pass.mask(0).unwrap();
        assert_eq!(screen_exposure(&pass, 201, 120), 255);

        let right = pass.prepare_right(camera, &clouds, 0.5);
        assert_ne!(right.pixels, left);
        assert_eq!(screen_exposure(&pass, 201, 120), 0);
        assert_eq!(pass.depth, depth);
        assert_eq!(pass.mask(0).unwrap().tile, layout.tile);
        assert_eq!(pass.mask(0).unwrap().origin, layout.origin);

        let mono = pass
            .prepare_left(camera, &clouds, 0.0)
            .unwrap()
            .pixels
            .to_vec();
        assert_eq!(pass.prepare_right(camera, &clouds, 0.0).pixels, mono);
    }

    #[test]
    fn shared_atlas_layout_keeps_pixels_visible_only_to_one_eye() {
        let mut pass = Pass::default();
        pass.insert(1, &art(64, 32), None, std::iter::repeat_n(255, 64 * 32));
        pass.insert(2, &art(4, 4), None, std::iter::repeat_n(191, 16));
        let camera = Camera::new(ORIGIN, 11.0);
        let caster = sprite(1, 52.31, None);
        let mut receiver = sprite(2, 44.31, Some(0));
        receiver.screen[0] = -1.75;
        receiver.opacity = 0.49;
        let clouds = [caster, receiver];

        assert!(pass.prepare_left(camera, &clouds, 0.0).is_none());
        let left = pass.prepare_left(camera, &clouds, 2.0).unwrap();
        assert!(left.pixels.iter().all(|&pixel| pixel == 255));
        assert_eq!(pass.mask(0).unwrap().origin, [0, 118]);
        pass.prepare_right(camera, &clouds, 2.0);
        assert_eq!(screen_exposure(&pass, 0, 120), 0);
        assert!(pass.prepare_left(camera, &clouds, 0.0).is_none());
        assert!(pass.mask(0).is_none());
    }

    #[test]
    fn triangle_uvs_interpolate_after_the_shadow_perspective_divide() {
        let camera = Camera::new(ORIGIN, 2.0);
        let local = [[-35.0, -64.0], [39.0, -6.0], [-23.0, 59.0]];
        let origin = [0.0, 5.5, 47.0];
        let shadow = local.map(|v| {
            let [x, y, z] = world_point(v, origin, [1.0; 2], false);
            camera.project(x, y, z)
        });
        let gradients = uv_gradients(local, shadow).unwrap();
        let middle = [0, 1].map(|axis| (local[0][axis] + local[1][axis] + local[2][axis]) / 3.0);
        let delta = [middle[0] - local[0][0], middle[1] - local[0][1]];
        let linear =
            [0, 1].map(|i| shadow[0][i] + gradients[i][0] * delta[0] + gradients[i][1] * delta[1]);
        let [x, y, z] = world_point(middle, origin, [1.0; 2], false);
        let reprojection = camera.project(x, y, z);
        for i in 0..2 {
            let expected = (shadow[0][i] + shadow[1][i] + shadow[2][i]) / 3.0;
            assert!((linear[i] - expected).abs() < 0.0001);
        }
        assert!((linear[1] - reprojection[1]).abs() > 0.01);
    }
}
