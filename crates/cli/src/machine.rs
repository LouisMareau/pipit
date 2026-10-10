//! One console of either kind, for the commands that work on both. The file's
//! extension decides: `.gb` and `.gbc` are Game Boy games, the rest Game Boy
//! Advance.

use std::path::Path;

use pipit_gba::{Gba, Keys};
use pipit_gbc::Gbc;

#[allow(clippy::large_enum_variant)]
pub enum Machine {
    Gba(Gba),
    Gbc(Gbc),
}

impl Machine {
    pub fn open(path: &Path, rom: Vec<u8>, bios: Option<Vec<u8>>) -> Self {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        if ext == "gb" || ext == "gbc" {
            Machine::Gbc(Gbc::new(rom))
        } else {
            Machine::Gba(Gba::new(rom, bios))
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Machine::Gba(g) => {
                format!("{} [{}] save: {:?}", g.title(), g.game_code(), g.bus.cart.save_type())
            }
            Machine::Gbc(g) => format!(
                "{} [{}] mapper: {:?}",
                g.title(),
                if g.is_color() { "GBC" } else { "GB" },
                g.bus.cart.mbc()
            ),
        }
    }

    pub fn run_frame(&mut self) {
        match self {
            Machine::Gba(g) => g.run_frame(),
            Machine::Gbc(g) => g.run_frame(),
        }
    }

    pub fn step(&mut self) {
        match self {
            Machine::Gba(g) => g.step(),
            Machine::Gbc(g) => g.step(),
        }
    }

    pub fn set_keys(&mut self, keys: Keys) {
        match self {
            Machine::Gba(g) => g.set_keys(keys),
            // The Game Boy's eight keys are the Advance's low byte.
            Machine::Gbc(g) => g.set_keys(keys.0 as u8),
        }
    }

    pub fn framebuffer(&self) -> &[u32] {
        match self {
            Machine::Gba(g) => g.framebuffer(),
            Machine::Gbc(g) => g.framebuffer(),
        }
    }

    pub fn size(&self) -> (u32, u32) {
        match self {
            Machine::Gba(_) => (pipit_gba::SCREEN_WIDTH as u32, pipit_gba::SCREEN_HEIGHT as u32),
            Machine::Gbc(_) => (pipit_gbc::SCREEN_WIDTH as u32, pipit_gbc::SCREEN_HEIGHT as u32),
        }
    }

    pub fn load_save_data(&mut self, data: &[u8]) {
        match self {
            Machine::Gba(g) => g.load_save_data(data),
            Machine::Gbc(g) => g.load_save_data(data),
        }
    }

    pub fn save_data(&self) -> Option<Vec<u8>> {
        match self {
            Machine::Gba(g) => g.save_data().map(<[u8]>::to_vec),
            Machine::Gbc(g) => g.save_data(),
        }
    }

    pub fn peek(&mut self, addr: u32) -> u8 {
        match self {
            Machine::Gba(g) => g.bus.load8(addr),
            Machine::Gbc(g) => g.bus.peek(addr as u16),
        }
    }

    pub fn instructions(&self) -> u64 {
        match self {
            Machine::Gba(g) => g.cpu.instructions,
            Machine::Gbc(g) => g.cpu.instructions,
        }
    }

    /// The CPU's registers, for `--regs`.
    pub fn registers(&self) -> String {
        match self {
            Machine::Gba(g) => {
                let mut out = String::new();
                for (i, v) in g.cpu.regs.iter().enumerate() {
                    out.push_str(&format!("r{i:<2}={v:08X} "));
                    if i % 4 == 3 {
                        out.push('\n');
                    }
                }
                out + &format!(
                    "cpsr={:08X} pc={:08X} cycles={}",
                    g.cpu.cpsr,
                    g.cpu.pc(),
                    g.bus.scheduler.now()
                )
            }
            Machine::Gbc(g) => format!(
                "AF={:04X} BC={:04X} DE={:04X} HL={:04X} SP={:04X} PC={:04X} IME={} cycles={}",
                g.cpu.af(),
                g.cpu.bc(),
                g.cpu.de(),
                g.cpu.hl(),
                g.cpu.sp,
                g.cpu.pc,
                u8::from(g.cpu.ime),
                g.bus.cycles
            ),
        }
    }

    /// One line per instruction about to run, for `--trace`.
    pub fn trace_line(&mut self) -> String {
        match self {
            Machine::Gba(g) => {
                let r = &g.cpu.regs;
                format!(
                    "{:08X} {:08X} [{}] r0={:08X} r1={:08X} r2={:08X} r3={:08X} r12={:08X} sp={:08X} lr={:08X} cpsr={:08X}",
                    g.cpu.pc(),
                    g.cpu.next_opcode(),
                    if g.cpu.is_thumb() { "T" } else { "A" },
                    r[0], r[1], r[2], r[3], r[12], r[13], r[14], g.cpu.cpsr
                )
            }
            Machine::Gbc(g) => format!(
                "{:04X} {:02X} AF={:04X} BC={:04X} DE={:04X} HL={:04X} SP={:04X}",
                g.cpu.pc,
                g.bus.peek(g.cpu.pc),
                g.cpu.af(),
                g.cpu.bc(),
                g.cpu.de(),
                g.cpu.hl(),
                g.cpu.sp
            ),
        }
    }
}
