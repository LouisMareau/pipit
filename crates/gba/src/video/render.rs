//! Scanline renderer (GBATEK "LCD I/O BG Control", "LCD OBJ", "LCD I/O Window",
//! "LCD I/O Color Special Effects").
//!
//! Each visible line is drawn in three passes: every enabled background into its
//! own line buffer, all sprites into one line buffer, then composition pixel by
//! pixel through the windows and colour effects.

use super::Video;
use crate::SCREEN_WIDTH;

/// Marks a transparent pixel in the 15-bit line buffers (bit 15 is unused by BGR555).
const TRANSPARENT: u16 = 0x8000;

const LAYER_BG0: usize = 0;
const LAYER_OBJ: usize = 4;
const LAYER_BACKDROP: usize = 5;

/// A sprite pixel waiting for composition.
#[derive(Clone, Copy)]
struct ObjPixel {
    colour: u16,
    priority: u8,
    /// OBJ mode 1: forces alpha blending with whatever lies beneath.
    semi_transparent: bool,
}

const OBJ_NONE: ObjPixel = ObjPixel { colour: TRANSPARENT, priority: 4, semi_transparent: false };

/// Per-scanline working buffers, kept between lines to avoid reallocation.
pub struct Scratch {
    bg: [[u16; SCREEN_WIDTH]; 4],
    obj: [ObjPixel; SCREEN_WIDTH],
    obj_window: [bool; SCREEN_WIDTH],
}

impl Default for Scratch {
    fn default() -> Self {
        Self {
            bg: [[TRANSPARENT; SCREEN_WIDTH]; 4],
            obj: [OBJ_NONE; SCREEN_WIDTH],
            obj_window: [false; SCREEN_WIDTH],
        }
    }
}

/// Converts a 15-bit BGR555 colour to 0xFFRRGGBB.
#[inline(always)]
pub fn bgr555_to_argb(c: u16) -> u32 {
    let r = u32::from(c & 0x1F);
    let g = u32::from((c >> 5) & 0x1F);
    let b = u32::from((c >> 10) & 0x1F);
    // Spread 5 bits over 8 (x << 3 | x >> 2) so white is exactly 0xFF.
    0xFF00_0000 | ((r << 3 | r >> 2) << 16) | ((g << 3 | g >> 2) << 8) | (b << 3 | b >> 2)
}

#[inline(always)]
fn rd16(mem: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([mem[off], mem[off + 1]])
}

/// Which layers and effects a window region lets through (WININ/WINOUT nibble).
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

impl Video {
    pub(super) fn render_scanline(&mut self) {
        let y = usize::from(self.vcount);
        let mode = self.dispcnt & 7;

        if self.dispcnt & (1 << 7) != 0 {
            // Forced blank: the LCD shows white.
            self.framebuffer[y * SCREEN_WIDTH..(y + 1) * SCREEN_WIDTH].fill(0xFFFF_FFFF);
            self.advance_affine();
            return;
        }

        for bg in 0..4 {
            self.scratch.bg[bg].fill(TRANSPARENT);
        }
        match mode {
            0 => {
                for bg in 0..4 {
                    if self.bg_enabled(bg) {
                        self.render_text_bg(bg, y);
                    }
                }
            }
            1 => {
                for bg in 0..2 {
                    if self.bg_enabled(bg) {
                        self.render_text_bg(bg, y);
                    }
                }
                if self.bg_enabled(2) {
                    self.render_affine_bg(2, y);
                }
            }
            2 => {
                for bg in 2..4 {
                    if self.bg_enabled(bg) {
                        self.render_affine_bg(bg, y);
                    }
                }
            }
            3..=5 if self.bg_enabled(2) => self.render_bitmap_bg(mode, y),
            _ => {}
        }

        self.scratch.obj.fill(OBJ_NONE);
        self.scratch.obj_window.fill(false);
        if self.dispcnt & (1 << 12) != 0 {
            self.render_sprites(y);
        }

        self.compose(y);
        self.advance_affine();
    }

    #[inline]
    fn bg_enabled(&self, bg: usize) -> bool {
        self.dispcnt & (1 << (8 + bg)) != 0
    }

    /// Affine reference points step by dmx/dmy once per line.
    fn advance_affine(&mut self) {
        for bg in 0..2 {
            self.bgx_internal[bg] = self.bgx_internal[bg].wrapping_add(i32::from(self.bgpb[bg]));
            self.bgy_internal[bg] = self.bgy_internal[bg].wrapping_add(i32::from(self.bgpd[bg]));
        }
    }

    // ---------------------------------------------------------------------------
    // Backgrounds
    // ---------------------------------------------------------------------------

    fn render_text_bg(&mut self, bg: usize, y: usize) {
        let cnt = self.bgcnt[bg];
        let char_base = usize::from((cnt >> 2) & 3) * 0x4000;
        let screen_base = usize::from((cnt >> 8) & 0x1F) * 0x800;
        let colours_256 = cnt & (1 << 7) != 0;
        let size = (cnt >> 14) & 3;
        let width_tiles = if size & 1 != 0 { 64 } else { 32 };
        let height_tiles = if size & 2 != 0 { 64 } else { 32 };

        let mosaic = cnt & (1 << 6) != 0;
        let (mosaic_w, mosaic_h) = if mosaic {
            (usize::from(self.mosaic & 0xF) + 1, usize::from((self.mosaic >> 4) & 0xF) + 1)
        } else {
            (1, 1)
        };
        let y = if mosaic { y - y % mosaic_h } else { y };

        let scroll_x = usize::from(self.bghofs[bg] & 0x1FF);
        let scroll_y = usize::from(self.bgvofs[bg] & 0x1FF);
        let map_y = (y + scroll_y) % (height_tiles * 8);
        let tile_y = map_y / 8;
        let line = &mut self.scratch.bg[bg];

        let mut x = 0usize;
        while x < SCREEN_WIDTH {
            let map_x = (x + scroll_x) % (width_tiles * 8);
            let tile_x = map_x / 8;
            // 512-wide maps keep their right half in the next 2K screen block;
            // 512-tall maps keep the bottom half two blocks (or one) further.
            let mut block = screen_base;
            if tile_x >= 32 {
                block += 0x800;
            }
            if tile_y >= 32 {
                block += if width_tiles == 64 { 0x1000 } else { 0x800 };
            }
            let entry_off = block + ((tile_y & 31) * 32 + (tile_x & 31)) * 2;
            let entry = rd16(&self.vram[..], entry_off & 0xFFFF);
            let tile = usize::from(entry & 0x3FF);
            let hflip = entry & (1 << 10) != 0;
            let vflip = entry & (1 << 11) != 0;
            let palette = usize::from((entry >> 12) & 0xF);

            let row = if vflip { 7 - map_y % 8 } else { map_y % 8 };
            let start_px = map_x % 8;
            let count = (8 - start_px).min(SCREEN_WIDTH - x);

            if colours_256 {
                let tile_addr = char_base + tile * 64 + row * 8;
                if tile_addr + 8 <= 0x10000 {
                    for i in 0..count {
                        let px = start_px + i;
                        let col = if hflip { 7 - px } else { px };
                        let index = self.vram[tile_addr + col];
                        line[x + i] = if index == 0 {
                            TRANSPARENT
                        } else {
                            rd16(&self.palette[..], usize::from(index) * 2)
                        };
                    }
                }
            } else {
                let tile_addr = char_base + tile * 32 + row * 4;
                if tile_addr + 4 <= 0x10000 {
                    for i in 0..count {
                        let px = start_px + i;
                        let col = if hflip { 7 - px } else { px };
                        let byte = self.vram[tile_addr + col / 2];
                        let index = usize::from(if col & 1 != 0 { byte >> 4 } else { byte & 0xF });
                        line[x + i] = if index == 0 {
                            TRANSPARENT
                        } else {
                            rd16(&self.palette[..], (palette * 16 + index) * 2)
                        };
                    }
                }
            }
            x += count;
        }

        if mosaic && mosaic_w > 1 {
            apply_mosaic(line, mosaic_w);
        }
    }

    fn render_affine_bg(&mut self, bg: usize, y: usize) {
        let cnt = self.bgcnt[bg];
        let char_base = usize::from((cnt >> 2) & 3) * 0x4000;
        let screen_base = usize::from((cnt >> 8) & 0x1F) * 0x800;
        let wrap = cnt & (1 << 13) != 0;
        let size_tiles = 16usize << ((cnt >> 14) & 3);
        let size_px = (size_tiles * 8) as i32;
        let a = bg - 2;

        let mosaic = cnt & (1 << 6) != 0;
        let mosaic_w = if mosaic { usize::from(self.mosaic & 0xF) + 1 } else { 1 };
        let mosaic_h = if mosaic { usize::from((self.mosaic >> 4) & 0xF) + 1 } else { 1 };

        let pa = i32::from(self.bgpa[a]);
        let pc = i32::from(self.bgpc[a]);
        // Vertical mosaic replays the reference point of the first line in the block.
        let back = (y % mosaic_h) as i32;
        let mut tx = self.bgx_internal[a].wrapping_sub(i32::from(self.bgpb[a]).wrapping_mul(back));
        let mut ty = self.bgy_internal[a].wrapping_sub(i32::from(self.bgpd[a]).wrapping_mul(back));
        let line = &mut self.scratch.bg[bg];

        for px in line.iter_mut() {
            let mut x = tx >> 8;
            let mut y = ty >> 8;
            tx = tx.wrapping_add(pa);
            ty = ty.wrapping_add(pc);
            if wrap {
                x &= size_px - 1;
                y &= size_px - 1;
            } else if x < 0 || y < 0 || x >= size_px || y >= size_px {
                *px = TRANSPARENT;
                continue;
            }
            let (x, y) = (x as usize, y as usize);
            let tile =
                usize::from(self.vram[(screen_base + (y / 8) * size_tiles + x / 8) & 0xFFFF]);
            let addr = char_base + tile * 64 + (y % 8) * 8 + x % 8;
            let index = if addr < 0x10000 { self.vram[addr] } else { 0 };
            *px = if index == 0 {
                TRANSPARENT
            } else {
                rd16(&self.palette[..], usize::from(index) * 2)
            };
        }

        if mosaic_w > 1 {
            apply_mosaic(line, mosaic_w);
        }
    }

    /// Modes 3-5: BG2 is a bitmap, still transformed by the BG2 affine parameters.
    fn render_bitmap_bg(&mut self, mode: u16, y: usize) {
        let (width, height, page) = match mode {
            3 => (240i32, 160i32, 0usize),
            4 => (240, 160, if self.dispcnt & (1 << 4) != 0 { 0xA000 } else { 0 }),
            _ => (160, 128, if self.dispcnt & (1 << 4) != 0 { 0xA000 } else { 0 }),
        };
        let mosaic = self.bgcnt[2] & (1 << 6) != 0;
        let mosaic_w = if mosaic { usize::from(self.mosaic & 0xF) + 1 } else { 1 };
        let mosaic_h = if mosaic { usize::from((self.mosaic >> 4) & 0xF) + 1 } else { 1 };
        let back = (y % mosaic_h) as i32;
        let pa = i32::from(self.bgpa[0]);
        let pc = i32::from(self.bgpc[0]);
        let mut tx = self.bgx_internal[0].wrapping_sub(i32::from(self.bgpb[0]).wrapping_mul(back));
        let mut ty = self.bgy_internal[0].wrapping_sub(i32::from(self.bgpd[0]).wrapping_mul(back));
        let line = &mut self.scratch.bg[2];

        for px in line.iter_mut() {
            let x = tx >> 8;
            let y = ty >> 8;
            tx = tx.wrapping_add(pa);
            ty = ty.wrapping_add(pc);
            if x < 0 || y < 0 || x >= width || y >= height {
                *px = TRANSPARENT;
                continue;
            }
            let i = (y * width + x) as usize;
            *px = if mode == 4 {
                let index = self.vram[page + i];
                if index == 0 {
                    TRANSPARENT
                } else {
                    rd16(&self.palette[..], usize::from(index) * 2)
                }
            } else {
                rd16(&self.vram[..], page + i * 2) & 0x7FFF
            };
        }
        if mosaic_w > 1 {
            apply_mosaic(line, mosaic_w);
        }
    }

    // ---------------------------------------------------------------------------
    // Sprites
    // ---------------------------------------------------------------------------

    fn render_sprites(&mut self, y: usize) {
        const SIZES: [[(i32, i32); 4]; 3] = [
            [(8, 8), (16, 16), (32, 32), (64, 64)],
            [(16, 8), (32, 8), (32, 16), (64, 32)],
            [(8, 16), (8, 32), (16, 32), (32, 64)],
        ];
        let mapping_1d = self.dispcnt & (1 << 6) != 0;
        let bitmap_mode = self.dispcnt & 7 >= 3;
        // Sprite rendering shares the line with the LCD: fewer cycles are available
        // when HBlank is reserved for the CPU (DISPCNT bit 5).
        let mut cycles: i32 = if self.dispcnt & (1 << 5) != 0 { 954 } else { 1210 };
        let mosaic_w = usize::from((self.mosaic >> 8) & 0xF) + 1;
        let mosaic_h = usize::from((self.mosaic >> 12) & 0xF) + 1;

        for n in 0..128 {
            let attr0 = rd16(&self.oam[..], n * 8);
            let attr1 = rd16(&self.oam[..], n * 8 + 2);
            let attr2 = rd16(&self.oam[..], n * 8 + 4);
            let affine = attr0 & (1 << 8) != 0;
            if !affine && attr0 & (1 << 9) != 0 {
                continue; // disabled
            }
            let shape = usize::from(attr0 >> 14);
            if shape == 3 {
                continue; // prohibited
            }
            let obj_mode = (attr0 >> 10) & 3;
            if obj_mode == 3 {
                continue;
            }
            let (w, h) = SIZES[shape][usize::from(attr1 >> 14)];
            let double = affine && attr0 & (1 << 9) != 0;
            let (box_w, box_h) = if double { (w * 2, h * 2) } else { (w, h) };

            let mut sy = i32::from(attr0 & 0xFF);
            if sy + box_h > 256 {
                sy -= 256;
            }
            let mut sx = i32::from(attr1 & 0x1FF);
            if sx >= 256 {
                sx -= 512;
            }
            let line_y = y as i32;
            if line_y < sy || line_y >= sy + box_h {
                continue;
            }
            let cost = if affine { 10 + 2 * box_w } else { box_w };
            cycles -= cost;
            if cycles < 0 {
                break;
            }

            let mosaic = attr0 & (1 << 12) != 0;
            let colours_256 = attr0 & (1 << 13) != 0;
            let priority = ((attr2 >> 10) & 3) as u8;
            let palette = usize::from((attr2 >> 12) & 0xF);
            let tile = usize::from(attr2 & 0x3FF);
            let semi = obj_mode == 1;
            let window = obj_mode == 2;

            // Row of the sprite box being drawn, before mosaic / flips / rotation.
            let mut local_y = line_y - sy;
            if mosaic {
                local_y -= (y % mosaic_h) as i32;
                if local_y < 0 {
                    local_y = 0;
                }
            }

            // Affine parameters (identity for regular sprites).
            let (pa, pb, pc, pd) = if affine {
                let group = usize::from((attr1 >> 9) & 0x1F) * 32;
                (
                    i32::from(rd16(&self.oam[..], group + 6) as i16),
                    i32::from(rd16(&self.oam[..], group + 14) as i16),
                    i32::from(rd16(&self.oam[..], group + 22) as i16),
                    i32::from(rd16(&self.oam[..], group + 30) as i16),
                )
            } else {
                (0x100, 0, 0, 0x100)
            };
            let hflip = !affine && attr1 & (1 << 12) != 0;
            let vflip = !affine && attr1 & (1 << 13) != 0;

            // Tile row stride in 2D mapping is 32 tiles (1024 bytes) per row of the
            // 32×32 tile sheet; in 1D mapping tiles follow each other.
            let tiles_per_row =
                if mapping_1d { w / 8 * if colours_256 { 2 } else { 1 } } else { 32 };

            let half_w = box_w / 2;
            let half_h = box_h / 2;
            // Texture coordinate at the centre of the box, in 8.8 fixed point.
            let dy = local_y - half_h;
            let base_tx = (w / 2) * 256 + pb * dy;
            let base_ty = (h / 2) * 256 + pd * dy;

            for bx in 0..box_w {
                let screen_x = sx + bx;
                if !(0..SCREEN_WIDTH as i32).contains(&screen_x) {
                    continue;
                }
                let dx = bx - half_w;
                let tx = (base_tx + pa * dx) >> 8;
                let ty = (base_ty + pc * dx) >> 8;
                if tx < 0 || ty < 0 || tx >= w || ty >= h {
                    continue;
                }
                let tx = if hflip { w - 1 - tx } else { tx } as usize;
                let ty = if vflip { h - 1 - ty } else { ty } as usize;

                let tile_x = tx / 8;
                let tile_y = ty / 8;
                let index = if colours_256 {
                    let t = tile + tile_y * tiles_per_row as usize + tile_x * 2;
                    let addr = 0x10000 + (t & 0x3FF) * 32 + (ty % 8) * 8 + tx % 8;
                    if bitmap_mode && addr < 0x14000 {
                        0
                    } else {
                        self.vram[addr]
                    }
                } else {
                    let t = tile + tile_y * tiles_per_row as usize + tile_x;
                    let addr = 0x10000 + (t & 0x3FF) * 32 + (ty % 8) * 4 + (tx % 8) / 2;
                    if bitmap_mode && addr < 0x14000 {
                        0
                    } else {
                        let byte = self.vram[addr];
                        if tx & 1 != 0 {
                            byte >> 4
                        } else {
                            byte & 0xF
                        }
                    }
                };
                if index == 0 {
                    continue;
                }
                let sxu = screen_x as usize;
                if window {
                    self.scratch.obj_window[sxu] = true;
                    continue;
                }
                let colour = if colours_256 {
                    rd16(&self.palette[..], 0x200 + usize::from(index) * 2)
                } else {
                    rd16(&self.palette[..], 0x200 + (palette * 16 + usize::from(index)) * 2)
                };
                let slot = &mut self.scratch.obj[sxu];
                // Earlier OAM entries win ties; a better priority always wins.
                if slot.colour == TRANSPARENT || priority < slot.priority {
                    *slot = ObjPixel { colour, priority, semi_transparent: semi };
                }
            }
        }

        if mosaic_w > 1 {
            // Horizontal OBJ mosaic applies to the composed sprite layer.
            let obj = &mut self.scratch.obj;
            let mut i = 0;
            while i < SCREEN_WIDTH {
                let src = obj[i];
                let end = (i + mosaic_w).min(SCREEN_WIDTH);
                for px in &mut obj[i + 1..end] {
                    if src.colour != TRANSPARENT {
                        *px = src;
                    }
                }
                i = end;
            }
        }
    }

    // ---------------------------------------------------------------------------
    // Composition
    // ---------------------------------------------------------------------------

    fn compose(&mut self, y: usize) {
        let backdrop = rd16(&self.palette[..], 0) & 0x7FFF;
        let win0_on = self.dispcnt & (1 << 13) != 0;
        let win1_on = self.dispcnt & (1 << 14) != 0;
        let objwin_on = self.dispcnt & (1 << 15) != 0;
        let any_window = win0_on || win1_on || objwin_on;

        let win0_y = win0_on && in_window_range(y, self.winv[0]);
        let win1_y = win1_on && in_window_range(y, self.winv[1]);
        let win_in0 = WindowFlags::from_bits(self.winin);
        let win_in1 = WindowFlags::from_bits(self.winin >> 8);
        let win_out = WindowFlags::from_bits(self.winout);
        let win_obj = WindowFlags::from_bits(self.winout >> 8);

        let blend_mode = (self.bldcnt >> 6) & 3;
        let first_targets = (self.bldcnt & 0x3F) as u8;
        let second_targets = ((self.bldcnt >> 8) & 0x3F) as u8;
        let eva = u32::from(self.bldalpha & 0x1F).min(16);
        let evb = u32::from((self.bldalpha >> 8) & 0x1F).min(16);
        let evy = u32::from(self.bldy & 0x1F).min(16);

        let bg_priority: [u8; 4] = [
            (self.bgcnt[0] & 3) as u8,
            (self.bgcnt[1] & 3) as u8,
            (self.bgcnt[2] & 3) as u8,
            (self.bgcnt[3] & 3) as u8,
        ];

        let out = &mut self.framebuffer[y * SCREEN_WIDTH..(y + 1) * SCREEN_WIDTH];
        for (x, px) in out.iter_mut().enumerate() {
            let flags = if !any_window {
                WindowFlags::ALL
            } else if win0_y && in_window_range(x, self.winh[0]) {
                win_in0
            } else if win1_y && in_window_range(x, self.winh[1]) {
                win_in1
            } else if objwin_on && self.scratch.obj_window[x] {
                win_obj
            } else {
                win_out
            };

            // Find the two topmost opaque layers in priority order. Among equal
            // priorities, OBJ beats backgrounds and lower BG numbers beat higher.
            let mut top = (LAYER_BACKDROP, backdrop);
            let mut second = (LAYER_BACKDROP, backdrop);
            let mut top_prio = 4u8;
            let mut second_prio = 4u8;
            let obj = self.scratch.obj[x];
            let obj_visible = obj.colour != TRANSPARENT && flags.layers & (1 << LAYER_OBJ) != 0;
            for prio in 0..4u8 {
                if obj_visible && obj.priority == prio {
                    push_layer(
                        &mut top,
                        &mut second,
                        &mut top_prio,
                        &mut second_prio,
                        LAYER_OBJ,
                        obj.colour,
                        prio,
                    );
                }
                for (bg, &bg_prio) in bg_priority.iter().enumerate() {
                    if bg_prio == prio && flags.layers & (1 << bg) != 0 {
                        let c = self.scratch.bg[bg][x];
                        if c != TRANSPARENT {
                            push_layer(
                                &mut top,
                                &mut second,
                                &mut top_prio,
                                &mut second_prio,
                                LAYER_BG0 + bg,
                                c,
                                prio,
                            );
                        }
                    }
                }
                if second_prio < 4 {
                    break;
                }
            }

            let mut colour = top.1;
            if flags.effects {
                let top_is_first = first_targets & (1 << top.0) != 0;
                let second_is_second = second_targets & (1 << second.0) != 0;
                let obj_semi = top.0 == LAYER_OBJ && obj.semi_transparent;
                if (obj_semi || (blend_mode == 1 && top_is_first)) && second_is_second {
                    colour = alpha_blend(top.1, second.1, eva, evb);
                } else if top_is_first || obj_semi {
                    match blend_mode {
                        2 => colour = brighten(top.1, evy),
                        3 => colour = darken(top.1, evy),
                        _ => {}
                    }
                }
            }
            *px = bgr555_to_argb(colour);
        }
    }
}

#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn push_layer(
    top: &mut (usize, u16),
    second: &mut (usize, u16),
    top_prio: &mut u8,
    second_prio: &mut u8,
    layer: usize,
    colour: u16,
    prio: u8,
) {
    if *top_prio == 4 {
        *top = (layer, colour);
        *top_prio = prio;
    } else if *second_prio == 4 {
        *second = (layer, colour);
        *second_prio = prio;
    }
}

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

fn apply_mosaic(line: &mut [u16; SCREEN_WIDTH], width: usize) {
    let mut i = 0;
    while i < SCREEN_WIDTH {
        let src = line[i];
        let end = (i + width).min(SCREEN_WIDTH);
        line[i + 1..end].fill(src);
        i = end;
    }
}

#[inline(always)]
fn alpha_blend(a: u16, b: u16, eva: u32, evb: u32) -> u16 {
    let mut out = 0u16;
    for shift in [0, 5, 10] {
        let ca = u32::from((a >> shift) & 0x1F);
        let cb = u32::from((b >> shift) & 0x1F);
        let c = ((ca * eva + cb * evb) / 16).min(31);
        out |= (c as u16) << shift;
    }
    out
}

#[inline(always)]
fn brighten(a: u16, evy: u32) -> u16 {
    let mut out = 0u16;
    for shift in [0, 5, 10] {
        let c = u32::from((a >> shift) & 0x1F);
        let c = c + ((31 - c) * evy) / 16;
        out |= (c as u16) << shift;
    }
    out
}

#[inline(always)]
fn darken(a: u16, evy: u32) -> u16 {
    let mut out = 0u16;
    for shift in [0, 5, 10] {
        let c = u32::from((a >> shift) & 0x1F);
        let c = c - (c * evy) / 16;
        out |= (c as u16) << shift;
    }
    out
}
