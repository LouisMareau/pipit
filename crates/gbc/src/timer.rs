//! DIV and TIMA (Pan Docs "Timer and Divider Registers").
//!
//! TIMA is clocked by a falling edge of one bit of the 16-bit divider, which
//! is what makes writes to DIV and TAC able to tick it. An overflow reloads
//! TIMA from TMA one M-cycle later, during which a write to TIMA cancels the
//! reload.

use serde::{Deserialize, Serialize};

const INTERRUPT: u8 = 1 << 2;

#[derive(Default, Serialize, Deserialize)]
pub struct Timer {
    div: u16,
    tima: u8,
    tma: u8,
    tac: u8,
    /// TIMA overflowed on the previous M-cycle: reload and interrupt now.
    overflow: bool,
    /// The reload is happening this M-cycle (a TMA write still lands in TIMA).
    reloading: bool,
}

impl Timer {
    pub fn new() -> Self {
        Self { div: 0xABCC, ..Default::default() }
    }

    fn bit(&self) -> bool {
        let shift = [9, 3, 5, 7][usize::from(self.tac & 3)];
        self.tac & 4 != 0 && self.div & (1 << shift) != 0
    }

    /// One M-cycle (four clocks of the CPU, whatever its speed).
    pub fn step(&mut self, if_: &mut u8) {
        self.reloading = false;
        if self.overflow {
            self.overflow = false;
            self.tima = self.tma;
            self.reloading = true;
            *if_ |= INTERRUPT;
        }
        let before = self.bit();
        self.div = self.div.wrapping_add(4);
        if before && !self.bit() {
            self.increment();
        }
    }

    fn increment(&mut self) {
        let (value, overflowed) = self.tima.overflowing_add(1);
        self.tima = value;
        if overflowed {
            self.overflow = true;
        }
    }

    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            0xFF04 => (self.div >> 8) as u8,
            0xFF05 => self.tima,
            0xFF06 => self.tma,
            0xFF07 => self.tac | 0xF8,
            _ => 0xFF,
        }
    }

    pub fn write(&mut self, addr: u16, value: u8) {
        match addr {
            0xFF04 => self.reset_div(),
            0xFF05 => {
                if self.reloading {
                    return;
                }
                self.tima = value;
                self.overflow = false;
            }
            0xFF06 => {
                self.tma = value;
                if self.reloading {
                    self.tima = value;
                }
            }
            0xFF07 => {
                let before = self.bit();
                self.tac = value & 7;
                if before && !self.bit() {
                    self.increment();
                }
            }
            _ => {}
        }
    }

    /// Writing DIV (or switching speed) clears the divider, which can tick TIMA.
    pub fn reset_div(&mut self) {
        let before = self.bit();
        self.div = 0;
        if before {
            self.increment();
        }
    }
}
