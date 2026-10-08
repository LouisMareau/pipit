//! Keypad input: KEYINPUT and KEYCNT (GBATEK "GBA Keypad Input").

use crate::irq::{Interrupt, Irq};

/// Pressed keys as a bit set. Bits follow the KEYINPUT layout, but here 1 = pressed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Keys(pub u16);

impl Keys {
    pub const A: Keys = Keys(1 << 0);
    pub const B: Keys = Keys(1 << 1);
    pub const SELECT: Keys = Keys(1 << 2);
    pub const START: Keys = Keys(1 << 3);
    pub const RIGHT: Keys = Keys(1 << 4);
    pub const LEFT: Keys = Keys(1 << 5);
    pub const UP: Keys = Keys(1 << 6);
    pub const DOWN: Keys = Keys(1 << 7);
    pub const R: Keys = Keys(1 << 8);
    pub const L: Keys = Keys(1 << 9);
    pub const NONE: Keys = Keys(0);
    pub const ALL: Keys = Keys(0x3FF);

    pub fn contains(self, other: Keys) -> bool {
        self.0 & other.0 == other.0
    }
}

impl std::ops::BitOr for Keys {
    type Output = Keys;
    fn bitor(self, rhs: Keys) -> Keys {
        Keys(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for Keys {
    fn bitor_assign(&mut self, rhs: Keys) {
        self.0 |= rhs.0;
    }
}

#[derive(Default)]
pub struct Keypad {
    keys: Keys,
    /// KEYCNT (0x4000132): bits 0-9 key mask, bit 14 IRQ enable, bit 15 AND/OR condition.
    keycnt: u16,
}

impl Keypad {
    pub fn new() -> Self {
        Self::default()
    }

    /// Updates the held keys and raises the keypad interrupt if KEYCNT asks for it.
    pub fn set_keys(&mut self, keys: Keys, irq: &mut Irq) {
        self.keys = Keys(keys.0 & 0x3FF);
        self.check_irq(irq);
    }

    pub fn keys(&self) -> Keys {
        self.keys
    }

    fn check_irq(&self, irq: &mut Irq) {
        if self.keycnt & (1 << 14) == 0 {
            return;
        }
        let mask = self.keycnt & 0x3FF;
        let hit = if self.keycnt & (1 << 15) != 0 {
            mask != 0 && self.keys.0 & mask == mask // AND: all selected keys
        } else {
            self.keys.0 & mask != 0 // OR: any selected key
        };
        if hit {
            irq.raise(Interrupt::Keypad);
            // Stop mode is left on a keypad interrupt.
            irq.stopped = false;
        }
    }

    pub fn read_io(&self, reg: u32) -> u16 {
        match reg {
            // KEYINPUT is active-low: 0 = pressed.
            0x130 => !self.keys.0 & 0x3FF,
            0x132 => self.keycnt,
            _ => 0,
        }
    }

    pub fn write_io(&mut self, reg: u32, value: u16, mask: u16, irq: &mut Irq) {
        if reg == 0x132 {
            self.keycnt = (self.keycnt & !mask) | (value & mask & 0xC3FF);
            self.check_irq(irq);
        }
    }
}
