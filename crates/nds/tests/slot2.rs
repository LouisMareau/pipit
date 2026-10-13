//! The GBA slot through the machine: a cartridge's header and save chip,
//! which CPU owns the slot, the Rumble Pak, an empty slot, and a save state
//! keeping the cartridge.

use pipit_arm::Bus as _;
use pipit_nds::memory::{View7, View9};
use pipit_nds::{Nds, Slot2};

/// A card with programs that only loop.
fn looping_rom() -> Vec<u8> {
    let mut rom = vec![0u8; 0x1000];
    rom[..12].copy_from_slice(b"SLOT TEST\0\0\0");
    rom[12..16].copy_from_slice(b"TEST");
    let word =
        |rom: &mut Vec<u8>, at: usize, v: u32| rom[at..at + 4].copy_from_slice(&v.to_le_bytes());
    word(&mut rom, 0x20, 0x200); // ARM9: offset, entry, address, size
    word(&mut rom, 0x24, 0x0200_0000);
    word(&mut rom, 0x28, 0x0200_0000);
    word(&mut rom, 0x2C, 4);
    word(&mut rom, 0x30, 0x300); // ARM7
    word(&mut rom, 0x34, 0x0380_0000);
    word(&mut rom, 0x38, 0x0380_0000);
    word(&mut rom, 0x3C, 4);
    word(&mut rom, 0x200, 0xEAFF_FFFE); // b .
    word(&mut rom, 0x300, 0xEAFF_FFFE);
    rom
}

/// A GBA cartridge with a 128 KB Flash chip.
fn gba_rom() -> Vec<u8> {
    let mut rom = vec![0u8; 0x1000];
    rom[0xA0..0xA9].copy_from_slice(b"SLOT TEST");
    rom[0xAC..0xB0].copy_from_slice(b"ABCD");
    rom[0xB2] = 0x96;
    rom[0x200..0x20C].copy_from_slice(b"FLASH1M_V103");
    rom
}

/// Programs one byte of the Flash chip through the ARM9.
fn program(nds: &mut Nds, addr: u32, value: u8) {
    let mut v = View9(&mut nds.bus);
    v.write8(0x0A00_5555, 0xAA, false);
    v.write8(0x0A00_2AAA, 0x55, false);
    v.write8(0x0A00_5555, 0xA0, false);
    v.write8(addr, value, false);
}

#[test]
fn the_cartridge_answers_the_cpu_that_owns_the_slot() {
    let mut nds = Nds::new(looping_rom(), None, None, None);
    nds.set_slot2(Slot2::cartridge(gba_rom()));
    {
        let mut v = View9(&mut nds.bus);
        assert_eq!(v.read16(0x0800_00AC, false), u16::from_le_bytes(*b"AB"));
        assert_eq!(v.read8(0x0800_00B2, false), 0x96);
        assert_eq!(v.read32(0x0800_00A0, false), u32::from_le_bytes(*b"SLOT"));
    }
    assert_eq!(View7(&mut nds.bus).read16(0x0800_00AC, false), 0, "the ARM7 reads zeros");
    // EXMEMCNT bit 7 hands the slot to the ARM7.
    View9(&mut nds.bus).write16(0x0400_0204, 0x6080, false);
    assert_eq!(View7(&mut nds.bus).read16(0x0800_00AC, false), u16::from_le_bytes(*b"AB"));
    assert_eq!(View9(&mut nds.bus).read16(0x0800_00AC, false), 0, "and the ARM9 reads zeros");
}

#[test]
fn the_save_chip_is_behind_the_ram_region() {
    let mut nds = Nds::new(looping_rom(), None, None, None);
    nds.set_slot2(Slot2::cartridge(gba_rom()));
    assert!(!nds.take_slot2_save_dirty());
    program(&mut nds, 0x0A00_0010, 0x42);
    assert_eq!(View9(&mut nds.bus).read8(0x0A00_0010, false), 0x42);
    assert_eq!(
        View9(&mut nds.bus).read16(0x0A00_0010, false),
        0x4242,
        "the bus is eight bits wide"
    );
    assert!(nds.take_slot2_save_dirty());
    assert_eq!(nds.slot2_save_data().map(|d| d[0x10]), Some(0x42));
    assert!(!nds.take_slot2_save_dirty());
}

#[test]
fn the_rumble_pak_reads_as_the_address_and_counts_toggles() {
    let mut nds = Nds::new(looping_rom(), None, None, None);
    nds.set_slot2(Slot2::rumble_pak());
    {
        let mut v = View9(&mut nds.bus);
        for i in 0..4096u32 {
            assert_eq!(v.read16(0x0800_0000 + i * 2, false), (i as u16) & 0xFFFD);
        }
    }
    assert!(!nds.take_rumble());
    View9(&mut nds.bus).write16(0x0800_0000, 2, false);
    View9(&mut nds.bus).write16(0x0800_0000, 2, false);
    assert!(nds.take_rumble(), "the first write toggled the actuator");
    assert!(!nds.take_rumble(), "the second did not");
    View9(&mut nds.bus).write16(0x0800_0000, 0, false);
    assert!(nds.take_rumble());
}

#[test]
fn an_empty_slot_reads_the_address_in_rom_and_ff_in_ram() {
    let mut nds = Nds::new(looping_rom(), None, None, None);
    let mut v = View9(&mut nds.bus);
    assert_eq!(v.read16(0x0800_0010, false), 8);
    assert_eq!(v.read32(0x0800_0000, false), 0x0001_0000);
    assert_eq!(v.read8(0x0A00_0000, false), 0xFF);
    v.write8(0x0A00_0000, 0x12, false);
    assert_eq!(v.read8(0x0A00_0000, false), 0xFF);
}

#[test]
fn a_save_state_keeps_the_cartridge() {
    let mut nds = Nds::new(looping_rom(), None, None, None);
    nds.set_slot2(Slot2::cartridge(gba_rom()));
    program(&mut nds, 0x0A00_0010, 0x42);
    let state = nds.save_state();
    program(&mut nds, 0x0A00_0011, 0x43);
    nds.load_state(&state).expect("the state loads");
    let mut v = View9(&mut nds.bus);
    assert_eq!(v.read16(0x0800_00AC, false), u16::from_le_bytes(*b"AB"), "the ROM stays");
    assert_eq!(v.read8(0x0A00_0010, false), 0x42);
    assert_eq!(v.read8(0x0A00_0011, false), 0xFF, "the later write is undone");
}
