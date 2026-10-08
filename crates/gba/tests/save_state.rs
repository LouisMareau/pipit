//! Save states must be exact: restoring one and running on has to produce the
//! same frames as never having stopped.

use std::path::PathBuf;

use pipit_gba::{Gba, StateError};

fn rom(relative: &str) -> Option<Vec<u8>> {
    let path: PathBuf =
        [env!("CARGO_MANIFEST_DIR"), "..", "..", "tests", "roms", relative].iter().collect();
    std::fs::read(&path).ok()
}

fn frame_hash(gba: &Gba) -> u64 {
    gba.framebuffer()
        .iter()
        .fold(0xcbf2_9ce4_8422_2325u64, |h, &px| (h ^ u64::from(px)).wrapping_mul(0x100_0000_01b3))
}

#[test]
fn restored_state_continues_identically() {
    let Some(data) = rom("FuzzARM/FuzzARM.gba") else {
        eprintln!("skipping: test ROM not found");
        return;
    };
    let mut gba = Gba::new(data.clone(), None);
    for _ in 0..40 {
        gba.run_frame();
    }
    let state = gba.save_state();
    println!("state size: {} bytes", state.len());

    for _ in 0..30 {
        gba.run_frame();
    }
    let expected = frame_hash(&gba);
    let expected_cycles = gba.bus.scheduler.now();

    let mut fresh = Gba::new(data, None);
    fresh.load_state(&state).expect("state loads");
    for _ in 0..30 {
        fresh.run_frame();
    }
    assert_eq!(frame_hash(&fresh), expected);
    assert_eq!(fresh.bus.scheduler.now(), expected_cycles);
}

#[test]
fn rejects_foreign_states() {
    let Some(data) = rom("gba-tests/ppu/hello.gba") else { return };
    let mut gba = Gba::new(data, None);
    assert_eq!(gba.load_state(b"nope"), Err(StateError::NotAState));
    let state = gba.save_state();
    let mut other = Gba::new(vec![0u8; 0x200], None);
    assert!(matches!(other.load_state(&state), Err(StateError::GameMismatch { .. })));
}
