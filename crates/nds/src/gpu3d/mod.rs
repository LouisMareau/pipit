//! The 3D engine (GBATEK "DS 3D Video"): the geometry engine takes commands
//! from its FIFO, transforms and lights vertices, clips and assembles
//! polygons; at VBlank a swap hands the polygons to the rendering engine,
//! which draws the frame engine A then shows as its BG0.
//!
//! Commands run as soon as their parameters are in: the FIFO never fills,
//! so the ARM9 never waits on it. The one wait the hardware imposes stays:
//! after SWAP_BUFFERS the commands that follow queue up until the swap.

pub mod geometry;
pub mod matrix;
pub mod render;
pub mod texture;

#[cfg(test)]
mod tests;

use crate::video::vram::Vram;
use matrix::Matrix;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

pub const WIDTH: usize = 256;
pub const HEIGHT: usize = 192;
pub const MAX_POLYGONS: usize = 2048;
pub const MAX_VERTICES: usize = 6144;

/// A vertex in clip space, with what the renderer interpolates.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Vertex {
    /// x, y, z, w in 20.12.
    pub pos: [i32; 4],
    /// 0–63 per channel.
    pub color: [u8; 3],
    /// s, t in 12.4.
    pub tex: [i32; 2],
    /// After clipping: the pixel position and the 24-bit depth.
    pub screen: [i32; 2],
    pub depth: i32,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Polygon {
    pub attr: u32,
    pub teximage: u32,
    pub pltt_base: u32,
    /// Its vertices in `vertices`, in order around the polygon.
    pub first: u32,
    pub count: u8,
    /// Drawn after the opaque polygons: alpha below 31, or a texture with alpha.
    pub translucent: bool,
}

/// The polygons submitted for one frame.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Scene {
    pub polygons: Vec<Polygon>,
    pub vertices: Vec<Vertex>,
}

/// What a primitive is being built from (BEGIN_VTXS).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Primitive {
    Triangles,
    Quads,
    TriangleStrip,
    QuadStrip,
}

#[derive(Serialize, Deserialize)]
pub struct Gpu3d {
    /// DISP3DCNT: texturing (0), highlight shading (1), alpha test (2),
    /// blending (3), edge marking (5), fog alpha only (6), fog (7), fog shift
    /// (8-11), rear plane from the clear image (14).
    pub disp3dcnt: u16,
    pub clear_color: u32,
    pub clear_depth: u16,
    pub clrimage_offset: u16,
    pub fog_color: u32,
    pub fog_offset: u16,
    pub fog_table: [u8; 32],
    pub toon_table: [u16; 32],
    pub edge_color: [u16; 8],
    pub alpha_ref: u16,
    pub onedot_depth: u16,
    /// GXSTAT bits 30-31: when the FIFO interrupt is wanted (1 below half, 2 empty).
    pub irq_mode: u8,
    pub stack_error: bool,
    pub box_result: bool,
    pub pos_result: [i32; 4],
    pub vec_result: [i16; 3],

    // The FIFO, as the packed words are taken apart: the commands waiting
    // with how many parameters each takes, and the parameters received.
    pending: VecDeque<(u8, u8)>,
    params: Vec<u32>,
    /// SWAP_BUFFERS was given: its parameters, until the swap at VBlank.
    swap: Option<u32>,

    // The geometry engine.
    pub mtx_mode: u8,
    pub proj: Matrix,
    pub pos: Matrix,
    pub vec: Matrix,
    pub tex: Matrix,
    #[serde(with = "pipit_common::snapshot::array")]
    pos_stack: [Matrix; 32],
    #[serde(with = "pipit_common::snapshot::array")]
    vec_stack: [Matrix; 32],
    proj_stack: Matrix,
    tex_stack: Matrix,
    pub sp_pos: u8,
    pub sp_proj: u8,
    pub color: [u8; 3],
    pub tex_raw: [i32; 2],
    pub texcoord: [i32; 2],
    pub polygon_attr: u32,
    pub teximage: u32,
    pub pltt_base: u32,
    pub lights: [[i32; 3]; 4],
    pub light_colors: [[u8; 3]; 4],
    pub diffuse: [u8; 3],
    pub ambient: [u8; 3],
    pub specular: [u8; 3],
    pub emission: [u8; 3],
    #[serde(with = "pipit_common::snapshot::bytes_box")]
    pub shininess: Box<[u8; 128]>,
    pub use_shininess: bool,
    pub viewport: [u8; 4],
    primitive: Primitive,
    /// The polygon attributes latched at BEGIN_VTXS.
    vtx_attr: u32,
    prim: [Vertex; 4],
    prim_count: usize,
    /// Strips: vertices form polygons with the previous ones.
    strip_odd: bool,
    last_vertex: [i32; 3],
    /// The scene being built, and the one handed over at the last swap.
    pub building: Scene,
    pub scene: Scene,
    /// The swap's parameters for the scene: manual translucent sort (0), W buffering (1).
    pub scene_flags: u32,
    pub overflow: bool,
    /// Set when the swap handed a scene over; the bus renders and clears it.
    pub render_pending: bool,
}

/// How many parameters a command takes; `None` for commands that do not exist.
fn param_count(cmd: u8) -> Option<u8> {
    Some(match cmd {
        0x10 | 0x12..=0x14 => 1,
        0x11 | 0x15 | 0x41 => 0,
        0x16 | 0x18 => 16,
        0x17 | 0x19 => 12,
        0x1A => 9,
        0x1B | 0x1C | 0x70 => 3,
        0x20..=0x22 | 0x24..=0x2B | 0x30..=0x33 | 0x40 | 0x50 | 0x60 | 0x72 => 1,
        0x23 | 0x71 => 2,
        0x34 => 32,
        _ => return None,
    })
}

impl Default for Gpu3d {
    fn default() -> Self {
        Self::new()
    }
}

impl Gpu3d {
    pub fn new() -> Self {
        Self {
            disp3dcnt: 0,
            clear_color: 0,
            clear_depth: 0,
            clrimage_offset: 0,
            fog_color: 0,
            fog_offset: 0,
            fog_table: [0; 32],
            toon_table: [0; 32],
            edge_color: [0; 8],
            alpha_ref: 0,
            onedot_depth: 0x7FFF,
            irq_mode: 0,
            stack_error: false,
            box_result: false,
            pos_result: [0; 4],
            vec_result: [0; 3],
            pending: VecDeque::new(),
            params: Vec::new(),
            swap: None,
            mtx_mode: 0,
            proj: Matrix::IDENTITY,
            pos: Matrix::IDENTITY,
            vec: Matrix::IDENTITY,
            tex: Matrix::IDENTITY,
            pos_stack: [Matrix::IDENTITY; 32],
            vec_stack: [Matrix::IDENTITY; 32],
            proj_stack: Matrix::IDENTITY,
            tex_stack: Matrix::IDENTITY,
            sp_pos: 0,
            sp_proj: 0,
            color: [31; 3],
            tex_raw: [0; 2],
            texcoord: [0; 2],
            polygon_attr: 0,
            teximage: 0,
            pltt_base: 0,
            lights: [[0, 0, -0x200]; 4],
            light_colors: [[0; 3]; 4],
            diffuse: [0; 3],
            ambient: [0; 3],
            specular: [0; 3],
            emission: [0; 3],
            shininess: Box::new([0; 128]),
            use_shininess: false,
            viewport: [0, 0, 255, 191],
            primitive: Primitive::Triangles,
            vtx_attr: 0,
            prim: [Vertex::default(); 4],
            prim_count: 0,
            strip_odd: false,
            last_vertex: [0; 3],
            building: Scene::default(),
            scene: Scene::default(),
            scene_flags: 0,
            overflow: false,
            render_pending: false,
        }
    }

    fn params_wanted(&self) -> usize {
        self.pending.iter().map(|&(_, n)| usize::from(n)).sum()
    }

    /// A word for the FIFO (0400h–043Fh): a pack of up to four commands,
    /// first in the low byte, or a parameter for the commands already in.
    pub fn write_fifo(&mut self, word: u32) {
        if self.params.len() < self.params_wanted() {
            self.params.push(word);
        } else {
            for i in 0..4 {
                let cmd = (word >> (i * 8)) as u8;
                if let Some(n) = param_count(cmd) {
                    self.pending.push_back((cmd, n));
                }
            }
        }
        self.run_ready();
    }

    /// A command written to its own port (0440h–05CBh): one parameter per
    /// write, so a multi-parameter command spans several writes.
    pub fn write_port(&mut self, reg: u32, word: u32) {
        let cmd = ((reg - 0x400) / 4) as u8;
        let Some(n) = param_count(cmd) else { return };
        let continuing = self.pending.back().is_some_and(|&(c, k)| c == cmd && k > 0)
            && self.params.len() < self.params_wanted();
        if !continuing {
            self.pending.push_back((cmd, n));
        }
        if n > 0 {
            self.params.push(word);
        }
        self.run_ready();
    }

    /// Runs the commands at the front of the queue whose parameters are in,
    /// until one is waiting for the swap.
    fn run_ready(&mut self) {
        while self.swap.is_none() {
            let Some(&(cmd, n)) = self.pending.front() else { break };
            let n = usize::from(n);
            if self.params.len() < n {
                break;
            }
            let params: Vec<u32> = self.params.drain(..n).collect();
            self.pending.pop_front();
            self.execute(cmd, &params);
        }
    }

    /// VBlank: a pending swap hands the scene over and renders it; returns
    /// whether the FIFO interrupt should fire (commands waiting ran).
    pub fn on_vblank(&mut self, vram: &Vram, out: &mut [u32]) -> bool {
        let Some(flags) = self.swap.take() else { return false };
        self.scene = std::mem::take(&mut self.building);
        self.scene_flags = flags;
        self.render(vram, out);
        self.run_ready();
        self.irq_wanted()
    }

    /// The FIFO interrupt condition: it never holds anything, so the
    /// condition is met whenever one is asked for.
    pub fn irq_wanted(&self) -> bool {
        matches!(self.irq_mode, 1 | 2)
    }

    pub fn gxstat(&self) -> u32 {
        u32::from(self.box_result) << 1
            | u32::from(self.sp_pos & 0x1F) << 8
            | u32::from(self.sp_proj & 1) << 13
            | u32::from(self.stack_error) << 15
            | 1 << 25
            | 1 << 26
            | u32::from(self.swap.is_some()) << 27
            | u32::from(self.irq_mode & 3) << 30
    }

    /// GXSTAT written: acknowledges the stack error, sets the interrupt mode;
    /// returns whether the interrupt should fire now.
    pub fn write_gxstat(&mut self, value: u32, mask: u32) -> bool {
        if mask & 0x8000 != 0 && value & 0x8000 != 0 {
            self.stack_error = false;
        }
        if mask & 0xC000_0000 != 0 {
            self.irq_mode = (value >> 30) as u8 & 3;
            return self.irq_wanted();
        }
        false
    }

    /// The clip matrix: position × projection.
    pub fn clip_matrix(&self) -> Matrix {
        let mut m = self.pos;
        m.mul_right(&self.proj);
        m
    }
}

impl Gpu3d {
    pub fn write_disp3dcnt(&mut self, value: u16, mask: u16) {
        let v = value & mask;
        // Bit 13 (polygon or vertex RAM overflow) is acknowledged by writing 1.
        if v & (1 << 13) != 0 {
            self.overflow = false;
        }
        self.disp3dcnt = (self.disp3dcnt & !mask) | (v & 0x4FFF);
    }

    /// The registers at 060h, 320h–3BFh and 600h–6A3h as 16-bit halves.
    pub fn read_io16(&self, reg: u32) -> u16 {
        let half = |v: u32| if reg & 2 == 0 { v as u16 } else { (v >> 16) as u16 };
        match reg {
            0x060 => self.disp3dcnt | u16::from(self.overflow) << 13,
            0x600 | 0x602 => half(self.gxstat()),
            0x604 => self.building.polygons.len() as u16,
            0x606 => self.building.vertices.len() as u16,
            0x620..=0x62E => half(self.pos_result[((reg - 0x620) / 4) as usize] as u32),
            0x630..=0x634 => self.vec_result[((reg - 0x630) / 2) as usize] as u16,
            0x640..=0x67E => half(self.clip_matrix().0[((reg - 0x640) / 4) as usize] as u32),
            0x680..=0x6A2 => {
                let i = ((reg - 0x680) / 4) as usize;
                half(self.vec.0[(i / 3) * 4 + i % 3] as u32)
            }
            _ => 0,
        }
    }

    /// A 16-bit register write; returns whether the FIFO interrupt should fire.
    pub fn write_io16(&mut self, reg: u32, value: u16, mask: u16) -> bool {
        let v = value & mask;
        let merge = |old: u16| (old & !mask) | v;
        match reg {
            0x330..=0x33E => {
                let i = ((reg - 0x330) / 2) as usize;
                self.edge_color[i] = merge(self.edge_color[i]) & 0x7FFF;
            }
            0x340 => self.alpha_ref = merge(self.alpha_ref) & 0x1F,
            0x350 => self.clear_color = (self.clear_color & !u32::from(mask)) | u32::from(v),
            0x352 => {
                self.clear_color =
                    (self.clear_color & !(u32::from(mask) << 16)) | (u32::from(v & 0x3F1F) << 16)
            }
            0x354 => self.clear_depth = merge(self.clear_depth) & 0x7FFF,
            0x356 => self.clrimage_offset = merge(self.clrimage_offset),
            0x358 => self.fog_color = (self.fog_color & !u32::from(mask)) | u32::from(v & 0x7FFF),
            0x35A => {
                self.fog_color =
                    (self.fog_color & !(u32::from(mask) << 16)) | (u32::from(v & 0x1F) << 16)
            }
            0x35C => self.fog_offset = merge(self.fog_offset) & 0x7FFF,
            0x360..=0x37E => {
                let i = (reg - 0x360) as usize;
                if mask & 0x00FF != 0 {
                    self.fog_table[i] = v as u8 & 0x7F;
                }
                if mask & 0xFF00 != 0 {
                    self.fog_table[i + 1] = (v >> 8) as u8 & 0x7F;
                }
            }
            0x380..=0x3BE => {
                let i = ((reg - 0x380) / 2) as usize;
                self.toon_table[i] = merge(self.toon_table[i]) & 0x7FFF;
            }
            0x600 => return self.write_gxstat(u32::from(value), u32::from(mask)),
            0x602 => return self.write_gxstat(u32::from(value) << 16, u32::from(mask) << 16),
            0x610 => self.onedot_depth = merge(self.onedot_depth) & 0x7FFF,
            _ => {}
        }
        false
    }

    /// A word for the FIFO or a command port.
    pub fn write_io32(&mut self, reg: u32, value: u32) {
        match reg {
            0x400..=0x43F => self.write_fifo(value),
            0x440..=0x5FF => self.write_port(reg, value),
            _ => {}
        }
    }
}
