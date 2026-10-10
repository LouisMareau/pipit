//! Game Boy and Game Boy Color emulation core.
//!
//! Like `pipit-gba`, the crate is free of I/O: it takes a ROM, produces frames
//! and audio samples, and leaves the rest to the front-ends. The machine is
//! stepped per CPU M-cycle through the bus (`memory.rs`), which keeps the
//! picture, sound, timer and serial port in step with every memory access.
//!
//! ```no_run
//! use pipit_gbc::{key, Gbc};
//!
//! let rom = std::fs::read("game.gbc").unwrap();
//! let mut gbc = Gbc::new(rom);
//! gbc.set_keys(key::A);
//! gbc.run_frame();
//! let pixels: &[u32] = gbc.framebuffer(); // 160 × 144, 0xFFRRGGBB
//! ```

pub mod audio;
pub mod cartridge;
pub mod color;
pub mod cpu;
pub mod joypad;
pub mod memory;
pub mod serial;
pub mod timer;
pub mod video;

pub use cartridge::Cartridge;
pub use joypad::key;
pub use memory::Bus;
pub use pipit_common::snapshot::{self, StateError};

use serde::{Deserialize, Serialize};

pub const SCREEN_WIDTH: usize = 160;
pub const SCREEN_HEIGHT: usize = 144;
/// The clock in normal speed, in Hz.
pub const CLOCK_HZ: u32 = 4_194_304;
/// Clocks per frame: 154 lines × 456 dots, 59.73 frames per second.
pub const CYCLES_PER_FRAME: u32 = 70_224;

/// Save-state file format.
const STATE_FORMAT: snapshot::Format = snapshot::Format { magic: *b"PIPC", version: 1 };

/// A whole Game Boy (Color).
#[derive(Serialize, Deserialize)]
pub struct Gbc {
    pub cpu: cpu::Cpu,
    pub bus: Bus,
}

impl Gbc {
    /// Creates a console with the ROM inserted: a Game Boy Color for a colour game,
    /// a classic Game Boy for the others. No boot ROM is needed.
    pub fn new(rom: Vec<u8>) -> Self {
        let cart = Cartridge::new(rom);
        let cgb = cart.is_color();
        Self { cpu: cpu::Cpu::new(cgb), bus: Bus::new(cart, cgb) }
    }

    /// Runs until the next frame has been fully drawn (VBlank start).
    pub fn run_frame(&mut self) {
        while !self.bus.video.take_frame_ready() {
            self.cpu.step(&mut self.bus);
        }
    }

    /// Runs one instruction (or one halted M-cycle).
    pub fn step(&mut self) {
        self.cpu.step(&mut self.bus);
    }

    /// The last completed frame as `0xFFRRGGBB` pixels, row-major, 160 × 144.
    pub fn framebuffer(&self) -> &[u32] {
        self.bus.video.framebuffer()
    }

    /// Sets which keys are held (bits from `key`).
    pub fn set_keys(&mut self, keys: u8) {
        self.bus.joypad.set_keys(keys, &mut self.bus.if_);
    }

    /// Takes the audio produced since the last call: interleaved stereo i16 at 32768 Hz.
    pub fn drain_audio(&mut self) -> Vec<i16> {
        self.bus.audio.drain_samples()
    }

    /// Battery-backed memory as a save file, if the cartridge has any.
    pub fn save_data(&self) -> Option<Vec<u8>> {
        self.bus.cart.save_data(self.bus.cycles, self.bus.unix_now())
    }

    pub fn load_save_data(&mut self, data: &[u8]) {
        let (now, unix) = (self.bus.cycles, self.bus.unix_now());
        self.bus.cart.load_save_data(data, now, unix);
    }

    pub fn take_save_dirty(&mut self) -> bool {
        self.bus.cart.take_save_dirty()
    }

    pub fn save_state(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.save_state_into(&mut out);
        out
    }

    pub fn save_state_into(&self, out: &mut Vec<u8>) {
        snapshot::write_into(&STATE_FORMAT, &self.title(), self, out);
    }

    pub fn load_state(&mut self, data: &[u8]) -> Result<(), StateError> {
        let mut restored: Gbc = snapshot::read(&STATE_FORMAT, &self.title(), data)?;
        restored.bus.cart.take_rom_from(&mut self.bus.cart);
        *self = restored;
        Ok(())
    }

    /// A hash of the whole machine state, to check that two emulations agree.
    pub fn state_hash(&self) -> u64 {
        self.save_state().iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
            (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
        })
    }

    /// Sets the cartridge's clock from a Unix timestamp (seconds).
    pub fn set_time(&mut self, unix_seconds: i64) {
        self.bus.set_time(unix_seconds);
    }

    pub fn title(&self) -> String {
        self.bus.cart.title().to_string()
    }

    pub fn is_color(&self) -> bool {
        self.bus.is_color()
    }

    /// Bytes the game sent through the serial port (test ROMs print this way).
    pub fn take_serial_output(&mut self) -> Vec<u8> {
        self.bus.serial.take_output()
    }
}
