//! Serial communication (GBATEK "GBA Communication Ports").
//!
//! Two modes do something. 16-bit multi-play is the link cable's mode, the one
//! most multiplayer games use (Pokémon's Cable Club among them). 32-bit normal
//! mode is what the Wireless Adapter talks over: when an [`Adapter`] is attached,
//! every transfer goes through it. UART, JOY bus and general purpose keep their
//! registers but never transfer anything.
//!
//! A console on its own behaves as if nothing were plugged in: it can run
//! transfers and only ever hears itself (or FFFFFFFFh in normal mode). Several
//! consoles are joined by [`crate::link::Link`], which runs them in lockstep and
//! carries the cable's words from each to all, or the adapters' commands between
//! them. The port talks to its link through [`Stop`]s: the console's run loop
//! returns one, the link acts on it, and running resumes.

use crate::irq::{Interrupt, Irq};
use crate::rfu::Adapter;
use crate::scheduler::{Event, Scheduler};
use crate::CLOCK_HZ;
use serde::{Deserialize, Serialize};

/// First register of the port; `regs[i]` is the halfword at `0x120 + 2·i`.
const BASE: u32 = 0x120;
const SIOCNT: usize = (0x128 - 0x120) / 2;
/// SIOMLT_SEND in multi-play mode, SIODATA8 in normal mode.
const SIOMLT_SEND: usize = (0x12A - 0x120) / 2;
const RCNT: usize = (0x134 - 0x120) / 2;
const SIO_INTERNAL_CLOCK: u16 = 1 << 0;
const SIO_FAST_CLOCK: u16 = 1 << 1;
const SIO_SI: u16 = 1 << 2;
const SIO_SO: u16 = 1 << 3;
const SIO_START: u16 = 1 << 7;
const SIO_IRQ_ENABLE: u16 = 1 << 14;

/// Cycles the adapter takes to clock one word into a listening console.
const ADAPTER_TRANSFER_CYCLES: u64 = 32 * 8 + 64;
/// Cycles between the radio rounds the link runs while adapters are attached
/// (about a quarter of a frame).
pub const TICK_CYCLES: u64 = 280_896 / 4;

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
    /// The wireless adapter has a request for the link (see [`Adapter::take_request`]).
    Rfu,
    /// Time for a radio round between the adapters.
    Tick,
}

#[derive(Serialize, Deserialize)]
pub struct Sio {
    /// Raw registers. Multi-play mode overlays its live bits on SIOCNT when read.
    #[serde(with = "pipit_common::snapshot::array")]
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
    /// A Wireless Adapter in the port.
    adapter: Option<Adapter>,
    /// RCNT drives SD high (the adapter's reset line).
    sd_high: bool,
    /// `busy` as a slave: waiting for the adapter to clock a word in.
    listening: bool,
}

impl Default for Sio {
    fn default() -> Self {
        Self {
            regs: [0; 0x30],
            busy: false,
            id: 0,
            count: 1,
            ready: false,
            stop: None,
            adapter: None,
            sd_high: false,
            listening: false,
        }
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

    /// The console drives the clock (normal mode, SIOCNT bit 0).
    fn internal_clock(&self) -> bool {
        self.regs[SIOCNT] & SIO_INTERNAL_CLOCK != 0
    }

    pub fn read_io(&self, reg: u32) -> u16 {
        let i = ((reg - BASE) / 2) as usize;
        if i != SIOCNT {
            return self.regs[i];
        }
        match self.mode() {
            Mode::MultiPlay => {
                // SI, SD, the id and the busy bit are read-only and come from the cable.
                (self.regs[SIOCNT] & 0x7F03)
                    | u16::from(self.id != 0) << 2
                    | u16::from(self.ready) << 3
                    | u16::from(self.id) << 4
                    | u16::from(self.busy) << 7
            }
            Mode::Normal8 | Mode::Normal32 => {
                let mut value = self.regs[SIOCNT] & !(SIO_START | SIO_SI);
                value |= u16::from(self.busy) << 7;
                if let Some(adapter) = &self.adapter {
                    let so = self.regs[SIOCNT] & SIO_SO != 0;
                    value |= u16::from(adapter.si(so, self.internal_clock())) << 2;
                }
                value
            }
            _ => self.regs[i],
        }
    }

    pub fn write_io(&mut self, reg: u32, value: u16, mask: u16, scheduler: &mut Scheduler) {
        let i = ((reg - BASE) / 2) as usize;
        let before = self.mode();
        self.regs[i] = (self.regs[i] & !mask) | (value & mask);
        let after = self.mode();
        if after != before {
            if self.linked() {
                self.stop = Some(Stop::ModeChange);
            }
            if matches!(before, Mode::Normal8 | Mode::Normal32) && self.busy {
                // A normal-mode transfer does not outlive the mode it started in.
                self.busy = false;
                self.listening = false;
                scheduler.cancel(Event::SioTransfer);
            }
        }
        if i == SIOCNT {
            let start = value & mask & SIO_START != 0;
            match after {
                // Only the parent can start a transfer, and only when none is in flight.
                Mode::MultiPlay => {
                    if start && self.id == 0 && !self.busy {
                        self.start(scheduler);
                    }
                }
                Mode::Normal8 | Mode::Normal32 => {
                    // The start bit stays set while a transfer is pending, so a
                    // console that armed the port as a slave and then takes the
                    // clock itself starts shifting at that moment.
                    let start = self.regs[SIOCNT] & SIO_START != 0;
                    if start && self.internal_clock() {
                        if !self.busy || self.listening {
                            self.busy = true;
                            self.listening = false;
                            let bits = if after == Mode::Normal32 { 32 } else { 8 };
                            let per_bit =
                                if self.regs[SIOCNT] & SIO_FAST_CLOCK != 0 { 8 } else { 64 };
                            scheduler.cancel(Event::SioTransfer);
                            scheduler.schedule(Event::SioTransfer, bits * per_bit);
                        }
                    } else if start {
                        if !self.busy {
                            self.busy = true;
                            self.listening = true;
                            if self.adapter.as_ref().is_some_and(Adapter::master_pending) {
                                // Listening as a slave, and the adapter has a word ready.
                                scheduler.schedule(Event::SioTransfer, ADAPTER_TRANSFER_CYCLES);
                            }
                        }
                    } else if mask & SIO_START != 0 && self.busy && self.listening {
                        // Stopped listening before the adapter clocked anything.
                        self.busy = false;
                        self.listening = false;
                        scheduler.cancel(Event::SioTransfer);
                    }
                }
                _ => {}
            }
        }
        if i == RCNT {
            // The adapter is reset by driving SD high in general-purpose mode.
            let sd_high = after == Mode::GeneralPurpose && self.regs[RCNT] & 0x22 == 0x22;
            if sd_high && !self.sd_high {
                if let Some(adapter) = &mut self.adapter {
                    adapter.reset();
                    self.busy = false;
                    self.listening = false;
                    scheduler.cancel(Event::SioTransfer);
                    self.stop = Some(Stop::Rfu);
                }
            }
            self.sd_high = sd_high;
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

    /// The scheduled end of a transfer this console (or its adapter) started.
    pub fn on_transfer_end(&mut self, irq: &mut Irq, now: u64) {
        match self.mode() {
            Mode::MultiPlay => {
                if self.linked() {
                    self.stop = Some(Stop::TransferEnd);
                } else {
                    let word = self.regs[SIOMLT_SEND];
                    self.complete([word, 0xFFFF, 0xFFFF, 0xFFFF], irq);
                }
            }
            mode @ (Mode::Normal8 | Mode::Normal32) => {
                let wide = mode == Mode::Normal32;
                let sent = if wide {
                    u32::from(self.regs[0]) | u32::from(self.regs[1]) << 16
                } else {
                    u32::from(self.regs[SIOMLT_SEND] & 0xFF)
                };
                let master = self.internal_clock();
                let received = match self.adapter.as_mut() {
                    // The adapter only speaks 32-bit words.
                    Some(_) if !wide => 0xFFFF_FFFF,
                    Some(adapter) if master => adapter.transfer(sent, now),
                    Some(adapter) => adapter.master_transfer(sent),
                    None => 0xFFFF_FFFF,
                };
                if trace_enabled() {
                    let who = if master { "console" } else { "adapter" };
                    eprintln!("sio {now}: {who} clocks {sent:08X} -> {received:08X}");
                }
                if wide {
                    self.regs[0] = received as u16;
                    self.regs[1] = (received >> 16) as u16;
                } else {
                    self.regs[SIOMLT_SEND] =
                        (self.regs[SIOMLT_SEND] & 0xFF00) | (received as u16 & 0xFF);
                }
                self.busy = false;
                self.listening = false;
                if self.regs[SIOCNT] & SIO_IRQ_ENABLE != 0 {
                    irq.raise(Interrupt::Serial);
                }
                if self.adapter.as_ref().is_some_and(Adapter::has_request) {
                    self.stop = Some(Stop::Rfu);
                }
            }
            _ => self.busy = false,
        }
    }

    /// Finishes a multi-play transfer with the word every console sent (FFFFh for
    /// empty slots).
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

    // --- Wireless Adapter ---

    /// Plugs a Wireless Adapter with device id `id` into the port.
    pub fn attach_adapter(&mut self, id: u16) {
        let mut adapter = Adapter::new();
        adapter.id = id;
        self.adapter = Some(adapter);
    }

    pub fn adapter(&self) -> Option<&Adapter> {
        self.adapter.as_ref()
    }

    pub fn adapter_mut(&mut self) -> Option<&mut Adapter> {
        self.adapter.as_mut()
    }

    /// Makes this console's clock pace the radio rounds (`Stop::Tick` every
    /// [`TICK_CYCLES`]); one console per link does this.
    pub fn start_ticks(&mut self, scheduler: &mut Scheduler) {
        scheduler.schedule(Event::SioTick, TICK_CYCLES);
    }

    pub fn on_tick(&mut self, scheduler: &mut Scheduler) {
        self.stop = Some(Stop::Tick);
        scheduler.schedule(Event::SioTick, TICK_CYCLES);
    }

    /// Has the adapter send a command (`28`, `29`, `27`) to its waiting console:
    /// clocked in right away if the console already listens, otherwise as soon as
    /// it does.
    pub fn adapter_send(&mut self, cmd: u8, payload: &[u32], scheduler: &mut Scheduler) {
        let Some(adapter) = &mut self.adapter else { return };
        adapter.send(cmd, payload);
        if self.busy && self.mode() == Mode::Normal32 && !self.internal_clock() {
            scheduler.cancel(Event::SioTransfer);
            scheduler.schedule(Event::SioTransfer, ADAPTER_TRANSFER_CYCLES);
        }
    }
}

/// Whether normal-mode transfers are printed to stderr (`PIPIT_SIO_TRACE` set),
/// a debugging aid for the adapter protocol. Never on the web.
#[cfg(not(target_arch = "wasm32"))]
fn trace_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("PIPIT_SIO_TRACE").is_some())
}

#[cfg(target_arch = "wasm32")]
fn trace_enabled() -> bool {
    false
}

/// How long a multi-play transfer takes: each console shifts out its 16 bits in
/// turn at the chosen baud rate, with a few bits of framing. About 0.4 ms for
/// two consoles at 115200 bps, 9 ms for four at 9600.
pub fn transfer_cycles(baud: u32, count: u8) -> u64 {
    let bits = 20 * u64::from(count) + 2;
    bits * u64::from(CLOCK_HZ) / u64::from(baud)
}
