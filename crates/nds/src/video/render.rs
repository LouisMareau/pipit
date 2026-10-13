//! Drawing one scanline of each engine: the display modes, then for the
//! graphics mode the backgrounds (`bg.rs`) and objects (`obj.rs`) composed
//! through the windows and colour effects, engine A's display capture, and
//! the master brightness that ends the pipeline.
//!
//! Pixels are kept at 18 bits (six per channel, red lowest) with bit 31 set
//! when opaque, as the hardware blends at that depth; a 3D pixel will carry
//! its alpha in bits 24–28.

use super::{Video, SCREEN_WIDTH};

pub const OPAQUE: u32 = 1 << 31;
pub const TRANSPARENT: u32 = 0;
const WHITE: u32 = OPAQUE | 0x3FFFF;

/// A 15-bit palette colour as an opaque 18-bit pixel.
#[inline(always)]
pub fn color15(c: u16) -> u32 {
    let e = |v: u16| {
        let v = u32::from(v & 0x1F);
        (v << 1) | (v >> 4)
    };
    OPAQUE | e(c) | e(c >> 5) << 6 | e(c >> 10) << 12
}

/// An 18-bit pixel as 0xFFRRGGBB.
#[inline(always)]
fn to_argb(p: u32) -> u32 {
    let e = |v: u32| {
        let v = v & 0x3F;
        (v << 2) | (v >> 4)
    };
    0xFF00_0000 | e(p) << 16 | e(p >> 6) << 8 | e(p >> 12)
}

/// An 18-bit pixel back to 15 bits with the alpha bit (capture, VRAM).
#[inline(always)]
fn to_bgr15(p: u32) -> u16 {
    let e = |v: u32| ((v >> 1) & 0x1F) as u16;
    e(p) | e(p >> 6) << 5 | e(p >> 12) << 10 | 0x8000
}

#[inline(always)]
pub fn rd16(mem: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([mem[at], mem[at + 1]])
}

/// An object pixel waiting for composition.
#[derive(Clone, Copy)]
pub struct ObjPixel {
    pub color: u32,
    pub priority: u8,
    /// 0 normal, 1 semi-transparent (OBJ mode 1), 2 bitmap with `alpha`.
    pub kind: u8,
    pub alpha: u8,
}

pub const OBJ_NONE: ObjPixel = ObjPixel { color: TRANSPARENT, priority: 4, kind: 0, alpha: 0 };

/// One engine's per-line working buffers.
pub struct Scratch {
    pub bg: [[u32; SCREEN_WIDTH]; 4],
    pub obj: [ObjPixel; SCREEN_WIDTH],
    pub obj_window: [bool; SCREEN_WIDTH],
    /// The composed graphics line, before capture and master brightness.
    pub line: [u32; SCREEN_WIDTH],
}

impl Default for Scratch {
    fn default() -> Self {
        Self {
            bg: [[TRANSPARENT; SCREEN_WIDTH]; 4],
            obj: [OBJ_NONE; SCREEN_WIDTH],
            obj_window: [false; SCREEN_WIDTH],
            line: [TRANSPARENT; SCREEN_WIDTH],
        }
    }
}

/// Which layers and effects a window region lets through (a WININ/WINOUT byte).
#[derive(Clone, Copy)]
struct WindowFlags {
    layers: u8,
    effects: bool,
}

impl WindowFlags {
    fn from_bits(bits: u16) -> Self {
        Self { layers: (bits & 0x1F) as u8, effects: bits & 0x20 != 0 }
    }

    const ALL: WindowFlags = WindowFlags { layers: 0x1F, effects: true };
}

const LAYER_OBJ: usize = 4;
const LAYER_BACKDROP: usize = 5;

/// Window bounds compare like the hardware's counters: when the start is past
/// the end, the window wraps around the edge of the screen.
#[inline(always)]
fn in_window_range(v: usize, reg: u16) -> bool {
    let start = usize::from(reg >> 8);
    let end = usize::from(reg & 0xFF);
    if start <= end {
        (start..end).contains(&v)
    } else {
        v >= start || v < end
    }
}

/// Mixes two pixels: `(a × eva + b × evb) / 16` per channel, saturating.
#[inline(always)]
fn blend(a: u32, b: u32, eva: u32, evb: u32) -> u32 {
    let mut out = OPAQUE;
    for shift in [0, 6, 12] {
        let ca = (a >> shift) & 0x3F;
        let cb = (b >> shift) & 0x3F;
        out |= ((ca * eva + cb * evb) / 16).min(63) << shift;
    }
    out
}

#[inline(always)]
fn brighten(a: u32, evy: u32) -> u32 {
    let mut out = OPAQUE;
    for shift in [0, 6, 12] {
        let c = (a >> shift) & 0x3F;
        out |= (c + ((63 - c) * evy) / 16) << shift;
    }
    out
}

#[inline(always)]
fn darken(a: u32, evy: u32) -> u32 {
    let mut out = OPAQUE;
    for shift in [0, 6, 12] {
        let c = (a >> shift) & 0x3F;
        out |= (c - (c * evy) / 16) << shift;
    }
    out
}

pub(super) fn apply_mosaic(line: &mut [u32; SCREEN_WIDTH], width: usize) {
    let mut i = 0;
    while i < SCREEN_WIDTH {
        let src = line[i];
        let end = (i + width).min(SCREEN_WIDTH);
        line[i + 1..end].fill(src);
        i = end;
    }
}

impl Video {
    /// Draws line `y` of both engines into their screens.
    pub fn render_line(&mut self, y: usize) {
        for e in 0..2 {
            let screen = self.screen_of(e);
            let engine = &self.engines[e];
            let powered = self.powcnt1 & (1 << [1, 9][e]) != 0;
            let display_mode =
                if e == 0 { (engine.dispcnt >> 16) & 3 } else { (engine.dispcnt >> 16) & 1 };
            let forced_blank = engine.dispcnt & (1 << 7) != 0;
            let capturing = e == 0 && self.dispcapcnt & (1 << 31) != 0;
            let graphics = display_mode == 1 || (capturing && self.dispcapcnt & (1 << 24) == 0);
            if graphics && powered && !forced_blank {
                self.render_graphics(e, y);
            } else if graphics {
                self.scratch[e].line.fill(WHITE);
            }
            if capturing {
                self.capture(y);
            }
            let mut line = [WHITE; SCREEN_WIDTH];
            if powered && !forced_blank {
                match display_mode {
                    1 => line = self.scratch[e].line,
                    2 => {
                        let bank = ((self.engines[e].dispcnt >> 18) & 3) as usize;
                        for (x, px) in line.iter_mut().enumerate() {
                            *px = color15(self.vram.read_bank16(bank, y * 512 + x * 2));
                        }
                    }
                    3 => {
                        for (x, px) in line.iter_mut().enumerate() {
                            *px = color15(self.fifo[x]);
                        }
                    }
                    _ => {}
                }
                if display_mode != 0 {
                    self.master_brightness(e, &mut line);
                }
            }
            let row = &mut self.screens[screen][y * SCREEN_WIDTH..(y + 1) * SCREEN_WIDTH];
            for (dst, src) in row.iter_mut().zip(line.iter()) {
                *dst = to_argb(*src);
            }
        }
    }

    /// MASTER_BRIGHT: fades the whole line towards white or black.
    fn master_brightness(&self, e: usize, line: &mut [u32; SCREEN_WIDTH]) {
        let reg = self.engines[e].master_bright;
        let factor = u32::from(reg & 0x1F).min(16);
        if factor == 0 {
            return;
        }
        match reg >> 14 {
            1 => line.iter_mut().for_each(|p| *p = brighten(*p, factor)),
            2 => line.iter_mut().for_each(|p| *p = darken(*p, factor)),
            _ => {}
        }
    }

    /// The graphics mode: backgrounds, objects, composition into `scratch.line`.
    fn render_graphics(&mut self, e: usize, y: usize) {
        for bg in &mut self.scratch[e].bg {
            bg.fill(TRANSPARENT);
        }
        self.render_bgs(e, y);
        self.scratch[e].obj.fill(OBJ_NONE);
        self.scratch[e].obj_window.fill(false);
        if self.engines[e].dispcnt & (1 << 12) != 0 {
            self.render_objs(e, y);
        }
        self.compose(e, y);
    }

    /// DISPCAPCNT: writes this line of the capture into its VRAM bank.
    fn capture(&mut self, y: usize) {
        let cap = self.dispcapcnt;
        let (w, h) = [(128, 128), (256, 64), (256, 128), (256, 192)][((cap >> 20) & 3) as usize];
        if y >= h {
            return;
        }
        let eva = (cap & 0x1F).min(16);
        let evb = ((cap >> 8) & 0x1F).min(16);
        let bank = ((cap >> 16) & 3) as usize;
        let write_offset = (((cap >> 18) & 3) << 15) as usize;
        let source_a_3d = cap & (1 << 24) != 0;
        let source_b_fifo = cap & (1 << 25) != 0;
        let display_mode = (self.engines[0].dispcnt >> 16) & 3;
        let read_bank = ((self.engines[0].dispcnt >> 18) & 3) as usize;
        let read_offset = if display_mode == 2 { 0 } else { (((cap >> 26) & 3) << 15) as usize };
        let source = (cap >> 29) & 3;
        for x in 0..w {
            // No 3D yet: its pixels are transparent.
            let a: u16 = if source_a_3d { 0 } else { to_bgr15(self.scratch[0].line[x]) };
            let b: u16 = if source_b_fifo {
                self.fifo[x]
            } else {
                self.vram.read_bank16(read_bank, read_offset + (y * w + x) * 2)
            };
            let value = match source {
                0 => a,
                1 => b,
                _ => {
                    let ca = if a & 0x8000 != 0 { a } else { 0 };
                    let cb = if b & 0x8000 != 0 { b } else { 0 };
                    let mut out = (a | b) & 0x8000;
                    for shift in [0, 5, 10] {
                        let va = u32::from((ca >> shift) & 0x1F);
                        let vb = u32::from((cb >> shift) & 0x1F);
                        out |= (((va * eva + vb * evb) / 16).min(31) as u16) << shift;
                    }
                    out
                }
            };
            self.vram.write_bank16(bank, (write_offset + (y * w + x) * 2) & 0x1_FFFF, value);
        }
    }
}

impl Video {
    fn compose(&mut self, e: usize, y: usize) {
        let engine = &self.engines[e];
        let scratch = &mut self.scratch[e];
        let dispcnt = engine.dispcnt;
        let backdrop = color15(rd16(&self.palette[..], e * 0x400));
        let win0_on = dispcnt & (1 << 13) != 0;
        let win1_on = dispcnt & (1 << 14) != 0;
        let objwin_on = dispcnt & (1 << 15) != 0;
        let any_window = win0_on || win1_on || objwin_on;
        let win0_y = win0_on && in_window_range(y, engine.winv(0));
        let win1_y = win1_on && in_window_range(y, engine.winv(1));
        let (winh0, winh1) = (engine.winh(0), engine.winh(1));
        let win_in0 = WindowFlags::from_bits(engine.winin());
        let win_in1 = WindowFlags::from_bits(engine.winin() >> 8);
        let win_out = WindowFlags::from_bits(engine.winout());
        let win_obj = WindowFlags::from_bits(engine.winout() >> 8);

        let bldcnt = engine.bldcnt();
        let blend_mode = (bldcnt >> 6) & 3;
        let first_targets = (bldcnt & 0x3F) as u8;
        let second_targets = ((bldcnt >> 8) & 0x3F) as u8;
        let eva = u32::from(engine.bldalpha() & 0x1F).min(16);
        let evb = u32::from((engine.bldalpha() >> 8) & 0x1F).min(16);
        let evy = u32::from(engine.bldy() & 0x1F).min(16);
        let bg_priority: [u8; 4] = std::array::from_fn(|n| (engine.bgcnt(n) & 3) as u8);
        let bg0_is_3d = e == 0 && (dispcnt & (1 << 3) != 0 || engine.mode() == 6);

        for x in 0..SCREEN_WIDTH {
            let flags = if !any_window {
                WindowFlags::ALL
            } else if win0_y && in_window_range(x, winh0) {
                win_in0
            } else if win1_y && in_window_range(x, winh1) {
                win_in1
            } else if objwin_on && scratch.obj_window[x] {
                win_obj
            } else {
                win_out
            };

            // The two topmost opaque layers: among equal priorities, objects
            // beat backgrounds and lower background numbers beat higher.
            let mut top = (LAYER_BACKDROP, backdrop);
            let mut second = (LAYER_BACKDROP, backdrop);
            let mut found = 0;
            let obj = scratch.obj[x];
            let obj_visible = obj.color != TRANSPARENT && flags.layers & (1 << LAYER_OBJ) != 0;
            'search: for prio in 0..4u8 {
                if obj_visible && obj.priority == prio {
                    if found == 0 {
                        top = (LAYER_OBJ, obj.color);
                    } else {
                        second = (LAYER_OBJ, obj.color);
                        break 'search;
                    }
                    found += 1;
                }
                for (bg, &bg_prio) in bg_priority.iter().enumerate() {
                    if bg_prio != prio || flags.layers & (1 << bg) == 0 {
                        continue;
                    }
                    let c = scratch.bg[bg][x];
                    if c == TRANSPARENT {
                        continue;
                    }
                    if found == 0 {
                        top = (bg, c);
                    } else {
                        second = (bg, c);
                        break 'search;
                    }
                    found += 1;
                }
            }

            let mut color = top.1;
            if flags.effects {
                let top_is_first = first_targets & (1 << top.0) != 0;
                let second_is_second = second_targets & (1 << second.0) != 0;
                let obj_semi = top.0 == LAYER_OBJ && obj.kind == 1;
                let obj_bitmap = top.0 == LAYER_OBJ && obj.kind == 2;
                let top_3d = top.0 == 0 && bg0_is_3d;
                if obj_bitmap && second_is_second {
                    let a = u32::from(obj.alpha) + 1;
                    color = blend(top.1, second.1, a, 16 - a);
                } else if top_3d && second_is_second {
                    let a = ((top.1 >> 24) & 0x1F) + 1;
                    color = blend(top.1, second.1, a / 2, 16 - a / 2);
                } else if (obj_semi || (blend_mode == 1 && top_is_first)) && second_is_second {
                    color = blend(top.1, second.1, eva, evb);
                } else if top_is_first || obj_semi || obj_bitmap {
                    match blend_mode {
                        2 => color = brighten(top.1, evy),
                        3 => color = darken(top.1, evy),
                        _ => {}
                    }
                }
            }
            scratch.line[x] = color | OPAQUE;
        }
    }
}
