//! The joypad register (Pan Docs "Joypad Input"): two selectable groups of
//! four lines, low when pressed.

use serde::{Deserialize, Serialize};

const INTERRUPT: u8 = 1 << 4;

/// Key bits as the front-ends pass them: the Game Boy Advance layout's low byte.
pub mod key {
    pub const A: u8 = 1 << 0;
    pub const B: u8 = 1 << 1;
    pub const SELECT: u8 = 1 << 2;
    pub const START: u8 = 1 << 3;
    pub const RIGHT: u8 = 1 << 4;
    pub const LEFT: u8 = 1 << 5;
    pub const UP: u8 = 1 << 6;
    pub const DOWN: u8 = 1 << 7;
}

#[derive(Default, Serialize, Deserialize)]
pub struct Joypad {
    /// P1 bits 4-5 as written: 0 selects the group.
    select: u8,
    keys: u8,
}

impl Joypad {
    pub fn new() -> Self {
        Self { select: 0x30, keys: 0 }
    }

    fn lines(&self) -> u8 {
        let mut low = 0;
        if self.select & 0x10 == 0 {
            low |= (self.keys >> 4) & 0x0F; // Right, Left, Up, Down
        }
        if self.select & 0x20 == 0 {
            low |= self.keys & 0x0F; // A, B, Select, Start
        }
        !low & 0x0F
    }

    pub fn read(&self) -> u8 {
        0xC0 | self.select | self.lines()
    }

    pub fn write(&mut self, value: u8) {
        self.select = value & 0x30;
    }

    /// A line going low (a selected key pressed) requests the joypad interrupt.
    pub fn set_keys(&mut self, keys: u8, if_: &mut u8) {
        let before = self.lines();
        self.keys = keys;
        if before & !self.lines() != 0 {
            *if_ |= INTERRUPT;
        }
    }
}
