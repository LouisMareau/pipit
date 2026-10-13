//! Drawing one scanline of each engine. So far: the display modes (off, VRAM
//! display) and text backgrounds in the standard palette; objects, affine and
//! bitmap backgrounds, windows, blending, extended palettes and the 3D layer
//! are still to come.

use super::vram::Region;
use super::{Video, SCREEN_WIDTH};

/// A 15-bit BGR colour as 0xFFRRGGBB.
#[inline]
pub fn rgb(color: u16) -> u32 {
    let c = |v: u16| {
        let v = (v & 0x1F) as u32;
        (v << 3) | (v >> 2)
    };
    0xFF00_0000 | c(color) << 16 | c(color >> 5) << 8 | c(color >> 10)
}

impl Video {
    /// Draws line `y` of both engines into their screens.
    pub fn render_line(&mut self, y: usize) {
        for engine in 0..2 {
            let screen = self.screen_of(engine);
            let mut line = [0xFFFF_FFFFu32; SCREEN_WIDTH];
            let enabled = self.powcnt1 & (1 << [1, 9][engine]) != 0;
            let dispcnt = self.engines[engine].dispcnt;
            if enabled {
                match (dispcnt >> 16) & 3 {
                    0 => {}
                    1 => self.render_graphics(engine, y, &mut line),
                    2 if engine == 0 => {
                        let bank = ((dispcnt >> 18) & 3) as usize;
                        let base = bank * 0x20000 + y * SCREEN_WIDTH * 2;
                        for (x, px) in line.iter_mut().enumerate() {
                            *px = rgb(self.vram.read16(Region::Lcdc, base + x * 2));
                        }
                    }
                    _ => line.fill(0xFF00_0000),
                }
            }
            let row = &mut self.screens[screen][y * SCREEN_WIDTH..(y + 1) * SCREEN_WIDTH];
            row.copy_from_slice(&line);
        }
    }

    /// Mode 1 (graphics): the backdrop, then the text backgrounds by priority.
    fn render_graphics(&mut self, engine: usize, y: usize, line: &mut [u32; SCREEN_WIDTH]) {
        let pal_base = engine * 0x400;
        let backdrop = u16::from_le_bytes([self.palette[pal_base], self.palette[pal_base + 1]]);
        line.fill(rgb(backdrop));
        let e = &self.engines[engine];
        let mode = e.dispcnt & 7;
        // Background `n` is a text layer in these modes (BG0 may be the 3D layer).
        let text: [bool; 4] = match mode {
            0 => [true, true, true, true],
            1 => [true, true, true, false],
            2 => [true, true, false, false],
            3 => [true, true, true, false],
            4 => [true, true, false, false],
            5 => [true, true, false, false],
            _ => [false; 4],
        };
        let bg0_is_3d = engine == 0 && (e.dispcnt & (1 << 3) != 0 || mode == 6);
        // Lowest priority first, so higher ones paint over.
        for priority in (0..4).rev() {
            for n in (0..4).rev() {
                let cnt = e.regs[n];
                if !text[n] || e.dispcnt & (1 << (8 + n)) == 0 || (cnt & 3) as usize != priority {
                    continue;
                }
                if n == 0 && bg0_is_3d {
                    continue;
                }
                self.render_text_bg(engine, n, y, line);
            }
        }
    }

    fn render_text_bg(&self, engine: usize, n: usize, y: usize, line: &mut [u32; SCREEN_WIDTH]) {
        let e = &self.engines[engine];
        let cnt = e.regs[n];
        let region = if engine == 0 { Region::BgA } else { Region::BgB };
        let (char_block, screen_block) = if engine == 0 {
            (((e.dispcnt >> 24) & 7) as usize * 0x10000, ((e.dispcnt >> 27) & 7) as usize * 0x10000)
        } else {
            (0, 0)
        };
        let char_base = char_block + ((cnt >> 2) & 0xF) as usize * 0x4000;
        let screen_base = screen_block + ((cnt >> 8) & 0x1F) as usize * 0x800;
        let colors256 = cnt & (1 << 7) != 0;
        let size = (cnt >> 14) & 3;
        let (width_tiles, height_tiles) = [(32, 32), (64, 32), (32, 64), (64, 64)][size as usize];
        let hofs = usize::from(e.regs[4 + n * 2]) & 0x1FF;
        let vofs = usize::from(e.regs[5 + n * 2]) & 0x1FF;
        let pal_base = engine * 0x400;
        let yy = (y + vofs) % (height_tiles * 8);
        let tile_row = yy / 8;
        for (x, out) in line.iter_mut().enumerate() {
            let xx = (x + hofs) % (width_tiles * 8);
            let tile_col = xx / 8;
            // 32×32-tile screen blocks, laid out left-right then top-bottom.
            let block = (tile_col / 32) + (tile_row / 32) * (width_tiles / 32);
            let entry_at = screen_base + block * 0x800 + ((tile_row % 32) * 32 + tile_col % 32) * 2;
            let entry = self.vram.read16(region, entry_at);
            let tile = usize::from(entry & 0x3FF);
            let mut px = xx % 8;
            let mut py = yy % 8;
            if entry & (1 << 10) != 0 {
                px = 7 - px;
            }
            if entry & (1 << 11) != 0 {
                py = 7 - py;
            }
            let (index, pal_offset) = if colors256 {
                (usize::from(self.vram.read8(region, char_base + tile * 64 + py * 8 + px)), 0)
            } else {
                let byte = self.vram.read8(region, char_base + tile * 32 + py * 4 + px / 2);
                let nibble = if px & 1 == 0 { byte & 0xF } else { byte >> 4 };
                (usize::from(nibble), usize::from(entry >> 12) * 16)
            };
            if index == 0 {
                continue;
            }
            let at = pal_base + (pal_offset + index) * 2;
            *out = rgb(u16::from_le_bytes([self.palette[at], self.palette[at + 1]]));
        }
    }
}
