//! One CPU's interrupt controller: IME, IE and IF (32 bits each on the DS), and
//! whether the CPU is halted.

use serde::{Deserialize, Serialize};

/// Interrupt sources, as bit numbers of IE and IF.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Interrupt {
    VBlank = 0,
    HBlank = 1,
    VCount = 2,
    Timer0 = 3,
    Timer1 = 4,
    Timer2 = 5,
    Timer3 = 6,
    /// ARM7 only: the real-time clock.
    Rtc = 7,
    Dma0 = 8,
    Dma1 = 9,
    Dma2 = 10,
    Dma3 = 11,
    Keypad = 12,
    GbaSlot = 13,
    IpcSync = 16,
    IpcSendEmpty = 17,
    IpcRecv = 18,
    CardDone = 19,
    CardIreq = 20,
    /// ARM9 only: geometry command FIFO.
    GxFifo = 21,
    /// ARM7 only.
    Hinge = 22,
    Spi = 23,
    Wifi = 24,
}

impl Interrupt {
    pub fn timer(n: usize) -> Self {
        [Self::Timer0, Self::Timer1, Self::Timer2, Self::Timer3][n]
    }

    pub fn dma(n: usize) -> Self {
        [Self::Dma0, Self::Dma1, Self::Dma2, Self::Dma3][n]
    }
}

#[derive(Default, Serialize, Deserialize)]
pub struct Irq {
    pub ime: bool,
    pub ie: u32,
    pub if_: u32,
    /// Sleeping until an enabled interrupt comes (HALTCNT on the ARM7, CP15 on
    /// the ARM9).
    pub halted: bool,
}

impl Irq {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline(always)]
    pub fn raise(&mut self, source: Interrupt) {
        self.if_ |= 1 << source as u32;
    }

    /// Whether an enabled interrupt is pending and IME allows it.
    #[inline(always)]
    pub fn should_interrupt(&self) -> bool {
        self.ime && (self.ie & self.if_) != 0
    }

    /// Whether a halted CPU wakes up: any enabled interrupt, regardless of IME.
    #[inline(always)]
    pub fn should_wake(&self) -> bool {
        (self.ie & self.if_) != 0
    }

    /// IME (208h), IE (210h), IF (214h): 16-bit halves at `reg`.
    pub fn read_io(&self, reg: u32) -> u16 {
        match reg {
            0x208 => u16::from(self.ime),
            0x210 => self.ie as u16,
            0x212 => (self.ie >> 16) as u16,
            0x214 => self.if_ as u16,
            0x216 => (self.if_ >> 16) as u16,
            _ => 0,
        }
    }

    pub fn write_io(&mut self, reg: u32, value: u16, mask: u16) {
        let value = value & mask;
        match reg {
            0x208 => {
                if mask & 1 != 0 {
                    self.ime = value & 1 != 0;
                }
            }
            0x210 => self.ie = (self.ie & !u32::from(mask)) | u32::from(value),
            0x212 => self.ie = (self.ie & !(u32::from(mask) << 16)) | (u32::from(value) << 16),
            // Writing 1 acknowledges.
            0x214 => self.if_ &= !u32::from(value),
            0x216 => self.if_ &= !(u32::from(value) << 16),
            _ => {}
        }
    }
}
