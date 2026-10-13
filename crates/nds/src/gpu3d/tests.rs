//! The 3D engine driven by commands, rendered at a swap, checked per pixel.

use super::*;
use crate::video::render::OPAQUE;
use crate::video::vram::{Region, Vram};

const RED: u32 = 0x001F;
const GREEN: u32 = 0x03E0;
const BLUE: u32 = 0x7C00;

/// 1.3.12 vertex coordinates from floats.
fn fx(v: f32) -> u32 {
    ((v * 4096.0) as i32 as u16) as u32
}

fn vtx16(x: f32, y: f32, z: f32) -> [u32; 2] {
    [fx(x) | fx(y) << 16, fx(z)]
}

/// A 15-bit colour as the frame holds it: six bits per channel, opaque.
fn pixel(c: u32, alpha: u32) -> u32 {
    let e = |v: u32| {
        let v = v & 0x1F;
        (v << 1) | (v >> 4)
    };
    OPAQUE | e(c) | e(c >> 5) << 6 | e(c >> 10) << 12 | alpha << 24
}

struct Rig {
    gpu: Gpu3d,
    vram: Vram,
    out: Vec<u32>,
}

impl Rig {
    fn new() -> Self {
        let mut gpu = Gpu3d::new();
        gpu.disp3dcnt = 1 | 1 << 3; // textures, blending
        gpu.clear_depth = 0x7FFF; // the rear plane at the back, as games set it
        Self { gpu, vram: Vram::new(), out: vec![0; WIDTH * HEIGHT] }
    }

    fn cmd(&mut self, cmd: u8, params: &[u32]) {
        let reg = 0x400 + u32::from(cmd) * 4;
        if params.is_empty() {
            self.gpu.write_port(reg, 0);
        }
        for &p in params {
            self.gpu.write_port(reg, p);
        }
    }

    /// Identity matrices, the full viewport, an opaque polygon drawn from
    /// both sides, ready for BEGIN_VTXS.
    fn setup(&mut self, attr: u32) {
        self.cmd(0x60, &[0xBFFF_0000]); // viewport 0,0 – 255,191
        self.cmd(0x10, &[0]);
        self.cmd(0x15, &[]);
        self.cmd(0x10, &[2]);
        self.cmd(0x15, &[]);
        self.cmd(0x29, &[attr]);
    }

    fn triangle(&mut self, color: u32, verts: [[f32; 3]; 3]) {
        self.cmd(0x40, &[0]);
        self.cmd(0x20, &[color]);
        for v in verts {
            self.cmd(0x23, &vtx16(v[0], v[1], v[2]));
        }
        self.cmd(0x41, &[]);
    }

    fn quad(&mut self, color: u32, z: f32, half: f32) {
        self.cmd(0x40, &[1]);
        self.cmd(0x20, &[color]);
        for (s, t, v) in [
            (0u32, 0u32, [-half, -half]),
            (16, 0, [half, -half]),
            (16, 16, [half, half]),
            (0, 16, [-half, half]),
        ] {
            self.cmd(0x22, &[s << 4 | (t << 4) << 16]);
            self.cmd(0x23, &vtx16(v[0], v[1], z));
        }
        self.cmd(0x41, &[]);
    }

    fn swap(&mut self, flags: u32) {
        self.cmd(0x50, &[flags]);
        self.gpu.on_vblank(&self.vram, &mut self.out);
    }

    fn px(&self, x: usize, y: usize) -> u32 {
        self.out[y * WIDTH + x]
    }
}

const OPAQUE_BOTH: u32 = 0x1F << 16 | 3 << 6;

#[test]
fn a_triangle_is_filled_and_culled_by_its_facing() {
    let mut r = Rig::new();
    r.setup(0x1F << 16 | 1 << 7); // front faces only
    r.triangle(RED, [[-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [0.0, 1.0, 0.0]]);
    r.swap(0);
    assert_eq!(r.px(128, 100), pixel(RED, 31));
    assert_eq!(r.px(10, 10), 0);
    assert_eq!(r.px(128, 191), pixel(RED, 31));
    // The same vertices the other way round face away: nothing is drawn.
    r.triangle(RED, [[0.0, 1.0, 0.0], [1.0, -1.0, 0.0], [-1.0, -1.0, 0.0]]);
    r.swap(0);
    assert_eq!(r.px(128, 100), 0);
    // Unless back faces are drawn too.
    r.setup(OPAQUE_BOTH);
    r.triangle(GREEN, [[0.0, 1.0, 0.0], [1.0, -1.0, 0.0], [-1.0, -1.0, 0.0]]);
    r.swap(0);
    assert_eq!(r.px(128, 100), pixel(GREEN, 31));
}

#[test]
fn the_nearer_polygon_wins_whatever_the_order() {
    let mut r = Rig::new();
    r.setup(OPAQUE_BOTH);
    r.quad(GREEN, 0.5, 0.9);
    r.quad(RED, -0.5, 0.5);
    r.swap(0);
    assert_eq!(r.px(128, 96), pixel(RED, 31));
    assert_eq!(r.px(20, 96), pixel(GREEN, 31));
    r.quad(RED, -0.5, 0.5);
    r.quad(GREEN, 0.5, 0.9);
    r.swap(0);
    assert_eq!(r.px(128, 96), pixel(RED, 31));
    // With W-buffering the nearer one still wins (both have w = 1: the order decides).
    r.quad(RED, -0.5, 0.5);
    r.quad(GREEN, 0.5, 0.9);
    r.swap(2);
    assert_eq!(r.px(20, 96), pixel(GREEN, 31));
}

#[test]
fn translucent_polygons_blend_over_what_is_behind() {
    let mut r = Rig::new();
    r.setup(OPAQUE_BOTH);
    r.quad(RED, 0.5, 0.9);
    r.cmd(0x29, &[15 << 16 | 3 << 6 | 1 << 24]); // alpha 15, polygon id 1
    r.quad(BLUE, -0.5, 0.5);
    r.swap(0);
    // (63 × 16 + 0 × 16) / 32 red, (0 + 63 × 16) / 32 blue: half and half.
    let p = r.px(128, 96);
    assert_eq!(p & 0x3F, 31);
    assert_eq!((p >> 12) & 0x3F, 31);
    assert_eq!(p >> 24 & 0x1F, 31); // the opaque pixel keeps its alpha
    assert_eq!(r.px(20, 96), pixel(RED, 31));
}

#[test]
fn textures_map_onto_polygons() {
    let mut r = Rig::new();
    r.setup(OPAQUE_BOTH);
    // Bank A holds textures, bank E the palettes.
    r.vram.write_cnt(0, 0x83);
    r.vram.write_cnt(4, 0x83);
    // A 16-colour 8 × 8 texture at 0: the left half colour 1, the right colour 2.
    for y in 0..8 {
        r.vram.write16(Region::Texture, y * 4, 0x1111);
        r.vram.write16(Region::Texture, y * 4 + 2, 0x2222);
    }
    r.vram.write16(Region::TexPalette, 2, GREEN as u16);
    r.vram.write16(Region::TexPalette, 4, BLUE as u16);
    r.cmd(0x2A, &[3 << 26]); // 16 colours, 8 × 8, at 0
    r.cmd(0x2B, &[0]);
    r.quad(0x7FFF, 0.0, 0.5);
    r.swap(0);
    assert_eq!(r.px(70, 96), pixel(GREEN, 31));
    assert_eq!(r.px(186, 96), pixel(BLUE, 31));
    // Modulation with a half-bright vertex colour halves the texel.
    r.cmd(0x2A, &[3 << 26]);
    r.quad(0x0210, 0.0, 0.5); // green 16
    r.swap(0);
    let p = r.px(70, 96);
    assert_eq!((p >> 6) & 0x3F, ((63 + 1) * (33 + 1) - 1) >> 6);
}

#[test]
fn toon_shading_takes_the_table_by_the_red_level() {
    let mut r = Rig::new();
    r.setup(OPAQUE_BOTH | 2 << 4);
    r.gpu.toon_table[31] = BLUE as u16;
    r.quad(RED, 0.0, 0.5); // red 31: table entry 31
    r.swap(0);
    assert_eq!(r.px(128, 96), pixel(BLUE, 31));
}

#[test]
fn packed_fifo_words_carry_several_commands() {
    let mut r = Rig::new();
    // MTX_MODE 3, MTX_IDENTITY, COLOR, then the parameters of the two that take one.
    r.gpu.write_fifo(0x20 << 16 | 0x15 << 8 | 0x10);
    r.gpu.write_fifo(3);
    r.gpu.write_fifo(GREEN);
    assert_eq!(r.gpu.mtx_mode, 3);
    assert_eq!(r.gpu.color, [0, 31, 0]);
    // Commands after SWAP_BUFFERS wait for the swap.
    r.gpu.write_fifo(0x20 << 8 | 0x50);
    r.gpu.write_fifo(0);
    r.gpu.write_fifo(RED);
    assert_eq!(r.gpu.color, [0, 31, 0]);
    assert_ne!(r.gpu.gxstat() & (1 << 27), 0);
    r.gpu.on_vblank(&r.vram, &mut r.out);
    assert_eq!(r.gpu.color, [31, 0, 0]);
    assert_eq!(r.gpu.gxstat() & (1 << 27), 0);
}

#[test]
fn the_box_test_sees_what_is_in_view() {
    let mut r = Rig::new();
    r.setup(OPAQUE_BOTH);
    r.cmd(0x70, &[fx(-0.5) | fx(-0.5) << 16, fx(-0.5) | fx(1.0) << 16, fx(1.0) | fx(1.0) << 16]);
    assert!(r.gpu.box_result);
    r.cmd(0x70, &[fx(2.0) | fx(2.0) << 16, fx(2.0) | fx(1.0) << 16, fx(1.0) | fx(1.0) << 16]);
    assert!(!r.gpu.box_result);
    assert_eq!(r.gpu.gxstat() & 2, 0);
}
