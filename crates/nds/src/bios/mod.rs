//! The two BIOSes: a user's images, or Pipit's replacements.
//!
//! The replacements hold only what runs as code: exception vectors and the
//! interrupt dispatchers (which call the game's handler through the pointer
//! the real BIOS uses: DTCM+3FFCh on the ARM9, 0380FFFCh on the ARM7). Every
//! SWI is handled in Rust, intercepted before the exception is taken.

pub mod swi;

pub const BIOS9_SIZE: usize = 0x1000;
pub const BIOS7_SIZE: usize = 0x4000;

/// `b target` from address `at`: the offset is in words relative to at+8.
fn branch(at: u32, target: u32) -> u32 {
    0xEA00_0000 | ((target.wrapping_sub(at + 8) >> 2) & 0x00FF_FFFF)
}

fn put(image: &mut [u8], at: usize, words: &[u32]) {
    for (i, w) in words.iter().enumerate() {
        image[at + i * 4..at + i * 4 + 4].copy_from_slice(&w.to_le_bytes());
    }
}

/// Vectors that hang on everything but IRQ (nothing else is reachable when
/// SWIs are replaced and the ROM boots directly).
fn vectors(irq_at: u32) -> [u32; 8] {
    [
        branch(0x00, 0x00),
        branch(0x04, 0x04),
        branch(0x08, 0x08),
        branch(0x0C, 0x0C),
        branch(0x10, 0x10),
        branch(0x14, 0x14),
        branch(0x18, irq_at),
        branch(0x1C, 0x1C),
    ]
}

/// The ARM9 replacement (at FFFF0000h).
pub fn stub9() -> Box<[u8; BIOS9_SIZE]> {
    let mut image = Box::new([0u8; BIOS9_SIZE]);
    const IRQ: u32 = 0x80;
    put(&mut image[..], 0, &vectors(IRQ));
    // The dispatcher: the handler pointer sits 4 bytes before the end of DTCM,
    // whose base comes from CP15.
    let irq: [u32; 10] = [
        0xE92D_500F, // stmfd sp!, {r0-r3, r12, lr}
        0xEE19_0F11, // mrc p15, 0, r0, c9, c1, 0   ; DTCM base and size
        0xE1A0_0620, // mov r0, r0, lsr #12
        0xE1A0_0600, // mov r0, r0, lsl #12
        0xE280_0901, // add r0, r0, #0x4000
        0xE510_1004, // ldr r1, [r0, #-4]
        0xE28F_E000, // add lr, pc, #0
        0xE12F_FF11, // bx r1
        0xE8BD_500F, // ldmfd sp!, {r0-r3, r12, lr}
        0xE25E_F004, // subs pc, lr, #4
    ];
    put(&mut image[..], IRQ as usize, &irq);
    image
}

/// The ARM7 replacement (at 00000000h).
pub fn stub7() -> Box<[u8; BIOS7_SIZE]> {
    let mut image = Box::new([0u8; BIOS7_SIZE]);
    const IRQ: u32 = 0x80;
    put(&mut image[..], 0, &vectors(IRQ));
    // The handler pointer is at 03FFFFFCh, the end of the ARM7's work RAM mirror.
    let irq: [u32; 6] = [
        0xE92D_500F, // stmfd sp!, {r0-r3, r12, lr}
        0xE3A0_0301, // mov r0, #0x04000000
        0xE28F_E000, // add lr, pc, #0
        0xE510_F004, // ldr pc, [r0, #-4]
        0xE8BD_500F, // ldmfd sp!, {r0-r3, r12, lr}
        0xE25E_F004, // subs pc, lr, #4
    ];
    put(&mut image[..], IRQ as usize, &irq);
    image
}
