//! The picture processing unit (Pan Docs "Rendering"): modes and their
//! interrupts dot by dot, and each line drawn in one go when its mode 3
//! starts, from the registers as they are at that moment. Classic games get
//! four shades; colour games get the colour palettes, tile attributes and the
//! second VRAM bank.

use pipit_common::snapshot;
use serde::{Deserialize, Serialize};

use crate::{SCREEN_HEIGHT, SCREEN_WIDTH};

const VBLANK: u8 = 1 << 0;
const STAT: u8 = 1 << 1;
const DMG_SHADES: [u32; 4] = [0xFFE0_F8D0, 0xFF88_C070, 0xFF34_6856, 0xFF08_1820];

fn line_u8() -> [u8; SCREEN_WIDTH] {
    [0; SCREEN_WIDTH]
}

fn line_bool() -> [bool; SCREEN_WIDTH] {
    [false; SCREEN_WIDTH]
}

#[derive(Serialize, Deserialize)]
pub struct Video {
    #[serde(with = "snapshot::bytes_box")]
    vram: Box<[u8; 0x4000]>,
    #[serde(with = "snapshot::array")]
    oam: [u8; 0xA0],
    #[serde(with = "snapshot::words_box")]
    framebuffer: Box<[u32; SCREEN_WIDTH * SCREEN_HEIGHT]>,
    lcdc: u8,
    stat: u8,
    scy: u8,
    scx: u8,
    ly: u8,
    lyc: u8,
    wy: u8,
    wx: u8,
    bgp: u8,
    obp0: u8,
    obp1: u8,
    vbk: u8,
    bcps: u8,
    ocps: u8,
    opri: u8,
    #[serde(with = "snapshot::array")]
    bg_pal: [u8; 64],
    #[serde(with = "snapshot::array")]
    obj_pal: [u8; 64],
    #[serde(with = "snapshot::array")]
    bg_rgb: [u32; 32],
    #[serde(with = "snapshot::array")]
    obj_rgb: [u32; 32],
    cgb: bool,
    /// Dot within the line, 0..456.
    dot: u16,
    mode: u8,
    /// Where mode 3 ends on this line (it stretches with scrolling and sprites).
    mode3_end: u16,
    /// Lines of the window drawn so far this frame.
    window_line: u8,
    frame_ready: bool,
    /// The STAT interrupt fires on this going high.
    stat_line: bool,
    hblank_started: bool,
    #[serde(skip, default = "line_u8")]
    line_color: [u8; SCREEN_WIDTH],
    #[serde(skip, default = "line_bool")]
    line_priority: [bool; SCREEN_WIDTH],
    #[serde(skip, default = "line_bool")]
    line_sprite: [bool; SCREEN_WIDTH],
}

impl Video {
    pub fn new(cgb: bool) -> Self {
        let mut video = Self {
            vram: Box::new([0; 0x4000]),
            oam: [0; 0xA0],
            framebuffer: Box::new([DMG_SHADES[0]; SCREEN_WIDTH * SCREEN_HEIGHT]),
            lcdc: 0x91,
            stat: 0,
            scy: 0,
            scx: 0,
            ly: 0,
            lyc: 0,
            wy: 0,
            wx: 0,
            bgp: 0xFC,
            obp0: 0xFF,
            obp1: 0xFF,
            vbk: 0,
            bcps: 0,
            ocps: 0,
            opri: 0,
            bg_pal: [0xFF; 64],
            obj_pal: [0xFF; 64],
            bg_rgb: [0xFFFF_FFFF; 32],
            obj_rgb: [0xFFFF_FFFF; 32],
            cgb,
            dot: 0,
            mode: 2,
            mode3_end: 252,
            window_line: 0,
            frame_ready: false,
            stat_line: false,
            hblank_started: false,
            line_color: [0; SCREEN_WIDTH],
            line_priority: [false; SCREEN_WIDTH],
            line_sprite: [false; SCREEN_WIDTH],
        };
        if cgb {
            video.framebuffer.fill(0xFFFF_FFFF);
        }
        video
    }

    pub fn framebuffer(&self) -> &[u32] {
        &self.framebuffer[..]
    }

    pub fn take_frame_ready(&mut self) -> bool {
        std::mem::replace(&mut self.frame_ready, false)
    }

    /// Mode 0 just began on a visible line (for HBlank DMA).
    pub fn take_hblank_started(&mut self) -> bool {
        std::mem::replace(&mut self.hblank_started, false)
    }

    fn lcd_on(&self) -> bool {
        self.lcdc & 0x80 != 0
    }

    pub fn read_vram(&self, addr: u16) -> u8 {
        self.vram[usize::from(self.vbk) * 0x2000 + usize::from(addr & 0x1FFF)]
    }

    pub fn write_vram(&mut self, addr: u16, value: u8) {
        self.vram[usize::from(self.vbk) * 0x2000 + usize::from(addr & 0x1FFF)] = value;
    }

    pub fn read_oam(&self, index: u8) -> u8 {
        self.oam[usize::from(index) % 0xA0]
    }

    pub fn write_oam(&mut self, index: u8, value: u8) {
        self.oam[usize::from(index) % 0xA0] = value;
    }

    /// 15-bit colour to 0xFFRRGGBB.
    fn rgb(c: u16) -> u32 {
        let expand = |v: u16| u32::from((v << 3) | (v >> 2));
        0xFF00_0000
            | (expand(c & 0x1F) << 16)
            | (expand((c >> 5) & 0x1F) << 8)
            | expand((c >> 10) & 0x1F)
    }

    pub fn read_io(&self, addr: u16) -> u8 {
        match addr {
            0xFF40 => self.lcdc,
            0xFF41 => {
                let mode = if self.lcd_on() { self.mode } else { 0 };
                0x80 | (self.stat & 0x78) | (u8::from(self.ly == self.lyc) << 2) | mode
            }
            0xFF42 => self.scy,
            0xFF43 => self.scx,
            0xFF44 => self.ly,
            0xFF45 => self.lyc,
            0xFF47 => self.bgp,
            0xFF48 => self.obp0,
            0xFF49 => self.obp1,
            0xFF4A => self.wy,
            0xFF4B => self.wx,
            0xFF4F if self.cgb => 0xFE | self.vbk,
            0xFF68 if self.cgb => self.bcps | 0x40,
            0xFF69 if self.cgb => self.bg_pal[usize::from(self.bcps & 0x3F)],
            0xFF6A if self.cgb => self.ocps | 0x40,
            0xFF6B if self.cgb => self.obj_pal[usize::from(self.ocps & 0x3F)],
            0xFF6C if self.cgb => self.opri | 0xFE,
            _ => 0xFF,
        }
    }

    pub fn write_io(&mut self, addr: u16, value: u8) {
        match addr {
            0xFF40 => {
                let was_on = self.lcd_on();
                self.lcdc = value;
                if was_on && !self.lcd_on() {
                    self.ly = 0;
                    self.dot = 0;
                    self.mode = 0;
                    self.stat_line = false;
                } else if !was_on && self.lcd_on() {
                    self.ly = 0;
                    self.dot = 0;
                    self.mode = 2;
                    self.window_line = 0;
                }
            }
            0xFF41 => self.stat = value & 0x78,
            0xFF42 => self.scy = value,
            0xFF43 => self.scx = value,
            0xFF45 => self.lyc = value,
            0xFF47 => self.bgp = value,
            0xFF48 => self.obp0 = value,
            0xFF49 => self.obp1 = value,
            0xFF4A => self.wy = value,
            0xFF4B => self.wx = value,
            0xFF4F if self.cgb => self.vbk = value & 1,
            0xFF68 if self.cgb => self.bcps = value & 0xBF,
            0xFF69 if self.cgb => {
                let i = usize::from(self.bcps & 0x3F);
                self.bg_pal[i] = value;
                self.bg_rgb[i / 2] =
                    Self::rgb(u16::from_le_bytes([self.bg_pal[i & !1], self.bg_pal[i | 1]]));
                if self.bcps & 0x80 != 0 {
                    self.bcps = 0x80 | ((self.bcps + 1) & 0x3F);
                }
            }
            0xFF6A if self.cgb => self.ocps = value & 0xBF,
            0xFF6B if self.cgb => {
                let i = usize::from(self.ocps & 0x3F);
                self.obj_pal[i] = value;
                self.obj_rgb[i / 2] =
                    Self::rgb(u16::from_le_bytes([self.obj_pal[i & !1], self.obj_pal[i | 1]]));
                if self.ocps & 0x80 != 0 {
                    self.ocps = 0x80 | ((self.ocps + 1) & 0x3F);
                }
            }
            0xFF6C if self.cgb => self.opri = value & 1,
            _ => {}
        }
    }

    /// `t` dots pass.
    pub fn step(&mut self, t: u32, if_: &mut u8) {
        if !self.lcd_on() {
            return;
        }
        for _ in 0..t {
            self.dot += 1;
            if self.ly < SCREEN_HEIGHT as u8 {
                if self.dot == 80 {
                    self.mode = 3;
                    self.render_line();
                } else if self.dot == self.mode3_end {
                    self.mode = 0;
                    self.hblank_started = true;
                } else if self.dot == 456 {
                    self.dot = 0;
                    self.ly += 1;
                    if self.ly == SCREEN_HEIGHT as u8 {
                        self.mode = 1;
                        *if_ |= VBLANK;
                        self.frame_ready = true;
                    } else {
                        self.mode = 2;
                    }
                }
            } else if self.dot == 456 {
                self.dot = 0;
                self.ly += 1;
                if self.ly == 154 {
                    self.ly = 0;
                    self.mode = 2;
                    self.window_line = 0;
                }
            }
            self.update_stat(if_);
        }
    }

    fn update_stat(&mut self, if_: &mut u8) {
        let line = (self.mode == 0 && self.stat & 0x08 != 0)
            || (self.mode == 1 && self.stat & 0x10 != 0)
            || (self.mode == 2 && self.stat & 0x20 != 0)
            || (self.ly == self.lyc && self.stat & 0x40 != 0);
        if line && !self.stat_line {
            *if_ |= STAT;
        }
        self.stat_line = line;
    }

    /// Draws the current line: background, window, then the sprites over them.
    fn render_line(&mut self) {
        let ly = usize::from(self.ly);
        let lcdc = self.lcdc;
        let row_start = ly * SCREEN_WIDTH;
        let window_on = lcdc & 0x20 != 0 && self.wy <= self.ly && self.wx < 167;
        let mut window_drawn = false;
        let bg_shown = self.cgb || lcdc & 1 != 0;
        for x in 0..SCREEN_WIDTH {
            let in_window = window_on && x + 7 >= usize::from(self.wx);
            let (map, tx, ty) = if in_window {
                window_drawn = true;
                let map = if lcdc & 0x40 != 0 { 0x1C00 } else { 0x1800 };
                (map, x + 7 - usize::from(self.wx), usize::from(self.window_line))
            } else {
                let map = if lcdc & 0x08 != 0 { 0x1C00 } else { 0x1800 };
                (map, (x + usize::from(self.scx)) & 0xFF, (ly + usize::from(self.scy)) & 0xFF)
            };
            let index = map + (ty / 8) * 32 + tx / 8;
            let tile = self.vram[index];
            let attr = if self.cgb { self.vram[0x2000 + index] } else { 0 };
            let bank = if attr & 0x08 != 0 { 0x2000 } else { 0 };
            let row = if attr & 0x40 != 0 { 7 - ty % 8 } else { ty % 8 };
            let tile_addr = if lcdc & 0x10 != 0 {
                usize::from(tile) * 16
            } else {
                (0x1000 + i32::from(tile as i8) * 16) as usize
            };
            let lo = self.vram[bank + tile_addr + row * 2];
            let hi = self.vram[bank + tile_addr + row * 2 + 1];
            let bit = if attr & 0x20 != 0 { tx % 8 } else { 7 - tx % 8 };
            let color = if bg_shown { (((hi >> bit) & 1) << 1) | ((lo >> bit) & 1) } else { 0 };
            self.line_color[x] = color;
            self.line_priority[x] = attr & 0x80 != 0;
            self.line_sprite[x] = false;
            self.framebuffer[row_start + x] = if self.cgb {
                self.bg_rgb[usize::from(attr & 7) * 4 + usize::from(color)]
            } else {
                DMG_SHADES[usize::from((self.bgp >> (color * 2)) & 3)]
            };
        }
        if window_drawn {
            self.window_line = self.window_line.wrapping_add(1);
        }

        // Up to ten sprites per line, in OAM order; classic hardware ranks them by
        // x first. The first to claim a pixel keeps it.
        let height = if lcdc & 0x04 != 0 { 16 } else { 8 };
        let mut sprites: Vec<(u8, usize)> = Vec::with_capacity(10);
        for i in 0..40 {
            let y = i32::from(self.oam[i * 4]) - 16;
            if (y..y + height).contains(&(ly as i32)) {
                sprites.push((self.oam[i * 4 + 1], i));
                if sprites.len() == 10 {
                    break;
                }
            }
        }
        self.mode3_end = 252 + u16::from(self.scx & 7) + 6 * sprites.len() as u16;
        if lcdc & 0x02 == 0 {
            return;
        }
        if !self.cgb || self.opri & 1 != 0 {
            sprites.sort_by_key(|&(x, i)| (x, i));
        }
        for (x, i) in sprites {
            let y = i32::from(self.oam[i * 4]) - 16;
            let attr = self.oam[i * 4 + 3];
            let mut tile = usize::from(self.oam[i * 4 + 2]);
            if height == 16 {
                tile &= 0xFE;
            }
            let mut row = (ly as i32 - y) as usize;
            if attr & 0x40 != 0 {
                row = height as usize - 1 - row;
            }
            let bank = if self.cgb && attr & 0x08 != 0 { 0x2000 } else { 0 };
            let lo = self.vram[bank + tile * 16 + row * 2];
            let hi = self.vram[bank + tile * 16 + row * 2 + 1];
            for px in 0..8usize {
                let sx = i32::from(x) - 8 + px as i32;
                if !(0..SCREEN_WIDTH as i32).contains(&sx) {
                    continue;
                }
                let sx = sx as usize;
                let bit = if attr & 0x20 != 0 { px } else { 7 - px };
                let color = (((hi >> bit) & 1) << 1) | ((lo >> bit) & 1);
                if color == 0 || self.line_sprite[sx] {
                    continue;
                }
                let bg = self.line_color[sx];
                let behind = if self.cgb {
                    lcdc & 1 != 0 && bg != 0 && (attr & 0x80 != 0 || self.line_priority[sx])
                } else {
                    attr & 0x80 != 0 && bg != 0
                };
                if behind {
                    continue;
                }
                self.line_sprite[sx] = true;
                self.framebuffer[row_start + sx] = if self.cgb {
                    self.obj_rgb[usize::from(attr & 7) * 4 + usize::from(color)]
                } else {
                    let pal = if attr & 0x10 != 0 { self.obp1 } else { self.obp0 };
                    DMG_SHADES[usize::from((pal >> (color * 2)) & 3)]
                };
            }
        }
    }
}
