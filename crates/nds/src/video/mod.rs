//! Display timing and the two 2D engines' registers; rendering is in
//! `render.rs`, the banks in `vram.rs`.
//!
//! Each scanline raises HBlank and then advances VCOUNT; the visible lines are
//! drawn at the start of their HBlank. Both CPUs see DISPSTAT and VCOUNT, each
//! with its own interrupt enables and VCOUNT match.

pub mod render;
pub mod vram;

use crate::scheduler::{Event, Scheduler, HBLANK_START, LINES, LINE_CYCLES};
use serde::{Deserialize, Serialize};
use vram::Vram;

pub const SCREEN_WIDTH: usize = 256;
pub const SCREEN_HEIGHT: usize = 192;

/// One 2D engine's registers (A at 0400_0000h, B at 0400_1000h).
#[derive(Clone, Serialize, Deserialize)]
pub struct Engine {
    pub dispcnt: u32,
    /// BGxCNT, offsets, affine parameters, windows, blending: 008h–05Fh, raw.
    #[serde(with = "pipit_common::snapshot::array")]
    pub regs: [u16; 0x2C],
    pub master_bright: u16,
}

impl Default for Engine {
    fn default() -> Self {
        Self { dispcnt: 0, regs: [0; 0x2C], master_bright: 0 }
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
    /// The two screens' pixels as 0xFFRRGGBB: index 0 is the top screen.
    #[serde(skip, default = "blank_screens")]
    pub screens: [Box<[u32; SCREEN_WIDTH * SCREEN_HEIGHT]>; 2],
    frame_ready: bool,
}

fn blank_screens() -> [Box<[u32; SCREEN_WIDTH * SCREEN_HEIGHT]>; 2] {
    [
        Box::new([0xFF00_0000; SCREEN_WIDTH * SCREEN_HEIGHT]),
        Box::new([0xFF00_0000; SCREEN_WIDTH * SCREEN_HEIGHT]),
    ]
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
            palette: Box::new([0; 0x800]),
            oam: Box::new([0; 0x800]),
            screens: blank_screens(),
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

    /// The visible part of the current line ended: draw it, enter HBlank.
    pub fn on_hblank_start(&mut self, at: u64, scheduler: &mut Scheduler) -> Signals {
        let mut signals = Signals { hblank_started: true, ..Signals::default() };
        if self.vcount < SCREEN_HEIGHT as u16 {
            self.render_line(self.vcount as usize);
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
        if self.vcount == LINES as u16 {
            self.vcount = 0;
        }
        if self.vcount == SCREEN_HEIGHT as u16 {
            self.frame_ready = true;
            signals.vblank_started = true;
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
