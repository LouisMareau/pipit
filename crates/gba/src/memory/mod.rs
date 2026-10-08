//! The memory bus: address decoding, mirrors, wait states and open-bus behaviour
//! (GBATEK "GBA Memory Map" and "GBA System Control").
//!
//! Every access is split in two: *what* comes back (`load*`/`store*`, used by the CPU
//! and DMA alike) and *how long* it takes (`wait16`/`wait32`, charged to the
//! scheduler). Keeping them apart lets DMA and the CPU share the decoding while
//! paying their own timings.

pub mod io;

use crate::audio::Audio;
use crate::bios::Bios;
use crate::cartridge::Cartridge;
use crate::dma::Dma;
use crate::irq::Irq;
use crate::keypad::Keypad;
use crate::scheduler::{Event, Scheduler};
use crate::timers::Timers;
use crate::video::Video;
use serde::{Deserialize, Serialize};

pub const EWRAM_SIZE: usize = 0x40000;
pub const IWRAM_SIZE: usize = 0x8000;

/// Memory regions by the top byte of the address.
pub mod region {
    pub const BIOS: u32 = 0x0;
    pub const EWRAM: u32 = 0x2;
    pub const IWRAM: u32 = 0x3;
    pub const IO: u32 = 0x4;
    pub const PALETTE: u32 = 0x5;
    pub const VRAM: u32 = 0x6;
    pub const OAM: u32 = 0x7;
    pub const ROM0: u32 = 0x8;
    pub const ROM0_HI: u32 = 0x9;
    pub const ROM1: u32 = 0xA;
    pub const ROM1_HI: u32 = 0xB;
    pub const ROM2: u32 = 0xC;
    pub const ROM2_HI: u32 = 0xD;
    pub const SRAM: u32 = 0xE;
    pub const SRAM_MIRROR: u32 = 0xF;
}

#[derive(Serialize, Deserialize)]
pub struct Bus {
    pub scheduler: Scheduler,
    pub video: Video,
    pub audio: Audio,
    pub dma: Dma,
    pub timers: Timers,
    pub irq: Irq,
    pub keypad: Keypad,
    pub cart: Cartridge,
    pub bios: Bios,

    #[serde(with = "crate::snapshot::bytes_box")]
    ewram: Box<[u8; EWRAM_SIZE]>,
    #[serde(with = "crate::snapshot::bytes_box")]
    iwram: Box<[u8; IWRAM_SIZE]>,

    /// WAITCNT (0x4000204).
    waitcnt: u16,
    /// Internal memory control (0x4000800): EWRAM wait states live in bits 24-27.
    memctrl: u32,
    /// POSTFLG (0x4000300).
    postflg: u8,
    /// Serial I/O registers are stored but not emulated (no link cable yet).
    #[serde(with = "crate::snapshot::array")]
    sio: [u16; 0x30],

    /// Cycles for a 16-bit access, indexed by `[region][sequential]`.
    wait16: [[u8; 2]; 16],
    /// Cycles for a 32-bit access, indexed by `[region][sequential]`.
    wait32: [[u8; 2]; 16],

    /// Last value seen on the bus; returned for reads of unmapped memory.
    pub open_bus: u32,
    /// Last opcode fetched from the BIOS; BIOS reads from outside the BIOS return it.
    pub(crate) bios_latch: u32,
    /// Address of the instruction currently executing (for BIOS read protection).
    pub pc: u32,
}

impl Bus {
    pub fn new(cart: Cartridge, bios_image: Option<Vec<u8>>) -> Self {
        let mut bus = Self {
            scheduler: Scheduler::new(),
            video: Video::new(),
            audio: Audio::new(),
            dma: Dma::new(),
            timers: Timers::new(),
            irq: Irq::new(),
            keypad: Keypad::new(),
            cart,
            bios: Bios::new(bios_image),
            ewram: Box::new([0; EWRAM_SIZE]),
            iwram: Box::new([0; IWRAM_SIZE]),
            waitcnt: 0,
            memctrl: 0x0D00_0020,
            postflg: 0,
            sio: [0; 0x30],
            wait16: [[1; 2]; 16],
            wait32: [[1; 2]; 16],
            open_bus: 0,
            bios_latch: 0xE129_F000,
            pc: 0,
        };
        bus.update_wait_states();
        bus.video.schedule_first(&mut bus.scheduler);
        bus.audio.schedule_first(&mut bus.scheduler);
        bus
    }

    // ---------------------------------------------------------------------------
    // Timing
    // ---------------------------------------------------------------------------

    /// Recomputes the wait-state tables from WAITCNT and the EWRAM control register.
    fn update_wait_states(&mut self) {
        const N_WAITS: [u8; 4] = [4, 3, 2, 8];
        let sram = 1 + N_WAITS[(self.waitcnt & 3) as usize];
        let ws0_n = 1 + N_WAITS[((self.waitcnt >> 2) & 3) as usize];
        let ws0_s = 1 + if self.waitcnt & (1 << 4) != 0 { 1 } else { 2 };
        let ws1_n = 1 + N_WAITS[((self.waitcnt >> 5) & 3) as usize];
        let ws1_s = 1 + if self.waitcnt & (1 << 7) != 0 { 1 } else { 4 };
        let ws2_n = 1 + N_WAITS[((self.waitcnt >> 8) & 3) as usize];
        let ws2_s = 1 + if self.waitcnt & (1 << 10) != 0 { 1 } else { 8 };
        // 0xD = 2 wait states (default), 0xE = 1 wait state. Other values hang real hardware.
        let ewram = 1 + (15 - ((self.memctrl >> 24) & 0xF) as u8).clamp(1, 14);

        for r in 0..16u32 {
            let (n, s) = match r {
                region::EWRAM => (ewram, ewram),
                region::PALETTE | region::VRAM => (1, 1),
                region::ROM0 | region::ROM0_HI => (ws0_n, ws0_s),
                region::ROM1 | region::ROM1_HI => (ws1_n, ws1_s),
                region::ROM2 | region::ROM2_HI => (ws2_n, ws2_s),
                region::SRAM | region::SRAM_MIRROR => (sram, sram),
                _ => (1, 1),
            };
            self.wait16[r as usize] = [n, s];
            // 32-bit accesses on 16-bit buses are two halfword transfers.
            let wide = matches!(
                r,
                region::EWRAM | region::PALETTE | region::VRAM | region::ROM0..=region::ROM2_HI
            );
            self.wait32[r as usize] = if wide { [n + s, s + s] } else { [n, s] };
        }
        // SRAM only has an 8-bit bus; a 32-bit access still costs one byte access.
        self.wait32[region::SRAM as usize] = [sram, sram];
        self.wait32[region::SRAM_MIRROR as usize] = [sram, sram];
    }

    /// Charges the cost of an 8/16-bit access at `addr`.
    #[inline(always)]
    pub fn wait16(&mut self, addr: u32, seq: bool) {
        let c = self.wait16[(addr >> 24) as usize & 0xF][seq as usize];
        self.scheduler.advance(u32::from(c));
    }

    /// Charges the cost of a 32-bit access at `addr`.
    #[inline(always)]
    pub fn wait32(&mut self, addr: u32, seq: bool) {
        let c = self.wait32[(addr >> 24) as usize & 0xF][seq as usize];
        self.scheduler.advance(u32::from(c));
    }

    /// Charges internal (I) cycles that touch no memory.
    #[inline(always)]
    pub fn idle(&mut self, cycles: u32) {
        self.scheduler.advance(cycles);
    }

    pub fn waitcnt(&self) -> u16 {
        self.waitcnt
    }

    // ---------------------------------------------------------------------------
    // Timed accesses (what the CPU uses)
    // ---------------------------------------------------------------------------

    #[inline]
    pub fn read8(&mut self, addr: u32, seq: bool) -> u8 {
        self.wait16(addr, seq);
        self.load8(addr)
    }

    #[inline]
    pub fn read16(&mut self, addr: u32, seq: bool) -> u16 {
        self.wait16(addr, seq);
        self.load16(addr)
    }

    #[inline]
    pub fn read32(&mut self, addr: u32, seq: bool) -> u32 {
        self.wait32(addr, seq);
        self.load32(addr)
    }

    #[inline]
    pub fn write8(&mut self, addr: u32, value: u8, seq: bool) {
        self.wait16(addr, seq);
        self.store8(addr, value);
    }

    #[inline]
    pub fn write16(&mut self, addr: u32, value: u16, seq: bool) {
        self.wait16(addr, seq);
        self.store16(addr, value);
    }

    #[inline]
    pub fn write32(&mut self, addr: u32, value: u32, seq: bool) {
        self.wait32(addr, seq);
        self.store32(addr, value);
    }

    /// Fetches an ARM opcode.
    #[inline]
    pub fn fetch32(&mut self, addr: u32, seq: bool) -> u32 {
        self.wait32(addr, seq);
        // Fetching from an address means executing there, which is what the BIOS
        // read protection keys on (an exception vector fetch must not be blocked).
        self.pc = addr;
        let op = self.load32(addr);
        self.open_bus = op;
        if addr >> 24 == region::BIOS {
            self.bios_latch = op;
        }
        op
    }

    /// Fetches a Thumb opcode.
    #[inline]
    pub fn fetch16(&mut self, addr: u32, seq: bool) -> u16 {
        self.wait16(addr, seq);
        self.pc = addr;
        let op = self.load16(addr);
        // Open-bus value after a Thumb fetch: the opcode in both halves is a good
        // approximation of the hardware's region-dependent behaviour.
        self.open_bus = u32::from(op) | (u32::from(op) << 16);
        if addr >> 24 == region::BIOS {
            self.bios_latch = self.open_bus;
        }
        op
    }

    // ---------------------------------------------------------------------------
    // Untimed accesses (address decoding only)
    // ---------------------------------------------------------------------------

    pub fn load8(&mut self, addr: u32) -> u8 {
        match addr >> 24 {
            region::BIOS => (self.bios_read(addr & !3) >> ((addr & 3) * 8)) as u8,
            region::EWRAM => self.ewram[(addr & 0x3FFFF) as usize],
            region::IWRAM => self.iwram[(addr & 0x7FFF) as usize],
            region::IO => (self.read_io(addr & !1) >> ((addr & 1) * 8)) as u8,
            region::PALETTE => self.video.palette[(addr & 0x3FF) as usize],
            region::VRAM => self.video.vram[vram_offset(addr)],
            region::OAM => self.video.oam[(addr & 0x3FF) as usize],
            region::ROM0..=region::ROM2_HI => self.cart.read_rom8(addr),
            region::SRAM | region::SRAM_MIRROR => self.cart.read_backup(addr),
            _ => (self.open_bus >> ((addr & 3) * 8)) as u8,
        }
    }

    /// Halfword read; the address is aligned here, except on the 8-bit SRAM bus.
    pub fn load16(&mut self, addr: u32) -> u16 {
        let raw = addr;
        let addr = addr & !1;
        match addr >> 24 {
            region::BIOS => (self.bios_read(addr & !3) >> ((addr & 2) * 8)) as u16,
            region::EWRAM => rd16(&self.ewram[..], (addr & 0x3FFFF) as usize),
            region::IWRAM => rd16(&self.iwram[..], (addr & 0x7FFF) as usize),
            region::IO => self.read_io(addr),
            region::PALETTE => rd16(&self.video.palette[..], (addr & 0x3FF) as usize),
            region::VRAM => rd16(&self.video.vram[..], vram_offset(addr)),
            region::OAM => rd16(&self.video.oam[..], (addr & 0x3FF) as usize),
            region::ROM0..=region::ROM2_HI => {
                self.cart.set_now(self.scheduler.now());
                self.cart.read_rom16(addr)
            }
            region::SRAM | region::SRAM_MIRROR => u16::from(self.cart.read_backup(raw)) * 0x0101,
            _ => (self.open_bus >> ((addr & 2) * 8)) as u16,
        }
    }

    /// Word read; the address is aligned here, except on the 8-bit SRAM bus.
    pub fn load32(&mut self, addr: u32) -> u32 {
        let raw = addr;
        let addr = addr & !3;
        match addr >> 24 {
            region::BIOS => self.bios_read(addr),
            region::EWRAM => rd32(&self.ewram[..], (addr & 0x3FFFF) as usize),
            region::IWRAM => rd32(&self.iwram[..], (addr & 0x7FFF) as usize),
            region::IO => u32::from(self.read_io(addr)) | (u32::from(self.read_io(addr + 2)) << 16),
            region::PALETTE => rd32(&self.video.palette[..], (addr & 0x3FF) as usize),
            region::VRAM => rd32(&self.video.vram[..], vram_offset(addr)),
            region::OAM => rd32(&self.video.oam[..], (addr & 0x3FF) as usize),
            region::ROM0..=region::ROM2_HI => self.cart.read_rom32(addr),
            region::SRAM | region::SRAM_MIRROR => {
                u32::from(self.cart.read_backup(raw)) * 0x0101_0101
            }
            _ => self.open_bus,
        }
    }

    pub fn store8(&mut self, addr: u32, value: u8) {
        match addr >> 24 {
            region::EWRAM => self.ewram[(addr & 0x3FFFF) as usize] = value,
            region::IWRAM => self.iwram[(addr & 0x7FFF) as usize] = value,
            region::IO => {
                let mask = if addr & 1 == 0 { 0x00FF } else { 0xFF00 };
                self.write_io(addr & !1, u16::from(value) * 0x0101, mask);
            }
            // Byte writes to palette and BG VRAM store the byte twice; byte writes to
            // OBJ VRAM and OAM are ignored (GBATEK "Writing 8bit Data to Video Memory").
            region::PALETTE => wr16(
                &mut self.video.palette[..],
                (addr & 0x3FE) as usize,
                u16::from(value) * 0x0101,
            ),
            region::VRAM => {
                let off = vram_offset(addr) & !1;
                if off < self.video.obj_vram_start() {
                    wr16(&mut self.video.vram[..], off, u16::from(value) * 0x0101);
                }
            }
            region::OAM => {}
            region::ROM0..=region::ROM2_HI => self.cart.write_rom8(addr, value),
            region::SRAM | region::SRAM_MIRROR => self.cart.write_backup(addr, value),
            _ => {}
        }
    }

    /// Halfword write; aligned here, except on the 8-bit SRAM bus where the byte
    /// on the lane selected by the raw address is stored.
    pub fn store16(&mut self, addr: u32, value: u16) {
        let raw = addr;
        let addr = addr & !1;
        match addr >> 24 {
            region::EWRAM => wr16(&mut self.ewram[..], (addr & 0x3FFFF) as usize, value),
            region::IWRAM => wr16(&mut self.iwram[..], (addr & 0x7FFF) as usize, value),
            region::IO => self.write_io(addr, value, 0xFFFF),
            region::PALETTE => wr16(&mut self.video.palette[..], (addr & 0x3FF) as usize, value),
            region::VRAM => wr16(&mut self.video.vram[..], vram_offset(addr), value),
            region::OAM => wr16(&mut self.video.oam[..], (addr & 0x3FF) as usize, value),
            region::ROM0..=region::ROM2_HI => {
                self.cart.set_now(self.scheduler.now());
                self.cart.write_rom16(addr, value)
            }
            // SRAM has an 8-bit bus: the byte on the lane selected by the address is stored.
            region::SRAM | region::SRAM_MIRROR => {
                self.cart.write_backup(raw, (value >> ((raw & 1) * 8)) as u8)
            }
            _ => {}
        }
    }

    /// Word write; aligned here, except on the 8-bit SRAM bus.
    pub fn store32(&mut self, addr: u32, value: u32) {
        let raw = addr;
        let addr = addr & !3;
        match addr >> 24 {
            region::EWRAM => wr32(&mut self.ewram[..], (addr & 0x3FFFF) as usize, value),
            region::IWRAM => wr32(&mut self.iwram[..], (addr & 0x7FFF) as usize, value),
            region::IO => {
                self.write_io(addr, value as u16, 0xFFFF);
                self.write_io(addr + 2, (value >> 16) as u16, 0xFFFF);
            }
            region::PALETTE => wr32(&mut self.video.palette[..], (addr & 0x3FF) as usize, value),
            region::VRAM => wr32(&mut self.video.vram[..], vram_offset(addr), value),
            region::OAM => wr32(&mut self.video.oam[..], (addr & 0x3FF) as usize, value),
            region::ROM0..=region::ROM2_HI => self.cart.write_rom32(addr, value),
            region::SRAM | region::SRAM_MIRROR => {
                self.cart.write_backup(raw, (value >> ((raw & 3) * 8)) as u8)
            }
            _ => {}
        }
    }

    /// BIOS reads are only allowed while executing from the BIOS; otherwise the
    /// last opcode fetched from it is returned (GBATEK "BIOS Memory Protection").
    fn bios_read(&self, addr: u32) -> u32 {
        if addr >= 0x4000 {
            return self.open_bus;
        }
        if self.pc < 0x4000 {
            self.bios.read32(addr)
        } else {
            self.bios_latch
        }
    }

    // ---------------------------------------------------------------------------
    // Events
    // ---------------------------------------------------------------------------

    /// Dispatches every due event, then any DMA they triggered.
    #[inline(never)]
    pub fn run_events(&mut self) {
        // `at` is the nominal time of the event; periodic events reschedule relative
        // to it so that running a few cycles late never accumulates drift.
        while let Some((event, at)) = self.scheduler.pop_due() {
            match event {
                Event::HBlankStart => self.video.on_hblank_start(
                    at,
                    &mut self.scheduler,
                    &mut self.irq,
                    &mut self.dma,
                ),
                Event::HBlankEnd => {
                    self.video.on_hblank_end(at, &mut self.scheduler, &mut self.irq, &mut self.dma)
                }
                Event::TimerOverflow(n) => self.timers.on_overflow(
                    n as usize,
                    at,
                    &mut self.scheduler,
                    &mut self.irq,
                    &mut self.audio,
                    &mut self.dma,
                ),
                Event::AudioSequencer => self.audio.on_sequencer(at, &mut self.scheduler),
                Event::AudioSample => self.audio.on_sample(at, &mut self.scheduler),
            }
        }
        if self.dma.has_pending() {
            self.run_dma();
        }
    }
}

/// VRAM is 96K mirrored in a 128K window; the upper 32K repeats (GBATEK "VRAM").
#[inline(always)]
fn vram_offset(addr: u32) -> usize {
    let off = addr & 0x1FFFF;
    (if off >= 0x18000 { off - 0x8000 } else { off }) as usize
}

#[inline(always)]
pub(crate) fn rd16(mem: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([mem[off], mem[off + 1]])
}

#[inline(always)]
pub(crate) fn rd32(mem: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([mem[off], mem[off + 1], mem[off + 2], mem[off + 3]])
}

#[inline(always)]
pub(crate) fn wr16(mem: &mut [u8], off: usize, v: u16) {
    mem[off..off + 2].copy_from_slice(&v.to_le_bytes());
}

#[inline(always)]
pub(crate) fn wr32(mem: &mut [u8], off: usize, v: u32) {
    mem[off..off + 4].copy_from_slice(&v.to_le_bytes());
}
