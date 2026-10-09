//! Serial communication (GBATEK "GBA Communication Ports").
//!
//! Only 16-bit multi-play mode is emulated: the mode the link cable uses for most
//! multiplayer games, Pokémon's Cable Club among them. The other modes (normal,
//! UART, JOY bus, general purpose) keep their registers but never transfer
//! anything.
//!
//! A console on its own behaves as if nothing were plugged in: it can run
//! transfers and only ever hears itself. Several consoles are joined by
//! [`crate::link::Link`], which runs them in lockstep and carries the words from
//! each to all. The port talks to its link through [`Stop`]s: the console's run
//! loop returns one, the link acts on it, and running resumes.

use crate::irq::{Interrupt, Irq};
use crate::scheduler::{Event, Scheduler};
use crate::CLOCK_HZ;
use serde::{Deserialize, Serialize};

/// First register of the port; `regs[i]` is the halfword at `0x120 + 2·i`.
const BASE: u32 = 0x120;
const SIOCNT: usize = (0x128 - 0x120) / 2;
const SIOMLT_SEND: usize = (0x12A - 0x120) / 2;
const RCNT: usize = (0x134 - 0x120) / 2;
const SIO_START: u16 = 1 << 7;
const SIO_IRQ_ENABLE: u16 = 1 << 14;

/// What the port is set up to do, decoded from RCNT and SIOCNT.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal8,
    Normal32,
    MultiPlay,
    Uart,
    GeneralPurpose,
    JoyBus,
}

/// Why a linked console's run loop stopped: its link has something to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stop {
    /// The port changed mode; the link re-evaluates whether everyone is ready.
    ModeChange,
    /// This console (the parent) started a transfer; the link latches every word.
    TransferStart,
    /// The transfer's time is up; the link delivers the words to everyone.
    TransferEnd,
}

#[derive(Serialize, Deserialize)]
pub struct Sio {
    /// Raw registers. Multi-play mode overlays its live bits on SIOCNT when read.
    #[serde(with = "crate::snapshot::array")]
    regs: [u16; 0x30],
    /// A transfer is in progress (SIOCNT bit 7).
    busy: bool,
    /// Position on the cable: 0 = parent, 1–3 = children (SIOCNT bits 4–5).
    id: u8,
    /// Consoles on the cable, this one included; 1 when nothing is plugged in.
    count: u8,
    /// Every console on the cable is in multi-play mode (SIOCNT bit 3, "SD").
    ready: bool,
    /// What the link should do next, if anything.
    stop: Option<Stop>,
}

impl Default for Sio {
    fn default() -> Self {
        Self { regs: [0; 0x30], busy: false, id: 0, count: 1, ready: false, stop: None }
    }
}

impl Sio {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mode(&self) -> Mode {
        let rcnt = self.regs[RCNT];
        let cnt = self.regs[SIOCNT];
        if rcnt & 0x8000 != 0 {
            if rcnt & 0x4000 != 0 {
                Mode::JoyBus
            } else {
                Mode::GeneralPurpose
            }
        } else if cnt & 0x2000 != 0 {
            if cnt & 0x1000 != 0 {
                Mode::Uart
            } else {
                Mode::MultiPlay
            }
        } else if cnt & 0x1000 != 0 {
            Mode::Normal32
        } else {
            Mode::Normal8
        }
    }

    pub fn read_io(&self, reg: u32) -> u16 {
        let i = ((reg - BASE) / 2) as usize;
        if i == SIOCNT && self.mode() == Mode::MultiPlay {
            // SI, SD, the id and the busy bit are read-only and come from the cable.
            (self.regs[SIOCNT] & 0x7F03)
                | u16::from(self.id != 0) << 2
                | u16::from(self.ready) << 3
                | u16::from(self.id) << 4
                | u16::from(self.busy) << 7
        } else {
            self.regs[i]
        }
    }

    pub fn write_io(&mut self, reg: u32, value: u16, mask: u16, scheduler: &mut Scheduler) {
        let i = ((reg - BASE) / 2) as usize;
        let before = self.mode();
        self.regs[i] = (self.regs[i] & !mask) | (value & mask);
        let after = self.mode();
        if after != before && self.linked() {
            self.stop = Some(Stop::ModeChange);
        }
        // Only the parent can start a transfer, and only when none is in flight.
        let start = i == SIOCNT && value & mask & SIO_START != 0;
        if start && after == Mode::MultiPlay && self.id == 0 && !self.busy {
            self.start(scheduler);
        }
    }

    fn start(&mut self, scheduler: &mut Scheduler) {
        self.busy = true;
        // The received words read as FFFFh for the duration of the transfer.
        self.regs[..4].fill(0xFFFF);
        scheduler.schedule(Event::SioTransfer, transfer_cycles(self.baud(), self.count));
        if self.linked() {
            self.stop = Some(Stop::TransferStart);
        }
    }

    /// What a child sees when the parent starts a transfer (called by the link).
    pub fn begin_as_child(&mut self) {
        self.busy = true;
        self.regs[..4].fill(0xFFFF);
    }

    /// The scheduled end of a transfer this console started.
    pub fn on_transfer_end(&mut self, irq: &mut Irq) {
        if self.linked() {
            self.stop = Some(Stop::TransferEnd);
        } else {
            let word = self.regs[SIOMLT_SEND];
            self.complete([word, 0xFFFF, 0xFFFF, 0xFFFF], irq);
        }
    }

    /// Finishes a transfer with the word every console sent (FFFFh for empty slots).
    pub fn complete(&mut self, words: [u16; 4], irq: &mut Irq) {
        self.regs[..4].copy_from_slice(&words);
        self.busy = false;
        if self.regs[SIOCNT] & SIO_IRQ_ENABLE != 0 {
            irq.raise(Interrupt::Serial);
        }
    }

    /// The word this console sends in the next transfer (SIOMLT_SEND).
    pub fn send_word(&self) -> u16 {
        self.regs[SIOMLT_SEND]
    }

    pub fn busy(&self) -> bool {
        self.busy
    }

    /// Takes the pending request for the link, if any.
    pub fn take_stop(&mut self) -> Option<Stop> {
        self.stop.take()
    }

    /// Puts the console on a cable of `count` consoles, at position `id`.
    pub fn attach(&mut self, id: u8, count: u8) {
        self.id = id;
        self.count = count;
    }

    pub fn set_ready(&mut self, ready: bool) {
        self.ready = ready;
    }

    fn linked(&self) -> bool {
        self.count > 1
    }

    fn baud(&self) -> u32 {
        [9600, 38400, 57600, 115200][(self.regs[SIOCNT] & 3) as usize]
    }
}

/// How long a multi-play transfer takes: each console shifts out its 16 bits in
/// turn at the chosen baud rate, with a few bits of framing. About 0.4 ms for
/// two consoles at 115200 bps, 9 ms for four at 9600.
pub fn transfer_cycles(baud: u32, count: u8) -> u64 {
    let bits = 20 * u64::from(count) + 2;
    bits * u64::from(CLOCK_HZ) / u64::from(baud)
}
