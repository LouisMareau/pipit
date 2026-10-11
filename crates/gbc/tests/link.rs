//! The link cable between two Game Boys. No ROM is needed: each console runs a
//! cartridge whose only instruction loops forever (`jr -2`), and the test pokes
//! the serial registers the way a game would.

use pipit_gbc::{Gbc, Link};

const SB: u16 = 0xFF01;
const SC: u16 = 0xFF02;
const IF: u16 = 0xFF0F;
const SERIAL_IRQ: u8 = 1 << 3;

fn console() -> Gbc {
    let mut rom = vec![0u8; 0x8000];
    for i in (0x100..0x8000).step_by(2) {
        rom[i] = 0x18;
        rom[i + 1] = 0xFE;
    }
    Gbc::new(rom)
}

fn write(gbc: &mut Gbc, addr: u16, value: u8) {
    gbc.bus.write(addr, value);
}

fn read(gbc: &mut Gbc, addr: u16) -> u8 {
    gbc.bus.peek(addr)
}

#[test]
fn alone_a_transfer_brings_back_ff() {
    let mut gbc = console();
    write(&mut gbc, SB, 0x42);
    write(&mut gbc, SC, 0x81);
    gbc.run_frame();
    assert_eq!(read(&mut gbc, SB), 0xFF);
    assert_eq!(read(&mut gbc, SC) & 0x80, 0, "transfer finished");
    assert_ne!(read(&mut gbc, IF) & SERIAL_IRQ, 0);
}

#[test]
fn the_clocking_console_swaps_bytes_with_the_other() {
    let mut link = Link::new(vec![console(), console()]);
    write(&mut link.nodes_mut()[1], SB, 0x34);
    write(&mut link.nodes_mut()[1], SC, 0x80);
    write(&mut link.nodes_mut()[0], SB, 0x12);
    write(&mut link.nodes_mut()[0], SC, 0x81);
    link.run_frame();
    assert_eq!(link.transfers(), 1);
    assert_eq!(link.take_transfers(), vec![[0x12, 0x34]]);
    let [master, slave] = link.nodes_mut() else { unreachable!() };
    assert_eq!(read(master, SB), 0x34);
    assert_eq!(read(slave, SB), 0x12);
    assert_eq!(read(master, SC) & 0x80, 0);
    assert_eq!(read(slave, SC) & 0x80, 0);
    assert_ne!(read(master, IF) & SERIAL_IRQ, 0);
    assert_ne!(read(slave, IF) & SERIAL_IRQ, 0);
}

#[test]
fn either_console_may_clock() {
    let mut link = Link::new(vec![console(), console()]);
    write(&mut link.nodes_mut()[0], SB, 0xAA);
    write(&mut link.nodes_mut()[0], SC, 0x80);
    write(&mut link.nodes_mut()[1], SB, 0x55);
    write(&mut link.nodes_mut()[1], SC, 0x81);
    link.run_frame();
    assert_eq!(link.take_transfers(), vec![[0x55, 0xAA]]);
    assert_eq!(read(&mut link.nodes_mut()[0], SB), 0x55);
    assert_eq!(read(&mut link.nodes_mut()[1], SB), 0xAA);
}

#[test]
fn an_unexpecting_partner_gets_the_byte_but_no_interrupt() {
    let mut link = Link::new(vec![console(), console()]);
    write(&mut link.nodes_mut()[1], SB, 0x77);
    write(&mut link.nodes_mut()[0], SB, 0x11);
    write(&mut link.nodes_mut()[0], SC, 0x81);
    link.run_frame();
    assert_eq!(read(&mut link.nodes_mut()[1], SB), 0x11);
    assert_eq!(read(&mut link.nodes_mut()[1], IF) & SERIAL_IRQ, 0);
    assert_eq!(read(&mut link.nodes_mut()[0], SB), 0x77);
}

#[test]
fn a_link_state_restores_both_consoles() {
    let mut link = Link::new(vec![console(), console()]);
    write(&mut link.nodes_mut()[1], SB, 0x34);
    write(&mut link.nodes_mut()[1], SC, 0x80);
    write(&mut link.nodes_mut()[0], SB, 0x12);
    write(&mut link.nodes_mut()[0], SC, 0x81);
    let state = link.save_state();
    link.run_frame();
    let expected = link.state_hash();
    let mut copy = Link::new(vec![console(), console()]);
    copy.load_state(&state).expect("state loads");
    copy.run_frame();
    assert_eq!(copy.state_hash(), expected);
    assert_eq!(copy.take_transfers(), vec![[0x12, 0x34]]);
}
