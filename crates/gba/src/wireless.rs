//! The radio between Wireless Adapters.
//!
//! Every console of a [`Link`] can carry an adapter. The link executes each
//! adapter's commands against the others (who is serving, who is connected to
//! whom) and runs the *rounds* that carry data frames over the air: a host's
//! frame goes to all its children, each child's frame to the host. A round runs
//! as soon as a host and all its children are waiting for one, and every
//! [`crate::sio::TICK_CYCLES`] otherwise, so a console that joins late or is
//! busy never stalls the others. The rules here come from what the games'
//! `librfu` driver expects of a real adapter.

use crate::link::Link;
use crate::rfu::{self, Adapter, Reply, Request, Role};
use crate::Gba;

/// A waiting console hears "nothing happened" (command 27) after this long.
pub const WAIT_TIMEOUT: u64 = crate::CLOCK_HZ as u64 / 2;

/// The device id handed to node `i`'s adapter: fixed, so a host keeps its
/// identity across sessions.
pub fn device_id(i: usize) -> u16 {
    0x2A51u16.wrapping_add(0x1D37u16.wrapping_mul(i as u16)) | 1
}

fn adapter(nodes: &mut [Gba], i: usize) -> &mut Adapter {
    nodes[i].bus.sio.adapter_mut().expect("a wireless node has an adapter")
}

fn adapter_ref(nodes: &[Gba], i: usize) -> &Adapter {
    nodes[i].bus.sio.adapter().expect("a wireless node has an adapter")
}

/// Each child of host `h` as `id | slot << 16`, in slot order.
fn child_list(nodes: &[Gba], h: usize) -> Vec<u32> {
    let host = adapter_ref(nodes, h);
    host.children
        .iter()
        .enumerate()
        .filter_map(|(slot, c)| {
            c.map(|c| u32::from(adapter_ref(nodes, c).id) | (slot as u32) << 16)
        })
        .collect()
}

/// Every host with room for `n`, seven words each: `id | slot << 16` then its
/// six broadcast words.
fn host_list(nodes: &[Gba], n: usize) -> Vec<u32> {
    let mut words = Vec::new();
    for (h, node) in nodes.iter().enumerate() {
        let Some(host) = node.bus.sio.adapter() else { continue };
        if h == n || host.role != Role::Host || !host.accepting || !host.has_room() {
            continue;
        }
        let slot = host.free_slot().unwrap_or(0) as u32;
        words.push(u32::from(host.id) | slot << 16);
        words.extend_from_slice(&host.broadcast);
    }
    words
}

/// The CP_POLL / CP_END word: the host's id and the slot, with a status byte
/// (0 = connected, 1 = still trying, 2 = failed).
fn connect_word(nodes: &[Gba], n: usize) -> u32 {
    let a = adapter_ref(nodes, n);
    match a.role {
        Role::Connecting { host, ok: true } | Role::Child { host } => {
            u32::from(adapter_ref(nodes, host).id) | u32::from(a.slot) << 16
        }
        Role::Connecting { ok: false, .. } => 0x0200_0000,
        _ => 0x0100_0000,
    }
}

impl Link {
    /// Executes a command from node `n`'s console.
    pub(crate) fn rfu_execute(&mut self, n: usize, cmd: u8, params: &[u32]) -> Reply {
        use rfu::*;
        let nodes = &mut self.nodes;
        match cmd {
            CMD_RESET | CMD_STOP_MODE => {
                self.rfu_disconnect_all(n);
                let a = adapter(&mut self.nodes, n);
                a.role = Role::Idle;
                a.accepting = false;
                a.searching = false;
                a.tx = None;
                a.rx = None;
                a.rx_slots = Default::default();
                let mut reply = Reply::empty();
                if cmd == CMD_STOP_MODE {
                    reply.then = Then::Reset;
                }
                reply
            }
            CMD_LINK_STATUS => Reply::ok(vec![adapter(nodes, n).strength_word()]),
            CMD_VERSION => Reply::ok(vec![0x0083_0117]),
            CMD_SYSTEM_STATUS => Reply::ok(vec![adapter(nodes, n).status_word()]),
            CMD_SLOT_STATUS => {
                let mut words = vec![adapter(nodes, n).status_word()];
                words.extend(child_list(nodes, n));
                Reply::ok(words)
            }
            CMD_CONFIG_STATUS => Reply::ok(vec![adapter(nodes, n).config]),
            CMD_GAME_CONFIG => {
                if params.len() >= 6 {
                    adapter(nodes, n).broadcast.copy_from_slice(&params[..6]);
                }
                Reply::empty()
            }
            CMD_SYSTEM_CONFIG => {
                adapter(nodes, n).config = params.first().copied().unwrap_or(0);
                Reply::empty()
            }
            CMD_SC_START => {
                let a = adapter(nodes, n);
                if matches!(a.role, Role::Child { .. } | Role::Connecting { .. }) {
                    return Reply::rejected();
                }
                a.role = Role::Host;
                a.accepting = true;
                Reply::empty()
            }
            CMD_SC_POLL => Reply::ok(child_list(nodes, n)),
            CMD_SC_END => {
                let a = adapter(nodes, n);
                a.accepting = false;
                if a.role == Role::Host && a.children.iter().all(Option::is_none) {
                    a.role = Role::Idle;
                }
                Reply::ok(child_list(nodes, n))
            }
            CMD_SP_START => {
                adapter(nodes, n).searching = true;
                Reply::empty()
            }
            CMD_SP_POLL => Reply::ok(host_list(nodes, n)),
            CMD_SP_END => {
                adapter(nodes, n).searching = false;
                Reply::empty()
            }
            CMD_CP_START => {
                let id = params.first().copied().unwrap_or(0) as u16;
                self.rfu_connect(n, id);
                Reply::empty()
            }
            CMD_CP_POLL => Reply::ok(vec![connect_word(nodes, n)]),
            CMD_CP_END => {
                let word = connect_word(nodes, n);
                let a = adapter(nodes, n);
                match a.role {
                    Role::Connecting { host, ok: true } => a.role = Role::Child { host },
                    Role::Connecting { ok: false, .. } => a.role = Role::Idle,
                    _ => {}
                }
                Reply::ok(vec![word])
            }
            CMD_DATA_TX => {
                adapter(nodes, n).tx = Some(Frame::from_words(params));
                Reply::empty()
            }
            CMD_DATA_TX_AND_CHANGE => {
                adapter(nodes, n).tx = Some(Frame::from_words(params));
                Reply::wait(true)
            }
            CMD_RESUME_RETRANSMIT_AND_CHANGE => {
                let a = adapter(nodes, n);
                a.tx = a.last_tx.clone();
                Reply::wait(true)
            }
            CMD_DATA_RX => {
                let a = adapter(nodes, n);
                let words = match a.role {
                    Role::Host => a.take_host_rx(),
                    Role::Child { .. } => a.take_child_rx(),
                    _ => Vec::new(),
                };
                Reply::ok(words)
            }
            CMD_MS_CHANGE | CMD_UNK35 => Reply::wait(false),
            CMD_DISCONNECT => {
                let mask = params.first().copied().unwrap_or(0xF) as u8;
                self.rfu_disconnect(n, mask);
                Reply::empty()
            }
            // Link recovery: connections here never drop by themselves, so there
            // is nothing to recover (status 2 = failed).
            CMD_CPR_START => Reply::empty(),
            CMD_CPR_POLL | CMD_CPR_END => Reply::ok(vec![2]),
            _ => Reply::rejected(),
        }
    }

    /// Node `n` asks to join the host whose id is `id`.
    fn rfu_connect(&mut self, n: usize, id: u16) {
        self.rfu_disconnect_all(n);
        let host = (0..self.nodes.len()).find(|&h| {
            h != n
                && self.nodes[h].bus.sio.adapter().is_some_and(|a| {
                    a.id == id && a.role == Role::Host && a.accepting && a.has_room()
                })
        });
        let Some(h) = host else {
            adapter(&mut self.nodes, n).role = Role::Connecting { host: n, ok: false };
            return;
        };
        let slot = adapter(&mut self.nodes, h).free_slot().unwrap_or(0);
        adapter(&mut self.nodes, h).children[slot] = Some(n);
        let a = adapter(&mut self.nodes, n);
        a.role = Role::Connecting { host: h, ok: true };
        a.slot = slot as u8;
    }

    /// Node `n` drops the connections in `mask`: its children's slots if it is a
    /// host, its own slot if it is a child.
    fn rfu_disconnect(&mut self, n: usize, mask: u8) {
        let a = adapter_ref(&self.nodes, n);
        match a.role {
            Role::Host => {
                for slot in 0..rfu::MAX_CHILDREN {
                    if mask & (1 << slot) == 0 {
                        continue;
                    }
                    if let Some(c) = adapter(&mut self.nodes, n).children[slot].take() {
                        let child = adapter(&mut self.nodes, c);
                        child.role = Role::Idle;
                        child.lost |= 1 << slot;
                        self.rfu_report_lost(c);
                    }
                }
            }
            Role::Child { host } | Role::Connecting { host, ok: true } => {
                let slot = usize::from(a.slot);
                if mask & (1 << slot) != 0 {
                    adapter(&mut self.nodes, n).role = Role::Idle;
                    let h = adapter(&mut self.nodes, host);
                    if h.children[slot] == Some(n) {
                        h.children[slot] = None;
                        h.lost |= 1 << slot;
                        self.rfu_report_lost(host);
                    }
                }
            }
            _ => adapter(&mut self.nodes, n).role = Role::Idle,
        }
    }

    pub(crate) fn rfu_disconnect_all(&mut self, n: usize) {
        self.rfu_disconnect(n, 0xF);
    }

    /// Tells node `i` about slots the other side dropped, if it is waiting.
    fn rfu_report_lost(&mut self, i: usize) {
        let a = adapter(&mut self.nodes, i);
        if a.lost == 0 || a.waiting.is_none() {
            return;
        }
        let payload = [u32::from(a.lost)];
        a.lost = 0;
        self.rfu_send(i, rfu::CMD_DISCONNECTED_AND_CHANGE, &payload);
    }

    fn rfu_send(&mut self, i: usize, cmd: u8, payload: &[u32]) {
        if cmd == rfu::CMD_DATA_READY_AND_CHANGE {
            self.wakes += 1;
        }
        if let Some(log) = &mut self.log {
            log.push(format!("{}: <- {cmd:02X} {}", i + 1, hex(payload)));
        }
        let bus = &mut self.nodes[i].bus;
        bus.sio.adapter_send(cmd, payload, &mut bus.scheduler);
    }

    /// Wakes node `i` with "data ready" if what it waits for has happened.
    fn rfu_wake(&mut self, i: usize) {
        let a = adapter_ref(&self.nodes, i);
        let Some(wait) = a.waiting else { return };
        let ready = match a.role {
            Role::Host if wait.sent_data => a.tx.is_none(),
            Role::Host => a.has_rx(),
            Role::Child { .. } => a.rx.is_some(),
            _ => false,
        };
        if ready {
            self.rfu_send(i, rfu::CMD_DATA_READY_AND_CHANGE, &[]);
        }
    }

    /// The host node `n` belongs to, with that host's children.
    fn rfu_group(&self, n: usize) -> Option<(usize, Vec<usize>)> {
        let h = match adapter_ref(&self.nodes, n).role {
            Role::Host => n,
            Role::Child { host } => host,
            _ => return None,
        };
        let children = adapter_ref(&self.nodes, h).children.iter().flatten().copied().collect();
        Some((h, children))
    }

    /// One round over the air for host `h`: its frame to every child, each
    /// child's frame to it, then whoever was waiting for this is woken.
    fn rfu_round(&mut self, h: usize) {
        let frame = adapter(&mut self.nodes, h).tx.take();
        if let Some(frame) = &frame {
            adapter(&mut self.nodes, h).last_tx = Some(frame.clone());
        }
        let children = adapter_ref(&self.nodes, h).children;
        for (slot, c) in children.iter().enumerate() {
            let Some(c) = *c else { continue };
            if let Some(frame) = &frame {
                adapter(&mut self.nodes, c).rx = Some(frame.clone());
            }
            if let Some(sent) = adapter(&mut self.nodes, c).tx.take() {
                let bytes = sent.child_bytes(slot).to_vec();
                adapter(&mut self.nodes, h).rx_slots[slot] = Some(bytes);
                adapter(&mut self.nodes, c).last_tx = Some(sent);
            }
        }
        self.rfu_wake(h);
        for c in children.iter().flatten() {
            self.rfu_wake(*c);
        }
    }

    /// Node `n`'s console has started waiting for its adapter.
    pub(crate) fn rfu_wait_began(&mut self, n: usize) {
        if adapter_ref(&self.nodes, n).lost != 0 {
            self.rfu_report_lost(n);
            return;
        }
        if let Some((h, children)) = self.rfu_group(n) {
            let all_waiting = adapter_ref(&self.nodes, h).waiting.is_some()
                && children.iter().all(|&c| adapter_ref(&self.nodes, c).waiting.is_some());
            if all_waiting {
                self.rfu_round(h);
            }
        }
        self.rfu_wake(n);
    }

    /// The periodic round: every host sends and receives, and consoles that have
    /// waited too long hear that nothing happened.
    pub(crate) fn rfu_tick(&mut self) {
        let now = self.nodes[0].bus.scheduler.now();
        for h in 0..self.nodes.len() {
            if self.nodes[h].bus.sio.adapter().is_some_and(|a| a.role == Role::Host) {
                self.rfu_round(h);
            }
        }
        for i in 0..self.nodes.len() {
            let waited = self.nodes[i]
                .bus
                .sio
                .adapter()
                .and_then(|a| a.waiting)
                .map(|w| now.saturating_sub(w.since));
            if waited.is_some_and(|t| t >= WAIT_TIMEOUT) {
                self.rfu_send(i, rfu::CMD_MS_CHANGE, &[]);
            }
        }
    }

    /// Handles whatever node `n`'s adapter asked for.
    pub(crate) fn rfu_request(&mut self, n: usize) {
        let request = self.nodes[n].bus.sio.adapter_mut().and_then(Adapter::take_request);
        let Some(request) = request else { return };
        match request {
            Request::Command { cmd, params } => {
                let reply = self.rfu_execute(n, cmd, &params);
                if let Some(log) = &mut self.log {
                    let then = match reply.then {
                        rfu::Then::Nothing => "",
                        rfu::Then::Wait { .. } => " (wait)",
                        rfu::Then::Reset => " (reset)",
                    };
                    let status = if reply.rejected { " rejected" } else { "" };
                    let (p, w) = (hex(&params), hex(&reply.words));
                    log.push(format!("{}: {cmd:02X} {p} -> {w}{then}{status}", n + 1));
                }
                adapter(&mut self.nodes, n).reply(reply);
            }
            Request::WaitBegan => self.rfu_wait_began(n),
            Request::Reset => {
                if let Some(log) = &mut self.log {
                    log.push(format!("{}: reset", n + 1));
                }
                self.rfu_disconnect_all(n);
            }
            Request::Login => {
                if let Some(log) = &mut self.log {
                    log.push(format!("{}: login", n + 1));
                }
            }
        }
    }
}

fn hex(words: &[u32]) -> String {
    words.iter().map(|w| format!("{w:08X}")).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rfu::{
        Role, Then, Wait, CMD_CP_END, CMD_CP_POLL, CMD_CP_START, CMD_DATA_RX, CMD_DATA_TX,
        CMD_DATA_TX_AND_CHANGE, CMD_DISCONNECT, CMD_GAME_CONFIG, CMD_LINK_STATUS, CMD_SC_POLL,
        CMD_SC_START, CMD_SP_POLL, CMD_SYSTEM_STATUS,
    };

    fn link(n: usize) -> Link {
        Link::wireless((0..n).map(|_| Gba::new(vec![0; 0x100], None)).collect())
    }

    fn connect(link: &mut Link, child: usize, host: usize) {
        let id = u32::from(adapter_ref(&link.nodes, host).id);
        assert_eq!(link.rfu_execute(child, CMD_CP_START, &[id]).words, vec![]);
        let word = link.rfu_execute(child, CMD_CP_POLL, &[]).words[0];
        assert_eq!(word >> 24, 0, "connected: {word:08X}");
        link.rfu_execute(child, CMD_CP_END, &[]);
        assert_eq!(adapter_ref(&link.nodes, child).role, Role::Child { host });
    }

    #[test]
    fn a_child_finds_and_joins_a_host() {
        let mut link = link(3);
        let broadcast = [0x1234, 1, 2, 3, 4, 5];
        link.rfu_execute(0, CMD_GAME_CONFIG, &broadcast);
        assert!(link.rfu_execute(1, CMD_SP_POLL, &[]).words.is_empty(), "nobody serves yet");
        link.rfu_execute(0, CMD_SC_START, &[]);
        let found = link.rfu_execute(1, CMD_SP_POLL, &[]).words;
        assert_eq!(found.len(), 7);
        assert_eq!(found[0] as u16, device_id(0));
        assert_eq!(&found[1..], &broadcast);
        connect(&mut link, 1, 0);
        connect(&mut link, 2, 0);
        let children = link.rfu_execute(0, CMD_SC_POLL, &[]).words;
        assert_eq!(children, vec![u32::from(device_id(1)), u32::from(device_id(2)) | 1 << 16]);
        assert_eq!(link.rfu_execute(0, CMD_LINK_STATUS, &[]).words, vec![0xFFFF]);
        assert_eq!(link.rfu_execute(2, CMD_LINK_STATUS, &[]).words, vec![0xFF00]);
        assert_eq!(link.rfu_execute(2, CMD_SYSTEM_STATUS, &[]).words[0] >> 16, 5 << 8 | 2);
    }

    #[test]
    fn frames_cross_in_a_round() {
        let mut link = link(2);
        link.rfu_execute(0, CMD_SC_START, &[]);
        connect(&mut link, 1, 0);
        // The host sends three bytes, the child (slot 0) two.
        let host_frame = [3, 0x00CC_BBAA];
        let child_frame = [2 << 8, 0x0000_2211];
        let reply = link.rfu_execute(0, CMD_DATA_TX_AND_CHANGE, &host_frame);
        assert_eq!(reply.then, Then::Wait { sent_data: true });
        link.rfu_execute(1, CMD_DATA_TX, &child_frame);
        adapter(&mut link.nodes, 0).waiting = Some(Wait { since: 0, sent_data: true });
        adapter(&mut link.nodes, 1).waiting = Some(Wait { since: 0, sent_data: false });
        link.rfu_wait_began(1);
        // Both were woken: the adapters hold a command for their consoles.
        assert!(adapter_ref(&link.nodes, 0).master_pending());
        assert!(adapter_ref(&link.nodes, 1).master_pending());
        assert_eq!(link.rfu_execute(1, CMD_DATA_RX, &[]).words, vec![3, 0x00CC_BBAA]);
        assert_eq!(link.rfu_execute(0, CMD_DATA_RX, &[]).words, vec![2 << 8, 0x2211]);
        assert!(link.rfu_execute(0, CMD_DATA_RX, &[]).words.is_empty(), "read once");
    }

    #[test]
    fn a_disconnect_reaches_the_other_side() {
        let mut link = link(2);
        link.rfu_execute(0, CMD_SC_START, &[]);
        connect(&mut link, 1, 0);
        link.rfu_execute(1, CMD_DISCONNECT, &[1]);
        assert_eq!(adapter_ref(&link.nodes, 1).role, Role::Idle);
        assert_eq!(adapter_ref(&link.nodes, 0).child_mask(), 0);
        assert_eq!(adapter_ref(&link.nodes, 0).lost, 1, "reported at the host's next wait");
        adapter(&mut link.nodes, 0).waiting = Some(Wait { since: 0, sent_data: false });
        link.rfu_wait_began(0);
        assert!(adapter_ref(&link.nodes, 0).master_pending());
        assert_eq!(adapter_ref(&link.nodes, 0).lost, 0);
    }
}
