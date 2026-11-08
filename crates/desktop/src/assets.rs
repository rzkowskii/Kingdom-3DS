//! Loads what `tools/extract` writes (sprite pages, `sprites.json`,
//! `clips.json`) and draws `scene` through macroquad.

use std::path::Path;
use std::rc::Rc;

use ::scene::cloud_mesh;
use ::scene::cloud_shadow::{self, Geometry, MAP_H, MAP_W};
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
    cloud_depth: Option<CloudDepth>,
    shadow_camera: Option<cloud_shadow::Camera>,
    /// Desktop capture fixtures can compare the same frame at full exposure.
    pub cloud_shadows_enabled: bool,
    /// Additive blending (glows, PostFX add color).
    additive: Material,
    /// Water reflections (see `REFLECT_FS`).
    reflect: Material,
    /// The previous frame's top screen, for `reflect`.
    pub previous: Option<Texture2D>,
}

const DEPTH_WIDTH: u32 = MAP_W as u32;
const DEPTH_HEIGHT: u32 = MAP_H as u32;
const DEPTH_MAX: f32 = 16_777_215.0;

/// The receiver samples the shadow camera's encoded depth in its fragment
/// shader. Macroquad's wrapper for a raw texture does not own its allocation.
struct CloudDepth {
    texture: Texture2D,
    bytes: Vec<u8>,
}

impl CloudDepth {
    fn new() -> Self {
        let texture = unsafe { mq::get_internal_gl() }.quad_context.new_texture(
            TextureAccess::Static,
            TextureSource::Empty,
            TextureParams {
                format: TextureFormat::RGBA8,
                width: DEPTH_WIDTH,
                height: DEPTH_HEIGHT,
                min_filter: FilterMode::Nearest,
                mag_filter: FilterMode::Nearest,
                ..Default::default()
            },
        );
        Self {
            texture: Texture2D::from_miniquad_texture(texture),
            bytes: vec![0; (DEPTH_WIDTH * DEPTH_HEIGHT * 4) as usize],
        }
    }

    fn upload(&mut self, depth: &[f32]) {
        encode_depth(depth, &mut self.bytes);
        let id = self.texture.raw_miniquad_id();
        let mut gl = unsafe { mq::get_internal_gl() };
        // Finish any draws using the previous map before replacing it.
        gl.flush();
        gl.quad_context.texture_update_part(
            id,
            0,
            0,
            DEPTH_WIDTH as i32,
            DEPTH_HEIGHT as i32,
            &self.bytes,
        );
    }
}

impl Drop for CloudDepth {
    fn drop(&mut self) {
        let id = self.texture.raw_miniquad_id();
        unsafe { mq::get_internal_gl() }
            .quad_context
            .delete_texture(id);
    }
}

pub(crate) fn encode_depth(depth: &[f32], bytes: &mut [u8]) {
    assert_eq!(bytes.len(), depth.len() * 4);
    for (&value, out) in depth.iter().zip(bytes.as_chunks_mut::<4>().0) {
        let word = (value.clamp(0.0, 1.0) * DEPTH_MAX).round() as u32;
        let [_, r, g, b] = word.to_be_bytes();
        *out = [r, g, b, 255];
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

pub(crate) const CLOUD_VS: &str = r#"#version 100
attribute vec3 position;
attribute vec2 texcoord;
attribute vec4 color0;
attribute vec4 normal;
varying highp vec2 uv;
varying highp vec4 shadow;
varying lowp vec4 color;
uniform mat4 Model;
uniform mat4 Projection;
void main() {
    gl_Position = Projection * Model * vec4(position, 1);
    color = color0 / 255.0;
    uv = texcoord;
    shadow = normal;
}
"#;

pub(crate) const CLOUD_FS: &str = r#"#version 100
precision lowp float;
varying highp vec2 uv;
varying highp vec4 shadow;
varying vec4 color;
uniform sampler2D Texture;
uniform highp sampler2D CloudDepth;
uniform vec4 CloudLight;
const highp vec2 depthSize = vec2(512.0, 256.0);
const highp vec3 depthWeights = vec3(65536.0, 256.0, 1.0);

highp float depthTexel(highp vec2 pixel) {
    highp vec2 coord = (pixel + 0.5) / depthSize;
    highp vec3 encoded = texture2D(CloudDepth, coord).rgb;
    return dot(encoded, depthWeights) / 65793.0;
}

highp float sampleDepth(highp vec2 pixel) {
    // Filter decoded RGB packing in the encoded camera-depth domain.
    highp vec2 corner = floor(pixel);
    highp vec2 t = pixel - corner;
    highp float a = mix(depthTexel(corner),
                        depthTexel(corner + vec2(1.0, 0.0)), t.x);
    highp float b = mix(depthTexel(corner + vec2(0.0, 1.0)),
                        depthTexel(corner + vec2(1.0, 1.0)), t.x);
    highp float depth = mix(a, b, t.y);
    return 1.0 / (depth * (1.0 / 20.0 - 1.0 / 0.3) + 1.0 / 0.3);
}

void main() {
    vec4 t = texture2D(Texture, uv);
    if (t.a == 0.0) {
        discard;
    }
    // The atlas stores (1 + source alpha) / 2 after the PC alpha cutoff.
    // Its complement is the unlit highlight's original alpha coefficient.
    float exposure = 1.0;
    if (shadow.w > 0.5 && t.a < 1.0) {
        highp vec2 pixel = shadow.xy;
        highp float a = sampleDepth(pixel);
        highp float b = sampleDepth(pixel + vec2(0.0, 2.0));
        exposure = (shadow.z - a < 0.05 ? 0.5 : 0.0)
                 + (shadow.z - b < 0.05 ? 0.5 : 0.0);
    }
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
        let mesh_path = dir.join(cloud_mesh::FILE_NAME);
        match std::fs::read(&mesh_path) {
            Ok(bytes) => sheet.link_cloud_meshes(world, &bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", mesh_path.display())),
        }
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
                    cloud_pass.insert(id, s, sheet.cloud_mesh(id), alpha);
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
                textures: vec!["CloudDepth".into()],
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
        let cloud_depth = (!cloud_sprites.is_empty()).then(CloudDepth::new);
        cloud.set_texture(
            "CloudDepth",
            cloud_depth
                .as_ref()
                .map_or_else(Texture2D::empty, |map| map.texture.clone()),
        );
        Ok(Assets {
            pages,
            reflect,
            smooth,
            sheet: Rc::new(sheet),
            recolor,
            cloud,
            cloud_pass,
            cloud_depth,
            shadow_camera: None,
            cloud_shadows_enabled: true,
            additive,
            previous: None,
        })
    }

    fn indexed_sprite(
        &self,
        id: u32,
        mesh: Option<&cloud_mesh::CloudMesh>,
        at: (f32, f32),
        d: &Draw,
        vertex_tints: Option<&[Color]>,
    ) {
        let sprite = self.sheet.get(id);
        let geometry = Geometry::new(sprite, mesh);
        let texture = &self.pages[usize::from(sprite.page)];
        let shadow = self.shadow_camera.zip(d.cloud_shadow);
        let vertices = geometry
            .vertices()
            .iter()
            .enumerate()
            .map(|(i, &local)| {
                let [x, y] = cloud_shadow::screen_point(local, at, d);
                // Keep the original UV attached to its vertex when mirroring.
                let u = (f32::from(sprite.x) + sprite.px + local[0]) / texture.width();
                let v = (f32::from(sprite.y) + sprite.py + local[1]) / texture.height();
                let tint = vertex_tints.map_or(d.tint, |tints| tints[i]);
                let mut vertex = Vertex::new(x, y, 0.0, u, v, mc(tint));
                if let Some((camera, receiver)) = shadow {
                    let scale = [d.affine[0] * d.scale, d.affine[3] * d.scale];
                    let [x, y, z] =
                        cloud_shadow::world_point(local, receiver.position, scale, d.flip);
                    let [u, v, clip_z] = camera.project(x, y, z);
                    vertex.normal = mq::vec4(u, v, clip_z, 1.0);
                }
                vertex
            })
            .collect();
        if let Some(light) = d.cloud_light {
            gl_use_material(&self.cloud);
            self.cloud.set_uniform("CloudLight", mc(light).to_vec());
        } else if d.additive {
            gl_use_material(&self.additive);
        }
        draw_mesh(&Mesh {
            vertices,
            indices: geometry.indices().to_vec(),
            texture: Some(texture.clone()),
        });
        if d.cloud_light.is_some() || d.additive {
            gl_use_default_material();
        }
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
        self.shadow_camera = None;
        let Some(texture) = self
            .cloud_depth
            .as_mut()
            .filter(|_| self.cloud_shadows_enabled)
        else {
            return false;
        };
        let Some(depth) = self.cloud_pass.prepare_depth(camera, clouds) else {
            return false;
        };
        texture.upload(depth);
        self.shadow_camera = Some(camera);
        true
    }

    fn sprite(&mut self, id: u32, at: (f32, f32), d: Draw) {
        if d.cloud_light.is_some() {
            self.indexed_sprite(id, self.sheet.cloud_mesh(id), at, &d, None);
            return;
        }
        let s = *self.sheet.get(id);
        if d.recolor.is_some() || d.overlay.is_some() {
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
        let page = if d.smooth { &self.smooth } else { &self.pages };
        if d.affine != [1.0, 0.0, 0.0, 1.0] || d.flip_y {
            let corners = ::scene::render::sprite_corners(&s, at, &d);
            let texture = &page[s.page as usize];
            let (pw, ph) = (texture.width(), texture.height());
            let (u0, u1) = (s.x as f32 / pw, (s.x + s.w) as f32 / pw);
            let (v0, v1) = (s.y as f32 / ph, (s.y + s.h) as f32 / ph);
            let (ul, ur) = if d.flip { (u1, u0) } else { (u0, u1) };
            let (vt, vb) = if d.flip_y { (v1, v0) } else { (v0, v1) };
            let uv = [(ul, vt), (ur, vt), (ur, vb), (ul, vb)];
            let vertices = corners
                .into_iter()
                .zip(uv)
                .map(|((x, y), (u, v))| Vertex::new(x, y, 0.0, u, v, mc(d.tint)))
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
        if d.recolor.is_some() || d.overlay.is_some() || d.additive {
            gl_use_default_material();
        }
    }

    fn atmosphere_sprite(
        &mut self,
        id: u32,
        mesh: &cloud_mesh::CloudMesh,
        at: (f32, f32),
        d: Draw,
        vertex_tints: Option<&[Color]>,
    ) {
        self.indexed_sprite(id, Some(mesh), at, &d, vertex_tints);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_upload_round_trips_24_bit_samples_in_row_order() {
        let depth = [0.0, 1.0, 0.5, 0.975_002, 0.993_261, 0.975_017];
        let mut bytes = [0; 24];
        encode_depth(&depth, &mut bytes);
        assert_eq!(
            &bytes[..12],
            &[0, 0, 0, 255, 255, 255, 255, 255, 128, 0, 0, 255]
        );
        for (actual, [r, g, b, _]) in depth.iter().zip(bytes.as_chunks::<4>().0) {
            let decoded = u32::from_be_bytes([0, *r, *g, *b]) as f32 / DEPTH_MAX;
            assert!((actual - decoded).abs() <= 1.0 / DEPTH_MAX);
        }
    }
}
