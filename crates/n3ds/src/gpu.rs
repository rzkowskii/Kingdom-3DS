//! The 3DS renderer: citro3d with one vertex shader, drawing the scene as
//! textured quads (like citro2d does in C).
//!
//! The scene arrives back to front. Changing texture pages costs a draw call,
//! and in painter's order neighbouring sprites are usually on different
//! pages, so instead each item gets a depth from its place in the order:
//!
//! - Opaque items (sprites on the 1-bit-alpha pages with a full-alpha tint,
//!   and solid fills) are drawn grouped by page, one call each, with the
//!   depth test keeping them in order and the alpha test cutting out clear
//!   pixels.
//! - Everything else (soft-edged sprites, fades, haze, glows) follows in
//!   painter's order, tested against that depth but not writing it.
//!
//! With the debug overlay enabled, X switches to plain painter's order.
//!
//! Stereo 3D: every vertex carries its item's depth (from the scene's
//! parallax), and the shader shifts it sideways per eye, so with the 3D
//! slider up the top screen's draw calls simply run again for the right
//! eye.
//!
//! Water samples each eye's previous reflection capture. After drawing both
//! eyes, an offscreen target adds reflection-only geometry to a copy of each
//! color buffer; the visible framebuffers are never changed by that pass.
//!
//! There are no fragment shaders on the PICA200, so recolours are drawn as
//! the sprite's two mask sprites (cut out by `pack3ds`) tinted on top.

use std::rc::Rc;

use citro3d_sys as c3d;
use ctru_sys as ctru;
use scene::cloud_shadow::{self, ATLAS_HEIGHT, ATLAS_WIDTH};
use scene::render::{Color, Draw, Rect, Renderer};
use scene::sheet::{PageFormat, Sheet};

static SHADER: &[u8] = citro3d_macros::include_shader!("../shaders/sprite.v.pica");

/// Quads per frame (both screens). 16k quads = 2 MiB of vertices.
const MAX_QUADS: usize = 16384;
/// Depth step between items in painter's order (24-bit depth buffer).
const Z_STEP: f32 = 1.5e-5;
/// The top screen's colour buffer: 240 x 400 (mounted sideways), RGBA8,
/// in rows of 8x8 tiles. The reflection texture is 256 x 512.
const TOP_TILE_ROW: u32 = 30 * 8 * 8 * 4;
const TOP_TILE_ROWS: u32 = 400 / 8;
const REFLECTION_TILE_ROW: u32 = 32 * 8 * 8 * 4;
/// libctru's `GX_CMDLIST_FLUSH`: flush only the command list. Without it,
/// `C3D_FrameEnd` flushes the whole linear heap from the data cache every
/// frame (milliseconds on the ARM11), so what the GPU reads is flushed by
/// hand instead: the pages once at load, the vertices each frame.
const GX_CMDLIST_FLUSH: u8 = 1 << 1;

#[repr(C)]
#[derive(Clone, Copy)]
struct Vertex {
    /// x, y, draw order and stereo depth.
    pos: [f32; 4],
    uv: [f32; 2],
    color: [u8; 4],
    /// Pixel coordinates in the cloud exposure atlas.
    cloud_uv: [u16; 2],
}

type Quad = [Vertex; 4];

/// What a run of quads is drawn with.
#[derive(Clone, Copy, PartialEq, Eq)]
struct State {
    /// Texture page, or untextured.
    page: Option<u16>,
    additive: bool,
    /// Linear filtering (scaled glows).
    smooth: bool,
    effect: Effect,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Effect {
    None,
    /// Unlit overlay RGB and interpolation alpha, packed RGBA8.
    Overlay(u32),
    /// Unlit cloud highlight RGB, packed RGBA8.
    Cloud(u32),
    /// Cloud highlight with the per-texel exposure in texture unit 1.
    ShadowedCloud(u32),
}

/// A step of a screen's drawing, kept to replay for the right eye.
#[derive(Clone, Copy)]
enum Call {
    /// Depth test for the opaque pass (`true`) or the translucent one.
    Pass(bool),
    Draw(State, usize, usize),
}

pub struct Screen {
    target: *mut c3d::C3D_RenderTarget,
    projection: c3d::C3D_Mtx,
}

pub struct Gpu {
    top: Screen,
    /// The top screen's right eye.
    right: Screen,
    bottom: Screen,
    program: Box<ctru::shaderProgram_s>,
    projection_loc: i32,
    eye_loc: i32,
    /// Eye separation for the farthest things, in pixels (0 = no 3D).
    pub stereo: f32,
    /// Depth given to quads from here on (see `Renderer::depth`).
    depth: f32,
    calls: Vec<Call>,
    pages: Vec<Box<c3d::C3D_Tex>>,
    page_size: Vec<(f32, f32)>,
    /// Pages with 1-bit alpha (their sprites can go in the opaque pass).
    page_hard: Vec<bool>,
    cloud_shadow: cloud_shadow::Pass,
    cloud_exposure: Option<Box<c3d::C3D_Tex>>,
    pub sheet: Rc<Sheet>,
    verts: *mut Vertex,
    indices: *mut u16,
    /// Quads written to `verts` this frame.
    quads: usize,
    /// This screen's opaque items, by page and as fills, copied into
    /// `verts` when the screen is flushed.
    opaque: Vec<Vec<Quad>>,
    solid: Vec<Quad>,
    /// The rest, already written into `verts` in painter's order: runs of
    /// (state, first quad, quads) drawn after the opaque pass.
    runs: Vec<(State, usize, usize)>,
    /// Items so far on this screen (their depth).
    order: u32,
    /// Plain painter's order, no depth (X).
    pub painter: bool,
    bound_page: Option<(u16, bool)>,
    bound_textured: Option<Env>,
    bound_effect: Option<Effect>,
    bound_additive: Option<bool>,
    /// Draw calls this frame.
    pub draws: u32,
    /// The page holding last frame's top screen, once one has been copied.
    reflection: Option<[u16; 2]>,
    reflection_ready: [bool; 2],
    /// Offscreen copy used to composite reflection-only geometry.
    reflection_scratch: Option<Screen>,
    /// Drawing the top screen.
    on_top: bool,
    /// How screen pixels map to reflection texels (Y cycles them, in case
    /// the copy comes out flipped on hardware).
    pub reflect_mode: u8,
}

/// How a quad's colour is made.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Env {
    Solid,
    Textured,
    ShadowedCloud,
    /// Texture colour, vertex alpha (the copied screen's alpha is unused).
    Reflection,
}

fn color(c: Color) -> [u8; 4] {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    [b(c.r), b(c.g), b(c.b), b(c.a)]
}

fn clear_color(c: Color) -> u32 {
    let [r, g, b, a] = color(c);
    u32::from_be_bytes([r, g, b, a])
}

/// PICA200 stores 8×8 tiles left to right, starting at the texture's top row.
const fn morton8(x: usize, y: usize) -> usize {
    (x & 1) | ((y & 1) << 1) | ((x & 2) << 1) | ((y & 2) << 2) | ((x & 4) << 2) | ((y & 4) << 3)
}

unsafe fn insert_cloud_alpha(
    pass: &mut cloud_shadow::Pass,
    id: u32,
    sprite: &scene::sheet::Sprite,
    tex: &mut c3d::C3D_Tex,
) -> Result<(), String> {
    let dims = tex.__bindgen_anon_2.__bindgen_anon_1;
    let (width, height) = (usize::from(dims.width), usize::from(dims.height));
    let (left, top) = (usize::from(sprite.x), usize::from(sprite.y));
    let right = left + usize::from(sprite.w);
    let bottom = top + usize::from(sprite.h);
    let mut bytes = 0;
    let data = c3d::C3D_Tex2DGetImagePtr(tex, 0, &mut bytes).cast::<u8>();
    if tex.fmt() != ctru::GPU_RGBA8
        || right > width
        || bottom > height
        || width % 8 != 0
        || height % 8 != 0
        || width.checked_mul(height).and_then(|n| n.checked_mul(4)) != Some(bytes as usize)
        || data.is_null()
    {
        return Err(format!("cloud sprite {id} has an invalid RGBA8 texture"));
    }
    let data = std::slice::from_raw_parts(data, bytes as usize);
    // RGBA8 is stored as A, B, G, R. Recover the baked cloud alpha once.
    let alpha = (top..bottom).flat_map(|y| {
        (left..right).map(move |x| {
            let texel = (y & !7) * width + (x & !7) * 8 + morton8(x, y);
            data[texel * 4]
        })
    });
    pass.insert(id, sprite, alpha);
    Ok(())
}

/// Offsets into eight input rows in GPU tile order.
const CLOUD_TILE_OFFSETS: [usize; 64] = {
    let mut offsets = [0; 64];
    let mut y = 0;
    while y < 8 {
        let mut x = 0;
        while x < 8 {
            offsets[morton8(x, y)] = y * ATLAS_WIDTH as usize + x;
            x += 1;
        }
        y += 1;
    }
    offsets
};

unsafe fn upload_cloud_exposure(tex: &mut c3d::C3D_Tex, atlas: cloud_shadow::Atlas<'_>) {
    let width = usize::from(ATLAS_WIDTH);
    let total = width * usize::from(ATLAS_HEIGHT);
    let active = width * usize::from(atlas.height);
    let data = std::slice::from_raw_parts_mut(tex.__bindgen_anon_1.data.cast::<u8>(), total);
    let data = &mut data[..active];
    for (rows, tiles) in atlas
        .pixels
        .chunks_exact(width * 8)
        .zip(data.chunks_exact_mut(width * 8))
    {
        for (tile_x, tile) in tiles.chunks_exact_mut(64).enumerate() {
            let source = &rows[tile_x * 8..];
            for (texel, &offset) in tile.iter_mut().zip(&CLOUD_TILE_OFFSETS) {
                *texel = source[offset];
            }
        }
    }
    let _ = ctru::GSPGPU_FlushDataCache(data.as_ptr().cast(), active as u32);
    // Rebind after the CPU write to invalidate the GPU's texture cache.
    c3d::C3D_TexBind(1, tex);
}

impl Screen {
    unsafe fn offscreen(w: f32, h: f32) -> Option<Screen> {
        let target = c3d::C3D_RenderTargetCreate(
            h as i32,
            w as i32,
            ctru::GPU_RB_RGBA8,
            c3d::C3D_DEPTHTYPE {
                __e: ctru::GPU_RB_DEPTH24,
            },
        );
        if target.is_null() {
            return None;
        }
        let mut projection = std::mem::zeroed();
        c3d::Mtx_OrthoTilt(&mut projection, 0.0, w, h, 0.0, 1.0, -1.0, true);
        Some(Screen { target, projection })
    }

    unsafe fn new(screen: ctru::gfxScreen_t, side: ctru::gfx3dSide_t, w: f32, h: f32) -> Screen {
        // The screens are mounted sideways: targets are 240 wide.
        let target = c3d::C3D_RenderTargetCreate(
            240,
            w as i32,
            ctru::GPU_RB_RGBA8,
            c3d::C3D_DEPTHTYPE {
                __e: ctru::GPU_RB_DEPTH24,
            },
        );
        assert!(!target.is_null(), "3DS render target allocation failed");
        let flags = c3d::GX_TRANSFER_FLIP_VERT(false)
            | c3d::GX_TRANSFER_OUT_TILED(false)
            | c3d::GX_TRANSFER_RAW_COPY(false)
            | c3d::GX_TRANSFER_IN_FORMAT(ctru::GX_TRANSFER_FMT_RGBA8)
            | c3d::GX_TRANSFER_OUT_FORMAT(ctru::GX_TRANSFER_FMT_RGB8)
            | c3d::GX_TRANSFER_SCALING(ctru::GX_TRANSFER_SCALE_NO);
        c3d::C3D_RenderTargetSetOutput(target, screen, side, flags);
        let mut projection = std::mem::zeroed();
        // Pixels, y down, from the top-left. z in -1..1 maps to depth 0..1
        // (later items are nearer).
        c3d::Mtx_OrthoTilt(&mut projection, 0.0, w, h, 0.0, 1.0, -1.0, true);
        Screen { target, projection }
    }
}

impl Gpu {
    /// Sets up citro3d (after `Gfx::new`) and loads the sheet's pages from
    /// `dir` (romfs).
    pub fn new(
        sheet: Rc<Sheet>,
        dir: &str,
        cloud_ids: impl Iterator<Item = u32>,
    ) -> Result<Gpu, String> {
        unsafe {
            if !c3d::C3D_Init(c3d::C3D_DEFAULT_CMDBUF_SIZE as usize) {
                return Err("citro3d failed to start".into());
            }
            let top = Screen::new(ctru::GFX_TOP, ctru::GFX_LEFT, 400.0, 240.0);
            let right = Screen::new(ctru::GFX_TOP, ctru::GFX_RIGHT, 400.0, 240.0);
            let bottom = Screen::new(ctru::GFX_BOTTOM, ctru::GFX_LEFT, 320.0, 240.0);

            // The shader. DVLB_ParseFile wants a word-aligned, writable copy.
            let words: Vec<u32> = SHADER
                .chunks(4)
                .map(|c| {
                    let mut b = [0u8; 4];
                    b[..c.len()].copy_from_slice(c);
                    u32::from_le_bytes(b)
                })
                .collect();
            let words = Box::leak(words.into_boxed_slice());
            let dvlb = ctru::DVLB_ParseFile(words.as_mut_ptr(), words.len() as u32 * 4);
            if dvlb.is_null() {
                return Err("bad shader".into());
            }
            let mut program: Box<ctru::shaderProgram_s> = Box::new(std::mem::zeroed());
            if ctru::shaderProgramInit(program.as_mut()) < 0 {
                return Err("shader program initialization failed".into());
            }
            if ctru::shaderProgramSetVsh(program.as_mut(), (*dvlb).DVLE) < 0 {
                return Err("vertex shader setup failed".into());
            }
            let projection_loc = ctru::shaderInstanceGetUniformLocation(
                program.vertexShader,
                c"projection".as_ptr(),
            ) as i32;
            let eye_loc =
                ctru::shaderInstanceGetUniformLocation(program.vertexShader, c"eye".as_ptr())
                    as i32;

            let mut pages = Vec::new();
            let mut page_size = Vec::new();
            for p in &sheet.pages {
                let path = format!("{dir}/{}", p.file);
                let data = std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?;
                let mut tex: Box<c3d::C3D_Tex> = Box::new(std::mem::zeroed());
                let t3x = c3d::Tex3DS_TextureImport(
                    data.as_ptr() as _,
                    data.len(),
                    tex.as_mut(),
                    std::ptr::null_mut(),
                    false,
                );
                if t3x.is_null() {
                    return Err(format!("{path}: not a texture (out of memory?)"));
                }
                c3d::Tex3DS_TextureFree(t3x);
                // Imported into linear memory by the CPU (see GX_CMDLIST_FLUSH).
                c3d::C3D_TexFlush(tex.as_mut());
                c3d::C3D_TexSetFilter(tex.as_mut(), ctru::GPU_NEAREST, ctru::GPU_NEAREST);
                page_size.push((p.w as f32, p.h as f32));
                pages.push(tex);
            }
            let mut page_hard: Vec<bool> = sheet
                .pages
                .iter()
                .map(|p| p.format == PageFormat::Rgba5551)
                .collect();
            let mut cloud_shadow = cloud_shadow::Pass::default();
            let mut cloud_ids = cloud_ids.peekable();
            let cloud_exposure = if cloud_ids.peek().is_some() {
                for id in cloud_ids {
                    let sprite = sheet.get(id);
                    insert_cloud_alpha(
                        &mut cloud_shadow,
                        id,
                        sprite,
                        pages[usize::from(sprite.page)].as_mut(),
                    )?;
                }
                let mut tex: Box<c3d::C3D_Tex> = Box::new(std::mem::zeroed());
                if c3d::C3D_TexInit(tex.as_mut(), ATLAS_WIDTH, ATLAS_HEIGHT, ctru::GPU_A8) {
                    tex.__bindgen_anon_1
                        .data
                        .cast::<u8>()
                        .write_bytes(255, usize::from(ATLAS_WIDTH) * usize::from(ATLAS_HEIGHT));
                    c3d::C3D_TexFlush(tex.as_mut());
                    c3d::C3D_TexSetFilter(tex.as_mut(), ctru::GPU_NEAREST, ctru::GPU_NEAREST);
                    c3d::C3D_TexSetWrap(
                        tex.as_mut(),
                        ctru::GPU_CLAMP_TO_EDGE,
                        ctru::GPU_CLAMP_TO_EDGE,
                    );
                    Some(tex)
                } else {
                    eprintln!("cloud shadows disabled: insufficient texture memory");
                    None
                }
            } else {
                None
            };
            // Separate retained captures for each eye; never reuse the left
            // image for right-eye water. A failed allocation disables reflections.
            let mut reflection_textures = Vec::new();
            for _ in 0..2 {
                let mut tex: Box<c3d::C3D_Tex> = Box::new(std::mem::zeroed());
                if !c3d::C3D_TexInitVRAM(tex.as_mut(), 256, 512, ctru::GPU_RGBA8) {
                    break;
                }
                c3d::C3D_TexSetFilter(tex.as_mut(), ctru::GPU_NEAREST, ctru::GPU_NEAREST);
                c3d::C3D_TexSetWrap(
                    tex.as_mut(),
                    ctru::GPU_CLAMP_TO_EDGE,
                    ctru::GPU_CLAMP_TO_EDGE,
                );
                reflection_textures.push(tex);
            }
            let mut reflection_scratch = Screen::offscreen(400.0, 240.0);
            let reflection = if reflection_textures.len() == 2 && reflection_scratch.is_some() {
                let ids = [pages.len() as u16, pages.len() as u16 + 1];
                for tex in reflection_textures {
                    pages.push(tex);
                    page_size.push((256.0, 512.0));
                    page_hard.push(true);
                }
                Some(ids)
            } else {
                for mut tex in reflection_textures {
                    c3d::C3D_TexDelete(tex.as_mut());
                }
                if let Some(scratch) = reflection_scratch.take() {
                    c3d::C3D_RenderTargetDelete(scratch.target);
                }
                eprintln!("water reflections disabled: insufficient render-target memory");
                None
            };

            let verts =
                ctru::linearAlloc(MAX_QUADS * 4 * std::mem::size_of::<Vertex>()) as *mut Vertex;
            let indices = ctru::linearAlloc(MAX_QUADS * 6 * 2) as *mut u16;
            if verts.is_null() || indices.is_null() {
                return Err("out of linear memory for vertices".into());
            }
            // Quad q uses vertices 4q..4q+3 as two triangles.
            for q in 0..MAX_QUADS {
                let v = (q * 4) as u16;
                for (k, i) in [0u16, 1, 2, 0, 2, 3].into_iter().enumerate() {
                    indices.add(q * 6 + k).write(v.wrapping_add(i));
                }
            }
            let _ = ctru::GSPGPU_FlushDataCache(indices as *const _, (MAX_QUADS * 12) as u32);
            let n = pages.len();
            Ok(Gpu {
                top,
                right,
                bottom,
                program,
                projection_loc,
                eye_loc,
                stereo: 0.0,
                depth: 0.0,
                calls: Vec::new(),
                pages,
                page_size,
                page_hard,
                cloud_shadow,
                cloud_exposure,
                sheet,
                verts,
                indices,
                quads: 0,
                opaque: vec![Vec::new(); n],
                solid: Vec::new(),
                runs: Vec::new(),
                order: 0,
                painter: false,
                bound_page: None,
                bound_textured: None,
                bound_effect: None,
                bound_additive: None,
                draws: 0,
                reflection,
                reflection_ready: [false; 2],
                reflection_scratch,
                on_top: false,
                reflect_mode: 0,
            })
        }
    }

    /// Starts a frame (waits for the last one to finish drawing).
    pub fn begin(&mut self) {
        unsafe {
            c3d::C3D_FrameBegin(c3d::C3D_FRAME_SYNCDRAW as u8);
            c3d::C3D_BindProgram(self.program.as_mut());
            let attr = c3d::C3D_GetAttrInfo();
            c3d::AttrInfo_Init(attr);
            c3d::AttrInfo_AddLoader(attr, 0, ctru::GPU_FLOAT, 4);
            c3d::AttrInfo_AddLoader(attr, 1, ctru::GPU_FLOAT, 2);
            c3d::AttrInfo_AddLoader(attr, 2, ctru::GPU_UNSIGNED_BYTE, 4);
            c3d::AttrInfo_AddLoader(attr, 3, ctru::GPU_SHORT, 2);
            let buf = c3d::C3D_GetBufInfo();
            c3d::BufInfo_Init(buf);
            c3d::BufInfo_Add(
                buf,
                self.verts as *const _,
                std::mem::size_of::<Vertex>() as isize,
                4,
                0x3210,
            );
            c3d::C3D_CullFace(ctru::GPU_CULL_NONE);
            // Clear pixels never count.
            c3d::C3D_AlphaTest(true, ctru::GPU_GREATER, 0);
        }
        self.quads = 0;
        self.draws = 0;
        self.unbind();
    }

    fn unbind(&mut self) {
        self.bound_page = None;
        self.bound_textured = None;
        self.bound_effect = None;
        self.bound_additive = None;
    }

    /// Vertices queued this frame.
    pub fn vertices(&self) -> usize {
        self.quads * 4
    }

    /// Draws on the top screen (`true`) or the touch screen from here on.
    pub fn screen(&mut self, top: bool) {
        self.flush();
        if self.on_top && !top {
            self.capture_top();
        }
        self.on_top = top;
        self.depth = 0.0;
        let s = if top { &self.top } else { &self.bottom };
        unsafe {
            c3d::C3D_RenderTargetClear(s.target, c3d::C3D_CLEAR_ALL, clear_color(Color::BLACK), 0);
            c3d::C3D_FrameDrawOn(s.target);
            c3d::C3D_FVUnifMtx4x4(ctru::GPU_VERTEX_SHADER, self.projection_loc, &s.projection);
            // Left eye: things behind the screen shift left.
            let eye = if top { -self.stereo / 2.0 } else { 0.0 };
            c3d::C3D_FVUnifSet(ctru::GPU_VERTEX_SHADER, self.eye_loc, eye, 0.0, 0.0, 0.0);
        }
        self.unbind();
    }

    /// Draws the top screen's calls again for the right eye.
    fn draw_right_eye(&mut self) {
        let calls = std::mem::take(&mut self.calls);
        unsafe {
            c3d::C3D_RenderTargetClear(
                self.right.target,
                c3d::C3D_CLEAR_ALL,
                clear_color(Color::BLACK),
                0,
            );
            c3d::C3D_FrameDrawOn(self.right.target);
            c3d::C3D_FVUnifMtx4x4(
                ctru::GPU_VERTEX_SHADER,
                self.projection_loc,
                &self.right.projection,
            );
            c3d::C3D_FVUnifSet(
                ctru::GPU_VERTEX_SHADER,
                self.eye_loc,
                self.stereo / 2.0,
                0.0,
                0.0,
                0.0,
            );
        }
        self.unbind();
        for &c in &calls {
            match c {
                Call::Pass(opaque) => self.depth_test(opaque),
                Call::Draw(mut state, first, n) => unsafe {
                    if let (Some(page), Some(pages)) = (state.page, self.reflection) {
                        let Some(page) = scene::render::reflection_page_for_right(
                            page,
                            pages,
                            self.reflection_ready[1],
                        ) else {
                            continue;
                        };
                        state.page = Some(page);
                    }
                    self.bind(state);
                    c3d::C3D_DrawElements(
                        ctru::GPU_TRIANGLES,
                        (n * 6) as i32,
                        c3d::C3D_UNSIGNED_SHORT as i32,
                        self.indices.add(first * 6) as *const _,
                    );
                    self.draws += 1;
                },
            }
        }
        self.calls = calls;
        self.unbind();
    }

    fn depth_test(&mut self, opaque: bool) {
        unsafe {
            if self.painter {
                c3d::C3D_DepthTest(false, ctru::GPU_ALWAYS, ctru::GPU_WRITE_COLOR);
            } else if opaque {
                c3d::C3D_DepthTest(true, ctru::GPU_GREATER, ctru::GPU_WRITE_ALL);
            } else {
                c3d::C3D_DepthTest(true, ctru::GPU_GREATER, ctru::GPU_WRITE_COLOR);
            }
        }
    }

    fn submit_before_copy(&self) {
        unsafe {
            let _ = ctru::GSPGPU_FlushDataCache(
                self.verts as *const _,
                (self.quads * 4 * std::mem::size_of::<Vertex>()) as u32,
            );
            c3d::C3D_FrameSplit(GX_CMDLIST_FLUSH);
        }
    }

    /// PC MainCamera.OnPostRender: copy the visible scene, then render the
    /// reflection camera's extra layer. The visible framebuffer is untouched.
    pub fn capture_reflections(&mut self, extras: impl FnMut(&mut Self)) {
        if !self.on_top {
            return;
        }
        self.flush(); // includes right-eye replay while on_top is true
        self.capture_reflections_after_flush(extras);
    }

    fn capture_reflections_after_flush(&mut self, mut extras: impl FnMut(&mut Self)) {
        self.on_top = false;
        let (Some(pages), Some(scratch)) = (self.reflection, &self.reflection_scratch) else {
            return;
        };
        let scratch_target = scratch.target;
        let projection = scratch.projection;
        let dim = |line: u32, gap: u32| ((gap / 16) << 16) | (line / 16);
        let eyes = if self.stereo > 0.0 { 2 } else { 1 };
        if eyes == 1 {
            self.reflection_ready[1] = false;
        }
        for (eye, page) in pages.into_iter().enumerate().take(eyes) {
            self.reflection_ready[eye] = false;
            let source = if eye == 0 {
                self.top.target
            } else {
                self.right.target
            };
            unsafe {
                c3d::C3D_RenderTargetClear(scratch_target, c3d::C3D_CLEAR_DEPTH, 0, 0);
                self.submit_before_copy();
                let result = ctru::GX_TextureCopy(
                    (*source).frameBuf.colorBuf as *mut u32,
                    dim(TOP_TILE_ROW, 0),
                    (*scratch_target).frameBuf.colorBuf as *mut u32,
                    dim(TOP_TILE_ROW, 0),
                    TOP_TILE_ROW * TOP_TILE_ROWS,
                    c3d::GX_TRANSFER_RAW_COPY(true),
                );
                if result < 0 {
                    continue;
                }
                c3d::C3D_FrameDrawOn(scratch_target);
                c3d::C3D_FVUnifMtx4x4(ctru::GPU_VERTEX_SHADER, self.projection_loc, &projection);
                let shift = if eye == 0 {
                    -self.stereo / 2.0
                } else {
                    self.stereo / 2.0
                };
                c3d::C3D_FVUnifSet(ctru::GPU_VERTEX_SHADER, self.eye_loc, shift, 0.0, 0.0, 0.0);
            }
            self.depth = 0.0;
            self.unbind();
            extras(self);
            self.flush();
            self.submit_before_copy();
            let result = unsafe {
                ctru::GX_TextureCopy(
                    (*scratch_target).frameBuf.colorBuf as *mut u32,
                    dim(TOP_TILE_ROW, 0),
                    self.pages[page as usize].__bindgen_anon_1.data as *mut u32,
                    dim(TOP_TILE_ROW, REFLECTION_TILE_ROW - TOP_TILE_ROW),
                    TOP_TILE_ROW * TOP_TILE_ROWS,
                    c3d::GX_TRANSFER_RAW_COPY(true),
                )
            };
            self.reflection_ready[eye] = result >= 0;
        }
        self.unbind();
    }

    fn capture_top(&mut self) {
        // screen()/end() have already flushed. Repeating flush here would
        // replay an empty right-eye list and clear its finished framebuffer.
        self.capture_reflections_after_flush(|_| {});
    }

    /// GPU time of the last finished frame, in ms.
    pub fn gpu_ms(&self) -> f32 {
        unsafe { c3d::C3D_GetDrawingTime() }
    }

    /// Ends the frame; the GPU draws it while the next one is prepared.
    pub fn end(&mut self) {
        self.flush();
        if self.on_top {
            self.capture_top();
            self.on_top = false;
        }
        unsafe {
            let _ = ctru::GSPGPU_FlushDataCache(
                self.verts as *const _,
                (self.quads * 4 * std::mem::size_of::<Vertex>()) as u32,
            );
            c3d::C3D_FrameEnd(GX_CMDLIST_FLUSH);
        }
    }

    /// Sets the GPU up for `state` (only what changed).
    unsafe fn bind(&mut self, state: State) {
        let mode = match state.page {
            None => Env::Solid,
            Some(p) if self.reflection.is_some_and(|pages| pages.contains(&p)) => Env::Reflection,
            Some(_) if matches!(state.effect, Effect::ShadowedCloud(_)) => Env::ShadowedCloud,
            Some(_) => Env::Textured,
        };
        if self.bound_textured != Some(mode) {
            let env = c3d::C3D_GetTexEnv(0);
            c3d::C3D_TexEnvInit(env);
            match mode {
                Env::Textured | Env::ShadowedCloud => {
                    c3d::C3D_TexEnvSrc(
                        env,
                        c3d::C3D_Both,
                        ctru::GPU_TEXTURE0,
                        ctru::GPU_PRIMARY_COLOR,
                        0,
                    );
                    c3d::C3D_TexEnvFunc(env, c3d::C3D_Both, ctru::GPU_MODULATE);
                    if mode == Env::ShadowedCloud {
                        // Keep coverage × fade in alpha while RGB builds the rim.
                        c3d::C3D_TexEnvSrc(
                            env,
                            c3d::C3D_RGB,
                            ctru::GPU_CONSTANT,
                            ctru::GPU_TEXTURE0,
                            0,
                        );
                        c3d::C3D_TexEnvOpRgb(
                            env,
                            ctru::GPU_TEVOP_RGB_SRC_COLOR,
                            ctru::GPU_TEVOP_RGB_ONE_MINUS_SRC_ALPHA,
                            ctru::GPU_TEVOP_RGB_SRC_COLOR,
                        );
                    }
                }
                Env::Reflection => {
                    c3d::C3D_TexEnvSrc(
                        env,
                        c3d::C3D_RGB,
                        ctru::GPU_TEXTURE0,
                        ctru::GPU_PRIMARY_COLOR,
                        0,
                    );
                    c3d::C3D_TexEnvFunc(env, c3d::C3D_RGB, ctru::GPU_MODULATE);
                    c3d::C3D_TexEnvSrc(env, c3d::C3D_Alpha, ctru::GPU_PRIMARY_COLOR, 0, 0);
                    c3d::C3D_TexEnvFunc(env, c3d::C3D_Alpha, ctru::GPU_REPLACE);
                }
                Env::Solid => {
                    c3d::C3D_TexEnvSrc(env, c3d::C3D_Both, ctru::GPU_PRIMARY_COLOR, 0, 0);
                    c3d::C3D_TexEnvFunc(env, c3d::C3D_Both, ctru::GPU_REPLACE);
                }
            }
            self.bound_textured = Some(mode);
        }
        if self.bound_effect != Some(state.effect) {
            let env = c3d::C3D_GetTexEnv(1);
            let tail = c3d::C3D_GetTexEnv(2);
            // Every effect keeps the alpha computed in stage 0.
            c3d::C3D_TexEnvInit(env);
            c3d::C3D_TexEnvInit(tail);
            match state.effect {
                Effect::None => {}
                Effect::Overlay(overlay) => {
                    c3d::C3D_TexEnvSrc(
                        env,
                        c3d::C3D_RGB,
                        ctru::GPU_CONSTANT,
                        ctru::GPU_PREVIOUS,
                        ctru::GPU_CONSTANT,
                    );
                    c3d::C3D_TexEnvOpRgb(
                        env,
                        ctru::GPU_TEVOP_RGB_SRC_COLOR,
                        ctru::GPU_TEVOP_RGB_SRC_COLOR,
                        ctru::GPU_TEVOP_RGB_SRC_ALPHA,
                    );
                    c3d::C3D_TexEnvFunc(env, c3d::C3D_RGB, ctru::GPU_INTERPOLATE);
                    c3d::C3D_TexEnvColor(env, overlay);
                }
                Effect::Cloud(light) => {
                    // Highlight × (1 − baked coverage) + the lit base.
                    c3d::C3D_TexEnvSrc(
                        env,
                        c3d::C3D_RGB,
                        ctru::GPU_CONSTANT,
                        ctru::GPU_TEXTURE0,
                        ctru::GPU_PREVIOUS,
                    );
                    c3d::C3D_TexEnvOpRgb(
                        env,
                        ctru::GPU_TEVOP_RGB_SRC_COLOR,
                        ctru::GPU_TEVOP_RGB_ONE_MINUS_SRC_ALPHA,
                        ctru::GPU_TEVOP_RGB_SRC_COLOR,
                    );
                    c3d::C3D_TexEnvFunc(env, c3d::C3D_RGB, ctru::GPU_MULTIPLY_ADD);
                    c3d::C3D_TexEnvColor(env, light);
                }
                Effect::ShadowedCloud(light) => {
                    c3d::C3D_TexEnvColor(c3d::C3D_GetTexEnv(0), light);
                    // Stage 1 gates the rim with the per-texel exposure.
                    c3d::C3D_TexEnvSrc(
                        env,
                        c3d::C3D_RGB,
                        ctru::GPU_PREVIOUS,
                        ctru::GPU_TEXTURE1,
                        0,
                    );
                    c3d::C3D_TexEnvOpRgb(
                        env,
                        ctru::GPU_TEVOP_RGB_SRC_COLOR,
                        ctru::GPU_TEVOP_RGB_SRC_ALPHA,
                        ctru::GPU_TEVOP_RGB_SRC_COLOR,
                    );
                    c3d::C3D_TexEnvFunc(env, c3d::C3D_RGB, ctru::GPU_MODULATE);
                    // Stage 2 adds the same ambient-lit base as ordinary clouds.
                    c3d::C3D_TexEnvSrc(
                        tail,
                        c3d::C3D_RGB,
                        ctru::GPU_TEXTURE0,
                        ctru::GPU_PRIMARY_COLOR,
                        ctru::GPU_PREVIOUS,
                    );
                    c3d::C3D_TexEnvFunc(tail, c3d::C3D_RGB, ctru::GPU_MULTIPLY_ADD);
                }
            }
            self.bound_effect = Some(state.effect);
        }
        if let Some(p) = state.page {
            if self.bound_page != Some((p, state.smooth)) {
                let tex = self.pages[p as usize].as_mut();
                let f = if state.smooth {
                    ctru::GPU_LINEAR
                } else {
                    ctru::GPU_NEAREST
                };
                c3d::C3D_TexSetFilter(tex, f, f);
                c3d::C3D_TexBind(0, tex);
                self.bound_page = Some((p, state.smooth));
            }
        }
        if self.bound_additive != Some(state.additive) {
            let dst = if state.additive {
                ctru::GPU_ONE
            } else {
                ctru::GPU_ONE_MINUS_SRC_ALPHA
            };
            c3d::C3D_AlphaBlend(
                ctru::GPU_BLEND_ADD,
                ctru::GPU_BLEND_ADD,
                ctru::GPU_SRC_ALPHA,
                dst,
                ctru::GPU_ZERO,
                ctru::GPU_ONE,
            );
            self.bound_additive = Some(state.additive);
        }
    }

    /// Copies `quads` into the frame's vertex buffer and draws them with `state`.
    fn draw(&mut self, state: State, quads: &[Quad]) {
        let n = quads.len().min(MAX_QUADS - self.quads);
        if n == 0 {
            return;
        }
        let first = self.quads;
        unsafe {
            std::ptr::copy_nonoverlapping(
                quads.as_ptr() as *const Vertex,
                self.verts.add(first * 4),
                n * 4,
            );
        }
        self.quads += n;
        self.draw_range(state, first, n);
    }

    /// Draws quads `first..first + n` of the vertex buffer with `state`.
    fn draw_range(&mut self, state: State, first: usize, n: usize) {
        unsafe {
            self.bind(state);
            c3d::C3D_DrawElements(
                ctru::GPU_TRIANGLES,
                (n * 6) as i32,
                c3d::C3D_UNSIGNED_SHORT as i32,
                self.indices.add(first * 6) as *const _,
            );
        }
        self.draws += 1;
        self.calls.push(Call::Draw(state, first, n));
    }

    /// Draws this screen's items: the opaque pass by page, then the rest in order.
    fn flush(&mut self) {
        self.calls.clear();
        self.depth_test(true);
        self.calls.push(Call::Pass(true));
        let solid = std::mem::take(&mut self.solid);
        self.draw(
            State {
                page: None,
                additive: false,
                smooth: false,
                effect: Effect::None,
            },
            &solid,
        );
        self.solid = solid;
        self.solid.clear();
        for p in 0..self.opaque.len() {
            let quads = std::mem::take(&mut self.opaque[p]);
            self.draw(
                State {
                    page: Some(p as u16),
                    additive: false,
                    smooth: false,
                    effect: Effect::None,
                },
                &quads,
            );
            self.opaque[p] = quads;
            self.opaque[p].clear();
        }
        // The translucent rest: tested against the opaque pass, not written.
        self.depth_test(false);
        self.calls.push(Call::Pass(false));
        let runs = std::mem::take(&mut self.runs);
        for &(state, first, n) in &runs {
            self.draw_range(state, first, n);
        }
        self.runs = runs;
        self.runs.clear();
        self.order = 0;
        if self.on_top && self.stereo > 0.0 {
            self.draw_right_eye();
        }
    }

    /// Queues a quad (corners clockwise from top-left) in its pass.
    fn quad(
        &mut self,
        state: State,
        corners: [(f32, f32); 4],
        uv: [(f32, f32); 4],
        colors: [Color; 4],
    ) {
        self.quad_with_shadow(state, corners, uv, colors, [[0; 2]; 4]);
    }

    fn quad_with_shadow(
        &mut self,
        state: State,
        corners: [(f32, f32); 4],
        uv: [(f32, f32); 4],
        colors: [Color; 4],
        shadow_uv: [[u16; 2]; 4],
    ) {
        let z = -0.999 + self.order as f32 * Z_STEP;
        self.order += 1;
        let q: Quad = [0, 1, 2, 3].map(|i| Vertex {
            pos: [corners[i].0, corners[i].1, z, self.depth],
            uv: [uv[i].0, uv[i].1],
            color: color(colors[i]),
            cloud_uv: shadow_uv[i],
        });
        let opaque = !self.painter
            && !state.additive
            // Page-only opaque buckets cannot retain a per-sprite TEV constant.
            && state.effect == Effect::None
            && colors.iter().all(|c| c.a >= 0.999)
            && state.page.is_none_or(|p| self.page_hard[p as usize]);
        if opaque {
            match state.page {
                Some(p) => self.opaque[p as usize].push(q),
                None => self.solid.push(q),
            }
            return;
        }
        // In painter's order, so straight into the vertex buffer; a quad
        // with the same state as the one before extends its run.
        if self.quads >= MAX_QUADS {
            return;
        }
        let i = self.quads;
        unsafe { (self.verts.add(i * 4) as *mut Quad).write(q) };
        self.quads += 1;
        match self.runs.last_mut() {
            Some((s, first, n)) if *s == state && *first + *n == i => *n += 1,
            _ => self.runs.push((state, i, 1)),
        }
    }

    /// One sprite image (no recolour) at `at`.
    fn sprite_part(&mut self, id: u32, at: (f32, f32), d: &Draw, tint: Color) {
        let s = *self.sheet.get(id);
        let (pw, ph) = self.page_size[s.page as usize];
        let corners = scene::render::sprite_corners(&s, at, d);
        // Texture v runs bottom-up.
        let (u0, u1) = (s.x as f32 / pw, (s.x + s.w) as f32 / pw);
        let (v0, v1) = (1.0 - s.y as f32 / ph, 1.0 - (s.y + s.h) as f32 / ph);
        let (ul, ur) = if d.flip { (u1, u0) } else { (u0, u1) };
        let (vt, vb) = if d.flip_y { (v1, v0) } else { (v0, v1) };
        let uv = [(ul, vt), (ur, vt), (ur, vb), (ul, vb)];
        let shadow = d.cloud_shadow.filter(|&[x, y]| {
            self.cloud_exposure.is_some()
                && x <= ATLAS_WIDTH.saturating_sub(s.w)
                && y <= ATLAS_HEIGHT.saturating_sub(s.h)
                && s.w <= ATLAS_WIDTH
                && s.h <= ATLAS_HEIGHT
        });
        let effect = if let Some(light) = d.cloud_light {
            let light = u32::from_le_bytes(color(light));
            if shadow.is_some() {
                Effect::ShadowedCloud(light)
            } else {
                Effect::Cloud(light)
            }
        } else if let Some(overlay) = d.overlay.filter(|c| c.a > 0.0) {
            // The PowerSprite2 overlay follows recoloring and lighting.
            Effect::Overlay(u32::from_le_bytes(color(overlay)))
        } else {
            Effect::None
        };
        let state = State {
            page: Some(s.page),
            additive: d.additive && d.cloud_light.is_none(),
            effect,
            smooth: d.smooth
                && d.cloud_light.is_none()
                && (d.scale != 1.0 || d.angle != 0.0 || d.affine != [1.0, 0.0, 0.0, 1.0]),
        };
        let shadow_uv = shadow.map_or([[0; 2]; 4], |[x, y]| {
            let (left, right) = if d.flip { (x + s.w, x) } else { (x, x + s.w) };
            let (top, bottom) = (ATLAS_HEIGHT - y, ATLAS_HEIGHT - y - s.h);
            let (top, bottom) = if d.flip_y {
                (bottom, top)
            } else {
                (top, bottom)
            };
            [[left, top], [right, top], [right, bottom], [left, bottom]]
        });
        self.quad_with_shadow(state, corners, uv, [tint; 4], shadow_uv);
    }

    fn solid_rect(&mut self, r: Rect, top: Color, bottom: Color, additive: bool) {
        let corners = [
            (r.x, r.y),
            (r.x + r.w, r.y),
            (r.x + r.w, r.y + r.h),
            (r.x, r.y + r.h),
        ];
        let state = State {
            page: None,
            additive,
            smooth: false,
            effect: Effect::None,
        };
        self.quad(state, corners, [(0.0, 0.0); 4], [top, top, bottom, bottom]);
    }
}

impl Renderer for Gpu {
    fn cloud_shadows(
        &mut self,
        camera: cloud_shadow::Camera,
        clouds: &[cloud_shadow::Sprite],
    ) -> bool {
        let Some(tex) = &mut self.cloud_exposure else {
            return false;
        };
        let Some(atlas) = self.cloud_shadow.prepare(camera, clouds) else {
            return false;
        };
        // The frame's synchronous begin has finished GPU use of the old atlas.
        unsafe { upload_cloud_exposure(tex.as_mut(), atlas) };
        true
    }

    fn clear(&mut self, c: Color) {
        // Painted rather than cleared so it sits at the back of this screen's order.
        self.solid_rect(Rect::new(0.0, 0.0, 400.0, 240.0), c, c, false);
    }

    fn sprite(&mut self, id: u32, at: (f32, f32), d: Draw) {
        let masks = self.sheet.get(id).masks;
        self.sprite_part(id, at, &d, d.tint);
        if d.cloud_light.is_some() {
            return;
        }
        // orig: Custom/PowerSprite2's recolour: keyed pixels × recolorA / recolorB.
        let (a, b) = d.recolor.unwrap_or((Color::WHITE, Color::WHITE));
        for (m, c) in masks.into_iter().zip([a, b]) {
            if let Some(m) = m {
                let tint = Color::new(c.r * d.tint.r, c.g * d.tint.g, c.b * d.tint.b, d.tint.a);
                self.sprite_part(m, at, &d, tint);
            }
        }
    }

    fn sliced(&mut self, id: u32, dest: Rect, border: (f32, f32, f32, f32), tint: Color) {
        let s = *self.sheet.get(id);
        let (pw, ph) = self.page_size[s.page as usize];
        let (l, b, r, t) = border;
        let (sw, sh) = (s.w as f32, s.h as f32);
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
        let state = State {
            page: Some(s.page),
            additive: false,
            smooth: false,
            effect: Effect::None,
        };
        for &(cx, cw, dx, dw) in &cols {
            for &(ry, rh, dy, dh) in &rows {
                if cw <= 0.0 || rh <= 0.0 || dw <= 0.0 || dh <= 0.0 {
                    continue;
                }
                let (x, y) = (dest.x + dx, dest.y + dy);
                let corners = [(x, y), (x + dw, y), (x + dw, y + dh), (x, y + dh)];
                let (u0, u1) = ((s.x as f32 + cx) / pw, (s.x as f32 + cx + cw) / pw);
                let (v0, v1) = (
                    1.0 - (s.y as f32 + ry) / ph,
                    1.0 - (s.y as f32 + ry + rh) / ph,
                );
                self.quad(
                    state,
                    corners,
                    [(u0, v0), (u1, v0), (u1, v1), (u0, v1)],
                    [tint; 4],
                );
            }
        }
    }

    fn rect(&mut self, r: Rect, c: Color) {
        self.solid_rect(r, c, c, false);
    }

    fn rect_lines(&mut self, r: Rect, t: f32, c: Color) {
        self.rect(Rect::new(r.x, r.y, r.w, t), c);
        self.rect(Rect::new(r.x, r.y + r.h - t, r.w, t), c);
        self.rect(Rect::new(r.x, r.y, t, r.h), c);
        self.rect(Rect::new(r.x + r.w - t, r.y, t, r.h), c);
    }

    fn gradient(&mut self, r: Rect, top: Color, bottom: Color) {
        self.solid_rect(r, top, bottom, false);
    }

    fn add(&mut self, r: Rect, c: Color) {
        self.solid_rect(r, c, c, true);
    }

    fn add_gradient(&mut self, r: Rect, top: Color, bottom: Color) {
        self.solid_rect(r, top, bottom, true);
    }

    fn circle(&mut self, x: f32, y: f32, radius: f32, c: Color) {
        const N: usize = 16;
        let state = State {
            page: None,
            additive: false,
            smooth: false,
            effect: Effect::None,
        };
        for i in 0..N {
            let a0 = i as f32 / N as f32 * std::f32::consts::TAU;
            let a1 = (i + 1) as f32 / N as f32 * std::f32::consts::TAU;
            let p0 = (x + radius * a0.cos(), y + radius * a0.sin());
            let p1 = (x + radius * a1.cos(), y + radius * a1.sin());
            // A triangle as a quad with a repeated corner.
            self.quad(state, [(x, y), p0, p1, p1], [(0.0, 0.0); 4], [c; 4]);
        }
    }

    fn circle_lines(&mut self, x: f32, y: f32, radius: f32, thickness: f32, c: Color) {
        const N: usize = 24;
        let r0 = radius - thickness / 2.0;
        let r1 = radius + thickness / 2.0;
        let state = State {
            page: None,
            additive: false,
            smooth: false,
            effect: Effect::None,
        };
        for i in 0..N {
            let a0 = i as f32 / N as f32 * std::f32::consts::TAU;
            let a1 = (i + 1) as f32 / N as f32 * std::f32::consts::TAU;
            let p = |r: f32, a: f32| (x + r * a.cos(), y + r * a.sin());
            self.quad(
                state,
                [p(r0, a0), p(r1, a0), p(r1, a1), p(r0, a1)],
                [(0.0, 0.0); 4],
                [c; 4],
            );
        }
    }

    fn depth(&mut self, d: f32) {
        self.depth = d;
    }

    fn reflect(&mut self, dest: Rect, src: Rect, tint: Color) {
        let Some(pages) = self.reflection.filter(|_| self.reflection_ready[0]) else {
            return;
        };
        // The copy is the colour buffer as is: sideways, screen column x in
        // memory row x, counted (like texture rows) from v = 1 down, so the
        // 400 copied rows fill v 1..112/512. Screen (x, y) is at texel
        // (240 - y, 512 - x).
        let page = pages[0];
        let mode = self.reflect_mode;
        let uv = |x: f32, y: f32| {
            let u = if mode & 1 == 0 { 240.0 - y } else { y };
            let v = if mode & 2 == 0 { 512.0 - x } else { 400.0 - x };
            (u / 256.0, v / 512.0)
        };
        let (x0, y0, x1, y1) = (src.x, src.y, src.x + src.w, src.y + src.h);
        let corners = [
            (dest.x, dest.y),
            (dest.x + dest.w, dest.y),
            (dest.x + dest.w, dest.y + dest.h),
            (dest.x, dest.y + dest.h),
        ];
        let state = State {
            page: Some(page),
            additive: true,
            smooth: false,
            effect: Effect::None,
        };
        self.quad(
            state,
            corners,
            [uv(x0, y0), uv(x1, y0), uv(x1, y1), uv(x0, y1)],
            [tint; 4],
        );
    }
}
