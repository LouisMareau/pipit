//! One CPU's four DMA channels: the registers and what starts them. The
//! transfer itself is in `memory.rs`, since it runs through that CPU's bus.

use serde::{Deserialize, Serialize};

/// What starts a transfer. The ARM9 has eight modes (control bits 27-29), the
/// ARM7 four (bits 28-29), some of them per channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timing {
    Immediate,
    VBlank,
    HBlank,
    /// ARM9: the start of each scanline's display.
    DisplayStart,
    /// ARM9: the main-memory display FIFO wants data.
    DisplayFifo,
    /// A data word from the DS cartridge.
    Card,
    /// The GBA slot.
    GbaSlot,
    /// ARM9: the geometry command FIFO is half empty.
    GxFifo,
    /// ARM7: wireless.
    Wifi,
}

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
pub struct Channel {
    pub sad: u32,
    pub dad: u32,
    pub control: u32,
    /// Internal copies, latched when the channel is enabled.
    pub src: u32,
    pub dst: u32,
    pub remaining: u32,
    pub pending: bool,
}

impl Channel {
    pub fn enabled(&self) -> bool {
        self.control & (1 << 31) != 0
    }

    pub fn word_size(&self) -> bool {
        self.control & (1 << 26) != 0
    }

    pub fn repeat(&self) -> bool {
        self.control & (1 << 25) != 0
    }

    pub fn irq(&self) -> bool {
        self.control & (1 << 30) != 0
    }

    pub fn src_control(&self) -> u32 {
        (self.control >> 23) & 3
    }

    pub fn dst_control(&self) -> u32 {
        (self.control >> 21) & 3
    }
}

#[derive(Serialize, Deserialize)]
pub struct Dma {
    pub channels: [Channel; 4],
    /// ARM9: the four DMAFILL words (0E0h), the source of a transfer from them.
    pub fill: [u32; 4],
    /// Whether this is the ARM9's controller (the ARM7's has fewer modes and a
    /// 16-bit count).
    arm9: bool,
    any_pending: bool,
}

impl Dma {
    pub fn new(arm9: bool) -> Self {
        Self { channels: [Channel::default(); 4], fill: [0; 4], arm9, any_pending: false }
    }

    #[inline(always)]
    pub fn has_pending(&self) -> bool {
        self.any_pending
    }

    pub fn timing(&self, n: usize, ch: &Channel) -> Timing {
        if self.arm9 {
            match (ch.control >> 27) & 7 {
                0 => Timing::Immediate,
                1 => Timing::VBlank,
                2 => Timing::HBlank,
                3 => Timing::DisplayStart,
                4 => Timing::DisplayFifo,
                5 => Timing::Card,
                6 => Timing::GbaSlot,
                _ => Timing::GxFifo,
            }
        } else {
            match (ch.control >> 28) & 3 {
                0 => Timing::Immediate,
                1 => Timing::VBlank,
                2 => Timing::Card,
                _ => {
                    if n & 1 == 0 {
                        Timing::Wifi
                    } else {
                        Timing::GbaSlot
                    }
                }
            }
        }
    }

    pub fn count_for(&self, ch: &Channel) -> u32 {
        let max = if self.arm9 { 0x20_0000 } else { 0x1_0000 };
        let count = ch.control & (max - 1);
        if count == 0 {
            max
        } else {
            count
        }
    }

    /// DMAxSAD, DMAxDAD, DMAxCNT (0B0h–0DFh) as 16-bit halves; the controls
    /// read back, the addresses do not.
    pub fn read_io(&self, reg: u32) -> u16 {
        let off = reg - 0xB0;
        let n = (off / 12) as usize;
        match off % 12 {
            8 => self.channels[n].control as u16,
            10 => (self.channels[n].control >> 16) as u16,
            _ => 0,
        }
    }

    pub fn write_io(&mut self, reg: u32, value: u16, mask: u16) {
        let off = reg - 0xB0;
        let n = (off / 12) as usize;
        let v = u32::from(value & mask);
        let m = u32::from(mask);
        let mut ch = self.channels[n];
        match off % 12 {
            0 => ch.sad = (ch.sad & !m) | v,
            2 => ch.sad = (ch.sad & !(m << 16)) | (v << 16),
            4 => ch.dad = (ch.dad & !m) | v,
            6 => ch.dad = (ch.dad & !(m << 16)) | (v << 16),
            8 => ch.control = (ch.control & !m) | v,
            10 => {
                let was_enabled = ch.enabled();
                ch.control = (ch.control & !(m << 16)) | (v << 16);
                if ch.enabled() && !was_enabled {
                    ch.src = ch.sad & 0x0FFF_FFFF;
                    ch.dst = ch.dad & 0x0FFF_FFFF;
                    ch.remaining = self.count_for(&ch);
                    if self.timing(n, &ch) == Timing::Immediate {
                        ch.pending = true;
                        self.any_pending = true;
                    }
                } else if !ch.enabled() {
                    ch.pending = false;
                }
            }
            _ => {}
        }
        self.channels[n] = ch;
    }

    /// Marks every enabled channel with the given timing as ready to run.
    pub fn trigger(&mut self, timing: Timing) {
        for n in 0..4 {
            let ch = self.channels[n];
            if ch.enabled() && self.timing(n, &ch) == timing {
                self.channels[n].pending = true;
                self.any_pending = true;
            }
        }
    }

    /// Takes the pending flags: which channels to run now, in priority order.
    pub fn take_pending(&mut self) -> [bool; 4] {
        self.any_pending = false;
        let mut out = [false; 4];
        for (slot, ch) in out.iter_mut().zip(self.channels.iter_mut()) {
            *slot = ch.pending;
            ch.pending = false;
        }
        out
    }
}
