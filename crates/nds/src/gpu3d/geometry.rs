//! The geometry engine's commands: matrices and their stacks, lighting,
//! vertices through the clip matrix into polygons, clipped against the view
//! volume and placed on the screen, and the box, position and vector tests.

use super::matrix::Matrix;
use super::{Gpu3d, Polygon, Primitive, Vertex, MAX_POLYGONS, MAX_VERTICES};

/// A 10-bit two's complement field of `word` at `shift`, as a signed value.
fn field10(word: u32, shift: u32) -> i32 {
    ((word >> shift) as i32) << 22 >> 22
}

/// A 16-bit field as a signed value.
fn field16(word: u32, shift: u32) -> i32 {
    i32::from((word >> shift) as u16 as i16)
}

fn rgb5(word: u32) -> [u8; 3] {
    [(word & 0x1F) as u8, ((word >> 5) & 0x1F) as u8, ((word >> 10) & 0x1F) as u8]
}

/// `(a, b) × the 2 × 2 top-left of a texture matrix` plus its translation,
/// shifted down by `shift`: the texture coordinate transforms.
fn tex_transform(m: &Matrix, a: i64, b: i64, c: i64, shift: u32, translate: bool) -> [i32; 2] {
    let m = &m.0;
    let mut out = [0i32; 2];
    for (col, o) in out.iter_mut().enumerate() {
        let mut acc = a * i64::from(m[col]) + b * i64::from(m[4 + col]) + c * i64::from(m[8 + col]);
        if translate {
            acc += i64::from(m[12 + col]);
        }
        *o = (acc >> shift) as i32;
    }
    out
}

impl Gpu3d {
    pub(super) fn execute(&mut self, cmd: u8, p: &[u32]) {
        match cmd {
            0x10 => self.mtx_mode = (p[0] & 3) as u8,
            0x11 => self.mtx_push(),
            0x12 => self.mtx_pop(p[0]),
            0x13 => self.mtx_store(p[0]),
            0x14 => self.mtx_restore(p[0]),
            0x15 => self.mtx_set(Matrix::IDENTITY),
            0x16 => self.mtx_set(Matrix::from_4x4(p)),
            0x17 => self.mtx_set(Matrix::from_4x3(p)),
            0x18 => self.mtx_mul(Matrix::from_4x4(p), true),
            0x19 => self.mtx_mul(Matrix::from_4x3(p), true),
            0x1A => self.mtx_mul(Matrix::from_3x3(p), true),
            // MTX_SCALE leaves the vector matrix alone in mode 2.
            0x1B => self.mtx_mul(Matrix::scale(p[0] as i32, p[1] as i32, p[2] as i32), false),
            0x1C => self.mtx_mul(Matrix::translation(p[0] as i32, p[1] as i32, p[2] as i32), true),
            0x20 => self.color = rgb5(p[0]),
            0x21 => self.normal(p[0]),
            0x22 => {
                self.tex_raw = [field16(p[0], 0), field16(p[0], 16)];
                self.texcoord = if (self.teximage >> 30) & 3 == 1 {
                    let [s, t] = self.tex_raw;
                    tex_transform(&self.tex, i64::from(s), i64::from(t), 0x1000, 12, true)
                } else {
                    self.tex_raw
                };
            }
            0x23 => self.vertex([field16(p[0], 0), field16(p[0], 16), field16(p[1], 0)]),
            0x24 => {
                self.vertex([field10(p[0], 0) << 6, field10(p[0], 10) << 6, field10(p[0], 20) << 6])
            }
            0x25 => self.vertex([field16(p[0], 0), field16(p[0], 16), self.last_vertex[2]]),
            0x26 => self.vertex([field16(p[0], 0), self.last_vertex[1], field16(p[0], 16)]),
            0x27 => self.vertex([self.last_vertex[0], field16(p[0], 0), field16(p[0], 16)]),
            0x28 => {
                let [x, y, z] = self.last_vertex;
                self.vertex([x + field10(p[0], 0), y + field10(p[0], 10), z + field10(p[0], 20)]);
            }
            0x29 => self.polygon_attr = p[0],
            0x2A => self.teximage = p[0],
            0x2B => self.pltt_base = p[0] & 0x1FFF,
            0x30 => {
                self.diffuse = rgb5(p[0]);
                self.ambient = rgb5(p[0] >> 16);
                if p[0] & 0x8000 != 0 {
                    self.color = self.diffuse;
                }
            }
            0x31 => {
                self.specular = rgb5(p[0]);
                self.emission = rgb5(p[0] >> 16);
                self.use_shininess = p[0] & 0x8000 != 0;
            }
            0x32 => {
                let dir = [field10(p[0], 0), field10(p[0], 10), field10(p[0], 20)];
                self.lights[(p[0] >> 30) as usize] = self.vec.transform_direction(dir);
            }
            0x33 => self.light_colors[(p[0] >> 30) as usize] = rgb5(p[0]),
            0x34 => {
                for (i, word) in p.iter().take(32).enumerate() {
                    self.shininess[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
                }
            }
            0x40 => {
                self.primitive = match p[0] & 3 {
                    0 => Primitive::Triangles,
                    1 => Primitive::Quads,
                    2 => Primitive::TriangleStrip,
                    _ => Primitive::QuadStrip,
                };
                self.vtx_attr = self.polygon_attr;
                self.prim_count = 0;
                self.strip_odd = false;
            }
            0x41 => {}
            0x50 => self.swap = Some(p[0]),
            0x60 => self.viewport = p[0].to_le_bytes(),
            0x70 => self.box_test(p),
            0x71 => {
                let v = [field16(p[0], 0), field16(p[0], 16), field16(p[1], 0), 0x1000];
                self.pos_result = self.clip_matrix().transform(v);
            }
            0x72 => {
                let v = self.vec.transform_direction([
                    field10(p[0], 0),
                    field10(p[0], 10),
                    field10(p[0], 20),
                ]);
                self.vec_result = [
                    (v[0] << 19 >> 19) as i16,
                    (v[1] << 19 >> 19) as i16,
                    (v[2] << 19 >> 19) as i16,
                ];
            }
            _ => {}
        }
    }

    // ---------------------------------------------------------------------
    // Matrices
    // ---------------------------------------------------------------------

    fn mtx_set(&mut self, m: Matrix) {
        match self.mtx_mode {
            0 => self.proj = m,
            1 => self.pos = m,
            2 => {
                self.pos = m;
                self.vec = m;
            }
            _ => self.tex = m,
        }
    }

    fn mtx_mul(&mut self, m: Matrix, vector_too: bool) {
        match self.mtx_mode {
            0 => self.proj.mul_left(&m),
            1 => self.pos.mul_left(&m),
            2 => {
                self.pos.mul_left(&m);
                if vector_too {
                    self.vec.mul_left(&m);
                }
            }
            _ => self.tex.mul_left(&m),
        }
    }

    fn mtx_push(&mut self) {
        match self.mtx_mode {
            0 => {
                if self.sp_proj == 1 {
                    self.stack_error = true;
                } else {
                    self.proj_stack = self.proj;
                    self.sp_proj = 1;
                }
            }
            1 | 2 => {
                if self.sp_pos >= 31 {
                    self.stack_error = true;
                }
                let at = usize::from(self.sp_pos & 31);
                self.pos_stack[at] = self.pos;
                self.vec_stack[at] = self.vec;
                self.sp_pos = (self.sp_pos + 1) & 63;
            }
            _ => self.tex_stack = self.tex,
        }
    }

    fn mtx_pop(&mut self, p: u32) {
        match self.mtx_mode {
            0 => {
                if self.sp_proj == 0 {
                    self.stack_error = true;
                }
                self.sp_proj = 0;
                self.proj = self.proj_stack;
            }
            1 | 2 => {
                // A signed 6-bit count: negative pops "upwards".
                let n = ((p & 0x3F) as i32) << 26 >> 26;
                let sp = i32::from(self.sp_pos) - n;
                if !(0..=31).contains(&sp) {
                    self.stack_error = true;
                }
                self.sp_pos = (sp & 63) as u8;
                let at = usize::from(self.sp_pos & 31);
                self.pos = self.pos_stack[at];
                self.vec = self.vec_stack[at];
            }
            _ => self.tex = self.tex_stack,
        }
    }

    fn mtx_store(&mut self, p: u32) {
        match self.mtx_mode {
            0 => self.proj_stack = self.proj,
            1 | 2 => {
                let at = (p & 0x1F) as usize;
                if at == 31 {
                    self.stack_error = true;
                }
                self.pos_stack[at] = self.pos;
                self.vec_stack[at] = self.vec;
            }
            _ => self.tex_stack = self.tex,
        }
    }

    fn mtx_restore(&mut self, p: u32) {
        match self.mtx_mode {
            0 => self.proj = self.proj_stack,
            1 | 2 => {
                let at = (p & 0x1F) as usize;
                if at == 31 {
                    self.stack_error = true;
                }
                self.pos = self.pos_stack[at];
                self.vec = self.vec_stack[at];
            }
            _ => self.tex = self.tex_stack,
        }
    }

    // ---------------------------------------------------------------------
    // Lighting and vertices
    // ---------------------------------------------------------------------

    /// NORMAL: lights the vertex colour from the material, the enabled
    /// lights and this normal (GBATEK's formula), and feeds the texture
    /// coordinates when they come from the normal.
    fn normal(&mut self, p: u32) {
        let raw = [field10(p, 0), field10(p, 10), field10(p, 20)];
        if (self.teximage >> 30) & 3 == 2 {
            let t = tex_transform(
                &self.tex,
                i64::from(raw[0]),
                i64::from(raw[1]),
                i64::from(raw[2]),
                21,
                false,
            );
            self.texcoord = [t[0] + self.tex_raw[0], t[1] + self.tex_raw[1]];
        }
        let n = self.vec.transform_direction(raw);
        let mut c =
            [i32::from(self.emission[0]), i32::from(self.emission[1]), i32::from(self.emission[2])];
        for i in 0..4 {
            if self.polygon_attr & (1 << i) == 0 {
                continue;
            }
            let l = self.lights[i];
            let diffuse = (-(l[0] * n[0] + l[1] * n[1] + l[2] * n[2]) >> 10).clamp(0, 255);
            // The half vector between the light and the line of sight (0, 0, -1).
            let (hx, hy, hz) = (l[0] >> 1, l[1] >> 1, (l[2] - 0x200) >> 1);
            let shine = (-(hx * n[0] + hy * n[1] + hz * n[2]) >> 10).clamp(0, 255);
            let mut shine = (shine * shine) >> 8;
            if self.use_shininess {
                shine = i32::from(self.shininess[(shine >> 1) as usize]);
            }
            let light = self.light_colors[i];
            for ch in 0..3 {
                let lc = i32::from(light[ch]);
                c[ch] += (i32::from(self.specular[ch]) * lc * shine) >> 13;
                c[ch] += (i32::from(self.diffuse[ch]) * lc * diffuse) >> 13;
                c[ch] += (i32::from(self.ambient[ch]) * lc) >> 5;
            }
        }
        self.color = [c[0].clamp(0, 31) as u8, c[1].clamp(0, 31) as u8, c[2].clamp(0, 31) as u8];
    }

    /// A vertex in 20.12 model coordinates: through the clip matrix, into
    /// the primitive being built.
    fn vertex(&mut self, v: [i32; 3]) {
        self.last_vertex = v;
        let pos = self.clip_matrix().transform([v[0], v[1], v[2], 0x1000]);
        if (self.teximage >> 30) & 3 == 3 {
            let t = tex_transform(
                &self.tex,
                i64::from(v[0]),
                i64::from(v[1]),
                i64::from(v[2]),
                24,
                false,
            );
            self.texcoord = [t[0] + self.tex_raw[0], t[1] + self.tex_raw[1]];
        }
        let six = |c: u8| (c << 1) | (c >> 4);
        let vertex = Vertex {
            pos,
            color: [six(self.color[0]), six(self.color[1]), six(self.color[2])],
            tex: self.texcoord,
            ..Vertex::default()
        };
        if self.prim_count < 4 {
            self.prim[self.prim_count] = vertex;
            self.prim_count += 1;
        }
        match self.primitive {
            Primitive::Triangles => {
                if self.prim_count == 3 {
                    self.polygon(&[0, 1, 2]);
                    self.prim_count = 0;
                }
            }
            Primitive::Quads => {
                if self.prim_count == 4 {
                    self.polygon(&[0, 1, 2, 3]);
                    self.prim_count = 0;
                }
            }
            Primitive::TriangleStrip => {
                if self.prim_count == 3 {
                    // Every other triangle is turned around so they all face the same way.
                    if self.strip_odd {
                        self.polygon(&[1, 0, 2]);
                    } else {
                        self.polygon(&[0, 1, 2]);
                    }
                    self.prim[0] = self.prim[1];
                    self.prim[1] = self.prim[2];
                    self.prim_count = 2;
                    self.strip_odd = !self.strip_odd;
                }
            }
            Primitive::QuadStrip => {
                if self.prim_count == 4 {
                    self.polygon(&[0, 1, 3, 2]);
                    self.prim[0] = self.prim[2];
                    self.prim[1] = self.prim[3];
                    self.prim_count = 2;
                }
            }
        }
    }

    /// A finished primitive: culled by its facing, clipped to the view
    /// volume, placed on the screen and stored.
    fn polygon(&mut self, order: &[usize]) {
        let attr = self.vtx_attr;
        let verts: Vec<Vertex> = order.iter().map(|&i| self.prim[i]).collect();
        let (a, b, c) = (verts[0].pos, verts[1].pos, verts[2].pos);
        let d = |p: i32, q: i32| i64::from(p) - i64::from(q);
        // The normal of the first three vertices in (x, y, w), against the
        // first vertex: negative means the polygon faces the viewer.
        let nx = d(a[1], b[1]) * d(c[3], b[3]) - d(a[3], b[3]) * d(c[1], b[1]);
        let ny = d(a[3], b[3]) * d(c[0], b[0]) - d(a[0], b[0]) * d(c[3], b[3]);
        let nz = d(a[0], b[0]) * d(c[1], b[1]) - d(a[1], b[1]) * d(c[0], b[0]);
        let dot = i64::from(b[0]) * nx + i64::from(b[1]) * ny + i64::from(b[3]) * nz;
        let front = dot < 0;
        if (front && attr & (1 << 7) == 0) || (!front && attr & (1 << 6) == 0) {
            return;
        }
        if attr & (1 << 12) == 0 && verts.iter().any(|v| v.pos[2] > v.pos[3]) {
            return;
        }
        let clipped = clip_polygon(&verts);
        if clipped.len() < 3 {
            return;
        }
        let scene = &mut self.building;
        if scene.polygons.len() >= MAX_POLYGONS
            || scene.vertices.len() + clipped.len() > MAX_VERTICES
        {
            self.overflow = true;
            return;
        }
        let first = scene.vertices.len() as u32;
        let [x1, y1, x2, y2] = self.viewport.map(i64::from);
        let (vw, vh) = (x2 - x1 + 1, y2 - y1 + 1);
        for mut v in clipped {
            let [x, y, z, w] = v.pos.map(i64::from);
            let w = w.max(1);
            v.screen = [
                ((x + w) * vw / (2 * w) + x1) as i32,
                ((w - y) * vh / (2 * w) + (191 - y2)) as i32,
            ];
            v.depth = ((z * 0x4000 / w + 0x3FFF) * 0x200).clamp(0, 0xFF_FFFF) as i32;
            scene.vertices.push(v);
        }
        let alpha = (attr >> 16) & 0x1F;
        let format = (self.teximage >> 26) & 7;
        scene.polygons.push(Polygon {
            attr,
            teximage: self.teximage,
            pltt_base: self.pltt_base,
            first,
            count: (scene.vertices.len() as u32 - first) as u8,
            translucent: (1..31).contains(&alpha) || matches!(format, 1 | 6),
        });
        let _ = y1;
    }

    /// BOX_TEST: whether any part of the box is inside the view volume.
    fn box_test(&mut self, p: &[u32]) {
        let (x, y, z) = (field16(p[0], 0), field16(p[0], 16), field16(p[1], 0));
        let (w, h, d) = (field16(p[1], 16), field16(p[2], 0), field16(p[2], 16));
        let clip = self.clip_matrix();
        let corner = |cx: i32, cy: i32, cz: i32| Vertex {
            pos: clip.transform([cx, cy, cz, 0x1000]),
            ..Vertex::default()
        };
        let corners = [
            corner(x, y, z),
            corner(x + w, y, z),
            corner(x + w, y + h, z),
            corner(x, y + h, z),
            corner(x, y, z + d),
            corner(x + w, y, z + d),
            corner(x + w, y + h, z + d),
            corner(x, y + h, z + d),
        ];
        const FACES: [[usize; 4]; 6] =
            [[0, 1, 2, 3], [4, 5, 6, 7], [0, 1, 5, 4], [2, 3, 7, 6], [0, 3, 7, 4], [1, 2, 6, 5]];
        self.box_result = FACES.iter().any(|face| {
            let verts: Vec<Vertex> = face.iter().map(|&i| corners[i]).collect();
            clip_polygon(&verts).len() >= 3
        });
    }
}

/// Clips a polygon against the six planes of the view volume (Sutherland–
/// Hodgman), interpolating colours and texture coordinates along the cuts.
pub(super) fn clip_polygon(input: &[Vertex]) -> Vec<Vertex> {
    let mut poly = input.to_vec();
    for plane in 0..6 {
        let axis = plane / 2;
        let sign: i64 = if plane % 2 == 0 { 1 } else { -1 };
        // How far outside: positive beyond x = w (or x = -w).
        let outside = |v: &Vertex| sign * i64::from(v.pos[axis]) - i64::from(v.pos[3]);
        let mut out = Vec::with_capacity(poly.len() + 1);
        for i in 0..poly.len() {
            let a = poly[i];
            let b = poly[(i + 1) % poly.len()];
            let (da, db) = (outside(&a), outside(&b));
            if da <= 0 {
                out.push(a);
            }
            if (da <= 0) != (db <= 0) {
                out.push(lerp(&a, &b, da, da - db));
            }
        }
        poly = out;
        if poly.is_empty() {
            break;
        }
    }
    poly
}

/// The vertex `num / den` of the way from `a` to `b`.
fn lerp(a: &Vertex, b: &Vertex, num: i64, den: i64) -> Vertex {
    let mix = |p: i64, q: i64| (p + (q - p) * num / den) as i32;
    let mut v = Vertex::default();
    for i in 0..4 {
        v.pos[i] = mix(i64::from(a.pos[i]), i64::from(b.pos[i]));
    }
    for i in 0..3 {
        v.color[i] = mix(i64::from(a.color[i]), i64::from(b.color[i])).clamp(0, 63) as u8;
    }
    for i in 0..2 {
        v.tex[i] = mix(i64::from(a.tex[i]), i64::from(b.tex[i]));
    }
    v
}
