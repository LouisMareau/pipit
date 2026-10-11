//! The Wireless Adapter (AGB-015) as the serial port sees it.
//!
//! The adapter plugs into the link port and talks to the console in 32-bit normal
//! mode. The console first proves an adapter is there with the "NINTENDO" login
//! exchange, then sends commands: a header word `9966LLCC` (`LL` parameter words,
//! command `CC`), the parameters, and `80000000` while it reads the answer, which
//! is a header `9966LLCC` with the command's acknowledgement (`CC | 80`) and `LL`
//! words. A few commands hand the clock to the adapter: the console then waits as
//! a slave until the adapter sends a command of its own (`28` "data ready", `29`
//! "disconnected", `27` "nothing"), acknowledged the same way, after which the
//! console is master again.
//!
//! This module holds one adapter's state and the word-by-word protocol. What goes
//! over the air, between adapters, is [`crate::wireless`]: the link owns every
//! console, so it executes each command against the other adapters.
//!
//! Sources: GBATEK "GBA Wireless Adapter", the `librfu` driver shipped in games
//! (its expectations are the ground truth here), and published notes on the
//! command set.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// A host adapter serves at most this many children.
pub const MAX_CHILDREN: usize = 4;

/// Header halfword of every command and acknowledgement.
pub const HEADER: u32 = 0x9966_0000;
/// Sent by whichever side is only listening.
pub const IDLE: u32 = 0x8000_0000;
/// The adapter's device id, the last word of the login.
pub const RFU_ID: u16 = 0x8001;
/// The login keys, "NINTENDO" two letters at a time.
pub const LOGIN_KEYS: [u16; 4] = [0x494E, 0x544E, 0x4E45, 0x4F44];

// Commands from the console.
pub const CMD_RESET: u8 = 0x10;
pub const CMD_LINK_STATUS: u8 = 0x11;
pub const CMD_VERSION: u8 = 0x12;
pub const CMD_SYSTEM_STATUS: u8 = 0x13;
pub const CMD_SLOT_STATUS: u8 = 0x14;
pub const CMD_CONFIG_STATUS: u8 = 0x15;
pub const CMD_GAME_CONFIG: u8 = 0x16;
pub const CMD_SYSTEM_CONFIG: u8 = 0x17;
pub const CMD_SC_START: u8 = 0x19;
pub const CMD_SC_POLL: u8 = 0x1A;
pub const CMD_SC_END: u8 = 0x1B;
pub const CMD_SP_START: u8 = 0x1C;
pub const CMD_SP_POLL: u8 = 0x1D;
pub const CMD_SP_END: u8 = 0x1E;
pub const CMD_CP_START: u8 = 0x1F;
pub const CMD_CP_POLL: u8 = 0x20;
pub const CMD_CP_END: u8 = 0x21;
pub const CMD_DATA_TX: u8 = 0x24;
pub const CMD_DATA_TX_AND_CHANGE: u8 = 0x25;
pub const CMD_DATA_RX: u8 = 0x26;
pub const CMD_MS_CHANGE: u8 = 0x27;
pub const CMD_DISCONNECT: u8 = 0x30;
pub const CMD_CPR_START: u8 = 0x32;
pub const CMD_CPR_POLL: u8 = 0x33;
pub const CMD_CPR_END: u8 = 0x34;
pub const CMD_UNK35: u8 = 0x35;
pub const CMD_RESUME_RETRANSMIT_AND_CHANGE: u8 = 0x37;
pub const CMD_STOP_MODE: u8 = 0x3D;
// Commands from the adapter, while it holds the clock.
pub const CMD_DATA_READY_AND_CHANGE: u8 = 0x28;
pub const CMD_DISCONNECTED_AND_CHANGE: u8 = 0x29;

/// The command byte of a rejected request, followed by one error word.
const REJECTED: u8 = 0xEE;

/// Where the adapter is in the console-driven protocol.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Phase {
    /// The "NINTENDO" exchange; the console is probing for an adapter.
    Login,
    /// Waiting for a command header.
    Idle,
    /// Collecting the command's parameter words.
    Params,
    /// Clocking out the acknowledgement.
    Reply,
}

/// The adapter's own transfers, once the console has handed it the clock.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Master {
    Off,
    /// Sending a command and its payload.
    Packet {
        words: Vec<u32>,
        pos: usize,
    },
    /// Reading the console's acknowledgement header.
    AckHeader,
    /// Reading the console's acknowledgement words.
    AckWords(u32),
}

/// What the link must do for the adapter (see [`Adapter::take_request`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Request {
    /// A complete command; the link answers it with [`Adapter::reply`].
    Command { cmd: u8, params: Vec<u32> },
    /// The console has handed the clock over and waits for the adapter.
    WaitBegan,
    /// The adapter was power cycled; the link drops its connections.
    Reset,
    /// The console just completed the login (for tracing).
    Login,
}

/// What happens once a command's acknowledgement has been read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Then {
    Nothing,
    /// The console becomes clock slave and waits for the adapter.
    Wait {
        sent_data: bool,
    },
    /// The adapter forgets everything and expects a new login.
    Reset,
}

/// The link's answer to a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub words: Vec<u32>,
    pub then: Then,
    /// The command was refused: the answer is `9966 01 EE` and an error code.
    pub rejected: bool,
}

impl Reply {
    pub fn ok(words: Vec<u32>) -> Self {
        Self { words, then: Then::Nothing, rejected: false }
    }

    pub fn empty() -> Self {
        Self::ok(Vec::new())
    }

    pub fn wait(sent_data: bool) -> Self {
        Self { words: Vec::new(), then: Then::Wait { sent_data }, rejected: false }
    }

    /// Error code 1: the command makes no sense in the adapter's state.
    pub fn rejected() -> Self {
        Self { words: vec![1], then: Then::Nothing, rejected: true }
    }
}

/// The adapter's part in the radio network.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    Idle,
    /// Serving: children connect to this adapter.
    Host,
    /// Asked to connect to a host (node index); `ok` says whether it worked.
    Connecting {
        host: usize,
        ok: bool,
    },
    /// Connected to a host (node index).
    Child {
        host: usize,
    },
}

/// A frame of data handed to the adapter with command 24/25.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame {
    /// The first word: a host puts its byte count in bits 0–6, a child in the
    /// five bits at `8 + 5·slot`.
    pub header: u32,
    pub bytes: Vec<u8>,
}

impl Frame {
    /// Builds a frame from the command's parameter words.
    pub fn from_words(words: &[u32]) -> Self {
        let header = words.first().copied().unwrap_or(0);
        let bytes = words[1.min(words.len())..].iter().flat_map(|w| w.to_le_bytes()).collect();
        Self { header, bytes }
    }

    /// The bytes a host meant to send (its header counts them in bits 0–6).
    pub fn host_bytes(&self) -> &[u8] {
        let n = (self.header & 0x7F) as usize;
        &self.bytes[..n.min(self.bytes.len())]
    }

    /// The bytes a child in `slot` meant to send.
    pub fn child_bytes(&self, slot: usize) -> &[u8] {
        let n = ((self.header >> (8 + 5 * slot)) & 0x1F) as usize;
        &self.bytes[..n.min(self.bytes.len())]
    }

    /// The frame as the words of a `DATA_RX` answer: header, then the bytes.
    pub fn to_words(header: u32, bytes: &[u8]) -> Vec<u32> {
        let mut words = vec![header];
        words.extend(bytes.chunks(4).map(|c| {
            let mut w = [0u8; 4];
            w[..c.len()].copy_from_slice(c);
            u32::from_le_bytes(w)
        }));
        words
    }
}

/// The console is waiting as clock slave since `since`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wait {
    pub since: u64,
    /// The wait came with a frame to send (command 25/37) rather than alone (27).
    pub sent_data: bool,
}

/// One Wireless Adapter: the serial protocol with its console, and its place in
/// the network that [`crate::wireless`] maintains across adapters.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Adapter {
    phase: Phase,
    /// High halfword of the previous login answer (its inverse goes out next).
    prev_high: u16,
    cmd: u8,
    len: usize,
    params: Vec<u32>,
    reply: VecDeque<u32>,
    then: Then,
    master: Master,
    request: Option<Request>,

    // Network state, maintained by the link.
    /// Device id, assigned by the link. Nonzero.
    pub id: u16,
    pub role: Role,
    /// A host accepts new children (SC_START … SC_END).
    pub accepting: bool,
    /// Searching for hosts (SP_START … SP_END).
    pub searching: bool,
    /// The six words of GAME_CONFIG: serial number, game name, user name.
    pub broadcast: [u32; 6],
    /// The SYSTEM_CONFIG word.
    pub config: u32,
    /// Host: the node connected in each slot.
    pub children: [Option<usize>; MAX_CHILDREN],
    /// Child: its slot at the host.
    pub slot: u8,
    /// A frame handed over with DATA_TX, not yet sent over the air.
    pub tx: Option<Frame>,
    /// The last frame sent, for RESUME_RETRANSMIT.
    pub last_tx: Option<Frame>,
    /// Child: the host's last frame, until DATA_RX reads it.
    pub rx: Option<Frame>,
    /// Host: each child's bytes since the last DATA_RX.
    pub rx_slots: [Option<Vec<u8>>; MAX_CHILDREN],
    /// The console waits as clock slave.
    pub waiting: Option<Wait>,
    /// Slots the other side disconnected, to report with command 29.
    pub lost: u8,
}

impl Default for Adapter {
    fn default() -> Self {
        Self::new()
    }
}

impl Adapter {
    pub fn new() -> Self {
        Self {
            phase: Phase::Login,
            prev_high: 0,
            cmd: 0,
            len: 0,
            params: Vec::new(),
            reply: VecDeque::new(),
            then: Then::Nothing,
            master: Master::Off,
            request: None,
            id: 0,
            role: Role::Idle,
            accepting: false,
            searching: false,
            broadcast: [0; 6],
            config: 0,
            children: [None; MAX_CHILDREN],
            slot: 0,
            tx: None,
            last_tx: None,
            rx: None,
            rx_slots: Default::default(),
            waiting: None,
            lost: 0,
        }
    }

    /// Power cycles the adapter (the console pulses the SD line). Connections are
    /// forgotten here; the link tells the other adapters.
    pub fn reset(&mut self) {
        let id = self.id;
        *self = Self::new();
        self.id = id;
        self.request = Some(Request::Reset);
    }

    /// Takes what the link must do, if anything.
    pub fn take_request(&mut self) -> Option<Request> {
        self.request.take()
    }

    /// Whether `take_request` would return something.
    pub fn has_request(&self) -> bool {
        self.request.is_some()
    }

    /// The link's answer to the command last taken with `take_request`.
    pub fn reply(&mut self, reply: Reply) {
        let cmd = if reply.rejected { REJECTED } else { self.cmd | 0x80 };
        self.reply.clear();
        self.reply.push_back(HEADER | ((reply.words.len() as u32) << 8) | u32::from(cmd));
        self.reply.extend(reply.words);
        self.then = reply.then;
        self.phase = Phase::Reply;
    }

    /// Whether the adapter is waiting for the console to read an acknowledgement.
    pub fn replying(&self) -> bool {
        self.phase == Phase::Reply
    }

    /// A transfer the console clocked: `sent` is its word, the result is the
    /// adapter's. `now` is the console's clock, for timing the waits.
    pub fn transfer(&mut self, sent: u32, now: u64) -> u32 {
        match self.phase {
            Phase::Login => self.login(sent),
            Phase::Idle => {
                if sent & 0xFFFF_0000 == HEADER {
                    self.cmd = sent as u8;
                    self.len = ((sent >> 8) & 0xFF) as usize;
                    self.params.clear();
                    if self.len == 0 {
                        self.complete_command();
                    } else {
                        self.phase = Phase::Params;
                    }
                }
                IDLE
            }
            Phase::Params => {
                self.params.push(sent);
                if self.params.len() >= self.len {
                    self.complete_command();
                }
                IDLE
            }
            Phase::Reply => {
                let word = self.reply.pop_front().unwrap_or(IDLE);
                if self.reply.is_empty() {
                    self.phase = Phase::Idle;
                    match self.then {
                        Then::Nothing => {}
                        Then::Wait { sent_data } => {
                            self.waiting = Some(Wait { since: now, sent_data });
                            self.request = Some(Request::WaitBegan);
                        }
                        Then::Reset => {
                            self.phase = Phase::Login;
                            self.prev_high = 0;
                        }
                    }
                    self.then = Then::Nothing;
                }
                word
            }
        }
    }

    /// The login exchange. The console sends its key halfword ("NI", "NT", "EN",
    /// "DO", then the device id) in the low half and the inverse of the adapter's
    /// previous high half in the high half; the adapter echoes the key in its high
    /// half and sends the inverse of its own previous high half in the low half.
    /// The console moves to the next key once three answers in a row agree.
    fn login(&mut self, sent: u32) -> u32 {
        let key = sent as u16;
        let out = (u32::from(key) << 16) | u32::from(!self.prev_high);
        // The id only counts after "DO": the first word after a reset may be
        // whatever the data register still held.
        let done = key == RFU_ID && self.prev_high == LOGIN_KEYS[3];
        self.prev_high = key;
        if done {
            self.phase = Phase::Idle;
            self.request = Some(Request::Login);
        }
        out
    }

    fn complete_command(&mut self) {
        let params = std::mem::take(&mut self.params);
        self.request = Some(Request::Command { cmd: self.cmd, params });
        // Until the link answers, the console reads idle words.
        self.phase = Phase::Reply;
        self.reply.clear();
    }
}

impl Adapter {
    /// The SI line as the console reads it (SIOCNT bit 2), given its own SO (bit
    /// 3) and whether it drives the clock. The driver's handshake wants the
    /// adapter to answer every SO change with the opposite level while the console
    /// is master, and with the same level while the adapter is.
    pub fn si(&self, so: bool, console_is_master: bool) -> bool {
        if console_is_master {
            !so
        } else {
            so
        }
    }

    /// Whether the adapter has a word to clock into the waiting console.
    pub fn master_pending(&self) -> bool {
        self.master != Master::Off
    }

    /// Queues a command for the console: the adapter clocks it out as soon as the
    /// console listens as a slave. Ends the current wait.
    pub fn send(&mut self, cmd: u8, payload: &[u32]) {
        let mut words = vec![HEADER | ((payload.len() as u32) << 8) | u32::from(cmd)];
        words.extend_from_slice(payload);
        self.master = Master::Packet { words, pos: 0 };
        self.waiting = None;
    }

    /// A transfer the adapter clocked: `sent` is the console's word, the result
    /// the adapter's.
    pub fn master_transfer(&mut self, sent: u32) -> u32 {
        match &mut self.master {
            Master::Off => IDLE,
            Master::Packet { words, pos } => {
                let word = words[*pos];
                *pos += 1;
                if *pos >= words.len() {
                    self.master = Master::AckHeader;
                }
                word
            }
            Master::AckHeader => {
                // 9966LLCC from the console; LL more words follow, which we skip.
                let n = (sent >> 8) & 0xFF;
                self.master = if sent & 0xFFFF_0000 == HEADER && n > 0 {
                    Master::AckWords(n)
                } else {
                    Master::Off
                };
                IDLE
            }
            Master::AckWords(n) => {
                *n -= 1;
                if *n == 0 {
                    self.master = Master::Off;
                }
                IDLE
            }
        }
    }

    /// Drops any queued command (the console left slave mode).
    pub fn cancel_master(&mut self) {
        self.master = Master::Off;
    }

    /// Slots with a connected child, as a bitmask.
    pub fn child_mask(&self) -> u8 {
        self.children.iter().enumerate().filter(|(_, c)| c.is_some()).map(|(i, _)| 1 << i).sum()
    }

    /// The first free slot of a host, if any.
    pub fn free_slot(&self) -> Option<usize> {
        self.children.iter().position(Option::is_none)
    }

    /// Children the SYSTEM_CONFIG word allows (`0x3C | n` in its high half,
    /// n = 4 − max).
    pub fn max_children(&self) -> usize {
        MAX_CHILDREN - ((self.config >> 16) & 3) as usize
    }

    /// Whether a host still has room for a child.
    pub fn has_room(&self) -> bool {
        let used = self.children.iter().filter(|c| c.is_some()).count();
        self.free_slot().is_some() && used < self.max_children()
    }

    /// The SYSTEM_STATUS word: id, the child's slot as a nibble, and the state.
    pub fn status_word(&self) -> u32 {
        let state: u32 = match self.role {
            Role::Idle if self.searching => 3,
            Role::Idle => 0,
            Role::Host if self.accepting => 2,
            Role::Host => 1,
            Role::Connecting { .. } => 4,
            Role::Child { .. } => 5,
        };
        let slot = match self.role {
            Role::Child { .. } => 1u32 << self.slot,
            _ => 0,
        };
        u32::from(self.id) | slot << 16 | state << 24
    }

    /// The LINK_STATUS word: one signal-strength byte per slot (FF = linked).
    pub fn strength_word(&self) -> u32 {
        match self.role {
            Role::Host => (0..MAX_CHILDREN)
                .filter(|&i| self.children[i].is_some())
                .map(|i| 0xFF << (8 * i))
                .sum(),
            Role::Child { .. } => 0xFF << (8 * u32::from(self.slot)),
            _ => 0,
        }
    }

    /// Everything a host has received from its children, as a DATA_RX answer:
    /// the header carries each child's byte count in the five bits at `8 + 5·slot`,
    /// the bytes follow slot by slot. Empty when nothing arrived.
    pub fn take_host_rx(&mut self) -> Vec<u32> {
        let mut header = 0u32;
        let mut bytes = Vec::new();
        let mut any = false;
        for (slot, data) in self.rx_slots.iter_mut().enumerate() {
            if let Some(data) = data.take() {
                any = true;
                header |= (data.len() as u32 & 0x1F) << (8 + 5 * slot);
                bytes.extend_from_slice(&data);
            }
        }
        if any {
            Frame::to_words(header, &bytes)
        } else {
            Vec::new()
        }
    }

    /// What a child received from its host, as a DATA_RX answer.
    pub fn take_child_rx(&mut self) -> Vec<u32> {
        match self.rx.take() {
            Some(frame) => Frame::to_words(frame.header, frame.host_bytes()),
            None => Vec::new(),
        }
    }

    /// Whether unread data waits for the console.
    pub fn has_rx(&self) -> bool {
        self.rx.is_some() || self.rx_slots.iter().any(Option::is_some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEYS: [u16; 4] = LOGIN_KEYS;

    /// Drives the console side of the login the way the games' driver does.
    #[test]
    fn login_like_the_driver() {
        let mut adapter = Adapter::new();
        let (mut count, mut send_id, mut recv_id, mut last_id) = (0usize, 0u16, 0u16, 0u16);
        // The data register still holds the end of a previous login.
        let mut word = 0x7FFE_8001u32;
        for _ in 0..40 {
            let got = adapter.transfer(word, 0);
            let (a, b) = (got as u16, (got >> 16) as u16);
            if last_id == 0 {
                if a == recv_id {
                    if count < 4 {
                        if recv_id == !send_id && b == !recv_id {
                            count += 1;
                        }
                    } else {
                        last_id = b;
                    }
                } else {
                    count = 0;
                }
            }
            send_id = if count < 4 { KEYS[count] } else { RFU_ID };
            recv_id = !b;
            word = u32::from(send_id) | u32::from(recv_id) << 16;
            if last_id != 0 {
                break;
            }
        }
        assert_eq!(last_id, RFU_ID);
        assert_eq!(adapter.phase, Phase::Idle);
    }

    #[test]
    fn command_round_trip() {
        let mut adapter = Adapter::new();
        adapter.phase = Phase::Idle;
        assert_eq!(adapter.transfer(HEADER | (1 << 8) | 0x17, 0), IDLE);
        assert_eq!(adapter.transfer(0x003C_0420, 0), IDLE);
        assert_eq!(
            adapter.take_request(),
            Some(Request::Command { cmd: 0x17, params: vec![0x003C_0420] })
        );
        adapter.reply(Reply::ok(vec![0xAB, 0xCD]));
        assert_eq!(adapter.transfer(IDLE, 0), HEADER | (2 << 8) | 0x97);
        assert_eq!(adapter.transfer(IDLE, 0), 0xAB);
        assert_eq!(adapter.transfer(IDLE, 0), 0xCD);
        assert_eq!(adapter.phase, Phase::Idle);
    }

    #[test]
    fn wait_then_adapter_command() {
        let mut adapter = Adapter::new();
        adapter.phase = Phase::Idle;
        assert_eq!(adapter.transfer(HEADER | 0x27, 0), IDLE);
        assert_eq!(adapter.take_request(), Some(Request::Command { cmd: 0x27, params: vec![] }));
        adapter.reply(Reply::wait(false));
        assert_eq!(adapter.transfer(IDLE, 7), HEADER | 0xA7);
        assert_eq!(adapter.take_request(), Some(Request::WaitBegan));
        assert_eq!(adapter.waiting, Some(Wait { since: 7, sent_data: false }));
        adapter.send(CMD_DATA_READY_AND_CHANGE, &[]);
        assert!(adapter.master_pending());
        assert_eq!(adapter.master_transfer(IDLE), HEADER | 0x28);
        assert_eq!(adapter.master_transfer(HEADER | 0xA8), IDLE);
        assert!(!adapter.master_pending());
        assert_eq!(adapter.waiting, None);
    }
}
