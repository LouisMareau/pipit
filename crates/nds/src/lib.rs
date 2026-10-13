//! Nintendo DS emulation core: the ARM9 and ARM7 and everything between them.
//!
//! Being built up stage by stage (see docs/roadmap.md). So far: both CPUs with
//! their memory maps, the interrupt controllers, timers, DMA, IPC, the card
//! interface, the divider, the two 2D engines' timing and text backgrounds,
//! and a BIOS replacement that boots a ROM directly from its header; the
//! wireless hardware, for consoles playing together (see [`link`]).

pub mod backup;
pub mod bios;
pub mod cart;
pub mod dma;
pub mod firmware;
pub mod gpu3d;
pub mod ipc;
pub mod irq;
pub mod link;
pub mod math;
pub mod memory;
pub mod rtc;
pub mod savedb;
pub mod scheduler;
pub mod slot2;
pub mod sound;
pub mod spi;
pub mod timers;
pub mod video;
pub mod wifi;

pub use link::Link;
pub use memory::Keys;
pub use pipit_common::snapshot::StateError;
pub use slot2::Slot2;
pub use video::{SCREEN_HEIGHT, SCREEN_WIDTH};

use memory::{run_dma, Bus, View7, View9, ARM7, ARM9, SLICE};
use pipit_arm::{mode, Arch, Bus as _, Cpu};
use pipit_common::snapshot;
use serde::{Deserialize, Serialize};

const STATE_FORMAT: snapshot::Format = snapshot::Format { magic: *b"PIPD", version: 2 };

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
    boxed(0xFF00_0000)
}

/// A boxed array filled with `value`, built on the heap: `Box::new([v; N])`
/// may go through the stack, which WebAssembly has little of.
pub(crate) fn boxed<T: Copy, const N: usize>(value: T) -> Box<[T; N]> {
    match vec![value; N].into_boxed_slice().try_into() {
        Ok(array) => array,
        Err(_) => unreachable!("the vector has N elements"),
    }
}

impl Nds {
    /// A console with `rom` in the slot. Without BIOS images the ROM boots
    /// directly from its header, with SWIs handled in Rust.
    pub fn new(
        rom: Vec<u8>,
        bios7: Option<Vec<u8>>,
        bios9: Option<Vec<u8>>,
        firmware: Option<Vec<u8>>,
    ) -> Self {
        let firmware = firmware.unwrap_or_else(firmware::default_image);
        let mut nds = Self {
            arm9: Cpu::new(Arch::V5TE),
            arm7: Cpu::new(Arch::V4T),
            bus: Bus::new(rom, bios7, bios9, firmware),
            frame: blank_frame(),
        };
        nds.bus.video.start(&mut nds.bus.scheduler);
        nds.bus.sound.start(&mut nds.bus.scheduler);
        nds.bus.scheduler.schedule_at(scheduler::Event::RtcSecond, rtc::CYCLES_PER_SECOND);
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
        put(bus, 0x027F_FC80, &firmware::user_settings_of(&bus.spi.firmware));
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

    /// Runs until the clock reaches `target`: whether a frame completed on the way.
    pub(crate) fn run_until(&mut self, target: u64) -> bool {
        let mut ready = false;
        while self.bus.scheduler.now() < target {
            self.run_slice();
            if self.bus.video.take_frame_ready() {
                self.compose();
                ready = true;
            }
        }
        ready
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

    pub(crate) fn compose(&mut self) {
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

    /// Takes the audio produced since the last call: interleaved stereo i16 at
    /// the mixer's 32.7 kHz (played as 32768 Hz).
    pub fn drain_audio(&mut self) -> Vec<i16> {
        self.bus.sound.drain_samples()
    }

    pub fn set_keys(&mut self, keys: Keys) {
        self.bus.set_keys(keys);
    }

    /// The pen on the bottom screen (x 0–255, y 0–191), or lifted.
    pub fn set_touch(&mut self, touch: Option<(u8, u8)>) {
        self.bus.set_touch(touch);
    }

    /// Sets the real-time clock from a Unix timestamp (seconds).
    pub fn set_time(&mut self, unix_seconds: i64) {
        let now = self.bus.time[ARM7];
        self.bus.rtc.set_time(unix_seconds, now);
        if let Slot2::Cartridge(cart) = &mut self.bus.slot2 {
            cart.set_time(unix_seconds);
        }
    }

    /// The card's save memory, once the game has shown what chip it has.
    pub fn save_data(&self) -> Option<&[u8]> {
        self.bus.cart.backup.data()
    }

    /// A save file from an earlier run; its size says what chip it is.
    pub fn load_save_data(&mut self, data: &[u8]) {
        self.bus.cart.backup.load(data);
    }

    /// Whether the save memory changed since the last call.
    pub fn take_save_dirty(&mut self) -> bool {
        self.bus.cart.backup.take_dirty()
    }

    /// Puts a GBA cartridge, the Rumble Pak or nothing in the GBA slot.
    pub fn set_slot2(&mut self, slot2: Slot2) {
        self.bus.slot2 = slot2;
    }

    pub fn slot2(&self) -> &Slot2 {
        &self.bus.slot2
    }

    /// The GBA cartridge's save memory, when one is in the slot.
    pub fn slot2_save_data(&self) -> Option<&[u8]> {
        self.bus.slot2.save_data()
    }

    pub fn load_slot2_save_data(&mut self, data: &[u8]) {
        self.bus.slot2.load_save_data(data);
    }

    /// Whether the GBA cartridge's save memory changed since the last call.
    pub fn take_slot2_save_dirty(&mut self) -> bool {
        self.bus.slot2.take_save_dirty()
    }

    /// Whether the Rumble Pak's actuator moved since the last call.
    pub fn take_rumble(&mut self) -> bool {
        self.bus.slot2.take_rumble()
    }

    /// Names the card's save chip (what the user knows better than the
    /// first write tells), `size` bytes of it; its contents start blank.
    pub fn set_save_type(&mut self, kind: backup::Kind, size: usize) {
        self.bus.cart.backup.set_kind_sized(kind, size);
    }

    /// The firmware image, with whatever the console wrote to it.
    pub fn firmware(&self) -> &[u8] {
        &self.bus.spi.firmware
    }

    /// Whether the firmware changed since the last call.
    pub fn take_firmware_dirty(&mut self) -> bool {
        std::mem::replace(&mut self.bus.spi.firmware_dirty, false)
    }

    /// The game asked the power management chip to switch the console off.
    pub fn powered_off(&self) -> bool {
        self.bus.spi.powered_off
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

    /// A hash of the whole machine state, to check that two emulations agree.
    pub fn state_hash(&self) -> u64 {
        let state = self.save_state();
        state.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
            (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
        })
    }

    pub fn load_state(&mut self, data: &[u8]) -> Result<(), StateError> {
        let mut restored: Nds = snapshot::read(&STATE_FORMAT, &self.bus.cart.game_code, data)?;
        restored.bus.take_images_from(&mut self.bus);
        *self = restored;
        self.compose();
        Ok(())
    }
}
