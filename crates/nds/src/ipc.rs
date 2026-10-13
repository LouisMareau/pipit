//! Inter-processor communication: IPCSYNC (a nibble each way plus a "remote
//! IRQ" line) and the two 16-word FIFOs, one in each direction.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

const FIFO_DEPTH: usize = 16;

/// One direction's FIFO.
#[derive(Default, Serialize, Deserialize)]
struct Fifo {
    words: VecDeque<u32>,
    /// The last word read, returned again on an empty read.
    last: u32,
}

/// What one CPU controls.
#[derive(Default, Serialize, Deserialize)]
pub struct Side {
    /// IPCSYNC bits 8–11: the nibble this CPU sends.
    pub sync_out: u8,
    /// IPCSYNC bit 14: this CPU wants an IRQ when the other side pings.
    pub sync_irq_enable: bool,
    /// IPCFIFOCNT bits 2, 10, 15.
    pub send_empty_irq: bool,
    pub recv_irq: bool,
    pub fifo_enable: bool,
    /// IPCFIFOCNT bit 14: an empty read or a full write happened.
    pub error: bool,
}

/// Both halves. Index 0 is the ARM9, 1 the ARM7.
#[derive(Default, Serialize, Deserialize)]
pub struct Ipc {
    pub sides: [Side; 2],
    /// `fifos[0]` carries ARM9 → ARM7, `fifos[1]` ARM7 → ARM9.
    fifos: [Fifo; 2],
}

/// Interrupts a register access asks for: `(this CPU, the other CPU)` as
/// pairs of "send empty" and "receive not empty" flags, plus a sync ping.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Signals {
    pub my_send_empty: bool,
    pub my_recv: bool,
    pub other_send_empty: bool,
    pub other_recv: bool,
    pub other_sync: bool,
}

impl Ipc {
    pub fn new() -> Self {
        Self::default()
    }

    /// IPCSYNC (180h) as `cpu` reads it.
    pub fn read_sync(&self, cpu: usize) -> u16 {
        let me = &self.sides[cpu];
        let other = &self.sides[cpu ^ 1];
        u16::from(other.sync_out)
            | u16::from(me.sync_out) << 8
            | u16::from(me.sync_irq_enable) << 14
    }

    /// IPCSYNC written by `cpu`.
    pub fn write_sync(&mut self, cpu: usize, value: u16, mask: u16) -> Signals {
        let mut signals = Signals::default();
        if mask & 0xFF00 != 0 {
            let me = &mut self.sides[cpu];
            me.sync_out = ((value >> 8) & 0xF) as u8;
            me.sync_irq_enable = value & (1 << 14) != 0;
            if value & (1 << 13) != 0 && self.sides[cpu ^ 1].sync_irq_enable {
                signals.other_sync = true;
            }
        }
        signals
    }

    /// IPCFIFOCNT (184h) as `cpu` reads it.
    pub fn read_cnt(&self, cpu: usize) -> u16 {
        let me = &self.sides[cpu];
        let send = &self.fifos[cpu];
        let recv = &self.fifos[cpu ^ 1];
        u16::from(send.words.is_empty())
            | u16::from(send.words.len() == FIFO_DEPTH) << 1
            | u16::from(me.send_empty_irq) << 2
            | u16::from(recv.words.is_empty()) << 8
            | u16::from(recv.words.len() == FIFO_DEPTH) << 9
            | u16::from(me.recv_irq) << 10
            | u16::from(me.error) << 14
            | u16::from(me.fifo_enable) << 15
    }

    /// IPCFIFOCNT written by `cpu`; enabling an IRQ whose condition already
    /// holds raises it at once.
    pub fn write_cnt(&mut self, cpu: usize, value: u16, mask: u16) -> Signals {
        let mut signals = Signals::default();
        if mask & 0x00FF != 0 {
            if value & (1 << 3) != 0 {
                self.fifos[cpu].words.clear();
            }
            let enable = value & (1 << 2) != 0;
            let me = &mut self.sides[cpu];
            if enable && !me.send_empty_irq && self.fifos[cpu].words.is_empty() {
                signals.my_send_empty = true;
            }
            me.send_empty_irq = enable;
        }
        if mask & 0xFF00 != 0 {
            let enable = value & (1 << 10) != 0;
            let me = &mut self.sides[cpu];
            if enable && !me.recv_irq && !self.fifos[cpu ^ 1].words.is_empty() {
                signals.my_recv = true;
            }
            me.recv_irq = enable;
            if value & (1 << 14) != 0 {
                me.error = false;
            }
            me.fifo_enable = value & (1 << 15) != 0;
        }
        signals
    }

    /// IPCFIFOSEND (188h) written by `cpu`: queues a word for the other side.
    pub fn send(&mut self, cpu: usize, value: u32) -> Signals {
        let mut signals = Signals::default();
        if !self.sides[cpu].fifo_enable {
            return signals;
        }
        let fifo = &mut self.fifos[cpu];
        if fifo.words.len() == FIFO_DEPTH {
            self.sides[cpu].error = true;
            return signals;
        }
        let was_empty = fifo.words.is_empty();
        fifo.words.push_back(value);
        signals.other_recv = was_empty && self.sides[cpu ^ 1].recv_irq;
        signals
    }

    /// IPCFIFORECV (100000h) read by `cpu`: takes a word the other side sent.
    pub fn receive(&mut self, cpu: usize) -> (u32, Signals) {
        let mut signals = Signals::default();
        let fifo = &mut self.fifos[cpu ^ 1];
        if !self.sides[cpu].fifo_enable {
            return (fifo.words.front().copied().unwrap_or(fifo.last), signals);
        }
        match fifo.words.pop_front() {
            Some(word) => {
                fifo.last = word;
                signals.other_send_empty =
                    fifo.words.is_empty() && self.sides[cpu ^ 1].send_empty_irq;
                (word, signals)
            }
            None => {
                self.sides[cpu].error = true;
                (fifo.last, signals)
            }
        }
    }
}
