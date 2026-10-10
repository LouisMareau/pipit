//! The open test suites, headless. Blargg's tests print their verdict through
//! the serial port; the acid2 tests draw a picture that is pinned by hash.
//! The ROMs live in `tests/roms` at the workspace root (see
//! `scripts/fetch-test-roms.sh`); a missing one skips its test.

use std::path::PathBuf;

use pipit_gbc::Gbc;

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

/// Runs until the test prints its verdict (or `max_frames` pass) and returns the text.
fn blargg(relative: &str, max_frames: u32) -> Option<String> {
    let mut gbc = Gbc::new(rom(relative)?);
    let mut text = String::new();
    for _ in 0..max_frames {
        gbc.run_frame();
        text.extend(gbc.take_serial_output().iter().map(|&b| b as char));
        if text.contains("Passed") || text.contains("Failed") {
            break;
        }
    }
    println!("{relative}:\n{text}");
    Some(text)
}

fn blargg_passes(relative: &str, max_frames: u32) {
    if let Some(text) = blargg(relative, max_frames) {
        assert!(text.contains("Passed"), "{relative} did not pass");
    }
}

#[test]
fn blargg_cpu_instrs() {
    blargg_passes("gb-test-roms/cpu_instrs/cpu_instrs.gb", 4500);
}

#[test]
fn blargg_instr_timing() {
    blargg_passes("gb-test-roms/instr_timing/instr_timing.gb", 600);
}

#[test]
fn blargg_mem_timing() {
    blargg_passes("gb-test-roms/mem_timing/mem_timing.gb", 600);
}

/// Mooneye's tests end on `LD B,B` with the Fibonacci numbers in the registers
/// when they pass (and 0x42 everywhere when they fail).
fn mooneye(relative: &str) -> Option<bool> {
    let mut gbc = Gbc::new(rom(relative)?);
    for _ in 0..20_000_000u32 {
        if gbc.bus.peek(gbc.cpu.pc) == 0x40 {
            let c = &gbc.cpu;
            return Some([c.b, c.c, c.d, c.e, c.h, c.l] == [3, 5, 8, 13, 21, 34]);
        }
        gbc.step();
    }
    Some(false)
}

/// The acceptance tests that apply to this core (no boot ROM, no Super Game Boy).
const MOONEYE_ACCEPTANCE: &[&str] = &[
    "add_sp_e_timing",
    "call_cc_timing",
    "call_cc_timing2",
    "call_timing",
    "call_timing2",
    "di_timing-GS",
    "div_timing",
    "ei_sequence",
    "ei_timing",
    "halt_ime0_ei",
    "halt_ime0_nointr_timing",
    "halt_ime1_timing",
    "halt_ime1_timing2-GS",
    "if_ie_registers",
    "intr_timing",
    "jp_cc_timing",
    "jp_timing",
    "ld_hl_sp_e_timing",
    "oam_dma_restart",
    "oam_dma_start",
    "oam_dma_timing",
    "pop_timing",
    "push_timing",
    "rapid_di_ei",
    "ret_cc_timing",
    "ret_timing",
    "reti_intr_timing",
    "reti_timing",
    "rst_timing",
    "bits/mem_oam",
    "bits/reg_f",
    "bits/unused_hwio-GS",
    "instr/daa",
    "interrupts/ie_push",
    "oam_dma/basic",
    "oam_dma/reg_read",
    "oam_dma/sources-GS",
    "timer/div_write",
    "timer/rapid_toggle",
    "timer/tim00",
    "timer/tim00_div_trigger",
    "timer/tim01",
    "timer/tim01_div_trigger",
    "timer/tim10",
    "timer/tim10_div_trigger",
    "timer/tim11",
    "timer/tim11_div_trigger",
    "timer/tima_reload",
    "timer/tima_write_reloading",
    "timer/tma_write_reloading",
];

#[test]
fn mooneye_acceptance() {
    let mut passed = Vec::new();
    let mut failed = Vec::new();
    for name in MOONEYE_ACCEPTANCE {
        match mooneye(&format!("mooneye/acceptance/{name}.gb")) {
            Some(true) => passed.push(*name),
            Some(false) => failed.push(*name),
            None => return,
        }
    }
    println!("mooneye: {} passed, {} failed: {}", passed.len(), failed.len(), failed.join(", "));
    // Sub-cycle timing is not modelled yet: these are known to fail. Anything else failing is a regression.
    let regressions: Vec<_> =
        failed.iter().filter(|f| !MOONEYE_KNOWN_FAILING.contains(f)).collect();
    assert!(regressions.is_empty(), "mooneye regressions: {regressions:?}");
    let fixed: Vec<_> = passed.iter().filter(|p| MOONEYE_KNOWN_FAILING.contains(p)).collect();
    assert!(fixed.is_empty(), "now passing, remove from the known-failing list: {fixed:?}");
}

/// Tests of when, within an instruction's cycles, a memory access happens; the
/// core times instructions as a whole, which these can tell apart.
const MOONEYE_KNOWN_FAILING: &[&str] = &[
    "add_sp_e_timing",
    "call_cc_timing",
    "call_cc_timing2",
    "call_timing",
    "call_timing2",
    "di_timing-GS",
    "ei_sequence",
    "ei_timing",
    "halt_ime0_ei",
    "halt_ime0_nointr_timing",
    "halt_ime1_timing2-GS",
    "jp_cc_timing",
    "jp_timing",
    "ld_hl_sp_e_timing",
    "oam_dma_restart",
    "oam_dma_start",
    "oam_dma_timing",
    "push_timing",
    "rapid_di_ei",
    "ret_cc_timing",
    "ret_timing",
    "reti_intr_timing",
    "reti_timing",
    "rst_timing",
    "bits/unused_hwio-GS",
    "oam_dma/reg_read",
    "oam_dma/sources-GS",
    "timer/rapid_toggle",
];

/// FNV-1a over the framebuffer, to pin down the picture a visual test produces.
fn frame_hash(gbc: &Gbc) -> u64 {
    gbc.framebuffer()
        .iter()
        .fold(0xcbf2_9ce4_8422_2325u64, |h, &px| (h ^ u64::from(px)).wrapping_mul(0x100_0000_01b3))
}

fn acid2(relative: &str) -> Option<u64> {
    let mut gbc = Gbc::new(rom(relative)?);
    for _ in 0..120 {
        gbc.run_frame();
    }
    let hash = frame_hash(&gbc);
    println!("{relative}: frame hash {hash:#018x}");
    Some(hash)
}

// The hashes pin pictures checked against the reference images by eye.
#[test]
fn dmg_acid2() {
    if let Some(hash) = acid2("acid2/dmg-acid2.gb") {
        assert_eq!(hash, 0xdb9e_9776_e034_1775);
    }
}

#[test]
fn cgb_acid2() {
    if let Some(hash) = acid2("acid2/cgb-acid2.gbc") {
        assert_eq!(hash, 0xe54f_895a_7fec_1ddc);
    }
}
