//! The link cable: multi-play transfers between consoles run in lockstep.
//!
//! No ROM is needed. Each console runs a cartridge whose only instruction loops
//! forever, and the test pokes the serial registers the way a game would.

use pipit_gba::{Gba, Link};

const SIOMULTI0: u32 = 0x0400_0120;
const SIOCNT: u32 = 0x0400_0128;
const SIOMLT_SEND: u32 = 0x0400_012A;
const RCNT: u32 = 0x0400_0134;
const IF: u32 = 0x0400_0202;
/// SIOCNT: multi-play mode, 115200 bps, interrupt on completion.
const MULTI: u16 = 0x2000 | 0x4000 | 3;
const START: u16 = 1 << 7;
const SI: u16 = 1 << 2;
const SD: u16 = 1 << 3;
const ID: u16 = 3 << 4;

/// A cartridge that is nothing but ARM `b .` (0xEAFFFFFE).
fn console() -> Gba {
    let rom = 0xEAFF_FFFEu32.to_le_bytes().iter().copied().cycle().take(0x400).collect();
    Gba::new(rom, None)
}

fn write(gba: &mut Gba, addr: u32, value: u16) {
    gba.bus.write_io(addr, value, 0xFFFF);
}

fn read(gba: &mut Gba, addr: u32) -> u16 {
    gba.bus.read_io(addr)
}

fn enter_multiplay(gba: &mut Gba) {
    write(gba, RCNT, 0);
    write(gba, SIOCNT, MULTI);
}

fn received(gba: &mut Gba) -> [u16; 4] {
    [0, 1, 2, 3].map(|i| read(gba, SIOMULTI0 + i * 2))
}

fn serial_irq(gba: &mut Gba) -> bool {
    read(gba, IF) & (1 << 7) != 0
}

#[test]
fn a_lone_console_hears_only_itself() {
    let mut gba = console();
    enter_multiplay(&mut gba);
    assert_eq!(read(&mut gba, SIOCNT) & (SI | SD | ID), 0, "nothing plugged in");
    write(&mut gba, SIOMLT_SEND, 0x1234);
    write(&mut gba, SIOCNT, MULTI | START);
    assert_ne!(read(&mut gba, SIOCNT) & START, 0, "busy while the transfer runs");
    let now = gba.bus.scheduler.now();
    gba.run_until(now + 20_000);
    assert_eq!(read(&mut gba, SIOCNT) & START, 0);
    assert_eq!(received(&mut gba), [0x1234, 0xFFFF, 0xFFFF, 0xFFFF]);
    assert!(serial_irq(&mut gba));
}

#[test]
fn roles_follow_cable_order() {
    let mut link = Link::new(vec![console(), console(), console()]);
    for node in link.nodes_mut() {
        enter_multiplay(node);
    }
    link.run_frame();
    let [parent, first, second] = link.nodes_mut() else { unreachable!() };
    assert_eq!(read(parent, SIOCNT) & (SI | SD | ID), SD, "parent: SI clear, id 0");
    assert_eq!(read(first, SIOCNT) & (SI | SD | ID), SI | SD | (1 << 4));
    assert_eq!(read(second, SIOCNT) & (SI | SD | ID), SI | SD | (2 << 4));
}

#[test]
fn not_ready_until_everyone_is_in_multiplay_mode() {
    let mut link = Link::new(vec![console(), console()]);
    enter_multiplay(&mut link.nodes_mut()[0]);
    link.run_frame();
    assert_eq!(read(&mut link.nodes_mut()[0], SIOCNT) & SD, 0);
    enter_multiplay(&mut link.nodes_mut()[1]);
    link.run_frame();
    assert_eq!(read(&mut link.nodes_mut()[0], SIOCNT) & SD, SD);
    assert_eq!(read(&mut link.nodes_mut()[1], SIOCNT) & SD, SD);
}

#[test]
fn a_transfer_gives_everyone_every_word() {
    for count in 2..=4 {
        let mut link = Link::new((0..count).map(|_| console()).collect());
        let mut expected = [0xFFFF; 4];
        for (i, node) in link.nodes_mut().iter_mut().enumerate() {
            enter_multiplay(node);
            expected[i] = 0x1111 * (i as u16 + 1);
            write(node, SIOMLT_SEND, expected[i]);
        }
        write(&mut link.nodes_mut()[0], SIOCNT, MULTI | START);
        link.run_frame();
        assert_eq!(link.transfers(), 1, "{count} consoles");
        for node in link.nodes_mut() {
            assert_eq!(received(node), expected, "{count} consoles");
            assert_eq!(read(node, SIOCNT) & START, 0, "{count} consoles: done");
            assert!(serial_irq(node), "{count} consoles: interrupt");
        }
        assert_eq!(link.take_transfers(), vec![expected]);
    }
}

#[test]
fn children_cannot_start_transfers() {
    let mut link = Link::new(vec![console(), console()]);
    for node in link.nodes_mut() {
        enter_multiplay(node);
    }
    write(&mut link.nodes_mut()[1], SIOCNT, MULTI | START);
    link.run_frame();
    assert_eq!(link.transfers(), 0);
    assert_eq!(read(&mut link.nodes_mut()[1], SIOCNT) & START, 0);
}

#[test]
fn words_are_latched_when_the_transfer_starts() {
    let mut link = Link::new(vec![console(), console()]);
    for node in link.nodes_mut() {
        enter_multiplay(node);
    }
    write(&mut link.nodes_mut()[1], SIOMLT_SEND, 0xBEEF);
    write(&mut link.nodes_mut()[0], SIOCNT, MULTI | START);
    let now = link.nodes()[0].bus.scheduler.now();
    // The start has been handled, the transfer is still in flight.
    link.run_until(now + 100);
    assert_ne!(read(&mut link.nodes_mut()[1], SIOCNT) & START, 0, "child sees busy");
    write(&mut link.nodes_mut()[1], SIOMLT_SEND, 0xDEAD);
    link.run_until(now + 20_000);
    assert_eq!(received(&mut link.nodes_mut()[0]), [0, 0xBEEF, 0xFFFF, 0xFFFF]);
}

#[test]
fn one_transfer_at_a_time() {
    let mut link = Link::new(vec![console(), console()]);
    for node in link.nodes_mut() {
        enter_multiplay(node);
    }
    write(&mut link.nodes_mut()[0], SIOCNT, MULTI | START);
    write(&mut link.nodes_mut()[0], SIOCNT, MULTI | START);
    link.run_frame();
    assert_eq!(link.transfers(), 1, "a start while busy is ignored");
}
