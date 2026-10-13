//! Consoles within range of each other: a data frame from one lands in the
//! other's receive buffer, and the hardware's multiplayer exchange (the host's
//! command, the client's reply, the host's acknowledgement) runs on its own.

use pipit_nds::memory::ARM7;
use pipit_nds::{Link, Nds};

const W_IF: usize = 0x010;
const W_IE: usize = 0x012;
const W_MACADDR: usize = 0x018;
const W_BSSID: usize = 0x020;
const W_AID_LOW: usize = 0x028;
const W_RXCNT: usize = 0x030;
const W_RXBUF_BEGIN: usize = 0x050;
const W_RXBUF_END: usize = 0x052;
const W_RXBUF_WR_ADDR: usize = 0x056;
const W_TXBUF_CMD: usize = 0x090;
const W_TXBUF_REPLY1: usize = 0x094;
const W_TXBUF_REPLY2: usize = 0x098;
const W_TXBUF_LOC1: usize = 0x0A0;
const W_TXREQ_SET: usize = 0x0AE;

const MAC_A: [u8; 6] = [0x02, 0, 0, 0, 0, 1];
const MAC_B: [u8; 6] = [0x02, 0, 0, 0, 0, 2];
/// The multicast address of a host's command.
const CMD: [u8; 6] = [0x03, 0x09, 0xBF, 0, 0, 0];

/// A card with programs that only loop.
fn looping_rom() -> Vec<u8> {
    let mut rom = vec![0u8; 0x1000];
    rom[..12].copy_from_slice(b"WIFI TEST\0\0\0");
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

fn pair() -> Link {
    let a = Nds::new(looping_rom(), None, None, None);
    let b = Nds::new(looping_rom(), None, None, None);
    Link::new(vec![a, b])
}

fn reg(nds: &mut Nds, off: usize, value: u16) {
    let bus = &mut nds.bus;
    bus.wifi.write16(off, value, 0, &mut bus.scheduler);
}

fn read(nds: &mut Nds, off: usize) -> u16 {
    nds.bus.wifi.read16(off, 0)
}

/// Receives into 4C00h–5F60h of the wireless RAM, like the usual drivers.
fn listen(nds: &mut Nds, mac: [u8; 6]) {
    for i in 0..3 {
        reg(nds, W_MACADDR + i * 2, u16::from_le_bytes([mac[i * 2], mac[i * 2 + 1]]));
    }
    reg(nds, W_RXBUF_BEGIN, 0x4C00);
    reg(nds, W_RXBUF_END, 0x5F60);
    reg(nds, W_RXBUF_WR_ADDR, 0x0600);
    reg(nds, W_RXCNT, 0x8080);
    reg(nds, W_IE, 0xFFFF);
}

/// Puts a frame, behind its transmit header, at `at` in the wireless RAM.
fn put_frame(nds: &mut Nds, at: usize, control: u16, to: [u8; 6], from: [u8; 6], body: &[u8]) {
    let mut bytes = vec![0u8; 12];
    bytes[8] = 0x14;
    let len = (24 + body.len() + 4) as u16;
    bytes[10..12].copy_from_slice(&len.to_le_bytes());
    bytes.extend_from_slice(&control.to_le_bytes());
    bytes.extend_from_slice(&[0, 0]);
    bytes.extend_from_slice(&to);
    bytes.extend_from_slice(&from);
    bytes.extend_from_slice(&from);
    bytes.extend_from_slice(&[0, 0]);
    bytes.extend_from_slice(body);
    nds.bus.wifi.ram[at..at + bytes.len()].copy_from_slice(&bytes);
}

/// The received frame at `at` in the buffer: its length and its body.
fn received(nds: &Nds, at: usize) -> (u16, &[u8]) {
    let rx = &nds.bus.wifi.ram[0xC00 + at..];
    let len = u16::from_le_bytes([rx[10], rx[11]]);
    (len, &rx[12 + 24..12 + usize::from(len) - 4])
}

#[test]
fn a_data_frame_reaches_the_other_console() {
    let mut link = pair();
    listen(&mut link.nodes_mut()[0], MAC_A);
    listen(&mut link.nodes_mut()[1], MAC_B);
    let a = &mut link.nodes_mut()[0];
    put_frame(a, 0, 0x0008, [0xFF; 6], MAC_A, b"hello");
    reg(a, W_TXBUF_LOC1, 0x8000);
    reg(a, W_TXREQ_SET, 0x0001);
    link.run_frame();

    let b = &mut link.nodes_mut()[1];
    assert_eq!(read(b, W_IF) & 1, 1, "receive complete");
    assert_ne!(b.bus.irq[ARM7].if_ & (1 << 24), 0, "the ARM7's wireless interrupt");
    let (len, body) = received(b, 0);
    assert_eq!(len, 24 + 5 + 4, "the length in the receive header counts the FCS");
    assert_eq!(body, b"hello");
    assert_eq!(link.frames(), 1);
    let a = &mut link.nodes_mut()[0];
    assert_eq!(read(a, W_IF) & 1, 0, "the sender does not hear itself");
    assert_eq!(a.bus.wifi.ram[0], 1, "the transmit header's status: sent");
    assert_eq!(read(a, W_TXBUF_LOC1) & 0x8000, 0, "a location sends once");
}

#[test]
fn the_host_command_collects_the_reply_and_acknowledges() {
    let mut link = pair();
    listen(&mut link.nodes_mut()[0], MAC_A);
    listen(&mut link.nodes_mut()[1], MAC_B);
    // B: client 1 of A's network, with a reply ready.
    let b = &mut link.nodes_mut()[1];
    reg(b, W_AID_LOW, 1);
    for i in 0..3 {
        reg(b, W_BSSID + i * 2, u16::from_le_bytes([MAC_A[i * 2], MAC_A[i * 2 + 1]]));
    }
    put_frame(b, 0x1000, 0x0158, MAC_A, MAC_B, b"pong");
    reg(b, W_TXBUF_REPLY1, 0x8000 | (0x1000 / 2));
    // A: a command to client 1 (client time, client mask, data).
    let a = &mut link.nodes_mut()[0];
    put_frame(a, 0, 0x0228, CMD, MAC_A, &[0x10, 0x00, 0x02, 0x00, b'p', b'i', b'n', b'g']);
    reg(a, W_TXBUF_CMD, 0x8000);
    reg(a, W_TXREQ_SET, 0x0002);
    link.run_frame();

    assert_eq!(link.frames(), 3, "command, reply, acknowledgement");
    let a = &mut link.nodes_mut()[0];
    assert_eq!(read(a, W_IF) & (1 | 1 << 12), 1 | 1 << 12, "reply received, exchange done");
    assert_eq!(a.bus.wifi.ram[0..2], [0x03, 0x00], "the command's status: sent, client 1 answered");
    assert_eq!(received(a, 0).1, b"pong");
    let b = &mut link.nodes_mut()[1];
    let (len, body) = received(b, 0);
    assert_eq!(&body[4..], b"ping", "the command's data");
    let ack = 12 + usize::from(len);
    let control =
        u16::from_le_bytes([b.bus.wifi.ram[0xC00 + ack + 12], b.bus.wifi.ram[0xC00 + ack + 13]]);
    assert_eq!(control, 0x0218, "the acknowledgement follows in the buffer");
    assert_eq!(read(b, W_TXBUF_REPLY1), 0, "the reply buffer was consumed");
    assert_eq!(read(b, W_TXBUF_REPLY2), 0x8800, "and moved to the second slot");
}
