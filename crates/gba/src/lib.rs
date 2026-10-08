//! Game Boy Advance emulation core.
//!
//! The crate is deliberately free of I/O: it takes a ROM, produces frames and audio
//! samples, and leaves everything else (windows, audio devices, files) to the
//! front-ends in `crates/wasm`, `crates/cli` and `web/`.
//!
//! ```no_run
//! use pipit_gba::{Gba, Keys};
//!
//! let rom = std::fs::read("game.gba").unwrap();
//! let mut gba = Gba::new(rom, None);
//! gba.set_keys(Keys::A);
//! gba.run_frame();
//! let pixels: &[u32] = gba.framebuffer(); // 240 × 160, 0xAARRGGBB
//! ```

pub mod audio;
pub mod bios;
pub mod cartridge;
pub mod cpu;
pub mod dma;
pub mod irq;
pub mod keypad;
pub mod memory;
pub mod scheduler;
pub mod snapshot;
pub mod timers;
pub mod video;

pub use cartridge::{Cartridge, SaveType};
pub use keypad::Keys;
pub use memory::Bus;
pub use snapshot::StateError;

use serde::{Deserialize, Serialize};

/// Screen width in pixels.
pub const SCREEN_WIDTH: usize = 240;
/// Screen height in pixels.
pub const SCREEN_HEIGHT: usize = 160;
/// CPU clock, in Hz.
pub const CLOCK_HZ: u32 = 16_777_216;
/// Cycles per frame: 228 scanlines × 1232 cycles.
pub const CYCLES_PER_FRAME: u32 = 280_896;

/// A whole Game Boy Advance.
#[derive(Serialize, Deserialize)]
pub struct Gba {
    pub cpu: cpu::Cpu,
    pub bus: Bus,
}

impl Gba {
    /// Creates a console with the given ROM inserted and (optionally) a real BIOS image.
    /// Without a BIOS image the built-in high-level BIOS is used.
    pub fn new(rom: Vec<u8>, bios: Option<Vec<u8>>) -> Self {
        let cart = Cartridge::new(rom);
        let mut bus = Bus::new(cart, bios);
        let mut cpu = cpu::Cpu::new();
        cpu.reset(&mut bus);
        Self { cpu, bus }
    }

    /// Runs the emulator until the next frame has been fully rendered (VBlank start).
    pub fn run_frame(&mut self) {
        while !self.bus.video.take_frame_ready() {
            self.cpu.step(&mut self.bus);
        }
    }

    /// Runs the CPU until the scheduler clock reaches `target` cycles.
    pub fn run_until(&mut self, target: u64) {
        while self.bus.scheduler.now() < target {
            self.cpu.step(&mut self.bus);
        }
    }

    /// Executes exactly one instruction (or one halted tick). Useful for debugging.
    pub fn step(&mut self) {
        self.cpu.step(&mut self.bus);
    }

    /// The last completed frame as `0xFFRRGGBB` pixels, row-major, 240 × 160.
    pub fn framebuffer(&self) -> &[u32] {
        self.bus.video.framebuffer()
    }

    /// Sets which keys are currently held.
    pub fn set_keys(&mut self, keys: Keys) {
        self.bus.keypad.set_keys(keys, &mut self.bus.irq);
    }

    /// Takes the audio samples produced since the last call: interleaved stereo i16.
    pub fn drain_audio(&mut self) -> Vec<i16> {
        self.bus.audio.drain_samples()
    }

    /// Current contents of the cartridge's backup memory (SRAM/Flash/EEPROM), if any.
    pub fn save_data(&self) -> Option<&[u8]> {
        self.bus.cart.save_data()
    }

    /// Restores backup memory, e.g. from a `.sav` file.
    pub fn load_save_data(&mut self, data: &[u8]) {
        self.bus.cart.load_save_data(data);
    }

    /// Whether backup memory changed since the last call (for the front-end's autosave).
    pub fn take_save_dirty(&mut self) -> bool {
        self.bus.cart.take_save_dirty()
    }

    /// Serializes the complete machine state (everything except the ROM and BIOS).
    pub fn save_state(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.save_state_into(&mut out);
        out
    }

    /// Like `save_state`, but reuses `out` (cleared first) to avoid allocating —
    /// front-ends snapshot many times per second for rewind.
    pub fn save_state_into(&self, out: &mut Vec<u8>) {
        snapshot::write_into(&self.game_code(), self, out);
    }

    /// Restores a state produced by `save_state` for the same game.
    pub fn load_state(&mut self, data: &[u8]) -> Result<(), StateError> {
        let mut restored: Gba = snapshot::read(&self.game_code(), data)?;
        restored.bus.cart.take_rom_from(&mut self.bus.cart);
        restored.bus.bios.take_image_from(&mut self.bus.bios);
        *self = restored;
        Ok(())
    }

    /// Sets the cartridge real-time clock from a Unix timestamp (seconds).
    pub fn set_time(&mut self, unix_seconds: i64) {
        self.bus.cart.set_now(self.bus.scheduler.now());
        self.bus.cart.set_time(unix_seconds);
    }

    /// Game title from the ROM header.
    pub fn title(&self) -> String {
        self.bus.cart.title()
    }

    /// Game code from the ROM header, e.g. `BPEE` for Pokémon Emerald (USA).
    pub fn game_code(&self) -> String {
        self.bus.cart.game_code()
    }
}
