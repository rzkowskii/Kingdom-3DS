//! Loads what `tools/extract` writes (sprite pages, `sprites.json`,
//! `clips.json`) and draws `scene` through macroquad.

use std::path::Path;
use std::rc::Rc;

use ::scene::cloud_shadow::{self, ATLAS_HEIGHT, ATLAS_WIDTH};
use ::scene::render::{Color, Draw, Rect, Renderer};
use ::scene::sheet::Sheet;
use game::data::WorldData;
use macroquad::miniquad::{
    BlendFactor, BlendState, BlendValue, Equation, TextureAccess, TextureFormat, TextureParams,
    TextureSource,
};
use macroquad::prelude as mq;
use macroquad::prelude::{
    draw_circle, draw_circle_lines, draw_mesh, draw_rectangle, draw_rectangle_lines,
    draw_texture_ex, gl_use_default_material, gl_use_material, load_material, load_texture, vec2,
    DrawTextureParams, FilterMode, Material, MaterialParams, Mesh, PipelineParams, ShaderSource,
    Texture2D, UniformDesc, UniformType, Vertex,
};

pub struct Assets {
    pub pages: Vec<Texture2D>,
    /// The same pages, linearly filtered (for `Draw::smooth`).
    smooth: Vec<Texture2D>,
    pub sheet: Rc<Sheet>,
    /// Recolors sprite pixels keyed by alpha. See `RECOLOR_FS`.
    recolor: Material,
    cloud: Material,
    cloud_pass: cloud_shadow::Pass,
    cloud_atlas: Option<CloudAtlas>,
    /// Desktop capture fixtures can compare the same frame at full exposure.
    pub cloud_shadows_enabled: bool,
    /// Additive blending (glows, PostFX add color).
    additive: Material,
    /// Water reflections (see `REFLECT_FS`).
    reflect: Material,
    /// The previous frame's top screen, for `reflect`.
    pub previous: Option<Texture2D>,
}

/// Single-channel exposure atlas. Macroquad's wrapper for a raw texture does
/// not own its allocation.
struct CloudAtlas(Texture2D);

impl CloudAtlas {
    fn new() -> Self {
        let texture = unsafe { mq::get_internal_gl() }.quad_context.new_texture(
            TextureAccess::Static,
            TextureSource::Empty,
            TextureParams {
                format: TextureFormat::Alpha,
                width: u32::from(ATLAS_WIDTH),
                height: u32::from(ATLAS_HEIGHT),
                min_filter: FilterMode::Nearest,
                mag_filter: FilterMode::Nearest,
                ..Default::default()
            },
        );
        Self(Texture2D::from_miniquad_texture(texture))
    }

    fn upload(&self, atlas: cloud_shadow::Atlas<'_>) {
        let id = self.0.raw_miniquad_id();
        let mut gl = unsafe { mq::get_internal_gl() };
        // Finish any draws using the previous mask before replacing it.
        gl.flush();
        gl.quad_context.texture_update_part(
            id,
            0,
            0,
            i32::from(ATLAS_WIDTH),
            i32::from(atlas.height),
            atlas.pixels,
        );
    }
}

impl Drop for CloudAtlas {
    fn drop(&mut self) {
        let id = self.0.raw_miniquad_id();
        unsafe { mq::get_internal_gl() }
            .quad_context
            .delete_texture(id);
    }
}

fn mc(c: Color) -> mq::Color {
    mq::Color::new(c.r, c.g, c.b, c.a)
}

const RECOLOR_FS: &str = r#"#version 100
precision lowp float;
varying vec2 uv;
varying vec4 color;
uniform sampler2D Texture;
uniform vec4 RecolorA;
uniform vec4 RecolorB;
uniform vec4 Overlay;
void main() {
    vec4 t = texture2D(Texture, uv);
    // The keyed pixels' red channel is their shade of the recolour.
    if (t.a > 0.66 && t.a < 0.72) {
        t = vec4(t.r * RecolorB.rgb, 1.0);
    } else if (t.a > 0.79 && t.a < 0.85) {
        t = vec4(t.r * RecolorA.rgb, 1.0);
    }
    t *= color;
    t.rgb = mix(t.rgb, Overlay.rgb, Overlay.a);
    gl_FragColor = t;
}
"#;

const PLAIN_FS: &str = r#"#version 100
precision lowp float;
varying vec2 uv;
varying vec4 color;
uniform sampler2D Texture;
void main() {
    gl_FragColor = texture2D(Texture, uv) * color;
}
"#;

const CLOUD_VS: &str = r#"#version 100
attribute vec3 position;
attribute vec2 texcoord;
attribute vec4 color0;
attribute vec4 normal;
varying highp vec2 uv;
varying highp vec3 shadow;
varying lowp vec4 color;
uniform mat4 Model;
uniform mat4 Projection;
void main() {
    gl_Position = Projection * Model * vec4(position, 1);
    color = color0 / 255.0;
    uv = texcoord;
    shadow = normal.xyz;
}
"#;

const CLOUD_FS: &str = r#"#version 100
precision lowp float;
varying highp vec2 uv;
varying highp vec3 shadow;
varying vec4 color;
uniform sampler2D Texture;
uniform sampler2D CloudExposure;
uniform vec4 CloudLight;
void main() {
    vec4 t = texture2D(Texture, uv);
    if (t.a == 0.0) {
        discard;
    }
    // The atlas stores (1 + source alpha) / 2 after the PC alpha cutoff.
    // Its complement is the unlit highlight's original alpha coefficient.
    float exposure = shadow.z > 0.5 ? texture2D(CloudExposure, shadow.xy).a : 1.0;
    gl_FragColor = vec4(t.rgb * color.rgb + (1.0 - t.a) * CloudLight.rgb * exposure,
                        t.a * color.a);
}
"#;

/// The water's reflection: the texture's colour only (the previous frame's
/// alpha is whatever its blending left), added over the water.
const REFLECT_FS: &str = r#"#version 100
precision lowp float;
varying vec2 uv;
varying vec4 color;
uniform sampler2D Texture;
void main() {
    gl_FragColor = vec4(texture2D(Texture, uv).rgb * color.rgb, color.a);
}
"#;

const RECOLOR_VS: &str = r#"#version 100
attribute vec3 position;
attribute vec2 texcoord;
attribute vec4 color0;
varying lowp vec2 uv;
varying lowp vec4 color;
uniform mat4 Model;
uniform mat4 Projection;
void main() {
    gl_Position = Projection * Model * vec4(position, 1);
    color = color0 / 255.0;
    uv = texcoord;
}
"#;

impl Assets {
    pub async fn load(dir: &Path, world: &WorldData) -> Result<Assets, String> {
        let read = |name: &str| {
            std::fs::read_to_string(dir.join(name))
                .map_err(|e| format!("{}: {e}", dir.join(name).display()))
        };
        let mut sheet = Sheet::from_json(
            &read("sprites.json")?,
            &read("clips.json")?,
            &read("font.json")?,
        )?;
        sheet.link(&world.sprites);
        let cloud_sprites = world
            .clouds
            .iter()
            .flat_map(game::data::CloudsDef::sprite_ids)
            .map(|id| {
                sheet.world(id).ok_or_else(|| {
                    format!("missing cloud sprite {}", world.sprites[usize::from(id)])
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut cloud_pass = cloud_shadow::Pass::default();
        let mut pages = Vec::new();
        let mut smooth = Vec::new();
        for (page, p) in sheet.pages.iter().enumerate() {
            let path = dir.join(&p.file);
            let tex = load_texture(&path.to_string_lossy())
                .await
                .map_err(|e| format!("{}: {e}", path.display()))?;
            tex.set_filter(FilterMode::Nearest);
            let image = tex.get_texture_data();
            for &id in &cloud_sprites {
                let s = sheet.get(id);
                if usize::from(s.page) == page {
                    let alpha = (s.y..s.y + s.h).flat_map(|y| {
                        let width = usize::from(image.width);
                        let start = (usize::from(y) * width + usize::from(s.x)) * 4;
                        let end = start + usize::from(s.w) * 4;
                        image.bytes[start..end]
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .map(|pixel| pixel[3])
                    });
                    cloud_pass.insert(id, s, alpha);
                }
            }
            let soft = Texture2D::from_image(&image);
            soft.set_filter(FilterMode::Linear);
            smooth.push(soft);
            pages.push(tex);
        }
        let blend = |dst: BlendFactor| PipelineParams {
            color_blend: Some(BlendState::new(
                Equation::Add,
                BlendFactor::Value(BlendValue::SourceAlpha),
                dst,
            )),
            ..Default::default()
        };
        let recolor = load_material(
            ShaderSource::Glsl {
                vertex: RECOLOR_VS,
                fragment: RECOLOR_FS,
            },
            MaterialParams {
                uniforms: vec![
                    UniformDesc::new("RecolorA", UniformType::Float4),
                    UniformDesc::new("RecolorB", UniformType::Float4),
                    UniformDesc::new("Overlay", UniformType::Float4),
                ],
                pipeline_params: blend(BlendFactor::OneMinusValue(BlendValue::SourceAlpha)),
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;
        let cloud = load_material(
            ShaderSource::Glsl {
                vertex: CLOUD_VS,
                fragment: CLOUD_FS,
            },
            MaterialParams {
                uniforms: vec![UniformDesc::new("CloudLight", UniformType::Float4)],
                pipeline_params: blend(BlendFactor::OneMinusValue(BlendValue::SourceAlpha)),
                textures: vec!["CloudExposure".into()],
            },
        )
        .map_err(|e| e.to_string())?;
        let additive = load_material(
            ShaderSource::Glsl {
                vertex: RECOLOR_VS,
                fragment: PLAIN_FS,
            },
            MaterialParams {
                pipeline_params: blend(BlendFactor::One),
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;
        let reflect = load_material(
            ShaderSource::Glsl {
                vertex: RECOLOR_VS,
                fragment: REFLECT_FS,
            },
            MaterialParams {
                pipeline_params: blend(BlendFactor::One),
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;
        let cloud_atlas = (!cloud_sprites.is_empty()).then(CloudAtlas::new);
        cloud.set_texture(
            "CloudExposure",
            cloud_atlas
                .as_ref()
                .map_or_else(Texture2D::empty, |atlas| atlas.0.clone()),
        );
        Ok(Assets {
            pages,
            reflect,
            smooth,
            sheet: Rc::new(sheet),
            recolor,
            cloud,
            cloud_pass,
            cloud_atlas,
            cloud_shadows_enabled: true,
            additive,
            previous: None,
        })
    }
}

/// A rectangle shaded from `top` to `bottom`.
fn gradient_mesh(r: Rect, top: Color, bottom: Color) {
    let v = |x: f32, y: f32, c: Color| Vertex::new(x, y, 0.0, 0.0, 0.0, mc(c));
    draw_mesh(&Mesh {
        vertices: vec![
            v(r.x, r.y, top),
            v(r.x + r.w, r.y, top),
            v(r.x + r.w, r.y + r.h, bottom),
            v(r.x, r.y + r.h, bottom),
        ],
        indices: vec![0, 1, 2, 0, 2, 3],
        texture: None,
    });
}

impl Renderer for Assets {
    fn clear(&mut self, c: Color) {
        mq::clear_background(mc(c));
    }

    fn cloud_shadows(
        &mut self,
        camera: cloud_shadow::Camera,
        clouds: &[cloud_shadow::Sprite],
    ) -> bool {
        let Some(atlas) = self
            .cloud_atlas
            .as_ref()
            .filter(|_| self.cloud_shadows_enabled)
        else {
            return false;
        };
        let Some(exposure) = self.cloud_pass.prepare(camera, clouds) else {
            return false;
        };
        atlas.upload(exposure);
        true
    }

    fn sprite(&mut self, id: u32, at: (f32, f32), d: Draw) {
        let s = *self.sheet.get(id);
        if let Some(light) = d.cloud_light {
            gl_use_material(&self.cloud);
            self.cloud.set_uniform("CloudLight", mc(light).to_vec());
        } else if d.recolor.is_some() || d.overlay.is_some() {
            let (a, b) = d.recolor.unwrap_or((Color::WHITE, Color::WHITE));
            gl_use_material(&self.recolor);
            self.recolor.set_uniform("RecolorA", mc(a).to_vec());
            self.recolor.set_uniform("RecolorB", mc(b).to_vec());
            self.recolor.set_uniform(
                "Overlay",
                mc(d.overlay.unwrap_or(Color::new(0., 0., 0., 0.))).to_vec(),
            );
        } else if d.additive {
            gl_use_material(&self.additive);
        }
        let page = if d.smooth && d.cloud_light.is_none() {
            &self.smooth
        } else {
            &self.pages
        };
        if d.affine != [1.0, 0.0, 0.0, 1.0] || d.flip_y || d.cloud_light.is_some() {
            let corners = ::scene::render::sprite_corners(&s, at, &d);
            let texture = &page[s.page as usize];
            let (pw, ph) = (texture.width(), texture.height());
            let (u0, u1) = (s.x as f32 / pw, (s.x + s.w) as f32 / pw);
            let (v0, v1) = (s.y as f32 / ph, (s.y + s.h) as f32 / ph);
            let (ul, ur) = if d.flip { (u1, u0) } else { (u0, u1) };
            let (vt, vb) = if d.flip_y { (v1, v0) } else { (v0, v1) };
            let uv = [(ul, vt), (ur, vt), (ur, vb), (ul, vb)];
            let shadow_uv = d
                .cloud_shadow
                .map(|tile| shadow_corners(tile, [s.w, s.h], d.flip, d.flip_y));
            let vertices = corners
                .into_iter()
                .zip(uv)
                .enumerate()
                .map(|(i, ((x, y), (u, v)))| {
                    let mut v = Vertex::new(x, y, 0.0, u, v, mc(d.tint));
                    if let Some(uv) = shadow_uv {
                        v.normal = mq::vec4(uv[i][0], uv[i][1], 1.0, 0.0);
                    }
                    v
                })
                .collect();
            draw_mesh(&Mesh {
                vertices,
                indices: vec![0, 1, 2, 0, 2, 3],
                texture: Some(texture.clone()),
            });
        } else {
            let px = if d.flip { s.w as f32 - s.px } else { s.px };
            let (mut x, mut y) = (at.0 - px * d.scale, at.1 - s.py * d.scale);
            if d.scale == 1.0 && d.angle == 0.0 {
                (x, y) = (x.round(), y.round());
            }
            draw_texture_ex(
                &page[s.page as usize],
                x,
                y,
                mc(d.tint),
                DrawTextureParams {
                    source: Some(mq::Rect::new(
                        s.x as f32, s.y as f32, s.w as f32, s.h as f32,
                    )),
                    dest_size: Some(vec2(s.w as f32 * d.scale, s.h as f32 * d.scale)),
                    rotation: d.angle,
                    pivot: (d.angle != 0.0).then_some(vec2(at.0, at.1)),
                    flip_x: d.flip,
                    ..Default::default()
                },
            );
        }
        if d.cloud_light.is_some() || d.recolor.is_some() || d.overlay.is_some() || d.additive {
            gl_use_default_material();
        }
    }

    /// orig: `Image.Type.Sliced`.
    fn sliced(&mut self, id: u32, dest: Rect, border: (f32, f32, f32, f32), tint: Color) {
        let s = *self.sheet.get(id);
        let (l, b, r, t) = border;
        let (sx, sy, sw, sh) = (s.x as f32, s.y as f32, s.w as f32, s.h as f32);
        let cols = [
            (0.0, l, 0.0, l),
            (l, sw - l - r, l, dest.w - l - r),
            (sw - r, r, dest.w - r, r),
        ];
        let rows = [
            (0.0, t, 0.0, t),
            (t, sh - t - b, t, dest.h - t - b),
            (sh - b, b, dest.h - b, b),
        ];
        for &(cx, cw, dx, dw) in &cols {
            for &(ry, rh, dy, dh) in &rows {
                if cw <= 0.0 || rh <= 0.0 || dw <= 0.0 || dh <= 0.0 {
                    continue;
                }
                draw_texture_ex(
                    &self.pages[s.page as usize],
                    dest.x + dx,
                    dest.y + dy,
                    mc(tint),
                    DrawTextureParams {
                        source: Some(mq::Rect::new(sx + cx, sy + ry, cw, rh)),
                        dest_size: Some(vec2(dw, dh)),
                        ..Default::default()
                    },
                );
            }
        }
    }

    fn rect(&mut self, r: Rect, c: Color) {
        draw_rectangle(r.x, r.y, r.w, r.h, mc(c));
    }

    fn rect_lines(&mut self, r: Rect, thickness: f32, c: Color) {
        draw_rectangle_lines(r.x, r.y, r.w, r.h, thickness, mc(c));
    }

    fn gradient(&mut self, r: Rect, top: Color, bottom: Color) {
        gradient_mesh(r, top, bottom);
    }

    fn add(&mut self, r: Rect, c: Color) {
        gl_use_material(&self.additive);
        draw_rectangle(r.x, r.y, r.w, r.h, mc(c));
        gl_use_default_material();
    }

    fn add_gradient(&mut self, r: Rect, top: Color, bottom: Color) {
        gl_use_material(&self.additive);
        gradient_mesh(r, top, bottom);
        gl_use_default_material();
    }

    fn circle(&mut self, x: f32, y: f32, radius: f32, c: Color) {
        draw_circle(x, y, radius, mc(c));
    }

    fn circle_lines(&mut self, x: f32, y: f32, radius: f32, thickness: f32, c: Color) {
        draw_circle_lines(x, y, radius, thickness, mc(c));
    }

    fn reflect(&mut self, dest: Rect, src: Rect, tint: Color) {
        let Some(t) = &self.previous else { return };
        gl_use_material(&self.reflect);
        draw_texture_ex(
            t,
            dest.x,
            dest.y,
            mc(tint),
            DrawTextureParams {
                source: Some(mq::Rect::new(
                    src.x,
                    src.y.min(src.y + src.h),
                    src.w,
                    src.h.abs(),
                )),
                dest_size: Some(vec2(dest.w, dest.h)),
                flip_y: src.h < 0.0,
                ..Default::default()
            },
        );
        gl_use_default_material();
    }
}

fn shadow_corners(tile: [u16; 2], size: [u16; 2], flip: bool, flip_y: bool) -> [[f32; 2]; 4] {
    let [x, y] = tile.map(f32::from);
    let [w, h] = size.map(f32::from);
    let u0 = x / f32::from(ATLAS_WIDTH);
    let u1 = (x + w) / f32::from(ATLAS_WIDTH);
    let v0 = y / f32::from(ATLAS_HEIGHT);
    let v1 = (y + h) / f32::from(ATLAS_HEIGHT);
    let (left, right) = if flip { (u1, u0) } else { (u0, u1) };
    let (top, bottom) = if flip_y { (v1, v0) } else { (v0, v1) };
    [[left, top], [right, top], [right, bottom], [left, bottom]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposure_tile_uvs_follow_both_source_flips() {
        let tile = [64, 128];
        let size = [32, 16];
        let left = 64.0 / 512.0;
        let right = 96.0 / 512.0;
        let top = 128.0 / 1024.0;
        let bottom = 144.0 / 1024.0;
        assert_eq!(
            shadow_corners(tile, size, false, false),
            [[left, top], [right, top], [right, bottom], [left, bottom]],
        );
        assert_eq!(
            shadow_corners(tile, size, true, false),
            [[right, top], [left, top], [left, bottom], [right, bottom]],
        );
        assert_eq!(
            shadow_corners(tile, size, false, true),
            [[left, bottom], [right, bottom], [right, top], [left, top]],
        );
    }
}
