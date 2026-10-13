//! The wireless hardware (GBATEK "DS Wireless Communications"), as far as
//! local play between consoles needs it: the registers at 04800000h, the
//! 8 KB of wireless RAM with the TX and RX buffers, the microsecond
//! counter with its beacon timing, and the multiplayer exchange the
//! hardware automates (a host's CMD frame, the clients' REPLY frames in
//! answer, the host's ACK). Frames go out through `take_outbox` and come
//! in through `receive`; `crate::link::Link` is the air between consoles.
//!
//! The 802.11 radio is not modelled: frames arrive whole and at once, and
//! the baseband and RF chips only remember what was written to them.

use crate::scheduler::{Event, Scheduler};
use serde::{Deserialize, Serialize};

pub const RAM_SIZE: usize = 0x2000;
/// ARM9 cycles per microsecond of the wireless counters (two per bus cycle).
pub const CYCLES_PER_US: u64 = 67;

// Register offsets, in bytes from 04800000h.
const W_ID: usize = 0x000;
const W_MODE_RST: usize = 0x004;
const W_IF: usize = 0x010;
const W_IE: usize = 0x012;
const W_MACADDR: usize = 0x018;
const W_BSSID: usize = 0x020;
const W_AID_LOW: usize = 0x028;
const W_RXCNT: usize = 0x030;
const W_POWERSTATE: usize = 0x040;
const W_RANDOM: usize = 0x044;
const W_POWERFORCE: usize = 0x048;
const W_RXBUF_BEGIN: usize = 0x050;
const W_RXBUF_END: usize = 0x052;
const W_RXBUF_WRCSR: usize = 0x054;
const W_RXBUF_WR_ADDR: usize = 0x056;
const W_RXBUF_RD_ADDR: usize = 0x058;
const W_RXBUF_READCSR: usize = 0x05A;
const W_RXBUF_COUNT: usize = 0x05C;
const W_RXBUF_RD_DATA: usize = 0x060;
const W_RXBUF_GAP: usize = 0x062;
const W_RXBUF_GAPDISP: usize = 0x064;
const W_TXBUF_WR_ADDR: usize = 0x068;
const W_TXBUF_COUNT: usize = 0x06C;
const W_TXBUF_WR_DATA: usize = 0x070;
const W_TXBUF_GAP: usize = 0x074;
const W_TXBUF_GAPDISP: usize = 0x076;
const W_TXBUF_BEACON: usize = 0x080;
const W_BEACONINT: usize = 0x08C;
const W_TXBUF_CMD: usize = 0x090;
const W_TXBUF_REPLY1: usize = 0x094;
const W_TXBUF_REPLY2: usize = 0x098;
const W_TXBUF_LOC1: usize = 0x0A0;
const W_TXBUF_LOC2: usize = 0x0A4;
const W_TXBUF_LOC3: usize = 0x0A8;
const W_TXREQ_RESET: usize = 0x0AC;
const W_TXREQ_SET: usize = 0x0AE;
const W_TXREQ_READ: usize = 0x0B0;
const W_TXBUF_RESET: usize = 0x0B4;
const W_TXBUSY: usize = 0x0B6;
const W_TXSTAT: usize = 0x0B8;
const W_CMD_TOTALTIME: usize = 0x0C0;
const W_US_COUNTCNT: usize = 0x0E8;
const W_US_COMPARECNT: usize = 0x0EA;
const W_US_COMPARE: usize = 0x0F0;
const W_US_COUNT: usize = 0x0F8;
const W_PRE_BEACON: usize = 0x110;
const W_POST_BEACON: usize = 0x130;
const W_BB_CNT: usize = 0x158;
const W_BB_WRITE: usize = 0x15A;
const W_BB_READ: usize = 0x15C;
const W_BB_BUSY: usize = 0x15E;
const W_RF_DATA2: usize = 0x17C;
const W_RF_DATA1: usize = 0x17E;
const W_RF_BUSY: usize = 0x180;
const W_RF_PINS: usize = 0x194;
const W_RF_STATUS: usize = 0x20C;
const W_IF_SET: usize = 0x210;

// W_IF bits.
const IRQ_RX_DONE: u16 = 1 << 0;
const IRQ_TX_DONE: u16 = 1 << 1;
const IRQ_TXBUF_COUNT: u16 = 1 << 8;
const IRQ_RXBUF_COUNT: u16 = 1 << 9;
const IRQ_RF_WAKEUP: u16 = 1 << 11;
const IRQ_MP_DONE: u16 = 1 << 12;
const IRQ_POST_BEACON: u16 = 1 << 13;
const IRQ_BEACON: u16 = 1 << 14;
const IRQ_PRE_BEACON: u16 = 1 << 15;

/// The multicast addresses the multiplayer frames go to.
pub const CMD_ADDRESS: [u8; 6] = [0x03, 0x09, 0xBF, 0x00, 0x00, 0x00];
pub const ACK_ADDRESS: [u8; 6] = [0x03, 0x09, 0xBF, 0x00, 0x00, 0x10];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Beacon,
    Data,
    Cmd,
    Reply,
    Ack,
}

/// A frame on the air: the 802.11 header and body, without the FCS.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Frame {
    pub kind: Kind,
    pub data: Vec<u8>,
    /// The sender's association id (its AID), which real receivers would
    /// work out from the addresses; the clients' replies carry it.
    pub from_aid: u16,
    /// The TX rate byte from the sender's header (0Ah 1 Mbit, 14h 2 Mbit).
    pub rate: u8,
}

/// A CMD frame's progress on the host: which clients still have to reply.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct Command {
    expected: u16,
    received: u16,
    /// Where the CMD frame's TX header is, for its status.
    header: usize,
    deadline: u64,
}

#[derive(Serialize, Deserialize)]
pub struct Wifi {
    #[serde(with = "pipit_common::snapshot::bytes_box")]
    pub ram: Box<[u8; RAM_SIZE]>,
    #[serde(with = "pipit_common::snapshot::array")]
    regs: [u16; 0x200],
    #[serde(with = "pipit_common::snapshot::bytes_box")]
    bb: Box<[u8; 0x69]>,
    #[serde(with = "pipit_common::snapshot::array")]
    rf: [u32; 0x40],
    /// The microsecond counter: its value when it last changed, and when.
    us_count: u64,
    us_cycles: u64,
    us_running: bool,
    random: u16,
    /// Frames sent since the link last took them.
    outbox: Vec<Frame>,
    /// A client's reply is due at the next exchange.
    reply_due: bool,
    command: Option<Command>,
    /// When the post-beacon interrupt is due, in cycles.
    post_beacon_at: Option<u64>,
    /// Whether the interrupt line was up when last checked (it is edge-triggered).
    irq_level: bool,
}

impl Default for Wifi {
    fn default() -> Self {
        Self::new()
    }
}

impl Wifi {
    pub fn new() -> Self {
        let mut wifi = Self {
            ram: crate::boxed(0),
            regs: [0; 0x200],
            bb: crate::boxed(0),
            rf: [0; 0x40],
            us_count: 0,
            us_cycles: 0,
            us_running: false,
            random: 0x7FFF,
            outbox: Vec::new(),
            reply_due: false,
            command: None,
            post_beacon_at: None,
            irq_level: false,
        };
        wifi.regs[W_ID / 2] = 0x1440;
        wifi.regs[W_POWERSTATE / 2] = 0x0200;
        wifi.regs[W_RF_STATUS / 2] = 0x0009;
        wifi.regs[W_RF_PINS / 2] = 0x0004;
        wifi.bb[0] = 0x6D;
        wifi.reset_config();
        wifi
    }

    /// The registers W_MODE_RST's bit 14 resets.
    fn reset_config(&mut self) {
        for (reg, value) in [
            (0x02C, 0x0707),
            (W_RXBUF_BEGIN, 0x4000),
            (W_RXBUF_END, 0x4800),
            (0x0BC, 0x0001),
            (0x0C8, 0x0401),
            (0x0D4, 0x0001),
            (0x0E0, 0x0008),
            (0x0EC, 0x3F03),
            (0x1A2, 0x0001),
            (0x224, 0x0003),
            (0x230, 0x0047),
        ] {
            self.regs[reg / 2] = value;
        }
    }

    #[inline]
    fn reg(&self, off: usize) -> u16 {
        self.regs[off / 2]
    }

    #[inline]
    fn set(&mut self, off: usize, value: u16) {
        self.regs[off / 2] = value;
    }

    pub fn mac(&self) -> [u8; 6] {
        let mut mac = [0; 6];
        for i in 0..3 {
            mac[i * 2..i * 2 + 2].copy_from_slice(&self.reg(W_MACADDR + i * 2).to_le_bytes());
        }
        mac
    }

    pub fn bssid(&self) -> [u8; 6] {
        let mut id = [0; 6];
        for i in 0..3 {
            id[i * 2..i * 2 + 2].copy_from_slice(&self.reg(W_BSSID + i * 2).to_le_bytes());
        }
        id
    }

    pub fn aid(&self) -> u16 {
        self.reg(W_AID_LOW) & 0xF
    }

    /// Whether the ARM7's wireless interrupt line is up.
    pub fn irq_line(&self) -> bool {
        self.reg(W_IF) & self.reg(W_IE) != 0
    }

    /// Whether the interrupt line rose since the last call.
    pub fn take_irq_edge(&mut self) -> bool {
        let level = self.irq_line();
        let rose = level && !self.irq_level;
        self.irq_level = level;
        rose
    }

    fn flag(&mut self, bits: u16) {
        let v = self.reg(W_IF) | bits;
        self.set(W_IF, v);
    }

    /// Frames sent since the last call, oldest first.
    pub fn take_outbox(&mut self) -> Vec<Frame> {
        std::mem::take(&mut self.outbox)
    }

    /// Puts a frame on the air. A console on its own keeps only the last few:
    /// nothing ever takes them.
    fn send(&mut self, frame: Frame) {
        if self.outbox.len() >= 64 {
            self.outbox.remove(0);
        }
        self.outbox.push(frame);
    }

    // ---------------------------------------------------------------------
    // The microsecond counter and the beacon timing
    // ---------------------------------------------------------------------

    fn us_now(&self, now: u64) -> u64 {
        if self.us_running {
            self.us_count + (now.saturating_sub(self.us_cycles)) / CYCLES_PER_US
        } else {
            self.us_count
        }
    }

    fn us_compare(&self) -> u64 {
        (0..4).fold(0u64, |acc, i| acc | u64::from(self.reg(W_US_COMPARE + i * 2)) << (i * 16))
    }

    fn set_us_compare(&mut self, value: u64) {
        for i in 0..4 {
            self.set(W_US_COMPARE + i * 2, (value >> (i * 16)) as u16);
        }
    }

    /// Latches the counter at `now` (before a change to how it runs).
    fn latch_us(&mut self, now: u64) {
        self.us_count = self.us_now(now);
        self.us_cycles = now;
    }

    /// Schedules the next timer event: the compare match (the beacon), the
    /// pre-beacon warning, or the post-beacon countdown, whichever is first.
    pub fn reschedule(&mut self, now: u64, scheduler: &mut Scheduler) {
        scheduler.cancel(Event::WifiTimer);
        let mut next: Option<u64> = self.post_beacon_at;
        if self.us_running && self.reg(W_US_COMPARECNT) & 1 != 0 {
            let count = self.us_now(now);
            let compare = self.us_compare();
            let at = |us: u64| now + us.saturating_sub(count) * CYCLES_PER_US;
            let beacon_at = at(compare);
            next = Some(next.map_or(beacon_at, |n| n.min(beacon_at)));
            let pre = u64::from(self.reg(W_PRE_BEACON));
            if pre > 0 && compare > count + pre {
                let pre_at = at(compare - pre);
                next = Some(next.map_or(pre_at, |n| n.min(pre_at)));
            }
        }
        if let Some(at) = next {
            scheduler.schedule_at(Event::WifiTimer, at.max(now + 1));
        }
    }

    /// A timer event came due.
    pub fn on_timer(&mut self, now: u64, scheduler: &mut Scheduler) {
        if let Some(at) = self.post_beacon_at {
            if now >= at {
                self.post_beacon_at = None;
                self.flag(IRQ_POST_BEACON);
            }
        }
        if self.us_running && self.reg(W_US_COMPARECNT) & 1 != 0 {
            let count = self.us_now(now);
            let compare = self.us_compare();
            let pre = u64::from(self.reg(W_PRE_BEACON));
            if pre > 0 && count + pre >= compare && count < compare {
                self.flag(IRQ_PRE_BEACON);
            }
            if count >= compare {
                self.flag(IRQ_BEACON);
                let beacon = self.reg(W_TXBUF_BEACON);
                if beacon & 0x8000 != 0 {
                    self.transmit(usize::from(beacon & 0xFFF) * 2, Kind::Beacon, now);
                }
                let post = u64::from(self.reg(W_POST_BEACON));
                if post > 0 {
                    self.post_beacon_at = Some(now + post * CYCLES_PER_US);
                }
                // The next beacon: the interval is in units of 1024 µs.
                let interval = (u64::from(self.reg(W_BEACONINT)) << 10).max(1024);
                let mut next = compare + interval;
                while next <= count {
                    next += interval;
                }
                self.set_us_compare(next);
            }
        }
        self.reschedule(now, scheduler);
    }

    // ---------------------------------------------------------------------
    // Transmitting
    // ---------------------------------------------------------------------

    /// Sends the frame whose 12-byte TX header is at `header` in the RAM.
    fn transmit(&mut self, header: usize, kind: Kind, now: u64) {
        let header = header & 0x1FFE;
        let len = usize::from(u16::from_le_bytes([
            self.ram[(header + 0xA) & 0x1FFF],
            self.ram[(header + 0xB) & 0x1FFF],
        ]));
        // The length counts the 4-byte FCS, which is not stored.
        let body_len = len.saturating_sub(4).min(RAM_SIZE);
        let mut data = Vec::with_capacity(body_len);
        for i in 0..body_len {
            data.push(self.ram[(header + 12 + i) & 0x1FFF]);
        }
        let rate = self.ram[(header + 8) & 0x1FFF];
        // The status halfword of the header: sent.
        self.ram[header] = 1;
        self.ram[header + 1] = 0;
        self.send(Frame { kind, data, from_aid: self.aid(), rate });
        let slot = match kind {
            Kind::Beacon => 0x0,
            Kind::Cmd => 0x1,
            Kind::Reply => 0x2,
            Kind::Ack => 0x3,
            Kind::Data => 0x4,
        };
        self.set(W_TXSTAT, 0x0001 | slot << 12);
        if kind == Kind::Cmd {
            // The clients named in the first halfword of the body must reply.
            // The body starts with the client time and the client mask.
            let at = (header + 12 + 26) & 0x1FFE;
            let expected = u16::from_le_bytes([self.ram[at], self.ram[at + 1]]) & 0xFFFE;
            let total = u64::from(self.reg(W_CMD_TOTALTIME)).max(100) * 10;
            self.command = Some(Command {
                expected,
                received: 0,
                header,
                deadline: now + total * CYCLES_PER_US,
            });
            if expected == 0 {
                self.finish_command();
            }
        } else {
            self.flag(IRQ_TX_DONE);
        }
    }

    /// A transmit request: every enabled slot in `req` goes out now.
    fn request(&mut self, req: u16, now: u64) {
        for (bit, reg, kind) in [
            (0x0001, W_TXBUF_LOC1, Kind::Data),
            (0x0004, W_TXBUF_LOC2, Kind::Data),
            (0x0008, W_TXBUF_LOC3, Kind::Data),
            (0x0002, W_TXBUF_CMD, Kind::Cmd),
        ] {
            let buf = self.reg(reg);
            if req & bit == 0 || buf & 0x8000 == 0 {
                continue;
            }
            if kind == Kind::Cmd && self.command.is_some() {
                continue;
            }
            self.transmit(usize::from(buf & 0xFFF) * 2, kind, now);
            // A location sends once; the CMD buffer stays armed.
            if kind == Kind::Data {
                self.set(reg, buf & 0x7FFF);
            }
        }
    }

    /// The host's CMD is complete: every reply came, or the time ran out.
    /// The hardware's ACK goes out and the game is told.
    fn finish_command(&mut self) {
        let Some(cmd) = self.command.take() else { return };
        let mut data = Vec::with_capacity(28);
        data.extend_from_slice(&0x0218u16.to_le_bytes());
        data.extend_from_slice(&0u16.to_le_bytes());
        data.extend_from_slice(&ACK_ADDRESS);
        data.extend_from_slice(&self.mac());
        data.extend_from_slice(&self.bssid());
        data.extend_from_slice(&0u16.to_le_bytes());
        data.extend_from_slice(&cmd.received.to_le_bytes());
        data.extend_from_slice(&0u16.to_le_bytes());
        self.send(Frame { kind: Kind::Ack, data, from_aid: 0, rate: 0x14 });
        // The CMD header's status carries which clients answered.
        let status = 0x0001 | (cmd.received & 0xFFFE);
        self.ram[cmd.header..cmd.header + 2].copy_from_slice(&status.to_le_bytes());
        self.set(W_TXSTAT, 0x0B01);
        self.flag(IRQ_TX_DONE | IRQ_MP_DONE);
    }

    /// Time passes: a CMD whose replies are late completes with what came.
    pub fn poll(&mut self, now: u64) {
        if self.command.is_some_and(|c| now >= c.deadline) {
            self.finish_command();
        }
        if self.reply_due {
            self.reply_due = false;
            let reply = self.reg(W_TXBUF_REPLY1);
            if reply & 0x8000 != 0 {
                self.transmit(usize::from(reply & 0xFFF) * 2, Kind::Reply, now);
                // The buffer moves to the second slot once sent.
                self.set(W_TXBUF_REPLY2, reply);
                self.set(W_TXBUF_REPLY1, 0);
            } else {
                // Nothing to say: an empty reply keeps the host's count right.
                let mut data = Vec::with_capacity(24);
                data.extend_from_slice(&0x0158u16.to_le_bytes());
                data.extend_from_slice(&0u16.to_le_bytes());
                data.extend_from_slice(&self.bssid());
                data.extend_from_slice(&self.mac());
                data.extend_from_slice(&self.bssid());
                data.extend_from_slice(&0u16.to_le_bytes());
                self.send(Frame { kind: Kind::Reply, data, from_aid: self.aid(), rate: 0x14 });
            }
        }
    }

    // ---------------------------------------------------------------------
    // Receiving
    // ---------------------------------------------------------------------

    /// A frame from the air. Returns whether anything changed for the game.
    pub fn receive(&mut self, frame: &Frame) -> bool {
        if self.reg(W_RXCNT) & 0x8000 == 0 || frame.data.len() < 24 {
            return false;
        }
        let addr1: [u8; 6] = frame.data[4..10].try_into().unwrap();
        let mine = addr1 == self.mac() || addr1 == [0xFF; 6] || addr1[..3] == [0x03, 0x09, 0xBF];
        if !mine {
            return false;
        }
        match frame.kind {
            Kind::Cmd => {
                let mask = match frame.data.get(26..28) {
                    Some(m) => u16::from_le_bytes([m[0], m[1]]),
                    None => 0,
                };
                if self.aid() != 0 && mask & (1 << self.aid()) != 0 {
                    self.reply_due = true;
                }
            }
            Kind::Reply => {
                if let Some(cmd) = &mut self.command {
                    cmd.received |= 1 << (frame.from_aid & 0xF);
                    if cmd.received & cmd.expected == cmd.expected {
                        self.finish_command();
                    }
                }
            }
            _ => {}
        }
        self.store(frame);
        true
    }

    /// Writes a frame into the RX buffer behind its 12-byte header.
    fn store(&mut self, frame: &Frame) {
        let flags: u16 = match frame.kind {
            Kind::Beacon => 0x8010,
            Kind::Data => 0x8018,
            Kind::Cmd => 0x8028,
            Kind::Reply => 0x8014,
            Kind::Ack => 0x802C,
        };
        let len = (frame.data.len() + 4) as u16;
        let header = [flags, 0x0040, 0x0000, 0x0010, u16::from(frame.rate), len];
        let mut words: Vec<u16> = header.to_vec();
        let mut bytes = frame.data.clone();
        bytes.extend_from_slice(&[0; 4]);
        if !bytes.len().is_multiple_of(2) {
            bytes.push(0);
        }
        words.extend(bytes.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])));
        let begin = usize::from(self.reg(W_RXBUF_BEGIN) & 0x1FFE) / 2;
        let end = usize::from(self.reg(W_RXBUF_END) & 0x1FFE) / 2;
        let mut csr = usize::from(self.reg(W_RXBUF_WRCSR) & 0xFFF);
        for w in words {
            self.ram[(csr * 2) & 0x1FFF] = w as u8;
            self.ram[(csr * 2 + 1) & 0x1FFF] = (w >> 8) as u8;
            csr += 1;
            if csr >= end && end > begin {
                csr = begin;
            }
            csr &= 0xFFF;
            let count = self.reg(W_RXBUF_COUNT);
            if count > 0 {
                self.set(W_RXBUF_COUNT, count - 1);
                if count == 1 {
                    self.flag(IRQ_RXBUF_COUNT);
                }
            }
        }
        // Frames start on word boundaries.
        csr = (csr + 1) & !1;
        if csr >= end && end > begin {
            csr = begin;
        }
        self.set(W_RXBUF_WRCSR, csr as u16);
        self.flag(IRQ_RX_DONE);
    }

    // ---------------------------------------------------------------------
    // Registers
    // ---------------------------------------------------------------------

    /// A halfword at `off` (0–7FFFh from 04800000h); the RAM is at 4000h.
    pub fn read16(&mut self, off: usize, now: u64) -> u16 {
        if (0x4000..0x6000).contains(&off) {
            let i = off - 0x4000;
            return u16::from_le_bytes([self.ram[i], self.ram[i + 1]]);
        }
        if off >= 0x400 {
            return 0;
        }
        match off {
            W_RXBUF_RD_DATA => {
                let csr = usize::from(self.reg(W_RXBUF_READCSR) & 0xFFF);
                let value = u16::from_le_bytes([
                    self.ram[(csr * 2) & 0x1FFF],
                    self.ram[(csr * 2 + 1) & 0x1FFF],
                ]);
                let mut next = csr + 1;
                let end = usize::from(self.reg(W_RXBUF_END) & 0x1FFE) / 2;
                let begin = usize::from(self.reg(W_RXBUF_BEGIN) & 0x1FFE) / 2;
                if next >= end && end > begin {
                    next = begin;
                }
                let gap = usize::from(self.reg(W_RXBUF_GAP) & 0x1FFE) / 2;
                if next == gap && gap != 0 {
                    next += usize::from(self.reg(W_RXBUF_GAPDISP) & 0xFFF);
                    if next >= end && end > begin {
                        next -= end - begin;
                    }
                }
                self.set(W_RXBUF_READCSR, (next & 0xFFF) as u16);
                value
            }
            W_RANDOM => {
                self.random = (self.random >> 1) ^ if self.random & 1 != 0 { 0x6000 } else { 0 };
                self.random & 0x7FF
            }
            W_US_COUNT..=0x0FE => (self.us_now(now) >> ((off - W_US_COUNT) * 8)) as u16,
            W_TXREQ_READ => self.reg(W_TXREQ_READ) | 0x0010,
            W_TXBUSY | W_BB_BUSY | W_RF_BUSY => 0,
            _ => self.reg(off),
        }
    }

    pub fn read8(&mut self, off: usize, now: u64) -> u8 {
        (self.read16(off & !1, now) >> ((off & 1) * 8)) as u8
    }

    pub fn write16(&mut self, off: usize, value: u16, now: u64, scheduler: &mut Scheduler) {
        if (0x4000..0x6000).contains(&off) {
            let i = off - 0x4000;
            self.ram[i..i + 2].copy_from_slice(&value.to_le_bytes());
            return;
        }
        if off >= 0x400 {
            return;
        }
        match off {
            W_ID | W_RXBUF_WRCSR | W_TXREQ_READ | W_TXBUSY | W_TXSTAT | W_BB_READ | W_RF_STATUS => {
            }
            W_MODE_RST => {
                if value & 0x2000 != 0 {
                    self.set(W_RXBUF_WR_ADDR, 0);
                    self.set(W_CMD_TOTALTIME, 0);
                }
                if value & 0x4000 != 0 {
                    for reg in [0x006, 0x008, 0x00A, W_AID_LOW, 0x02A] {
                        self.set(reg, 0);
                    }
                    for i in 0..3 {
                        self.set(W_MACADDR + i * 2, 0);
                        self.set(W_BSSID + i * 2, 0);
                    }
                    self.reset_config();
                }
                self.set(off, value & 0x8FFF);
            }
            W_IF => {
                let v = self.reg(W_IF) & !value;
                self.set(W_IF, v);
            }
            W_IF_SET => self.flag(value),
            W_RXCNT => {
                if value & 0x0080 != 0 {
                    let wr = self.reg(W_RXBUF_WR_ADDR) & 0xFFF;
                    self.set(W_RXBUF_WRCSR, wr);
                }
                self.set(W_RXCNT, value & 0xFF0E);
            }
            W_POWERSTATE => {
                if value & 2 != 0 {
                    self.set(W_POWERSTATE, 0);
                    self.flag(IRQ_RF_WAKEUP);
                }
            }
            W_POWERFORCE => {
                if value & 0x8000 != 0 {
                    if value & 1 != 0 {
                        self.set(W_POWERSTATE, 0x0200);
                    } else {
                        self.set(W_POWERSTATE, 0);
                        self.flag(IRQ_RF_WAKEUP);
                    }
                }
            }
            W_RXBUF_WR_ADDR => self.set(off, value & 0xFFF),
            W_RXBUF_RD_ADDR => {
                self.set(off, value & 0x1FFE);
                self.set(W_RXBUF_READCSR, (value & 0x1FFE) >> 1);
            }
            W_TXBUF_WR_DATA => {
                let mut addr = usize::from(self.reg(W_TXBUF_WR_ADDR) & 0x1FFE);
                self.ram[addr..addr + 2].copy_from_slice(&value.to_le_bytes());
                addr += 2;
                let gap = usize::from(self.reg(W_TXBUF_GAP) & 0x1FFE);
                if addr == gap && gap != 0 {
                    addr += usize::from(self.reg(W_TXBUF_GAPDISP)) * 2;
                }
                self.set(W_TXBUF_WR_ADDR, (addr & 0x1FFE) as u16);
                let count = self.reg(W_TXBUF_COUNT);
                if count > 0 {
                    self.set(W_TXBUF_COUNT, count - 1);
                    if count == 1 {
                        self.flag(IRQ_TXBUF_COUNT);
                    }
                }
            }
            W_TXREQ_RESET => {
                let v = self.reg(W_TXREQ_READ) & !value;
                self.set(W_TXREQ_READ, v);
            }
            W_TXREQ_SET => {
                let v = self.reg(W_TXREQ_READ) | (value & 0x000F);
                self.set(W_TXREQ_READ, v);
                self.request(v, now);
            }
            W_TXBUF_LOC1 | W_TXBUF_LOC2 | W_TXBUF_LOC3 | W_TXBUF_CMD => {
                self.set(off, value);
                let req = self.reg(W_TXREQ_READ);
                let bit = match off {
                    W_TXBUF_LOC1 => 0x1,
                    W_TXBUF_CMD => 0x2,
                    W_TXBUF_LOC2 => 0x4,
                    _ => 0x8,
                };
                if req & bit != 0 && value & 0x8000 != 0 {
                    self.request(bit, now);
                }
            }
            W_TXBUF_RESET => {
                for (bit, reg) in [
                    (0x1, W_TXBUF_LOC1),
                    (0x2, W_TXBUF_CMD),
                    (0x4, W_TXBUF_LOC2),
                    (0x8, W_TXBUF_LOC3),
                    (0x10, W_TXBUF_BEACON),
                ] {
                    if value & bit != 0 {
                        let v = self.reg(reg) & 0x7FFF;
                        self.set(reg, v);
                    }
                }
            }
            W_US_COUNTCNT => {
                self.latch_us(now);
                self.us_running = value & 1 != 0;
                self.set(off, value & 1);
                self.reschedule(now, scheduler);
            }
            W_US_COMPARECNT | W_PRE_BEACON | W_BEACONINT => {
                self.set(off, value);
                self.reschedule(now, scheduler);
            }
            W_US_COMPARE..=0x0F6 => {
                self.set(off, if off == W_US_COMPARE { value & 0xFFFE } else { value });
                self.reschedule(now, scheduler);
            }
            W_US_COUNT..=0x0FE => {
                self.latch_us(now);
                let shift = (off - W_US_COUNT) * 8;
                self.us_count =
                    (self.us_count & !(0xFFFFu64 << shift)) | (u64::from(value) << shift);
                self.reschedule(now, scheduler);
            }
            W_POST_BEACON => {
                self.set(off, value);
                if value == 0 {
                    self.post_beacon_at = None;
                }
            }
            W_BB_CNT => {
                let index = usize::from(value & 0xFF);
                match value >> 12 {
                    5 if index < self.bb.len() && index != 0 => {
                        self.bb[index] = self.reg(W_BB_WRITE) as u8
                    }
                    6 => {
                        let v = self.bb.get(index).copied().unwrap_or(0);
                        self.set(W_BB_READ, u16::from(v));
                    }
                    _ => {}
                }
                self.set(off, value);
            }
            W_RF_DATA2 | W_RF_DATA1 => {
                self.set(off, value);
                let word = u32::from(self.reg(W_RF_DATA1)) | u32::from(self.reg(W_RF_DATA2)) << 16;
                self.rf[(word >> 18) as usize & 0x3F] = word & 0x3FFFF;
            }
            _ => self.set(off, value),
        }
    }

    pub fn write8(&mut self, off: usize, value: u8, now: u64, scheduler: &mut Scheduler) {
        let word = self.read16(off & !1, now);
        let v = if off & 1 != 0 {
            (word & 0x00FF) | u16::from(value) << 8
        } else {
            (word & 0xFF00) | u16::from(value)
        };
        self.write16(off & !1, v, now, scheduler);
    }
}
