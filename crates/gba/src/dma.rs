//! The four DMA channels (GBATEK "GBA DMA Transfers").
//!
//! Channel registers live in `Dma`; the transfer itself is `Bus::run_dma`, because
//! it has to read and write through the bus while the CPU is paused.

use crate::irq::Interrupt;
use crate::memory::Bus;
use serde::{Deserialize, Serialize};

/// What starts a transfer (control bits 12-13).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timing {
    Immediate = 0,
    VBlank = 1,
    HBlank = 2,
    /// Channel 1-2: sound FIFO. Channel 3: video capture. Channel 0: prohibited.
    Special = 3,
}

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
pub struct Channel {
    pub sad: u32,
    pub dad: u32,
    pub count: u16,
    pub control: u16,
    /// Internal copies, latched when the channel is enabled.
    src: u32,
    dst: u32,
    remaining: u32,
    pending: bool,
}

impl Channel {
    fn enabled(&self) -> bool {
        self.control & (1 << 15) != 0
    }

    fn timing(&self) -> Timing {
        match (self.control >> 12) & 3 {
            0 => Timing::Immediate,
            1 => Timing::VBlank,
            2 => Timing::HBlank,
            _ => Timing::Special,
        }
    }

    fn word_size(&self) -> bool {
        self.control & (1 << 10) != 0
    }

    fn repeat(&self) -> bool {
        self.control & (1 << 9) != 0
    }
}

#[derive(Default, Serialize, Deserialize)]
pub struct Dma {
    pub channels: [Channel; 4],
    /// Last value transferred; reads of open bus by DMA return it.
    latch: u32,
    any_pending: bool,
}

impl Dma {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline(always)]
    pub fn has_pending(&self) -> bool {
        self.any_pending
    }

    pub fn read_io(&self, reg: u32) -> u16 {
        let n = ((reg - 0xB0) / 12) as usize;
        match (reg - 0xB0) % 12 {
            // SAD, DAD and the count are write-only.
            10 => self.channels[n].control,
            _ => 0,
        }
    }

    pub fn write_io(&mut self, reg: u32, value: u16, mask: u16) {
        let n = ((reg - 0xB0) / 12) as usize;
        let ch = &mut self.channels[n];
        let v = u32::from(value & mask);
        let m = u32::from(mask);
        match (reg - 0xB0) % 12 {
            0 => ch.sad = (ch.sad & !m) | v,
            2 => ch.sad = (ch.sad & !(m << 16)) | (v << 16),
            4 => ch.dad = (ch.dad & !m) | v,
            6 => ch.dad = (ch.dad & !(m << 16)) | (v << 16),
            8 => ch.count = (ch.count & !mask) | (value & mask),
            10 => {
                let was_enabled = ch.enabled();
                // Bit 11 (gamepak DRQ) exists on channel 3 only; bits 0-4 are unused.
                let valid = if n == 3 { 0xFFE0 } else { 0xF7E0 };
                ch.control = (ch.control & !mask) | (value & mask & valid);
                if ch.enabled() && !was_enabled {
                    let src_mask = if n == 0 { 0x07FF_FFFF } else { 0x0FFF_FFFF };
                    let dst_mask = if n == 3 { 0x0FFF_FFFF } else { 0x07FF_FFFF };
                    ch.src = ch.sad & src_mask;
                    ch.dst = ch.dad & dst_mask;
                    ch.remaining = Self::count_for(n, ch);
                    if ch.timing() == Timing::Immediate {
                        ch.pending = true;
                        self.any_pending = true;
                    }
                } else if !ch.enabled() {
                    ch.pending = false;
                }
            }
            _ => {}
        }
    }

    fn count_for(n: usize, ch: &Channel) -> u32 {
        let max = if n == 3 { 0x1_0000 } else { 0x4000 };
        if ch.count == 0 {
            max
        } else {
            u32::from(ch.count) & (max - 1)
        }
    }

    /// Marks every enabled channel with the given timing as ready to run.
    pub fn trigger(&mut self, timing: Timing) {
        for ch in &mut self.channels {
            if ch.enabled() && ch.timing() == timing {
                ch.pending = true;
                self.any_pending = true;
            }
        }
    }

    /// Video capture: channel 3 in special mode, on HBlank of lines 2-162.
    pub fn trigger_video_capture(&mut self) {
        let ch = &mut self.channels[3];
        if ch.enabled() && ch.timing() == Timing::Special {
            ch.pending = true;
            self.any_pending = true;
        }
    }

    /// Sound FIFO `fifo` (0 = A, 1 = B) wants 4 more words: run the channel (1 or 2)
    /// whose destination is that FIFO, if it is set up for sound DMA.
    pub fn request_fifo(&mut self, fifo: usize) {
        let fifo_addr = 0x0400_00A0 + 4 * fifo as u32;
        for ch in &mut self.channels[1..3] {
            if ch.enabled() && ch.timing() == Timing::Special && ch.dst == fifo_addr {
                ch.pending = true;
                self.any_pending = true;
            }
        }
    }
}

impl Bus {
    /// Runs every pending channel in priority order (0 first).
    pub fn run_dma(&mut self) {
        self.dma.any_pending = false;
        for n in 0..4 {
            if self.dma.channels[n].pending {
                self.dma.channels[n].pending = false;
                self.run_channel(n);
            }
        }
    }

    fn run_channel(&mut self, n: usize) {
        let mut ch = self.dma.channels[n];
        let fifo_mode = n != 0 && n != 3 && ch.timing() == Timing::Special;
        let word = ch.word_size() || fifo_mode;
        let size: u32 = if word { 4 } else { 2 };
        let count = if fifo_mode { 4 } else { ch.remaining };

        let src_step: i32 = match (ch.control >> 7) & 3 {
            0 => size as i32,
            1 => -(size as i32),
            _ => 0,
        };
        let dst_step: i32 = if fifo_mode {
            0
        } else {
            match (ch.control >> 5) & 3 {
                0 | 3 => size as i32,
                1 => -(size as i32),
                _ => 0,
            }
        };

        // EEPROM decides between the 512 B and 8 KB protocol from the first DMA size.
        if n == 3 && self.cart.is_eeprom_addr(ch.dst) || self.cart.is_eeprom_addr(ch.src) {
            self.cart.eeprom_dma_hint(count);
        }
        // Reading ROM through DMA aborts any instruction prefetch in progress.
        self.cart.prefetch_reset();

        // 2 internal cycles to start; 2 more when both ends are in the cartridge.
        let src_rom = (0x08..=0x0D).contains(&(ch.src >> 24));
        let dst_rom = (0x08..=0x0D).contains(&(ch.dst >> 24));
        self.idle(if src_rom && dst_rom { 4 } else { 2 });

        let mut seq = false;
        for _ in 0..count {
            if word {
                let src = ch.src & !3;
                let value = if src >= 0x0200_0000 || self.pc < 0x4000 {
                    self.read32(src, seq)
                } else {
                    // DMA from the BIOS / unmapped low addresses returns the DMA latch.
                    self.wait32(src, seq);
                    self.dma.latch
                };
                self.dma.latch = value;
                self.write32(ch.dst & !3, value, seq);
            } else {
                let src = ch.src & !1;
                let value = if src >= 0x0200_0000 || self.pc < 0x4000 {
                    u32::from(self.read16(src, seq))
                } else {
                    self.wait16(src, seq);
                    self.dma.latch
                };
                self.dma.latch = value | (value << 16);
                self.write16(ch.dst & !1, value as u16, seq);
            }
            ch.src = ch.src.wrapping_add(src_step as u32);
            ch.dst = ch.dst.wrapping_add(dst_step as u32);
            seq = true;
        }

        if ch.control & (1 << 14) != 0 {
            self.irq.raise(Interrupt::dma(n));
        }

        if ch.repeat() && ch.timing() != Timing::Immediate {
            ch.remaining = Dma::count_for(n, &ch);
            if !fifo_mode && (ch.control >> 5) & 3 == 3 {
                let dst_mask = if n == 3 { 0x0FFF_FFFF } else { 0x07FF_FFFF };
                ch.dst = ch.dad & dst_mask;
            }
        } else {
            ch.control &= !(1 << 15);
        }
        self.dma.channels[n] = ch;
    }
}
