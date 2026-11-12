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
    /// Half-size copies (box filtered, premultiplied) for drawing images scaled down:
    /// without them a 240 px knob drawn at 76 px came out grainy and jagged.
    mips: Vec<Tex>,
}

impl Tex {
    fn new(w: usize, h: usize, px: Vec<Color32>, with_mips: bool) -> Tex {
        let mut t = Tex { w, h, px, mips: Vec::new() };
        if with_mips {
            t.build_mips();
        }
        t
    }

    fn build_mips(&mut self) {
        self.mips.clear();
        let (mut w, mut h) = (self.w, self.h);
        let mut src: &[Color32] = &self.px;
        let mut out: Vec<Tex> = Vec::new();
        while w > 1 || h > 1 {
            let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
            let mut px = Vec::with_capacity(nw * nh);
            for y in 0..nh {
                for x in 0..nw {
                    let mut acc = [0u32; 4];
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let sx = (2 * x + dx).min(w - 1);
                        let sy = (2 * y + dy).min(h - 1);
                        let c = src[sy * w + sx].to_array();
                        for i in 0..4 {
                            acc[i] += c[i] as u32;
                        }
                    }
                    px.push(Color32::from_rgba_premultiplied(
                        ((acc[0] + 2) / 4) as u8, ((acc[1] + 2) / 4) as u8, ((acc[2] + 2) / 4) as u8, ((acc[3] + 2) / 4) as u8,
                    ));
                }
            }
            out.push(Tex { w: nw, h: nh, px, mips: Vec::new() });
            w = nw;
            h = nh;
            src = &out.last().unwrap().px;
            if nw <= 4 && nh <= 4 {
                break;
            }
        }
        self.mips = out;
    }

    /// The level to sample when one screen pixel covers `rho` texels.
    fn level(&self, rho: f32) -> &Tex {
        // tiny tolerance so an exact 2x / 4x reduction picks its own level despite rounding
        let rho = rho * 1.001;
        if !(rho >= 2.0) || self.mips.is_empty() {
            return self;
        }
        let l = (rho.log2().floor() as usize).min(self.mips.len());
        &self.mips[l - 1]
    }
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

    /// Take new / updated textures. Returns true if anything changed.
    pub fn apply_textures(&mut self, set: &[(TextureId, egui::epaint::ImageDelta)]) -> bool {
        for (id, d) in set {
            let (iw, ih, pixels, is_image): (usize, usize, Vec<Color32>, bool) = match &d.image {
                ImageData::Color(img) => (img.size[0], img.size[1], img.pixels.clone(), true),
                ImageData::Font(f) => (f.size[0], f.size[1], f.srgba_pixels(None).collect(), false),
            };
            match d.pos {
                None => {
                    // the font atlas is drawn 1:1 and patched often: no mips for it
                    self.textures.insert(*id, Tex::new(iw, ih, pixels, is_image && iw.max(ih) >= 8));
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
                        if !t.mips.is_empty() {
                            t.build_mips();
                        }
                    }
                }
            }
        }
        !set.is_empty()
    }

    /// Draw the next frame in full (e.g. when taking over from the GPU renderer).
    pub fn force_redraw(&mut self) {
        self.force = true;
        self.prev.clear();
    }

    pub fn free_textures(&mut self, ids: &[TextureId]) {
        for id in ids {
            self.textures.remove(id);
        }
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
        // textures first: egui sends every texture only once, so it must be taken even when the
        // window is minimised and nothing is drawn
        let changed = self.apply_textures(&full_output.textures_delta.set) || self.force;
        if w == 0 || h == 0 {
            self.free_textures(&full_output.textures_delta.free);
            full_output.textures_delta.free.clear();
            self.force = true;
            return;
        }
        let shapes = std::mem::take(&mut full_output.shapes);
        let delta = &mut full_output.textures_delta;
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
    // texels covered by one screen pixel (constant over the triangle) -> mip level; nearest
    // sampling only where the texture really lands 1:1 on pixels (text at 100 %)
    let (tex, nearest) = if solid_uv {
        (tex, false)
    } else {
        let a0_ = (p1y - p2y) * inv_area;
        let a1_ = (p2y - p0y) * inv_area;
        let a2_ = -a0_ - a1_;
        let b0_ = (p2x - p1x) * inv_area;
        let b1_ = (p0x - p2x) * inv_area;
        let b2_ = -b0_ - b1_;
        let (tw, th) = (tex.w as f32, tex.h as f32);
        let dudx = (a0_ * v0.uv.x + a1_ * v1.uv.x + a2_ * v2.uv.x) * tw;
        let dvdx = (a0_ * v0.uv.y + a1_ * v1.uv.y + a2_ * v2.uv.y) * th;
        let dudy = (b0_ * v0.uv.x + b1_ * v1.uv.x + b2_ * v2.uv.x) * tw;
        let dvdy = (b0_ * v0.uv.y + b1_ * v1.uv.y + b2_ * v2.uv.y) * th;
        let rho = dudx.hypot(dvdx).max(dudy.hypot(dvdy));
        let one_to_one = (ppp - 1.0).abs() < 1e-4 && (rho - 1.0).abs() < 0.02;
        (tex.level(rho), one_to_one)
    };
    // d(w)/dx per pixel
    let a0 = (p1y - p2y) * inv_area;
    let a1 = (p2y - p0y) * inv_area;
    let a2 = -a0 - a1;
    // d(w)/dy per pixel, for the fill rule
    let b0 = (p2x - p1x) * inv_area;
    let b1 = (p0x - p2x) * inv_area;
    let b2 = -b0 - b1;
    // Exact coverage with the top-left fill rule (as GPUs do). The edge function of a shared
    // edge is bitwise the negation in the neighbouring triangle, so every pixel centre belongs
    // to exactly one of them: no double-drawn seams or specks, no holes.
    let top_left = |a: f32, b: f32| a > 0.0 || (a == 0.0 && b > 0.0);
    let (tl0, tl1, tl2) = (top_left(a0, b0), top_left(a1, b1), top_left(a2, b2));
    let sgn = if area > 0.0 { 1.0f32 } else { -1.0 };
    let edge = |ax: f32, ay: f32, bx: f32, by: f32, px: f32, py: f32| (ax - px) * (by - py) - (ay - py) * (bx - px);
    let inside = |px: f32, py: f32| -> bool {
        let e0 = edge(p1x, p1y, p2x, p2y, px, py) * sgn;
        let e1 = edge(p2x, p2y, p0x, p0y, px, py) * sgn;
        let e2 = edge(p0x, p0y, p1x, p1y, px, py) * sgn;
        (e0 > 0.0 || (e0 == 0.0 && tl0)) && (e1 > 0.0 || (e1 == 0.0 && tl1)) && (e2 > 0.0 || (e2 == 0.0 && tl2))
    };
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
        if empty {
            continue;
        }
        // the estimate above can be off by a pixel at the ends: settle them exactly
        let (mut k0, mut k1) = ((klo as i32 - 1).max(0), (khi as i32 + 1).min(max_x - min_x - 1));
        let cx = |k: i32| (min_x + k) as f32 + 0.5;
        while k0 <= k1 && !inside(cx(k0), py) {
            k0 += 1;
        }
        while k1 >= k0 && !inside(cx(k1), py) {
            k1 -= 1;
        }
        if k0 > k1 {
            continue;
        }
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

#[cfg(test)]
mod raster_tests {
    use super::*;
    use egui::epaint::{Vertex, WHITE_UV};
    use egui::{Color32, Pos2};

    /// Rasterise a fill strip (like a spectrum) and return the framebuffer.
    fn strip(n: usize, ppp: f32) -> (Vec<u32>, i32, i32) {
        let (w, h) = (300i32, 150i32);
        let mut fb = vec![pack(0, 0, 0); (w * h) as usize];
        let tex = Tex::new(1, 1, vec![Color32::WHITE], false);
        let col = Color32::from_white_alpha(26);
        let mut verts = Vec::new();
        for i in 0..=n {
            let x = 10.0 + 170.0 * i as f32 / n as f32;
            let y = 30.0 + 10.0 * ((i as f32) * 0.7).sin();
            verts.push(Vertex { pos: Pos2::new(x, y), uv: WHITE_UV, color: col });
            verts.push(Vertex { pos: Pos2::new(x, 90.0), uv: WHITE_UV, color: col });
        }
        for i in 1..=n {
            let k = 2 * i;
            for t in [[k - 2, k - 1, k], [k - 1, k + 1, k]] {
                raster_triangle(&mut fb, w, (0, 0, w, h), ppp, &tex, &verts[t[0]], &verts[t[1]], &verts[t[2]]);
            }
        }
        (fb, w, h)
    }

    /// A fan of thin and fat triangles (like a rounded rectangle): no holes, nothing twice.
    #[test]
    fn fans_have_no_holes() {
        let (w, h) = (300i32, 200i32);
        for ppp in [1.0f32, 1.25, 1.5, 2.0] {
            let mut fb = vec![pack(0, 0, 0); (w * h) as usize];
            let tex = Tex::new(1, 1, vec![Color32::WHITE], false);
            let col = Color32::from_white_alpha(26);
            let c = Pos2::new(70.3, 60.7);
            let n = 23;
            let pts: Vec<Pos2> = (0..n).map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                let r = if i % 3 == 0 { 50.0 } else { 47.3 };
                Pos2::new(c.x + r * a.cos(), c.y + r * 0.8 * a.sin())
            }).collect();
            let v = |p: Pos2| Vertex { pos: p, uv: WHITE_UV, color: col };
            for i in 1..n - 1 {
                raster_triangle(&mut fb, w, (0, 0, w, h), ppp, &tex, &v(pts[0]), &v(pts[i]), &v(pts[i + 1]));
            }
            let mut bad = 0;
            for y in ((c.y - 25.0) * ppp) as i32..((c.y + 25.0) * ppp) as i32 {
                for x in ((c.x - 30.0) * ppp) as i32..((c.x + 30.0) * ppp) as i32 {
                    let val = fb[(y * w + x) as usize] & 255;
                    if !(89..=91).contains(&val) {
                        bad += 1;
                    }
                }
            }
            assert_eq!(bad, 0, "ppp {ppp}: {bad} pixels with a hole or drawn twice");
        }
    }

    #[test]
    fn shared_edges_are_drawn_once() {
        for (n, ppp) in [(37usize, 1.0f32), (170, 1.0), (85, 1.0), (300, 1.0), (85, 1.5)] {
            let (fb, w, _) = strip(n, ppp);
            // egui encodes white_alpha(26) in gamma space: one layer over black = 90, two = ~171
            let mut over = 0;
            let mut seen = std::collections::BTreeMap::new();
            for y in (42.0 * ppp) as i32..(88.0 * ppp) as i32 {
                for x in (12.0 * ppp) as i32..(178.0 * ppp) as i32 {
                    let v = fb[(y * w + x) as usize] & 255;
                    *seen.entry(v).or_insert(0) += 1;
                    if v > 91 || v < 89 {
                        over += 1;
                    }
                }
            }
            println!("n {n} ppp {ppp}: values {seen:?}");
            assert_eq!(over, 0, "{over} pixels drawn more than once (n {n}, ppp {ppp})");
        }
    }
}
