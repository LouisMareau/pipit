//! Runs the open-source test ROM suites headless.
//!
//! The ROMs live in `tests/roms` at the workspace root (see
//! `scripts/fetch-test-roms.sh`). A missing ROM skips its test rather than failing,
//! so the unit tests still run on a fresh checkout.

use std::path::PathBuf;

use pipit_gba::Gba;

fn rom(relative: &str) -> Option<Vec<u8>> {
    let path: PathBuf =
        [env!("CARGO_MANIFEST_DIR"), "..", "..", "tests", "roms", relative].iter().collect();
    match std::fs::read(&path) {
        Ok(data) => Some(data),
        Err(_) => {
            eprintln!("skipping: {} not found (run scripts/fetch-test-roms.sh)", path.display());
            None
        }
    }
}

fn run(relative: &str, frames: u32) -> Option<Gba> {
    let mut gba = Gba::new(rom(relative)?, None);
    for _ in 0..frames {
        gba.run_frame();
    }
    Some(gba)
}

/// jsmolka's tests leave the number of the first failing test in r12 (0 = all passed).
fn jsmolka(relative: &str) {
    if let Some(gba) = run(&format!("gba-tests/{relative}"), 120) {
        assert_eq!(gba.cpu.regs[12], 0, "{relative}: failed test number {}", gba.cpu.regs[12]);
    }
}

/// FNV-1a over the framebuffer, to pin down the picture a visual test produces.
fn frame_hash(gba: &Gba) -> u64 {
    gba.framebuffer()
        .iter()
        .fold(0xcbf2_9ce4_8422_2325u64, |h, &px| (h ^ u64::from(px)).wrapping_mul(0x100_0000_01b3))
}

#[test]
fn jsmolka_arm() {
    jsmolka("arm/arm.gba");
}

#[test]
fn jsmolka_thumb() {
    jsmolka("thumb/thumb.gba");
}

#[test]
fn jsmolka_memory() {
    jsmolka("memory/memory.gba");
}

#[test]
fn jsmolka_bios() {
    jsmolka("bios/bios.gba");
}

#[test]
fn jsmolka_nes() {
    jsmolka("nes/nes.gba");
}

#[test]
fn jsmolka_unsafe() {
    jsmolka("unsafe/unsafe.gba");
}

#[test]
fn jsmolka_save_sram() {
    jsmolka("save/sram.gba");
}

#[test]
fn jsmolka_save_flash64() {
    jsmolka("save/flash64.gba");
}

#[test]
fn jsmolka_save_flash128() {
    jsmolka("save/flash128.gba");
}

#[test]
fn jsmolka_save_none() {
    jsmolka("save/none.gba");
}

/// The PPU tests are visual: the hashes were taken from a frame checked by eye
/// against the reference screenshots in the suite.
#[test]
fn jsmolka_ppu_pictures() {
    for (name, expected) in [
        ("gba-tests/ppu/hello.gba", 0xc702_ae1d_c905_3882_u64),
        ("gba-tests/ppu/shades.gba", 0x0b5c_9262_3697_d125),
        ("gba-tests/ppu/stripes.gba", 0x91ff_9f28_ca3c_2a25),
    ] {
        if let Some(gba) = run(name, 10) {
            let hash = frame_hash(&gba);
            assert_eq!(hash, expected, "{name}: frame hash {hash:#x}");
        }
    }
}

/// FuzzARM runs thousands of random ARM/Thumb instructions against precomputed
/// results. The test counter in r12 reaches zero only when every test passed; a
/// failure stops the loop with the counter still live.
#[test]
fn fuzzarm() {
    for name in ["FuzzARM/FuzzARM.gba", "FuzzARM/ARM_Any.gba", "FuzzARM/THUMB_Any.gba"] {
        if let Some(gba) = run(name, 3000) {
            assert_eq!(gba.cpu.regs[12], 0, "{name}: stopped with {} tests left", gba.cpu.regs[12]);
        }
    }
}
