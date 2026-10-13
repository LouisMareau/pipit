//! What both CPUs share, and the view each one runs on.
//!
//! The ARM9 and ARM7 see different memory maps over the same hardware: `View9`
//! and `View7` implement `pipit_arm::Bus` for each over the one `Bus` here.
//! Each CPU keeps its own clock (`time`) in ARM9 cycles; the scheduler's clock
//! is where the last slice ended, and events run between slices.

pub mod arm7;
pub mod arm9;
mod io;

pub use arm7::View7;
pub use arm9::{Cp15, View9};

use crate::bios::{self, BIOS7_SIZE, BIOS9_SIZE};
use crate::cart::Cart;
use crate::dma::{Dma, Timing};
use crate::ipc::{self, Ipc};
use crate::irq::{Interrupt, Irq};
use crate::math::Math;
use crate::rtc::{self, Rtc};
use crate::scheduler::{Event, Scheduler};
use crate::sound::{self, Sound};
use crate::spi::Spi;
use crate::timers::Timers;
use crate::video::{self, Video, SCREEN_HEIGHT};
use serde::{Deserialize, Serialize};

pub const ARM9: usize = 0;
pub const ARM7: usize = 1;

/// How far one CPU runs before the other gets its turn, in ARM9 cycles.
pub const SLICE: u64 = 64;

/// Which keys are held, one bit each (the KEYINPUT order, then X and Y).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keys(pub u16);

impl Keys {
    pub const A: u16 = 1 << 0;
    pub const B: u16 = 1 << 1;
    pub const SELECT: u16 = 1 << 2;
    pub const START: u16 = 1 << 3;
    pub const RIGHT: u16 = 1 << 4;
    pub const LEFT: u16 = 1 << 5;
    pub const UP: u16 = 1 << 6;
    pub const DOWN: u16 = 1 << 7;
    pub const R: u16 = 1 << 8;
    pub const L: u16 = 1 << 9;
    pub const X: u16 = 1 << 10;
    pub const Y: u16 = 1 << 11;
}

#[derive(Serialize, Deserialize)]
pub struct Bus {
    pub scheduler: Scheduler,
    /// Each CPU's own clock, in ARM9 cycles.
    pub time: [u64; 2],
    /// Where the current slice ends; a halted CPU skips straight to it.
    pub target: u64,
    /// The address of each CPU's executing instruction (for debugging).
    pub pc: [u32; 2],
    #[serde(with = "pipit_common::snapshot::bytes_box")]
    pub main_ram: Box<[u8; 0x40_0000]>,
    #[serde(with = "pipit_common::snapshot::bytes_box")]
    pub shared_wram: Box<[u8; 0x8000]>,
    /// WRAMCNT: how the shared WRAM is split (0 all ARM9 … 3 all ARM7).
    pub wramcnt: u8,
    #[serde(with = "pipit_common::snapshot::bytes_box")]
    pub arm7_wram: Box<[u8; 0x1_0000]>,
    #[serde(with = "pipit_common::snapshot::bytes_box")]
    pub itcm: Box<[u8; 0x8000]>,
    #[serde(with = "pipit_common::snapshot::bytes_box")]
    pub dtcm: Box<[u8; 0x4000]>,
    pub cp15: Cp15,
    #[serde(skip, default = "bios::stub9")]
    pub bios9: Box<[u8; BIOS9_SIZE]>,
    #[serde(skip, default = "bios::stub7")]
    pub bios7: Box<[u8; BIOS7_SIZE]>,
    /// Whether each CPU's SWIs are handled in Rust (no user BIOS for it).
    pub hle: [bool; 2],
    /// An IntrWait is in progress on this CPU (see `bios::swi`).
    pub intr_wait_continuing: [bool; 2],
    pub video: Video,
    pub cart: Cart,
    pub ipc: Ipc,
    pub irq: [Irq; 2],
    pub timers: [Timers; 2],
    pub dma: [Dma; 2],
    pub math: Math,
    /// KEYINPUT: bits 0-9, active low.
    pub keys: u16,
    /// EXTKEYIN (ARM7): X, Y, the pen and the lid, active low.
    pub ext_keys: u8,
    pub keycnt: [u16; 2],
    pub exmemcnt: u16,
    /// The ARM7's own bits of EXMEMSTAT (0-6).
    pub exmemstat7: u16,
    pub postflg: [u8; 2],
    pub spi: Spi,
    pub rtc: Rtc,
    pub powcnt2: u16,
    pub wifiwaitcnt: u16,
    pub sound: Sound,
}

impl Bus {
    pub fn new(
        rom: Vec<u8>,
        bios7: Option<Vec<u8>>,
        bios9: Option<Vec<u8>>,
        firmware: Vec<u8>,
    ) -> Self {
        let mut bus = Self {
            scheduler: Scheduler::new(),
            time: [0; 2],
            target: 0,
            pc: [0; 2],
            main_ram: crate::boxed(0),
            shared_wram: crate::boxed(0),
            wramcnt: 0,
            arm7_wram: crate::boxed(0),
            itcm: crate::boxed(0),
            dtcm: crate::boxed(0),
            cp15: Cp15::default(),
            bios9: bios::stub9(),
            bios7: bios::stub7(),
            hle: [bios9.is_none(), bios7.is_none()],
            intr_wait_continuing: [false; 2],
            video: Video::new(),
            cart: Cart::new(rom),
            ipc: Ipc::new(),
            irq: [Irq::new(), Irq::new()],
            timers: [Timers::new(0), Timers::new(4)],
            dma: [Dma::new(true), Dma::new(false)],
            math: Math::default(),
            keys: 0x03FF,
            ext_keys: 0x7F,
            keycnt: [0; 2],
            exmemcnt: 0,
            exmemstat7: 0,
            postflg: [0; 2],
            spi: Spi::new(firmware),
            rtc: Rtc::new(),
            powcnt2: 0,
            wifiwaitcnt: 0,
            sound: Sound::new(),
        };
        if let Some(image) = bios9 {
            let n = image.len().min(BIOS9_SIZE);
            bus.bios9[..n].copy_from_slice(&image[..n]);
        }
        if let Some(image) = bios7 {
            let n = image.len().min(BIOS7_SIZE);
            bus.bios7[..n].copy_from_slice(&image[..n]);
        }
        bus
    }

    #[inline(always)]
    pub fn raise(&mut self, cpu: usize, source: Interrupt) {
        self.irq[cpu].raise(source);
    }

    /// Which CPU the cartridge slot belongs to (EXMEMCNT bit 11).
    pub fn card_owner(&self) -> usize {
        if self.exmemcnt & (1 << 11) != 0 {
            ARM7
        } else {
            ARM9
        }
    }

    pub fn set_keys(&mut self, keys: Keys) {
        self.keys = !keys.0 & 0x03FF;
        let x = keys.0 & Keys::X != 0;
        let y = keys.0 & Keys::Y != 0;
        self.ext_keys = (self.ext_keys & 0xFC) | u8::from(!x) | u8::from(!y) << 1;
    }

    /// Restoring a state keeps the ROM and BIOS images of the running machine.
    pub(crate) fn take_images_from(&mut self, other: &mut Bus) {
        self.cart.take_rom_from(&mut other.cart);
        std::mem::swap(&mut self.bios9, &mut other.bios9);
        std::mem::swap(&mut self.bios7, &mut other.bios7);
        self.spi.take_firmware_from(&mut other.spi);
    }

    /// The pen on the bottom screen, as pixel coordinates, or lifted.
    pub fn set_touch(&mut self, touch: Option<(u8, u8)>) {
        self.spi.touch = touch.map(|(x, y)| (u16::from(x) * 16, u16::from(y) * 16));
        self.ext_keys = (self.ext_keys & !(1 << 6)) | u8::from(touch.is_none()) << 6;
    }

    /// Runs every event due at the scheduler's clock.
    pub fn run_events(&mut self) {
        while let Some((event, at)) = self.scheduler.pop_due() {
            match event {
                Event::HBlankStart => {
                    let signals = self.video.on_hblank_start(at, &mut self.scheduler);
                    self.video_signals(signals);
                }
                Event::HBlankEnd => {
                    let signals = self.video.on_hblank_end(at, &mut self.scheduler);
                    self.video_signals(signals);
                }
                Event::Timer(i) => {
                    let cpu = usize::from(i / 4);
                    let (timers, irq) = (&mut self.timers[cpu], &mut self.irq[cpu]);
                    timers.on_overflow(usize::from(i % 4), at, &mut self.scheduler, irq);
                }
                Event::CardWord => {
                    self.cart.on_word();
                    let owner = self.card_owner();
                    self.dma[owner].trigger(Timing::Card);
                }
                Event::AudioSample => self.audio_sample(at),
                Event::RtcSecond => {
                    if self.rtc.on_second(at) {
                        self.raise(ARM7, Interrupt::Rtc);
                    }
                    self.scheduler.schedule_at(Event::RtcSecond, at + rtc::CYCLES_PER_SECOND);
                }
                Event::MathDone => {}
            }
        }
    }

    fn video_signals(&mut self, signals: video::Signals) {
        for cpu in 0..2 {
            let bits = signals.irq[cpu];
            if bits & 1 != 0 {
                self.raise(cpu, Interrupt::VBlank);
            }
            if bits & 2 != 0 {
                self.raise(cpu, Interrupt::HBlank);
            }
            if bits & 4 != 0 {
                self.raise(cpu, Interrupt::VCount);
            }
        }
        let visible = self.video.vcount < SCREEN_HEIGHT as u16;
        if signals.vblank_started {
            self.dma[ARM9].trigger(Timing::VBlank);
            self.dma[ARM7].trigger(Timing::VBlank);
        }
        if signals.hblank_started && visible {
            self.dma[ARM9].trigger(Timing::HBlank);
        }
        if signals.line_started && visible {
            self.dma[ARM9].trigger(Timing::DisplayStart);
        }
        if signals.line_started && visible && self.video.fifo_in_use() {
            // The main memory display FIFO takes the line as 32 requests of 4 words.
            for _ in 0..32 {
                self.dma[ARM9].trigger(Timing::DisplayFifo);
                if !self.dma[ARM9].has_pending() {
                    break;
                }
                run_dma(&mut View9(self));
            }
        }
    }

    /// One mix sample of the sound unit, reading and writing through the
    /// ARM7's view of memory.
    fn audio_sample(&mut self, at: u64) {
        self.scheduler.schedule_at(Event::AudioSample, at + sound::CYCLES_PER_SAMPLE);
        let (sound, mut mem) = self.sound_parts();
        sound.sample(&mut mem);
    }

    /// The sound unit and the memory it works on, borrowed side by side.
    pub(crate) fn sound_parts(&mut self) -> (&mut Sound, SoundMemory<'_>) {
        let mem = SoundMemory {
            main_ram: &mut self.main_ram[..],
            shared_wram: &mut self.shared_wram[..],
            arm7_wram: &mut self.arm7_wram[..],
            wramcnt: self.wramcnt,
        };
        (&mut self.sound, mem)
    }

    /// Interrupts an IPC register access asked for.
    pub(crate) fn apply_ipc(&mut self, cpu: usize, s: ipc::Signals) {
        if s.my_send_empty {
            self.raise(cpu, Interrupt::IpcSendEmpty);
        }
        if s.my_recv {
            self.raise(cpu, Interrupt::IpcRecv);
        }
        if s.other_send_empty {
            self.raise(cpu ^ 1, Interrupt::IpcSendEmpty);
        }
        if s.other_recv {
            self.raise(cpu ^ 1, Interrupt::IpcRecv);
        }
        if s.other_sync {
            self.raise(cpu ^ 1, Interrupt::IpcSync);
        }
    }

    /// A card transfer may just have ended: raise its interrupt if enabled.
    pub(crate) fn card_check(&mut self) {
        if self.cart.finished {
            self.cart.finished = false;
            if self.cart.auxspicnt & (1 << 14) != 0 {
                let owner = self.card_owner();
                self.raise(owner, Interrupt::CardDone);
            }
        }
    }
}

/// One CPU's view of the bus: `pipit_arm::Bus` plus which CPU it is.
pub trait View: pipit_arm::Bus {
    const CPU: usize;
    fn bus(&mut self) -> &mut Bus;
}

/// Runs the DMA channels of `V`'s CPU that are ready, in priority order.
pub fn run_dma<V: View>(view: &mut V) {
    let cpu = V::CPU;
    let pending = view.bus().dma[cpu].take_pending();
    for (n, &ready) in pending.iter().enumerate() {
        if !ready {
            continue;
        }
        let dma = &view.bus().dma[cpu];
        let ch = dma.channels[n];
        if !ch.enabled() || ch.remaining == 0 {
            continue;
        }
        let timing = dma.timing(n, &ch);
        let count = dma.count_for(&ch);
        let word = ch.word_size();
        let step: u32 = if word { 4 } else { 2 };
        let src_step = match ch.src_control() {
            0 | 3 => step,
            1 => step.wrapping_neg(),
            _ => 0,
        };
        let dst_step = match ch.dst_control() {
            0 | 3 => step,
            1 => step.wrapping_neg(),
            _ => 0,
        };
        let units = match timing {
            Timing::Card => 1,
            Timing::DisplayFifo => ch.remaining.min(4),
            _ => ch.remaining,
        };
        let (mut src, mut dst) = (ch.src, ch.dst);
        for i in 0..units {
            let seq = i != 0;
            if word {
                let v = view.read32(src, seq);
                view.write32(dst, v, seq);
            } else {
                let v = view.read16(src, seq);
                view.write16(dst, v, seq);
            }
            src = src.wrapping_add(src_step);
            dst = dst.wrapping_add(dst_step);
        }
        let bus = view.bus();
        let ch = &mut bus.dma[cpu].channels[n];
        ch.src = src;
        ch.dst = dst;
        ch.remaining -= units;
        if ch.remaining == 0 {
            if ch.repeat() && timing != Timing::Immediate {
                ch.remaining = count;
                if ch.dst_control() == 3 {
                    ch.dst = ch.dad & 0x0FFF_FFFF;
                }
            } else {
                ch.control &= !(1 << 31);
            }
            if ch.irq() {
                bus.irq[cpu].raise(Interrupt::dma(n));
            }
        }
    }
}

/// Main memory and the work RAMs as the sound unit reads samples from them
/// and the captures write to them: no timing, no I/O.
pub struct SoundMemory<'a> {
    main_ram: &'a mut [u8],
    shared_wram: &'a mut [u8],
    arm7_wram: &'a mut [u8],
    wramcnt: u8,
}

impl SoundMemory<'_> {
    #[inline(always)]
    fn slot(&self, addr: u32) -> Option<(usize, usize)> {
        match addr >> 24 {
            0x02 => Some((0, (addr & 0x3F_FFFF) as usize)),
            0x03 => match arm7::wram7(self.wramcnt, addr) {
                (true, i) => Some((1, i)),
                (false, i) => Some((2, i)),
            },
            _ => None,
        }
    }

    #[inline(always)]
    fn bank(&self, which: usize) -> &[u8] {
        match which {
            0 => self.main_ram,
            1 => self.shared_wram,
            _ => self.arm7_wram,
        }
    }

    #[inline(always)]
    fn bank_mut(&mut self, which: usize) -> &mut [u8] {
        match which {
            0 => self.main_ram,
            1 => self.shared_wram,
            _ => self.arm7_wram,
        }
    }
}

impl sound::Memory for SoundMemory<'_> {
    #[inline(always)]
    fn read8(&self, addr: u32) -> u8 {
        match self.slot(addr) {
            Some((bank, i)) => self.bank(bank)[i],
            None => 0,
        }
    }
    #[inline(always)]
    fn read16(&self, addr: u32) -> u16 {
        let addr = addr & !1;
        u16::from_le_bytes([self.read8(addr), self.read8(addr + 1)])
    }
    #[inline(always)]
    fn read32(&self, addr: u32) -> u32 {
        let addr = addr & !3;
        u32::from(self.read16(addr)) | u32::from(self.read16(addr + 2)) << 16
    }
    #[inline(always)]
    fn write8(&mut self, addr: u32, value: u8) {
        if let Some((bank, i)) = self.slot(addr) {
            self.bank_mut(bank)[i] = value;
        }
    }
    #[inline(always)]
    fn write16(&mut self, addr: u32, value: u16) {
        let addr = addr & !1;
        self.write8(addr, value as u8);
        self.write8(addr + 1, (value >> 8) as u8);
    }
}
