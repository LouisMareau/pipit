//! The two 2D engines and the display timing. Rendering is in `render.rs`
//! (composition, effects, capture), `bg.rs` (backgrounds) and `obj.rs`
//! (objects); the banks are in `vram.rs`.
//!
//! Each scanline raises HBlank and then advances VCOUNT; the visible lines are
//! drawn at the start of their HBlank. Both CPUs see DISPSTAT and VCOUNT, each
//! with its own interrupt enables and VCOUNT match.

pub mod bg;
pub mod obj;
pub mod render;
pub mod vram;

#[cfg(test)]
mod tests;

use crate::scheduler::{Event, Scheduler, HBLANK_START, LINES, LINE_CYCLES};
use serde::{Deserialize, Serialize};
use vram::Vram;

pub const SCREEN_WIDTH: usize = 256;
pub const SCREEN_HEIGHT: usize = 192;

/// One 2D engine's registers (A at 0400_0000h, B at 0400_1000h).
#[derive(Clone, Serialize, Deserialize)]
pub struct Engine {
    pub dispcnt: u32,
    /// BGxCNT, offsets, affine parameters, windows, mosaic, blending: 008h–05Fh.
    #[serde(with = "pipit_common::snapshot::array")]
    pub regs: [u16; 0x2C],
    pub master_bright: u16,
    /// BG2 and BG3's reference points as written (28-bit, sign extended).
    pub bgx: [i32; 2],
    pub bgy: [i32; 2],
    /// The internal reference points, stepped once per line.
    pub bgx_internal: [i32; 2],
    pub bgy_internal: [i32; 2],
}

impl Default for Engine {
    fn default() -> Self {
        let mut regs = [0; 0x2C];
        // PA and PD start as the identity.
        regs[12] = 0x100;
        regs[15] = 0x100;
        regs[20] = 0x100;
        regs[23] = 0x100;
        Self {
            dispcnt: 0,
            regs,
            master_bright: 0,
            bgx: [0; 2],
            bgy: [0; 2],
            bgx_internal: [0; 2],
            bgy_internal: [0; 2],
        }
    }
}

impl Engine {
    #[inline(always)]
    pub fn mode(&self) -> u32 {
        self.dispcnt & 7
    }

    #[inline(always)]
    pub fn bg_enabled(&self, n: usize) -> bool {
        self.dispcnt & (1 << (8 + n)) != 0
    }

    #[inline(always)]
    pub fn bgcnt(&self, n: usize) -> u16 {
        self.regs[n]
    }

    #[inline(always)]
    pub fn hofs(&self, n: usize) -> usize {
        usize::from(self.regs[4 + n * 2] & 0x1FF)
    }

    #[inline(always)]
    pub fn vofs(&self, n: usize) -> usize {
        usize::from(self.regs[5 + n * 2] & 0x1FF)
    }

    /// PA, PB, PC, PD of BG2 (`a` = 0) or BG3 (`a` = 1).
    #[inline(always)]
    pub fn affine(&self, a: usize) -> [i32; 4] {
        let at = 12 + a * 8;
        [
            i32::from(self.regs[at] as i16),
            i32::from(self.regs[at + 1] as i16),
            i32::from(self.regs[at + 2] as i16),
            i32::from(self.regs[at + 3] as i16),
        ]
    }

    pub fn winh(&self, n: usize) -> u16 {
        self.regs[28 + n]
    }

    pub fn winv(&self, n: usize) -> u16 {
        self.regs[30 + n]
    }

    pub fn winin(&self) -> u16 {
        self.regs[32]
    }

    pub fn winout(&self) -> u16 {
        self.regs[33]
    }

    pub fn mosaic(&self) -> u16 {
        self.regs[34]
    }

    pub fn bldcnt(&self) -> u16 {
        self.regs[36]
    }

    pub fn bldalpha(&self) -> u16 {
        self.regs[37]
    }

    pub fn bldy(&self) -> u16 {
        self.regs[38]
    }

    /// A register at engine offset `reg` (000h–06Fh); write-only ones read 0.
    pub fn read_reg(&self, reg: u32) -> u16 {
        match reg {
            0x000 => self.dispcnt as u16,
            0x002 => (self.dispcnt >> 16) as u16,
            0x008..=0x00E | 0x048 | 0x04A | 0x050 | 0x052 => self.regs[((reg - 8) / 2) as usize],
            0x06C => self.master_bright,
            _ => 0,
        }
    }

    pub fn write_reg(&mut self, reg: u32, value: u16, mask: u16) {
        let v = u32::from(value & mask);
        let m = u32::from(mask);
        match reg {
            0x000 => self.dispcnt = (self.dispcnt & !m) | v,
            0x002 => self.dispcnt = (self.dispcnt & !(m << 16)) | (v << 16),
            0x008..=0x05F => {
                let i = ((reg - 8) / 2) as usize;
                self.regs[i] = (self.regs[i] & !mask) | (value & mask);
                // BG2X/Y at 028h–02Eh, BG3X/Y at 038h–03Eh: reload the internal point.
                if (0x028..=0x02E).contains(&reg) || (0x038..=0x03E).contains(&reg) {
                    let a = usize::from(reg >= 0x038);
                    let base = 16 + a * 8;
                    let word = |i: usize| {
                        ((u32::from(self.regs[i + 1]) << 16 | u32::from(self.regs[i])) << 4) as i32
                            >> 4
                    };
                    self.bgx[a] = word(base);
                    self.bgy[a] = word(base + 2);
                    self.bgx_internal[a] = self.bgx[a];
                    self.bgy_internal[a] = self.bgy[a];
                }
            }
            0x06C => self.master_bright = (self.master_bright & !mask) | (value & mask & 0xC01F),
            _ => {}
        }
    }

    /// Frame start: the internal reference points restart from the written ones.
    pub fn reload_affine(&mut self) {
        self.bgx_internal = self.bgx;
        self.bgy_internal = self.bgy;
    }

    /// After each visible line, the reference points step by PB and PD.
    pub fn advance_affine(&mut self) {
        for a in 0..2 {
            let [_, pb, _, pd] = self.affine(a);
            self.bgx_internal[a] = self.bgx_internal[a].wrapping_add(pb);
            self.bgy_internal[a] = self.bgy_internal[a].wrapping_add(pd);
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct Video {
    pub engines: [Engine; 2],
    /// DISPSTAT for the ARM9 (0) and ARM7 (1): the enables and VCOUNT setting.
    pub dispstat: [u16; 2],
    pub vcount: u16,
    pub in_hblank: bool,
    /// POWCNT1: bit 0 LCDs, bit 1 engine A, bit 9 engine B, bit 15 swap.
    pub powcnt1: u16,
    pub vram: Vram,
    #[serde(with = "pipit_common::snapshot::bytes_box")]
    pub palette: Box<[u8; 0x800]>,
    #[serde(with = "pipit_common::snapshot::bytes_box")]
    pub oam: Box<[u8; 0x800]>,
    /// DISPCAPCNT (064h): engine A's display capture.
    pub dispcapcnt: u32,
    /// DISP3DCNT (060h), kept for the 3D engine.
    pub disp3dcnt: u16,
    /// DISP_MMEM_FIFO (068h): the line DMA pushes from main memory.
    #[serde(skip, default = "blank_fifo")]
    fifo: Box<[u16; SCREEN_WIDTH]>,
    #[serde(skip)]
    fifo_pos: usize,
    /// The two screens' pixels as 0xFFRRGGBB: index 0 is the top screen.
    #[serde(skip, default = "blank_screens")]
    pub screens: [Box<[u32; SCREEN_WIDTH * SCREEN_HEIGHT]>; 2],
    #[serde(skip)]
    scratch: Box<[render::Scratch; 2]>,
    frame_ready: bool,
}

fn blank_screens() -> [Box<[u32; SCREEN_WIDTH * SCREEN_HEIGHT]>; 2] {
    [crate::boxed(0xFF00_0000), crate::boxed(0xFF00_0000)]
}

fn blank_fifo() -> Box<[u16; SCREEN_WIDTH]> {
    crate::boxed(0)
}

/// What a scanline event asks the system to do.
#[derive(Clone, Copy, Debug, Default)]
pub struct Signals {
    /// Interrupts for the ARM9 and ARM7: bits 0 VBlank, 1 HBlank, 2 VCount.
    pub irq: [u8; 2],
    pub vblank_started: bool,
    pub hblank_started: bool,
    pub line_started: bool,
}

impl Default for Video {
    fn default() -> Self {
        Self::new()
    }
}

impl Video {
    pub fn new() -> Self {
        Self {
            engines: [Engine::default(), Engine::default()],
            dispstat: [0; 2],
            vcount: 0,
            in_hblank: false,
            powcnt1: 0x820F,
            vram: Vram::new(),
            palette: crate::boxed(0),
            oam: crate::boxed(0),
            dispcapcnt: 0,
            disp3dcnt: 0,
            fifo: blank_fifo(),
            fifo_pos: 0,
            screens: blank_screens(),
            scratch: Box::default(),
            frame_ready: false,
        }
    }

    /// Schedules the first scanline events; call once at power-on.
    pub fn start(&self, scheduler: &mut Scheduler) {
        scheduler.schedule_at(Event::HBlankStart, HBLANK_START);
        scheduler.schedule_at(Event::HBlankEnd, LINE_CYCLES);
    }

    pub fn take_frame_ready(&mut self) -> bool {
        std::mem::replace(&mut self.frame_ready, false)
    }

    /// DISPSTAT as `cpu` reads it.
    pub fn read_dispstat(&self, cpu: usize) -> u16 {
        let vblank = (192..LINES as u16 - 1).contains(&self.vcount);
        let target = Self::vcount_target(self.dispstat[cpu]);
        (self.dispstat[cpu] & 0xFFB8)
            | u16::from(vblank)
            | u16::from(self.in_hblank) << 1
            | u16::from(self.vcount == target) << 2
    }

    fn vcount_target(dispstat: u16) -> u16 {
        (dispstat >> 8) | ((dispstat & 0x80) << 1)
    }

    pub fn write_dispstat(&mut self, cpu: usize, value: u16, mask: u16) {
        let mask = mask & 0xFFB8;
        self.dispstat[cpu] = (self.dispstat[cpu] & !mask) | (value & mask);
    }

    /// The main memory display FIFO took a word: two pixels of the current line.
    pub fn push_fifo(&mut self, word: u32) {
        if self.fifo_pos + 1 < SCREEN_WIDTH {
            self.fifo[self.fifo_pos] = word as u16;
            self.fifo[self.fifo_pos + 1] = (word >> 16) as u16;
        }
        self.fifo_pos += 2;
    }

    /// Whether the main memory FIFO feeds anything: engine A's display mode 3,
    /// or a capture whose source B is the FIFO.
    pub fn fifo_in_use(&self) -> bool {
        (self.engines[0].dispcnt >> 16) & 3 == 3
            || self.dispcapcnt & (1 << 31 | 1 << 25) == (1 << 31 | 1 << 25)
    }

    /// The visible part of the current line ended: draw it, enter HBlank.
    pub fn on_hblank_start(&mut self, at: u64, scheduler: &mut Scheduler) -> Signals {
        let mut signals = Signals { hblank_started: true, ..Signals::default() };
        if self.vcount < SCREEN_HEIGHT as u16 {
            self.render_line(self.vcount as usize);
            for engine in &mut self.engines {
                engine.advance_affine();
            }
        }
        self.in_hblank = true;
        for cpu in 0..2 {
            if self.dispstat[cpu] & (1 << 4) != 0 {
                signals.irq[cpu] |= 2;
            }
        }
        scheduler.schedule_at(Event::HBlankStart, at + LINE_CYCLES);
        signals
    }

    /// The line ended: next VCOUNT, VBlank and VCOUNT-match interrupts.
    pub fn on_hblank_end(&mut self, at: u64, scheduler: &mut Scheduler) -> Signals {
        let mut signals = Signals { line_started: true, ..Signals::default() };
        self.in_hblank = false;
        self.vcount += 1;
        self.fifo_pos = 0;
        if self.vcount == LINES as u16 {
            self.vcount = 0;
            for engine in &mut self.engines {
                engine.reload_affine();
            }
        }
        if self.vcount == SCREEN_HEIGHT as u16 {
            self.frame_ready = true;
            signals.vblank_started = true;
            // A capture lasts one frame.
            self.dispcapcnt &= !(1 << 31);
            for cpu in 0..2 {
                if self.dispstat[cpu] & (1 << 3) != 0 {
                    signals.irq[cpu] |= 1;
                }
            }
        }
        for cpu in 0..2 {
            if self.dispstat[cpu] & (1 << 5) != 0
                && self.vcount == Self::vcount_target(self.dispstat[cpu])
            {
                signals.irq[cpu] |= 4;
            }
        }
        scheduler.schedule_at(Event::HBlankEnd, at + LINE_CYCLES);
        signals
    }

    /// Which screen (0 top, 1 bottom) engine `engine` is shown on.
    pub fn screen_of(&self, engine: usize) -> usize {
        let a_on_top = self.powcnt1 & (1 << 15) != 0;
        if (engine == 0) == a_on_top {
            0
        } else {
            1
        }
    }
}
