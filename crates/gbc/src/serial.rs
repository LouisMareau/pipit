//! The serial port (Pan Docs "Serial Data Transfer"). With nothing plugged in
//! a transfer clocked from here completes with 0xFF received, and one waiting
//! for an external clock never does. Bytes sent are kept for test ROMs, which
//! print through the port.

use serde::{Deserialize, Serialize};

const INTERRUPT: u8 = 1 << 3;

#[derive(Default, Serialize, Deserialize)]
pub struct Serial {
    sb: u8,
    sc: u8,
    /// Bits still to shift in the current transfer.
    bits: u8,
    /// Clocks until the next bit.
    countdown: u32,
    #[serde(skip)]
    output: Vec<u8>,
}

impl Serial {
    pub fn new() -> Self {
        Self::default()
    }

    /// `t` clocks passed (at the port's own rate, which the speed switch does not change).
    pub fn step(&mut self, t: u32, cgb: bool, if_: &mut u8) {
        if self.bits == 0 || self.sc & 0x81 != 0x81 {
            return;
        }
        let period = if cgb && self.sc & 2 != 0 { 16 } else { 512 };
        self.countdown += t;
        while self.countdown >= period && self.bits > 0 {
            self.countdown -= period;
            self.sb = (self.sb << 1) | 1;
            self.bits -= 1;
        }
        if self.bits == 0 {
            self.sc &= 0x7F;
            *if_ |= INTERRUPT;
        }
    }

    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            0xFF01 => self.sb,
            0xFF02 => self.sc | 0x7C,
            _ => 0xFF,
        }
    }

    pub fn write(&mut self, addr: u16, value: u8) {
        match addr {
            0xFF01 => self.sb = value,
            0xFF02 => {
                self.sc = value & 0x83;
                if value & 0x80 != 0 {
                    self.output.push(self.sb);
                    if value & 1 != 0 {
                        self.bits = 8;
                        self.countdown = 0;
                    }
                }
            }
            _ => {}
        }
    }

    /// Everything sent since the last call.
    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.output)
    }
}
