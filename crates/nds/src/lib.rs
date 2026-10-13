//! Nintendo DS emulation core: the ARM9 and ARM7 and everything between them.
//!
//! Being built up stage by stage (see docs/roadmap.md). So far: both CPUs with
//! their memory maps, the interrupt controllers, timers, DMA, IPC, the card
//! interface, the divider, the two 2D engines' timing and text backgrounds,
//! and a BIOS replacement that boots a ROM directly from its header.

pub mod bios;
pub mod cart;
pub mod dma;
pub mod ipc;
pub mod irq;
pub mod math;
pub mod memory;
pub mod scheduler;
pub mod timers;
pub mod video;

pub use memory::Keys;
pub use pipit_common::snapshot::StateError;
pub use video::{SCREEN_HEIGHT, SCREEN_WIDTH};

use memory::{run_dma, Bus, View7, View9, ARM7, ARM9, SLICE};
use pipit_arm::{mode, Arch, Bus as _, Cpu};
use pipit_common::snapshot;
use serde::{Deserialize, Serialize};

const STATE_FORMAT: snapshot::Format = snapshot::Format { magic: *b"PIPD", version: 1 };

/// A whole Nintendo DS.
#[derive(Serialize, Deserialize)]
pub struct Nds {
    pub arm9: Cpu,
    pub arm7: Cpu,
    pub bus: Bus,
    /// Both screens stacked, top above bottom, as 0xFFRRGGBB (256 × 384).
    #[serde(skip, default = "blank_frame")]
    frame: Box<[u32; SCREEN_WIDTH * SCREEN_HEIGHT * 2]>,
}

fn blank_frame() -> Box<[u32; SCREEN_WIDTH * SCREEN_HEIGHT * 2]> {
    Box::new([0xFF00_0000; SCREEN_WIDTH * SCREEN_HEIGHT * 2])
}

/// CRC-16 as the firmware and the BIOS compute it (polynomial A001h, reflected).
pub fn crc16(init: u16, bytes: &[u8]) -> u16 {
    let mut crc = init;
    for &b in bytes {
        crc ^= u16::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xA001 } else { crc >> 1 };
        }
    }
    crc
}

/// The firmware's user settings as the BIOS copies them to main memory: the
/// defaults of a console without a firmware image (English, a touch screen
/// calibrated so that ADC = 16 × pixel).
fn user_settings() -> [u8; 0x74] {
    let mut s = [0u8; 0x74];
    s[0..2].copy_from_slice(&5u16.to_le_bytes());
    s[3] = 1;
    s[4] = 1;
    for (i, c) in "PIPIT".encode_utf16().enumerate() {
        s[6 + i * 2..8 + i * 2].copy_from_slice(&c.to_le_bytes());
    }
    s[0x1A..0x1C].copy_from_slice(&5u16.to_le_bytes());
    let calibration: [u8; 12] =
        [0x00, 0x01, 0x00, 0x01, 0x10, 0x10, 0x00, 0x0F, 0x00, 0x0B, 0xF0, 0xB0];
    s[0x58..0x64].copy_from_slice(&calibration);
    s[0x64..0x66].copy_from_slice(&0x0021u16.to_le_bytes());
    let crc = crc16(0xFFFF, &s[..0x70]);
    s[0x72..0x74].copy_from_slice(&crc.to_le_bytes());
    s
}

impl Nds {
    /// A console with `rom` in the slot. Without BIOS images the ROM boots
    /// directly from its header, with SWIs handled in Rust.
    pub fn new(rom: Vec<u8>, bios7: Option<Vec<u8>>, bios9: Option<Vec<u8>>) -> Self {
        let mut nds = Self {
            arm9: Cpu::new(Arch::V5TE),
            arm7: Cpu::new(Arch::V4T),
            bus: Bus::new(rom, bios7, bios9),
            frame: blank_frame(),
        };
        nds.bus.video.start(&mut nds.bus.scheduler);
        nds.direct_boot();
        nds
    }

    pub fn title(&self) -> &str {
        &self.bus.cart.title
    }

    pub fn game_code(&self) -> &str {
        &self.bus.cart.game_code
    }

    /// What the BIOS leaves behind when it boots a card: the two programs in
    /// place, the header and card id in main memory, and both CPUs at their
    /// entry points in System mode.
    fn direct_boot(&mut self) {
        let bus = &mut self.bus;
        let (p9, p7) = (bus.cart.arm9, bus.cart.arm7);
        bus.cp15.write(9, 1, 0, 0x0300_000A);
        bus.cp15.write(9, 1, 1, 0x0000_0020);
        bus.cp15.write(1, 0, 0, 0x0005_2078);
        let slice = |from: u32, size: u32| -> Vec<u8> {
            let rom = bus.cart.rom();
            let from = (from as usize).min(rom.len());
            let to = (from + size as usize).min(rom.len());
            rom[from..to].to_vec()
        };
        let (code9, code7) = (slice(p9.rom_offset, p9.size), slice(p7.rom_offset, p7.size));
        let mut view = View9(bus);
        for (i, b) in code9.iter().enumerate() {
            view.write8(p9.ram_address.wrapping_add(i as u32), *b, true);
        }
        let mut view = View7(bus);
        for (i, b) in code7.iter().enumerate() {
            view.write8(p7.ram_address.wrapping_add(i as u32), *b, true);
        }
        let header: Vec<u8> = bus.cart.rom().iter().take(0x170).copied().collect();
        let put = |bus: &mut Bus, addr: u32, bytes: &[u8]| {
            let at = (addr & 0x3F_FFFF) as usize;
            bus.main_ram[at..at + bytes.len()].copy_from_slice(bytes);
        };
        let chip = bus.cart.chip_id.to_le_bytes();
        put(bus, 0x027F_FE00, &header);
        for addr in [0x027F_F800, 0x027F_F804, 0x027F_FC00, 0x027F_FC04] {
            put(bus, addr, &chip);
        }
        put(bus, 0x027F_F808, &header[0x15E..0x160]);
        put(bus, 0x027F_F80A, &header[0x6C..0x6E]);
        put(bus, 0x027F_F850, &0x5835u16.to_le_bytes());
        put(bus, 0x027F_FC10, &0x5835u16.to_le_bytes());
        put(bus, 0x027F_FC30, &0xFFFFu16.to_le_bytes());
        put(bus, 0x027F_FC40, &0x0001u16.to_le_bytes());
        put(bus, 0x027F_FC80, &user_settings());
        bus.wramcnt = 3;
        bus.postflg = [1, 1];
        bus.video.powcnt1 = 0x820F;
        bus.exmemcnt = 0x6000;
        bus.time = [0; 2];

        self.arm9.reset();
        self.arm9.set_stack(mode::SVC, 0x0300_3FC0);
        self.arm9.set_stack(mode::IRQ, 0x0300_3F80);
        self.arm9.set_cpsr(mode::SYS);
        self.arm9.regs[13] = 0x0300_2F7C;
        self.arm9.regs[12] = p9.entry;
        self.arm9.regs[14] = p9.entry;
        self.arm9.vector_base = bus.cp15.vector_base();
        self.arm9.refill_arm(&mut View9(bus), p9.entry);

        self.arm7.reset();
        self.arm7.set_stack(mode::SVC, 0x0380_FFC0);
        self.arm7.set_stack(mode::IRQ, 0x0380_FFB0);
        self.arm7.set_cpsr(mode::SYS);
        self.arm7.regs[13] = 0x0380_FD80;
        self.arm7.regs[12] = p7.entry;
        self.arm7.regs[14] = p7.entry;
        self.arm7.refill_arm(&mut View7(bus), p7.entry);
        bus.time = [0; 2];
    }

    /// Runs until the next frame is complete.
    pub fn run_frame(&mut self) {
        loop {
            self.run_slice();
            if self.bus.video.take_frame_ready() {
                self.compose();
                return;
            }
        }
    }

    /// Each CPU runs to the end of the slice (or its next event), then the
    /// events that came due run.
    fn run_slice(&mut self) {
        let bus = &mut self.bus;
        let now = bus.scheduler.now();
        let next = bus.scheduler.next_at();
        let mut target = now + SLICE;
        if let Some(at) = next {
            target = target.min(at.max(now + 1));
        }
        let asleep = |cpu: usize| {
            bus.irq[cpu].halted && !bus.irq[cpu].should_wake() && !bus.dma[cpu].has_pending()
        };
        if asleep(ARM9) && asleep(ARM7) {
            if let Some(at) = next {
                target = at.max(now + 1);
            }
        }
        bus.target = target;
        {
            let mut view = View9(bus);
            if view.0.dma[ARM9].has_pending() {
                run_dma(&mut view);
            }
            while view.0.time[ARM9] < target {
                self.arm9.step(&mut view);
            }
        }
        {
            let mut view = View7(bus);
            if view.0.dma[ARM7].has_pending() {
                run_dma(&mut view);
            }
            while view.0.time[ARM7] < target {
                self.arm7.step(&mut view);
            }
        }
        bus.scheduler.set_now(target);
        bus.run_events();
    }

    /// One ARM9 instruction, with the ARM7 kept up to the same time (tracing).
    pub fn step(&mut self) {
        let bus = &mut self.bus;
        bus.target = bus.time[ARM9] + 1;
        self.arm9.step(&mut View9(bus));
        let t = bus.time[ARM9];
        bus.target = t;
        let mut view = View7(bus);
        while view.0.time[ARM7] < t {
            self.arm7.step(&mut view);
        }
        bus.scheduler.set_now(bus.time[ARM9].min(bus.time[ARM7]));
        bus.run_events();
    }

    /// One ARM7 instruction, with the ARM9 kept up to the same time (tracing).
    pub fn step7(&mut self) {
        let bus = &mut self.bus;
        bus.target = bus.time[ARM7] + 1;
        self.arm7.step(&mut View7(bus));
        let t = bus.time[ARM7];
        bus.target = t;
        let mut view = View9(bus);
        while view.0.time[ARM9] < t {
            self.arm9.step(&mut view);
        }
        bus.scheduler.set_now(bus.time[ARM9].min(bus.time[ARM7]));
        bus.run_events();
    }

    fn compose(&mut self) {
        let n = SCREEN_WIDTH * SCREEN_HEIGHT;
        self.frame[..n].copy_from_slice(&self.bus.video.screens[0][..]);
        self.frame[n..].copy_from_slice(&self.bus.video.screens[1][..]);
    }

    /// The last frame's two screens as one image, top above bottom: 256 × 384.
    pub fn framebuffer(&self) -> &[u32] {
        &self.frame[..]
    }

    /// The last frame's screens: index 0 is the top one.
    pub fn screens(&self) -> [&[u32]; 2] {
        [&self.bus.video.screens[0][..], &self.bus.video.screens[1][..]]
    }

    pub fn set_keys(&mut self, keys: Keys) {
        self.bus.set_keys(keys);
    }

    /// Instructions executed by both CPUs since boot.
    pub fn instructions(&self) -> u64 {
        self.arm9.instructions + self.arm7.instructions
    }

    pub fn save_state(&self) -> Vec<u8> {
        let mut out = Vec::new();
        snapshot::write_into(&STATE_FORMAT, &self.bus.cart.game_code, self, &mut out);
        out
    }

    pub fn load_state(&mut self, data: &[u8]) -> Result<(), StateError> {
        let mut restored: Nds = snapshot::read(&STATE_FORMAT, &self.bus.cart.game_code, data)?;
        restored.bus.take_images_from(&mut self.bus);
        *self = restored;
        self.compose();
        Ok(())
    }
}
