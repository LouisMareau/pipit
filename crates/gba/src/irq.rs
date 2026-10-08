//! Interrupt controller: IE, IF, IME and the halt state (GBATEK "GBA Interrupt Control").

/// Interrupt sources, as bit positions in IE / IF.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum Interrupt {
    VBlank = 1 << 0,
    HBlank = 1 << 1,
    VCount = 1 << 2,
    Timer0 = 1 << 3,
    Timer1 = 1 << 4,
    Timer2 = 1 << 5,
    Timer3 = 1 << 6,
    Serial = 1 << 7,
    Dma0 = 1 << 8,
    Dma1 = 1 << 9,
    Dma2 = 1 << 10,
    Dma3 = 1 << 11,
    Keypad = 1 << 12,
    GamePak = 1 << 13,
}

impl Interrupt {
    pub fn timer(n: usize) -> Self {
        [Self::Timer0, Self::Timer1, Self::Timer2, Self::Timer3][n]
    }

    pub fn dma(n: usize) -> Self {
        [Self::Dma0, Self::Dma1, Self::Dma2, Self::Dma3][n]
    }
}

#[derive(Default)]
pub struct Irq {
    /// Interrupt Enable register (0x4000200).
    pub ie: u16,
    /// Interrupt Request flags (0x4000202). Bits are acknowledged by writing 1.
    pub if_: u16,
    /// Interrupt Master Enable (0x4000208), only bit 0 matters.
    pub ime: bool,
    /// CPU is halted (HALTCNT / SWI Halt) until `ie & if_` becomes non-zero.
    pub halted: bool,
    /// CPU is stopped (HALTCNT bit 7) until a keypad / gamepak / serial interrupt.
    pub stopped: bool,
}

impl Irq {
    pub fn new() -> Self {
        Self::default()
    }

    /// Flags an interrupt as requested. The CPU picks it up on its next step if
    /// enabled and not masked.
    #[inline]
    pub fn raise(&mut self, source: Interrupt) {
        self.if_ |= source as u16;
    }

    /// Whether an enabled interrupt is pending and IME allows it.
    #[inline(always)]
    pub fn should_interrupt(&self) -> bool {
        self.ime && (self.ie & self.if_) != 0
    }

    /// Whether a halted CPU should wake up: any enabled interrupt, regardless of IME.
    #[inline(always)]
    pub fn should_wake(&self) -> bool {
        (self.ie & self.if_) != 0
    }

    pub fn read_io(&self, reg: u32) -> u16 {
        match reg {
            0x200 => self.ie,
            0x202 => self.if_,
            0x208 => self.ime as u16,
            _ => 0,
        }
    }

    pub fn write_io(&mut self, reg: u32, value: u16, mask: u16) {
        match reg {
            0x200 => self.ie = (self.ie & !mask) | (value & mask & 0x3FFF),
            // Writing 1 acknowledges; only the bytes actually written take part.
            0x202 => self.if_ &= !(value & mask),
            0x208 if mask & 0x00FF != 0 => self.ime = value & 1 != 0,
            _ => {}
        }
    }
}
