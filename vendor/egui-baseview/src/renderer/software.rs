//! CPU renderer for egui (NyashkTune): rasterises egui's tessellated meshes into a BGRA buffer and
//! presents it with a plain X11 PutImage. No OpenGL involved, so it behaves the same on every
//! GPU / driver / compositor.

use baseview::{PhySize, Window};
use egui::epaint::{ImageData, Primitive, TextureId};
use egui::{Color32, FullOutput};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone, Default)]
pub struct GraphicsConfig {}

struct Tex {
    w: usize,
    h: usize,
    px: Vec<Color32>,
}

pub struct Renderer {
    textures: HashMap<TextureId, Tex>,
    fb: Vec<u32>,
    prev: Vec<u32>,
    fb_size: (u32, u32),
    last_hash: u64,
    force: bool,
}

#[derive(Debug)]
pub struct SoftwareError;
impl std::fmt::Display for SoftwareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "software renderer error")
    }
}

impl Renderer {
    pub fn new(_window: &Window, _config: GraphicsConfig) -> Result<Self, SoftwareError> {
        Ok(Self { textures: HashMap::new(), fb: Vec::new(), prev: Vec::new(), fb_size: (0, 0), last_hash: 0, force: true })
    }

    pub fn max_texture_side(&self) -> usize {
        8192
    }

    pub fn render(
        &mut self,
        window: &Window,
        bg_color: egui::Rgba,
        physical_size: PhySize,
        pixels_per_point: f32,
        egui_ctx: &mut egui::Context,
        full_output: &mut FullOutput,
    ) {
        let (w, h) = (physical_size.width, physical_size.height);
        if w == 0 || h == 0 {
            return;
        }
        let shapes = std::mem::take(&mut full_output.shapes);
        let delta = &mut full_output.textures_delta;

        let mut changed = self.force;
        for (id, d) in &delta.set {
            changed = true;
            let (iw, ih, pixels): (usize, usize, Vec<Color32>) = match &d.image {
                ImageData::Color(img) => (img.size[0], img.size[1], img.pixels.clone()),
                ImageData::Font(f) => (f.size[0], f.size[1], f.srgba_pixels(None).collect()),
            };
            match d.pos {
                None => {
                    self.textures.insert(*id, Tex { w: iw, h: ih, px: pixels });
                }
                Some([x0, y0]) => {
                    if let Some(t) = self.textures.get_mut(id) {
                        for row in 0..ih {
                            let dy = y0 + row;
                            if dy >= t.h {
                                break;
                            }
                            for col in 0..iw {
                                let dx = x0 + col;
                                if dx >= t.w {
                                    break;
                                }
                                t.px[dy * t.w + dx] = pixels[row * iw + col];
                            }
                        }
                    }
                }
            }
        }

        let prims = egui_ctx.tessellate(shapes, pixels_per_point);

        // Skip all work when nothing changed since the last presented frame.
        let mut hasher = DefaultHasher::new();
        (w, h).hash(&mut hasher);
        for cp in &prims {
            for v in [cp.clip_rect.min.x, cp.clip_rect.min.y, cp.clip_rect.max.x, cp.clip_rect.max.y] {
                v.to_bits().hash(&mut hasher);
            }
            if let Primitive::Mesh(m) = &cp.primitive {
                m.indices.hash(&mut hasher);
                match m.texture_id { TextureId::Managed(i) => (0u8, i).hash(&mut hasher), TextureId::User(i) => (1u8, i).hash(&mut hasher) }
                for v in &m.vertices {
                    v.pos.x.to_bits().hash(&mut hasher);
                    v.pos.y.to_bits().hash(&mut hasher);
                    v.uv.x.to_bits().hash(&mut hasher);
                    v.uv.y.to_bits().hash(&mut hasher);
                    v.color.to_array().hash(&mut hasher);
                }
            }
        }
        let hash = hasher.finish();
        for id in delta.free.drain(..) {
            self.textures.remove(&id);
        }
        if !changed && hash == self.last_hash && self.fb_size == (w, h) {
            return;
        }
        self.last_hash = hash;
        self.force = false;

        if self.fb_size != (w, h) {
            self.fb = vec![0u32; (w * h) as usize];
            self.prev = Vec::new();
            self.fb_size = (w, h);
        }
        // clear
        let bg = pack(
            (bg_color.r().clamp(0.0, 1.0) * 255.0) as u32,
            (bg_color.g().clamp(0.0, 1.0) * 255.0) as u32,
            (bg_color.b().clamp(0.0, 1.0) * 255.0) as u32,
        );
        self.fb.fill(bg);

        for cp in &prims {
            let Primitive::Mesh(mesh) = &cp.primitive else { continue };
            let Some(tex) = self.textures.get(&mesh.texture_id) else { continue };
            let cx0 = ((cp.clip_rect.min.x * pixels_per_point).round().max(0.0)) as i32;
            let cy0 = ((cp.clip_rect.min.y * pixels_per_point).round().max(0.0)) as i32;
            let cx1 = ((cp.clip_rect.max.x * pixels_per_point).round().min(w as f32)) as i32;
            let cy1 = ((cp.clip_rect.max.y * pixels_per_point).round().min(h as f32)) as i32;
            if cx1 <= cx0 || cy1 <= cy0 {
                continue;
            }
            for tri in mesh.indices.chunks_exact(3) {
                let v0 = &mesh.vertices[tri[0] as usize];
                let v1 = &mesh.vertices[tri[1] as usize];
                let v2 = &mesh.vertices[tri[2] as usize];
                raster_triangle(
                    &mut self.fb, w as i32, (cx0, cy0, cx1, cy1), pixels_per_point, tex, v0, v1, v2,
                );
            }
        }

        // Present only the rectangle that differs from the last presented frame.
        let (wu, hu) = (w as usize, h as usize);
        if self.prev.len() != self.fb.len() {
            self.prev = self.fb.clone();
            let bytes = as_bytes(&self.fb);
            window.put_image_bgra(w, h, bytes);
            return;
        }
        let (mut y0, mut y1) = (usize::MAX, 0usize);
        let (mut x0, mut x1) = (usize::MAX, 0usize);
        for y in 0..hu {
            let a = &self.fb[y * wu..(y + 1) * wu];
            let b = &self.prev[y * wu..(y + 1) * wu];
            if a == b {
                continue;
            }
            y0 = y0.min(y);
            y1 = y;
            let first = a.iter().zip(b).position(|(p, q)| p != q).unwrap_or(0);
            let last = a.iter().zip(b).rposition(|(p, q)| p != q).unwrap_or(wu - 1);
            x0 = x0.min(first);
            x1 = x1.max(last);
        }
        if y0 == usize::MAX {
            return;
        }
        let bytes = as_bytes(&self.fb);
        window.put_image_bgra_rect(w, x0 as u32, y0 as u32, (x1 - x0 + 1) as u32, (y1 - y0 + 1) as u32, bytes);
        for y in y0..=y1 {
            self.prev[y * wu + x0..y * wu + x1 + 1].copy_from_slice(&self.fb[y * wu + x0..y * wu + x1 + 1]);
        }
    }
}

#[inline]
fn pack(r: u32, g: u32, b: u32) -> u32 {
    0xFF00_0000 | (r << 16) | (g << 8) | b
}

fn as_bytes(v: &[u32]) -> &[u8] {
    // SAFETY: u32 has no padding and any alignment is valid for u8.
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 4) }
}

#[inline(always)]
fn div255(x: u32) -> u32 {
    let x = x + 128;
    (x + (x >> 8)) >> 8
}

/// Premultiplied source-over onto an opaque destination pixel.
#[inline(always)]
fn blend(d: u32, sr: u32, sg: u32, sb: u32, sa: u32) -> u32 {
    let inv = 255 - sa;
    let r = (sr + div255(((d >> 16) & 255) * inv)).min(255);
    let g = (sg + div255(((d >> 8) & 255) * inv)).min(255);
    let b = (sb + div255((d & 255) * inv)).min(255);
    pack(r, g, b)
}

#[inline]
fn sample_bilinear(tex: &Tex, u: f32, v: f32) -> [u32; 4] {
    let x = u * tex.w as f32 - 0.5;
    let y = v * tex.h as f32 - 0.5;
    let x0 = x.floor();
    let y0 = y.floor();
    let fx = x - x0;
    let fy = y - y0;
    let ix0 = (x0 as i32).clamp(0, tex.w as i32 - 1) as usize;
    let iy0 = (y0 as i32).clamp(0, tex.h as i32 - 1) as usize;
    let ix1 = (x0 as i32 + 1).clamp(0, tex.w as i32 - 1) as usize;
    let iy1 = (y0 as i32 + 1).clamp(0, tex.h as i32 - 1) as usize;
    let p = |ix: usize, iy: usize| tex.px[iy * tex.w + ix].to_array();
    let (a, b, c, d) = (p(ix0, iy0), p(ix1, iy0), p(ix0, iy1), p(ix1, iy1));
    let mut out = [0u32; 4];
    for i in 0..4 {
        let top = a[i] as f32 * (1.0 - fx) + b[i] as f32 * fx;
        let bot = c[i] as f32 * (1.0 - fx) + d[i] as f32 * fx;
        out[i] = (top * (1.0 - fy) + bot * fy + 0.5) as u32;
    }
    out
}

#[inline(always)]
fn sample_nearest(tex: &Tex, u: f32, v: f32) -> [u32; 4] {
    let ix = ((u * tex.w as f32) as i32).clamp(0, tex.w as i32 - 1) as usize;
    let iy = ((v * tex.h as f32) as i32).clamp(0, tex.h as i32 - 1) as usize;
    let c = tex.px[iy * tex.w + ix].to_array();
    [c[0] as u32, c[1] as u32, c[2] as u32, c[3] as u32]
}

#[allow(clippy::too_many_arguments)]
fn raster_triangle(
    fb: &mut [u32], stride: i32, clip: (i32, i32, i32, i32), ppp: f32, tex: &Tex,
    v0: &egui::epaint::Vertex, v1: &egui::epaint::Vertex, v2: &egui::epaint::Vertex,
) {
    let (p0x, p0y) = (v0.pos.x * ppp, v0.pos.y * ppp);
    let (p1x, p1y) = (v1.pos.x * ppp, v1.pos.y * ppp);
    let (p2x, p2y) = (v2.pos.x * ppp, v2.pos.y * ppp);
    let area = (p1x - p0x) * (p2y - p0y) - (p1y - p0y) * (p2x - p0x);
    if area.abs() < 1e-6 || !area.is_finite() {
        return;
    }
    let min_x = (p0x.min(p1x).min(p2x).floor() as i32).max(clip.0);
    let max_x = (p0x.max(p1x).max(p2x).ceil() as i32).min(clip.2);
    let min_y = (p0y.min(p1y).min(p2y).floor() as i32).max(clip.1);
    let max_y = (p0y.max(p1y).max(p2y).ceil() as i32).min(clip.3);
    if max_x <= min_x || max_y <= min_y {
        return;
    }
    let inv_area = 1.0 / area;
    let c0 = v0.color.to_array();
    let c1 = v1.color.to_array();
    let c2 = v2.color.to_array();
    let flat_colour = c0 == c1 && c1 == c2;
    let solid_uv = v0.uv == v1.uv && v1.uv == v2.uv;
    let nearest = (ppp - 1.0).abs() < 1e-4;
    // d(w)/dx per pixel
    let a0 = (p1y - p2y) * inv_area;
    let a1 = (p2y - p0y) * inv_area;
    let a2 = -a0 - a1;
    const EPS: f32 = -1e-4;
    let n = (max_x - min_x) as f32;
    // constant texel for solid-uv triangles
    let tc = if solid_uv { sample_bilinear(tex, v0.uv.x, v0.uv.y) } else { [0; 4] };
    let fc = [c0[0] as u32, c0[1] as u32, c0[2] as u32, c0[3] as u32];
    // constant premultiplied source when both texel and colour are constant
    let const_src = if solid_uv && flat_colour {
        Some((div255(tc[0] * fc[0]), div255(tc[1] * fc[1]), div255(tc[2] * fc[2]), div255(tc[3] * fc[3])))
    } else {
        None
    };
    let px0 = min_x as f32 + 0.5;
    for y in min_y..max_y {
        let py = y as f32 + 0.5;
        let w0r = ((p1x - px0) * (p2y - py) - (p1y - py) * (p2x - px0)) * inv_area;
        let w1r = ((p2x - px0) * (p0y - py) - (p2y - py) * (p0x - px0)) * inv_area;
        let w2r = 1.0 - w0r - w1r;
        // x range (in pixels from min_x) where all barycentrics are inside
        let (mut klo, mut khi) = (0.0f32, n - 1.0);
        let mut empty = false;
        for (wr, a) in [(w0r, a0), (w1r, a1), (w2r, a2)] {
            if a > 1e-9 {
                klo = klo.max(((EPS - wr) / a).ceil());
            } else if a < -1e-9 {
                khi = khi.min(((EPS - wr) / a).floor());
            } else if wr < EPS {
                empty = true;
            }
        }
        if empty || klo > khi {
            continue;
        }
        let (k0, k1) = (klo as i32, khi as i32);
        let row = (y * stride + min_x) as usize;
        if let Some((sr, sg, sb, sa)) = const_src {
            if sa == 0 && sr == 0 && sg == 0 && sb == 0 {
                continue;
            }
            let span = &mut fb[row + k0 as usize..=row + k1 as usize];
            if sa == 255 {
                span.fill(pack(sr, sg, sb));
            } else {
                for d in span {
                    *d = blend(*d, sr, sg, sb, sa);
                }
            }
            continue;
        }
        for k in k0..=k1 {
            let kf = k as f32;
            let w0 = w0r + a0 * kf;
            let w1 = w1r + a1 * kf;
            let w2 = 1.0 - w0 - w1;
            let t = if solid_uv {
                tc
            } else {
                let u = w0 * v0.uv.x + w1 * v1.uv.x + w2 * v2.uv.x;
                let v = w0 * v0.uv.y + w1 * v1.uv.y + w2 * v2.uv.y;
                if nearest { sample_nearest(tex, u, v) } else { sample_bilinear(tex, u, v) }
            };
            let col = if flat_colour {
                fc
            } else {
                [
                    (w0 * c0[0] as f32 + w1 * c1[0] as f32 + w2 * c2[0] as f32 + 0.5).clamp(0.0, 255.0) as u32,
                    (w0 * c0[1] as f32 + w1 * c1[1] as f32 + w2 * c2[1] as f32 + 0.5).clamp(0.0, 255.0) as u32,
                    (w0 * c0[2] as f32 + w1 * c1[2] as f32 + w2 * c2[2] as f32 + 0.5).clamp(0.0, 255.0) as u32,
                    (w0 * c0[3] as f32 + w1 * c1[3] as f32 + w2 * c2[3] as f32 + 0.5).clamp(0.0, 255.0) as u32,
                ]
            };
            let sr = div255(t[0] * col[0]);
            let sg = div255(t[1] * col[1]);
            let sb = div255(t[2] * col[2]);
            let sa = div255(t[3] * col[3]);
            if sa == 0 && sr == 0 && sg == 0 && sb == 0 {
                continue;
            }
            let o = row + k as usize;
            fb[o] = blend(fb[o], sr, sg, sb, sa);
        }
    }
}
