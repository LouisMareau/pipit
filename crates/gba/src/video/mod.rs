//! LCD controller (GBATEK "GBA LCD Video Controller").
//!
//! This module owns the video registers, VRAM/palette/OAM and the scanline timing.
//! Pixels are produced by `render.rs`, one scanline at a time, when HBlank starts.

mod render;

use crate::dma::{Dma, Timing};
use crate::irq::{Interrupt, Irq};
use crate::scheduler::{Event, Scheduler};
use crate::{SCREEN_HEIGHT, SCREEN_WIDTH};
use serde::{Deserialize, Serialize};

pub const PALETTE_SIZE: usize = 0x400;
pub const VRAM_SIZE: usize = 0x18000;
pub const OAM_SIZE: usize = 0x400;

/// Cycles per scanline and where HBlank begins within it. The HBlank flag rises a
/// little after the 960 visible cycles, as measured on hardware.
pub const CYCLES_PER_LINE: u64 = 1232;
pub const HBLANK_START: u64 = 1006;
pub const LINES_PER_FRAME: u16 = 228;
pub const VBLANK_LINE: u16 = 160;

#[derive(Serialize, Deserialize)]
pub struct Video {
    pub dispcnt: u16,
    pub dispstat: u16,
    pub vcount: u16,
    pub bgcnt: [u16; 4],
    pub bghofs: [u16; 4],
    pub bgvofs: [u16; 4],
    /// Affine parameters for BG2 (index 0) and BG3 (index 1).
    pub bgpa: [i16; 2],
    pub bgpb: [i16; 2],
    pub bgpc: [i16; 2],
    pub bgpd: [i16; 2],
    /// Reference points as written (28-bit signed, 8 fractional bits).
    pub bgx: [i32; 2],
    pub bgy: [i32; 2],
    /// Internal reference points, advanced each scanline.
    bgx_internal: [i32; 2],
    bgy_internal: [i32; 2],
    pub winh: [u16; 2],
    pub winv: [u16; 2],
    pub winin: u16,
    pub winout: u16,
    pub mosaic: u16,
    pub bldcnt: u16,
    pub bldalpha: u16,
    pub bldy: u16,

    #[serde(with = "crate::snapshot::bytes_box")]
    pub palette: Box<[u8; PALETTE_SIZE]>,
    #[serde(with = "crate::snapshot::bytes_box")]
    pub vram: Box<[u8; VRAM_SIZE]>,
    #[serde(with = "crate::snapshot::bytes_box")]
    pub oam: Box<[u8; OAM_SIZE]>,

    #[serde(with = "crate::snapshot::words_box")]
    framebuffer: Box<[u32; SCREEN_WIDTH * SCREEN_HEIGHT]>,
    frame_ready: bool,
    #[serde(skip)]
    scratch: render::Scratch,
}

impl Default for Video {
    fn default() -> Self {
        Self::new()
    }
}

impl Video {
    pub fn new() -> Self {
        Self {
            dispcnt: 0,
            dispstat: 0,
            vcount: 0,
            bgcnt: [0; 4],
            bghofs: [0; 4],
            bgvofs: [0; 4],
            bgpa: [0x100; 2],
            bgpb: [0; 2],
            bgpc: [0; 2],
            bgpd: [0x100; 2],
            bgx: [0; 2],
            bgy: [0; 2],
            bgx_internal: [0; 2],
            bgy_internal: [0; 2],
            winh: [0; 2],
            winv: [0; 2],
            winin: 0,
            winout: 0,
            mosaic: 0,
            bldcnt: 0,
            bldalpha: 0,
            bldy: 0,
            palette: Box::new([0; PALETTE_SIZE]),
            vram: Box::new([0; VRAM_SIZE]),
            oam: Box::new([0; OAM_SIZE]),
            framebuffer: Box::new([0; SCREEN_WIDTH * SCREEN_HEIGHT]),
            frame_ready: false,
            scratch: render::Scratch::default(),
        }
    }

    pub fn framebuffer(&self) -> &[u32] {
        &self.framebuffer[..]
    }

    /// Returns true once per frame, when VBlank begins.
    pub fn take_frame_ready(&mut self) -> bool {
        std::mem::replace(&mut self.frame_ready, false)
    }

    /// Byte offset in VRAM where OBJ tiles begin for the current mode.
    pub fn obj_vram_start(&self) -> usize {
        if self.dispcnt & 7 >= 3 {
            0x14000
        } else {
            0x10000
        }
    }

    /// Each HBlankStart schedules the HBlankEnd that follows it, and vice versa.
    pub fn schedule_first(&self, scheduler: &mut Scheduler) {
        scheduler.schedule_at(Event::HBlankStart, HBLANK_START);
    }

    pub fn on_hblank_start(
        &mut self,
        at: u64,
        scheduler: &mut Scheduler,
        irq: &mut Irq,
        dma: &mut Dma,
    ) {
        self.dispstat |= 1 << 1;
        if self.vcount < VBLANK_LINE {
            self.render_scanline();
            dma.trigger(Timing::HBlank);
        }
        if (2..=VBLANK_LINE + 2).contains(&self.vcount) {
            dma.trigger_video_capture();
        }
        if self.dispstat & (1 << 4) != 0 {
            irq.raise(Interrupt::HBlank);
        }
        scheduler.schedule_at(Event::HBlankEnd, at + (CYCLES_PER_LINE - HBLANK_START));
    }

    pub fn on_hblank_end(
        &mut self,
        at: u64,
        scheduler: &mut Scheduler,
        irq: &mut Irq,
        dma: &mut Dma,
    ) {
        self.dispstat &= !(1 << 1);
        self.vcount += 1;
        if self.vcount == LINES_PER_FRAME {
            self.vcount = 0;
            // Affine backgrounds restart from their reference points each frame.
            self.bgx_internal = self.bgx;
            self.bgy_internal = self.bgy;
        }
        match self.vcount {
            VBLANK_LINE => {
                self.dispstat |= 1;
                self.frame_ready = true;
                dma.trigger(Timing::VBlank);
                if self.dispstat & (1 << 3) != 0 {
                    irq.raise(Interrupt::VBlank);
                }
            }
            // The VBlank flag is clear on the last line (GBATEK: lines 160-226 only).
            227 => self.dispstat &= !1,
            _ => {}
        }
        if self.vcount == self.dispstat >> 8 {
            self.dispstat |= 1 << 2;
            if self.dispstat & (1 << 5) != 0 {
                irq.raise(Interrupt::VCount);
            }
        } else {
            self.dispstat &= !(1 << 2);
        }
        scheduler.schedule_at(Event::HBlankStart, at + HBLANK_START);
    }

    pub fn read_io(&self, reg: u32, open_bus: u32) -> u16 {
        match reg {
            0x00 => self.dispcnt,
            0x02 => 0, // green swap: unused by games, reads as written (always 0 here)
            0x04 => self.dispstat,
            0x06 => self.vcount,
            0x08..=0x0E => self.bgcnt[((reg - 8) / 2) as usize],
            0x48 => self.winin,
            0x4A => self.winout,
            0x50 => self.bldcnt,
            0x52 => self.bldalpha,
            _ => (open_bus >> ((reg & 2) * 8)) as u16,
        }
    }

    pub fn write_io(&mut self, reg: u32, value: u16, mask: u16) {
        let set = |old: u16, keep: u16| (old & !mask) | (value & mask & keep);
        match reg {
            0x00 => self.dispcnt = set(self.dispcnt, 0xFFF7),
            0x04 => self.dispstat = set(self.dispstat, 0xFF38),
            0x08..=0x0E => {
                let n = ((reg - 8) / 2) as usize;
                // Bits 13 (wraparound) is only meaningful for affine BGs 2-3.
                let keep = if n < 2 { 0xDFFF } else { 0xFFFF };
                self.bgcnt[n] = set(self.bgcnt[n], keep);
            }
            0x10..=0x1E => {
                let n = ((reg - 0x10) / 4) as usize;
                if reg & 2 == 0 {
                    self.bghofs[n] = set(self.bghofs[n], 0x1FF);
                } else {
                    self.bgvofs[n] = set(self.bgvofs[n], 0x1FF);
                }
            }
            0x20..=0x3E => {
                let bg = ((reg - 0x20) / 0x10) as usize;
                match (reg - 0x20) % 0x10 {
                    0x0 => self.bgpa[bg] = set(self.bgpa[bg] as u16, 0xFFFF) as i16,
                    0x2 => self.bgpb[bg] = set(self.bgpb[bg] as u16, 0xFFFF) as i16,
                    0x4 => self.bgpc[bg] = set(self.bgpc[bg] as u16, 0xFFFF) as i16,
                    0x6 => self.bgpd[bg] = set(self.bgpd[bg] as u16, 0xFFFF) as i16,
                    0x8 => self.set_ref(bg, false, 0, value, mask),
                    0xA => self.set_ref(bg, false, 16, value, mask),
                    0xC => self.set_ref(bg, true, 0, value, mask),
                    0xE => self.set_ref(bg, true, 16, value, mask),
                    _ => {}
                }
            }
            0x40 => self.winh[0] = set(self.winh[0], 0xFFFF),
            0x42 => self.winh[1] = set(self.winh[1], 0xFFFF),
            0x44 => self.winv[0] = set(self.winv[0], 0xFFFF),
            0x46 => self.winv[1] = set(self.winv[1], 0xFFFF),
            0x48 => self.winin = set(self.winin, 0x3F3F),
            0x4A => self.winout = set(self.winout, 0x3F3F),
            0x4C => self.mosaic = set(self.mosaic, 0xFFFF),
            0x50 => self.bldcnt = set(self.bldcnt, 0x3FFF),
            0x52 => self.bldalpha = set(self.bldalpha, 0x1F1F),
            0x54 => self.bldy = set(self.bldy, 0x1F),
            _ => {}
        }
    }

    /// Writes half of a BGxX / BGxY reference register. The 28-bit value is sign
    /// extended, and the internal register follows the write immediately.
    fn set_ref(&mut self, bg: usize, y: bool, shift: u32, value: u16, mask: u16) {
        let reg = if y { &mut self.bgy[bg] } else { &mut self.bgx[bg] };
        let m = u32::from(mask) << shift;
        let raw = (*reg as u32 & !m) | ((u32::from(value) << shift) & m);
        *reg = ((raw << 4) as i32) >> 4;
        if y {
            self.bgy_internal[bg] = *reg;
        } else {
            self.bgx_internal[bg] = *reg;
        }
    }
}
