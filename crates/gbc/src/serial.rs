//! The serial port (Pan Docs "Serial Data Transfer"). A transfer clocked from
//! here shifts eight bits out and in; one waiting for an external clock does
//! nothing until a partner clocks it. With nothing plugged in the byte that
//! comes back is 0xFF. Consoles are joined by [`crate::link::Link`], which
//! carries the bytes; the port tells it when a transfer starts and ends
//! through [`Stop`]s. Bytes sent are kept for test ROMs, which print this way.

use serde::{Deserialize, Serialize};

const INTERRUPT: u8 = 1 << 3;

/// Why a linked console's run loop stopped: its link has something to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stop {
    /// This console started clocking a transfer; the link latches both bytes.
    TransferStart,
    /// The eight bits have been clocked; the link delivers the bytes.
    TransferEnd,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Serial {
    sb: u8,
    sc: u8,
    /// Bits still to shift in the current transfer.
    bits: u8,
    /// Clocks until the next bit.
    countdown: u32,
    /// On a cable: the link, not the port, finishes transfers.
    linked: bool,
    stop: Option<Stop>,
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
            if self.linked {
                self.stop = Some(Stop::TransferEnd);
            } else {
                self.finish(if_);
            }
        }
    }

    fn finish(&mut self, if_: &mut u8) {
        self.sc &= 0x7F;
        *if_ |= INTERRUPT;
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
                        if self.linked {
                            self.stop = Some(Stop::TransferStart);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// The byte waiting to go out (SB).
    pub fn byte(&self) -> u8 {
        self.sb
    }

    /// The clocking console's transfer finished: `received` is the partner's byte.
    pub fn complete(&mut self, received: u8, if_: &mut u8) {
        self.sb = received;
        self.finish(if_);
    }

    /// The partner clocked a byte through this port. The shift register always
    /// takes it; the interrupt only comes when a transfer was expected.
    pub fn receive(&mut self, received: u8, if_: &mut u8) {
        self.sb = received;
        if self.sc & 0x80 != 0 {
            self.finish(if_);
        }
    }

    pub fn attach(&mut self, linked: bool) {
        self.linked = linked;
    }

    pub fn take_stop(&mut self) -> Option<Stop> {
        self.stop.take()
    }

    /// Everything sent since the last call.
    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.output)
    }
}
