//! What a backend draws with. Coordinates are screen pixels, y down, from
//! the screen's top-left (400×240 top, 320×240 touch screen).

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const WHITE: Color = Color::new(1.0, 1.0, 1.0, 1.0);
    pub const BLACK: Color = Color::new(0.0, 0.0, 0.0, 1.0);

    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Color {
        Color { r, g, b, a }
    }

    pub fn rgb(c: [f32; 3]) -> Color {
        Color::new(c[0], c[1], c[2], 1.0)
    }

    pub fn rgba(c: [f32; 4]) -> Color {
        Color::new(c[0], c[1], c[2], c[3])
    }

    pub fn bytes(c: [u8; 4]) -> Color {
        let f = |v: u8| v as f32 / 255.0;
        Color::new(f(c[0]), f(c[1]), f(c[2]), f(c[3]))
    }

    pub fn times(self, o: Color) -> Color {
        Color::new(self.r * o.r, self.g * o.g, self.b * o.b, self.a * o.a)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn contains(&self, p: (f32, f32)) -> bool {
        p.0 >= self.x && p.0 < self.x + self.w && p.1 >= self.y && p.1 < self.y + self.h
    }
}

/// How to draw a sprite.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Draw {
    pub flip: bool,
    pub flip_y: bool,
    /// Row-major screen-space linear transform, applied around the sprite pivot.
    pub affine: [f32; 4],
    pub tint: Color,
    pub scale: f32,
    /// Radians, clockwise on screen, around the pivot.
    pub angle: f32,
    /// orig: `SpriteFX` `recolorA` / `recolorB`.
    pub recolor: Option<(Color, Color)>,
    /// SpriteFX color overlay: RGB interpolation, preserving sprite coverage.
    pub overlay: Option<Color>,
    /// ShadowReceiver material with coverage baked into the cloud sprite's alpha.
    /// RGB is the light after applying the cloud's shadow exposure. Alpha is unused.
    pub cloud_light: Option<Color>,
    /// Top-left of the cloud's exposure mask in the dynamic atlas.
    pub cloud_shadow: Option<[u16; 2]>,
    pub additive: bool,
    /// Filtered smoothly when scaled (glows); everything else stays pixel-sharp.
    pub smooth: bool,
}

impl Draw {
    pub fn tint(tint: Color) -> Draw {
        Draw {
            flip: false,
            flip_y: false,
            affine: [1.0, 0.0, 0.0, 1.0],
            tint,
            scale: 1.0,
            angle: 0.0,
            recolor: None,
            overlay: None,
            cloud_light: None,
            cloud_shadow: None,
            additive: false,
            smooth: false,
        }
    }

    pub fn flip(mut self, flip: bool) -> Draw {
        self.flip = flip;
        self
    }

    pub fn flip_y(mut self, flip: bool) -> Draw {
        self.flip_y = flip;
        self
    }

    pub fn affine(mut self, basis: [f32; 4]) -> Draw {
        self.affine = basis;
        self
    }

    pub fn recolor(mut self, a: Color, b: Color) -> Draw {
        self.recolor = Some((a, b));
        self
    }

    pub fn angle(mut self, angle: f32) -> Draw {
        self.angle = angle;
        self
    }

    pub fn overlay(mut self, color: Color) -> Draw {
        self.overlay = Some(color);
        self
    }

    /// Selects the cloud material. Tint RGB is the authored ambient times
    /// the cloud's base color, and tint alpha is vertex coverage.
    pub fn cloud_light(mut self, light: Color) -> Draw {
        self.cloud_light = Some(Color::new(light.r, light.g, light.b, 1.0));
        self
    }

    pub fn scale(mut self, scale: f32) -> Draw {
        self.scale = scale;
        self
    }

    pub fn additive(mut self) -> Draw {
        self.additive = true;
        self
    }

    pub fn smooth(mut self) -> Draw {
        self.smooth = true;
        self
    }
}

/// Shared desktop/3DS geometry. UV flips change the pivot's offset but never
/// mirror a child translation; whole-object reflection belongs in `affine`.
pub fn sprite_corners(s: &crate::sheet::Sprite, at: (f32, f32), d: &Draw) -> [(f32, f32); 4] {
    let px = if d.flip { s.w as f32 - s.px } else { s.px };
    let py = if d.flip_y { s.h as f32 - s.py } else { s.py };
    let rel = [
        (-px, -py),
        (s.w as f32 - px, -py),
        (s.w as f32 - px, s.h as f32 - py),
        (-px, s.h as f32 - py),
    ];
    let [a, b, c, e] = d.affine;
    let (sin, cos) = d.angle.sin_cos();
    let mut corners = rel.map(|(x, y)| {
        let (x, y) = ((a * x + b * y) * d.scale, (c * x + e * y) * d.scale);
        (at.0 + x * cos - y * sin, at.1 + x * sin + y * cos)
    });
    if d.scale == 1.0 && d.angle == 0.0 && d.affine == [1.0, 0.0, 0.0, 1.0] {
        for p in &mut corners {
            *p = (p.0.round(), p.1.round());
        }
    }
    corners
}

/// Stereo replay must sample the same eye's previous reflection. If a right
/// capture is unavailable, omit water reflection instead of borrowing left.
pub fn reflection_page_for_right(page: u16, pages: [u16; 2], right_ready: bool) -> Option<u16> {
    if page == pages[0] {
        right_ready.then_some(pages[1])
    } else {
        Some(page)
    }
}

/// Implemented by each backend (macroquad on desktop, citro3d on 3DS).
/// Calls arrive in painter's order.
pub trait Renderer {
    /// Prepare the scene's cloud depth map and upload the visible receivers' masks.
    fn cloud_shadows(
        &mut self,
        _camera: crate::cloud_shadow::Camera,
        _clouds: &[crate::cloud_shadow::Sprite],
    ) -> bool {
        false
    }

    fn clear(&mut self, c: Color);
    /// Sprite `id` (in the backend's `Sheet`) with its pivot at `at`.
    fn sprite(&mut self, id: u32, at: (f32, f32), d: Draw);
    /// 9-sliced into `dest`; `border` is (left, bottom, right, top) pixels.
    fn sliced(&mut self, id: u32, dest: Rect, border: (f32, f32, f32, f32), tint: Color);
    fn rect(&mut self, r: Rect, c: Color);
    fn rect_lines(&mut self, r: Rect, thickness: f32, c: Color);
    /// A vertical gradient.
    fn gradient(&mut self, r: Rect, top: Color, bottom: Color);
    /// Adds `c` over `r`. orig: `PostFX` additive colour.
    fn add(&mut self, r: Rect, c: Color);
    /// `add` with a vertical gradient from `top` to `bottom`. Backends
    /// without one add the average colour.
    fn add_gradient(&mut self, r: Rect, top: Color, bottom: Color) {
        let mid = |a: f32, b: f32| (a + b) / 2.0;
        self.add(
            r,
            Color::new(
                mid(top.r, bottom.r),
                mid(top.g, bottom.g),
                mid(top.b, bottom.b),
                mid(top.a, bottom.a),
            ),
        );
    }
    fn circle(&mut self, x: f32, y: f32, radius: f32, c: Color);
    fn circle_lines(&mut self, x: f32, y: f32, radius: f32, thickness: f32, c: Color);
    /// Pixels `src` of the previous frame's top screen (a negative `h` flips
    /// them) stretched into `dest`, multiplied by `tint` and *added* over
    /// what's there (the source's own alpha is ignored). orig: the
    /// `MainCamera` reflection buffer the `Water` samples. Backends without
    /// one draw nothing.
    fn reflect(&mut self, _dest: Rect, _src: Rect, _tint: Color) {}
    /// Stereo depth of what's drawn next: 0 at the screen, 1 farthest
    /// behind it, negative in front. Backends without 3D ignore it.
    fn depth(&mut self, _d: f32) {}
}

#[cfg(test)]
mod transform_tests {
    use super::*;
    use crate::sheet::Sprite;

    #[test]
    fn cloud_material_keeps_vertex_opacity_and_ignores_light_alpha() {
        let tint = Color::new(0.34, 0.28, 0.21, 0.4);
        let plain = Draw::tint(tint);
        let cloud = plain.cloud_light(Color::new(0.42, 0.12, 0.08, 0.0));
        assert_eq!(plain.cloud_light, None);
        assert_eq!(cloud.tint, tint);
        assert_eq!(cloud.cloud_light, Some(Color::new(0.42, 0.12, 0.08, 1.0)));
        assert_eq!(
            plain.cloud_light(Color::BLACK).cloud_light,
            Some(Color::BLACK)
        );
    }

    #[test]
    fn stereo_reflection_replay_never_samples_the_other_eyes_capture() {
        assert_eq!(reflection_page_for_right(7, [7, 8], true), Some(8));
        assert_eq!(reflection_page_for_right(7, [7, 8], false), None);
        assert_eq!(reflection_page_for_right(3, [7, 8], false), Some(3));
    }
    #[test]
    fn affine_quad_preserves_off_center_pivot_shear_and_both_renderer_flips() {
        let s = Sprite {
            page: 0,
            x: 0,
            y: 0,
            w: 4,
            h: 2,
            px: 1.0,
            py: 0.5,
            masks: [None, None],
        };
        let d = Draw::tint(Color::WHITE).affine([2.0, 1.0, -1.0, 3.0]);
        assert_eq!(
            sprite_corners(&s, (20.0, 30.0), &d),
            [(17.5, 29.5), (25.5, 25.5), (27.5, 31.5), (19.5, 35.5)]
        );
        let d = d.flip(true).flip_y(true);
        assert_eq!(
            sprite_corners(&s, (20.0, 30.0), &d),
            [(12.5, 28.5), (20.5, 24.5), (22.5, 30.5), (14.5, 34.5)]
        );
    }
    #[test]
    fn whole_object_mirror_reflects_rotated_quad_about_root_not_child_pivot() {
        let transform = game::data::Affine2 {
            basis: [0.0, -2.0, 3.0, 0.0],
            translation: [5.0, 2.0],
        };
        let sprite = Sprite {
            page: 0,
            x: 0,
            y: 0,
            w: 4,
            h: 2,
            px: 1.0,
            py: 0.5,
            masks: [None, None],
        };
        let a = sprite_corners(
            &sprite,
            (105.0, 48.0),
            &Draw::tint(Color::WHITE).affine(transform.screen_basis(false)),
        );
        let b = sprite_corners(
            &sprite,
            (95.0, 48.0),
            &Draw::tint(Color::WHITE).affine(transform.screen_basis(true)),
        );
        for (a, b) in a.into_iter().zip(b) {
            assert_eq!(b, (200.0 - a.0, a.1));
        }
    }
}
