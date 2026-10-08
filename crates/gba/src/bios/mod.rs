//! BIOS: either a user-supplied image or Pipit's own replacement.
//!
//! The replacement is a 16 KB image containing only what runs as real code: the
//! exception vectors, a reset stub that jumps to the cartridge, and the interrupt
//! dispatcher that calls the game's handler at `0x03007FFC`. Every SWI function is
//! implemented in Rust (`swi.rs`) and intercepted by the CPU before the exception
//! is taken, so no Nintendo code is needed.

pub mod swi;

use serde::{Deserialize, Serialize};

pub const BIOS_SIZE: usize = 0x4000;

#[derive(Serialize, Deserialize)]
pub struct Bios {
    /// Not part of save states: a restored state keeps whatever BIOS is loaded.
    #[serde(skip, default = "build_stub")]
    image: Box<[u8; BIOS_SIZE]>,
    /// True when SWI calls are handled in Rust instead of executed from `image`.
    pub hle: bool,
}

impl Bios {
    pub fn new(image: Option<Vec<u8>>) -> Self {
        match image {
            Some(data) if data.len() == BIOS_SIZE => {
                let mut image = Box::new([0u8; BIOS_SIZE]);
                image.copy_from_slice(&data);
                Self { image, hle: false }
            }
            _ => Self { image: build_stub(), hle: true },
        }
    }

    /// Moves the BIOS image out of `other` (restoring a save state keeps it).
    pub(crate) fn take_image_from(&mut self, other: &mut Bios) {
        std::mem::swap(&mut self.image, &mut other.image);
    }

    #[inline]
    pub fn read32(&self, addr: u32) -> u32 {
        let off = (addr as usize & (BIOS_SIZE - 1)) & !3;
        u32::from_le_bytes([
            self.image[off],
            self.image[off + 1],
            self.image[off + 2],
            self.image[off + 3],
        ])
    }
}

/// Assembles the replacement BIOS. Each entry is a hand-encoded ARM instruction.
fn build_stub() -> Box<[u8; BIOS_SIZE]> {
    let mut image = Box::new([0u8; BIOS_SIZE]);

    // `b target` from address `at`: offset is in words relative to at+8.
    let branch =
        |at: u32, target: u32| 0xEA00_0000 | (((target.wrapping_sub(at + 8)) >> 2) & 0x00FF_FFFF);

    const RESET: u32 = 0x40;
    const IRQ: u32 = 0x80;
    let vectors: [u32; 8] = [
        branch(0x00, RESET), // reset
        branch(0x04, 0x04),  // undefined instruction: hang
        branch(0x08, 0x08),  // SWI: never reached in HLE mode
        branch(0x0C, 0x0C),  // prefetch abort
        branch(0x10, 0x10),  // data abort
        branch(0x14, 0x14),  // reserved
        branch(0x18, IRQ),   // IRQ
        branch(0x1C, 0x1C),  // FIQ
    ];

    // Reset: the CPU is already set up by `Cpu::reset`; just enter the cartridge.
    let reset: [u32; 2] = [
        0xE3A0_E302, // mov lr, #0x08000000
        0xE12F_FF1E, // bx lr
    ];

    // IRQ dispatcher, identical to the original BIOS routine documented in GBATEK.
    // The two words after it are never executed: they are what the read
    // protection latch exposes after an interrupt returns (the pipeline has
    // fetched 8 bytes ahead), which software can observe.
    let irq: [u32; 8] = [
        0xE92D_500F, // stmfd sp!, {r0-r3, r12, lr}
        0xE3A0_0301, // mov r0, #0x04000000
        0xE28F_E000, // add lr, pc, #0
        0xE510_F004, // ldr pc, [r0, #-4]       ; user handler at 0x03007FFC
        0xE8BD_500F, // ldmfd sp!, {r0-r3, r12, lr}
        0xE25E_F004, // subs pc, lr, #4
        0x0000_0000,
        0xE55E_C002, // latch value seen after the IRQ returns
    ];

    let mut put = |at: u32, words: &[u32]| {
        for (i, w) in words.iter().enumerate() {
            let off = at as usize + i * 4;
            image[off..off + 4].copy_from_slice(&w.to_le_bytes());
        }
    };
    put(0, &vectors);
    put(RESET, &reset);
    put(IRQ, &irq);
    image
}
