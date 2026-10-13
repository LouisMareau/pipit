//! The rendering engine: the rear plane, then every polygon scanline by
//! scanline with a depth buffer, perspective-correct colours and texture
//! coordinates, the shading modes, alpha blending and test, stencil
//! shadows, then edge marking and fog over the finished frame.

use super::texture::{self, Texel};
use super::{Gpu3d, Polygon, HEIGHT, WIDTH};
use crate::video::render::OPAQUE;
use crate::video::vram::{Region, Vram};

const NO_ID: u8 = 0xFF;

/// The frame as it is drawn: colour (six bits per channel, red lowest),
/// alpha, depth, the ids of the opaque and translucent polygons at each
/// pixel, the fog flag and the shadow stencil.
struct Buffers {
    color: Vec<u32>,
    alpha: Vec<u8>,
    depth: Vec<i32>,
    opaque_id: Vec<u8>,
    trans_id: Vec<u8>,
    fog: Vec<bool>,
    stencil: Vec<bool>,
}

impl Buffers {
    fn new() -> Self {
        let n = WIDTH * HEIGHT;
        Self {
            color: vec![0; n],
            alpha: vec![0; n],
            depth: vec![0; n],
            opaque_id: vec![0; n],
            trans_id: vec![NO_ID; n],
            fog: vec![false; n],
            stencil: vec![false; n],
        }
    }
}

/// A 15-bit colour as six bits per channel.
#[inline]
fn six(c: u16) -> [i32; 3] {
    let e = |v: u16| {
        let v = i32::from(v & 0x1F);
        (v << 1) | (v >> 4)
    };
    [e(c), e(c >> 5), e(c >> 10)]
}

#[inline]
fn pack(c: [i32; 3]) -> u32 {
    (c[0] as u32 & 0x3F) | (c[1] as u32 & 0x3F) << 6 | (c[2] as u32 & 0x3F) << 12
}

#[inline]
fn unpack(p: u32) -> [i32; 3] {
    [(p & 0x3F) as i32, ((p >> 6) & 0x3F) as i32, ((p >> 12) & 0x3F) as i32]
}

/// An edge of a polygon at the scanline being drawn: everything the span
/// interpolates, colours and texture coordinates divided by w so that the
/// interpolation is perspective-correct.
#[derive(Clone, Copy)]
struct Point {
    x: f32,
    depth: f32,
    inv_w: f32,
    color: [f32; 3],
    tex: [f32; 2],
}

impl Point {
    fn of(v: &super::Vertex) -> Point {
        let inv_w = 1.0 / (v.pos[3].max(1) as f32);
        Point {
            x: v.screen[0] as f32,
            depth: v.depth as f32,
            inv_w,
            color: [
                v.color[0] as f32 * inv_w,
                v.color[1] as f32 * inv_w,
                v.color[2] as f32 * inv_w,
            ],
            tex: [v.tex[0] as f32 * inv_w, v.tex[1] as f32 * inv_w],
        }
    }

    fn mix(a: &Point, b: &Point, t: f32) -> Point {
        let m = |p: f32, q: f32| p + (q - p) * t;
        Point {
            x: m(a.x, b.x),
            depth: m(a.depth, b.depth),
            inv_w: m(a.inv_w, b.inv_w),
            color: [
                m(a.color[0], b.color[0]),
                m(a.color[1], b.color[1]),
                m(a.color[2], b.color[2]),
            ],
            tex: [m(a.tex[0], b.tex[0]), m(a.tex[1], b.tex[1])],
        }
    }
}

impl Gpu3d {
    /// The rear plane: the clear colour and depth, or the clear image from
    /// texture slots 2 (colour) and 3 (depth and fog).
    fn clear(&self, vram: &Vram, buf: &mut Buffers) {
        let cc = self.clear_color;
        let id = ((cc >> 24) & 0x3F) as u8;
        buf.opaque_id.fill(id);
        if self.disp3dcnt & (1 << 14) == 0 {
            buf.color.fill(pack(six(cc as u16)));
            buf.alpha.fill(((cc >> 16) & 0x1F) as u8);
            buf.depth.fill(i32::from(self.clear_depth & 0x7FFF) * 0x200 + 0x1FF);
            buf.fog.fill(cc & (1 << 15) != 0);
            return;
        }
        let (xo, yo) =
            ((self.clrimage_offset & 0xFF) as usize, (self.clrimage_offset >> 8) as usize);
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let at = (((y + yo) & 0xFF) * 256 + ((x + xo) & 0xFF)) * 2;
                let color = vram.read16(Region::Texture, 0x4_0000 + at);
                let depth = vram.read16(Region::Texture, 0x6_0000 + at);
                let i = y * WIDTH + x;
                buf.color[i] = pack(six(color));
                buf.alpha[i] = if color & 0x8000 != 0 { 31 } else { 0 };
                buf.depth[i] = i32::from(depth & 0x7FFF) * 0x200 + 0x1FF;
                buf.fog[i] = depth & 0x8000 != 0;
            }
        }
    }

    /// Draws the scene the last swap handed over into `out`, 256 × 192
    /// pixels in the 2D engine's format (18-bit colour, alpha in bits 24-28).
    pub(super) fn render(&self, vram: &Vram, out: &mut [u32]) {
        let mut buf = Buffers::new();
        self.clear(vram, &mut buf);
        let polys = &self.scene.polygons;
        let mut order: Vec<usize> = (0..polys.len()).filter(|&i| !polys[i].translucent).collect();
        let mut translucent: Vec<usize> =
            (0..polys.len()).filter(|&i| polys[i].translucent).collect();
        if self.scene_flags & 1 == 0 {
            // Automatic sorting: by the top of each polygon, back to front among equals.
            translucent.sort_by_key(|&i| self.top_of(&polys[i]));
        }
        order.extend(translucent);
        let wbuffer = self.scene_flags & 2 != 0;
        let mut prev_mask = false;
        for i in order {
            self.draw(vram, &polys[i], wbuffer, &mut buf, &mut prev_mask);
        }
        if self.disp3dcnt & (1 << 5) != 0 {
            self.edge_mark(&mut buf);
        }
        if self.disp3dcnt & (1 << 7) != 0 {
            self.fog(&mut buf);
        }
        for (i, px) in out.iter_mut().enumerate().take(WIDTH * HEIGHT) {
            let a = u32::from(buf.alpha[i]);
            *px = buf.color[i] | a << 24 | if a > 0 { OPAQUE } else { 0 };
        }
    }

    fn top_of(&self, poly: &Polygon) -> i32 {
        let verts = &self.scene.vertices
            [poly.first as usize..poly.first as usize + usize::from(poly.count)];
        verts.iter().map(|v| v.screen[1]).min().unwrap_or(0)
    }

    fn draw(
        &self,
        vram: &Vram,
        poly: &Polygon,
        wbuffer: bool,
        buf: &mut Buffers,
        prev_mask: &mut bool,
    ) {
        let verts = &self.scene.vertices
            [poly.first as usize..poly.first as usize + usize::from(poly.count)];
        let attr = poly.attr;
        let mode = (attr >> 4) & 3;
        let id = ((attr >> 24) & 0x3F) as u8;
        let mut poly_alpha = ((attr >> 16) & 0x1F) as u8;
        if poly_alpha == 0 {
            // Wireframe: drawn solid for now.
            poly_alpha = 31;
        }
        let shadow_mask = mode == 3 && id == 0;
        let shadow = mode == 3 && id != 0;
        if shadow_mask && !*prev_mask {
            buf.stencil.fill(false);
        }
        *prev_mask = shadow_mask;
        let fog = attr & (1 << 15) != 0;
        let depth_equal = attr & (1 << 14) != 0;
        let trans_depth = attr & (1 << 11) != 0;
        let textured = self.disp3dcnt & 1 != 0 && (poly.teximage >> 26) & 7 != 0;
        let blending = self.disp3dcnt & (1 << 3) != 0;
        let alpha_test = (self.disp3dcnt & (1 << 2) != 0).then_some((self.alpha_ref & 0x1F) as u8);

        let ymin = verts.iter().map(|v| v.screen[1]).min().unwrap_or(0).max(0);
        let ymax = verts.iter().map(|v| v.screen[1]).max().unwrap_or(0).min(HEIGHT as i32);
        for y in ymin..ymax {
            // Where the polygon's edges cross this line: the leftmost and rightmost.
            let mut left: Option<Point> = None;
            let mut right: Option<Point> = None;
            for i in 0..verts.len() {
                let (a, b) = (&verts[i], &verts[(i + 1) % verts.len()]);
                let (ya, yb) = (a.screen[1], b.screen[1]);
                if ya == yb {
                    continue;
                }
                let (top, bottom, lo, hi) = if ya < yb { (a, b, ya, yb) } else { (b, a, yb, ya) };
                if y < lo || y >= hi {
                    continue;
                }
                let p = Point::mix(
                    &Point::of(top),
                    &Point::of(bottom),
                    (y - lo) as f32 / (hi - lo) as f32,
                );
                if left.is_none_or(|l| p.x < l.x) {
                    left = Some(p);
                }
                if right.is_none_or(|r| p.x > r.x) {
                    right = Some(p);
                }
            }
            let (Some(l), Some(r)) = (left, right) else { continue };
            let xl = (l.x - 0.5).ceil() as i32;
            let xr = (r.x - 0.5).ceil() as i32;
            let span = (r.x - l.x).max(1e-6);
            for x in xl.max(0)..xr.min(WIDTH as i32) {
                let t = (x as f32 + 0.5 - l.x) / span;
                let p = Point::mix(&l, &r, t);
                let w = 1.0 / p.inv_w;
                let depth = if wbuffer { (w as i32).clamp(0, 0xFF_FFFF) } else { p.depth as i32 };
                let i = y as usize * WIDTH + x as usize;
                let old = buf.depth[i];
                let pass = if depth_equal {
                    (old - 0x200..=old + 0x200).contains(&depth)
                } else {
                    depth < old
                };
                if shadow_mask {
                    if !pass {
                        buf.stencil[i] = true;
                    }
                    continue;
                }
                if !pass || (shadow && (!buf.stencil[i] || buf.opaque_id[i] == id)) {
                    continue;
                }
                let vc =
                    [(p.color[0] * w) as i32, (p.color[1] * w) as i32, (p.color[2] * w) as i32];
                let texel = textured.then(|| {
                    texture::sample(
                        vram,
                        poly.teximage,
                        poly.pltt_base,
                        (p.tex[0] * w) as i32,
                        (p.tex[1] * w) as i32,
                    )
                });
                let (color, alpha) = self.shade(mode, vc, texel, poly_alpha);
                if alpha == 0 || alpha_test.is_some_and(|reference| alpha <= reference) {
                    continue;
                }
                if alpha == 31 || !blending {
                    buf.color[i] = pack(color);
                    buf.alpha[i] = alpha;
                    buf.depth[i] = depth;
                    buf.opaque_id[i] = id;
                    buf.trans_id[i] = NO_ID;
                    buf.fog[i] = fog;
                } else {
                    // A translucent pixel never goes over one of the same polygon id.
                    if buf.trans_id[i] == id {
                        continue;
                    }
                    let dst = unpack(buf.color[i]);
                    let a = i32::from(alpha);
                    let mixed = [
                        (color[0] * (a + 1) + dst[0] * (31 - a)) >> 5,
                        (color[1] * (a + 1) + dst[1] * (31 - a)) >> 5,
                        (color[2] * (a + 1) + dst[2] * (31 - a)) >> 5,
                    ];
                    buf.color[i] = pack(mixed);
                    buf.alpha[i] = buf.alpha[i].max(alpha);
                    if trans_depth {
                        buf.depth[i] = depth;
                        buf.fog[i] = fog;
                    }
                    buf.trans_id[i] = id;
                }
            }
        }
    }

    /// A pixel's colour (six bits per channel) and alpha from the shading
    /// mode, the vertex colour, the texel (`None` when the polygon has no
    /// texture) and the polygon's alpha.
    fn shade(
        &self,
        mode: u32,
        v: [i32; 3],
        texel: Option<Texel>,
        poly_alpha: u8,
    ) -> ([i32; 3], u8) {
        let pa = i32::from(poly_alpha);
        let tex = match texel {
            None => None,
            Some(None) => return (v, 0),
            Some(Some((c, a))) => Some((six(c), i32::from(a))),
        };
        let modulate = |a: [i32; 3], b: [i32; 3]| {
            [
                ((a[0] + 1) * (b[0] + 1) - 1) >> 6,
                ((a[1] + 1) * (b[1] + 1) - 1) >> 6,
                ((a[2] + 1) * (b[2] + 1) - 1) >> 6,
            ]
        };
        let tex_alpha = |ta: i32| (((ta + 1) * (pa + 1) - 1) >> 5) as u8;
        match mode {
            1 => match tex {
                Some((tc, ta)) => {
                    let decal = |t: i32, c: i32| (t * ta + c * (31 - ta)) / 31;
                    ([decal(tc[0], v[0]), decal(tc[1], v[1]), decal(tc[2], v[2])], poly_alpha)
                }
                None => (v, poly_alpha),
            },
            2 => {
                let toon = six(self.toon_table[(v[0] >> 1) as usize]);
                let highlight = self.disp3dcnt & (1 << 1) != 0;
                match tex {
                    Some((tc, ta)) => {
                        let color = if highlight {
                            let m = modulate(tc, v);
                            [
                                (m[0] + toon[0]).min(63),
                                (m[1] + toon[1]).min(63),
                                (m[2] + toon[2]).min(63),
                            ]
                        } else {
                            modulate(tc, toon)
                        };
                        (color, tex_alpha(ta))
                    }
                    None => {
                        let color = if highlight {
                            [
                                (v[0] + toon[0]).min(63),
                                (v[1] + toon[1]).min(63),
                                (v[2] + toon[2]).min(63),
                            ]
                        } else {
                            toon
                        };
                        (color, poly_alpha)
                    }
                }
            }
            _ => match tex {
                Some((tc, ta)) => (modulate(tc, v), tex_alpha(ta)),
                None => (v, poly_alpha),
            },
        }
    }

    /// Edge marking: a pixel whose neighbour belongs to another polygon that
    /// lies behind it takes the edge colour of its polygon id's group.
    fn edge_mark(&self, buf: &mut Buffers) {
        let ids = buf.opaque_id.clone();
        let depths = buf.depth.clone();
        let clear_id = ((self.clear_color >> 24) & 0x3F) as u8;
        for y in 0..HEIGHT as i32 {
            for x in 0..WIDTH as i32 {
                let i = y as usize * WIDTH + x as usize;
                if buf.alpha[i] == 0 {
                    continue;
                }
                let id = ids[i];
                let edge =
                    [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)].iter().any(|&(nx, ny)| {
                        if nx < 0 || ny < 0 || nx >= WIDTH as i32 || ny >= HEIGHT as i32 {
                            return clear_id != id;
                        }
                        let j = ny as usize * WIDTH + nx as usize;
                        ids[j] != id && depths[j] > depths[i]
                    });
                if edge {
                    buf.color[i] = pack(six(self.edge_color[usize::from(id >> 3)]));
                }
            }
        }
    }

    /// Fog: pixels flagged for it fade towards the fog colour by a density
    /// read from the table at their depth.
    fn fog(&self, buf: &mut Buffers) {
        let shift = (self.disp3dcnt >> 8) & 0xF;
        let fog_color = six(self.fog_color as u16);
        let fog_alpha = ((self.fog_color >> 16) & 0x1F) as i32;
        let alpha_only = self.disp3dcnt & (1 << 6) != 0;
        for i in 0..WIDTH * HEIGHT {
            if !buf.fog[i] {
                continue;
            }
            let z = buf.depth[i] >> 9;
            let index = ((z - i32::from(self.fog_offset)) << shift) >> 10;
            let density = i32::from(self.fog_table[index.clamp(0, 31) as usize]);
            if density == 0 {
                continue;
            }
            if !alpha_only {
                let c = unpack(buf.color[i]);
                let mix = |ch: i32, f: i32| (ch * (128 - density) + f * density) >> 7;
                buf.color[i] = pack([
                    mix(c[0], fog_color[0]),
                    mix(c[1], fog_color[1]),
                    mix(c[2], fog_color[2]),
                ]);
            }
            let a = i32::from(buf.alpha[i]);
            buf.alpha[i] = ((a * (128 - density) + fog_alpha * density) >> 7) as u8;
        }
    }
}
