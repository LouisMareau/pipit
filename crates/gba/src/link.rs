//! A link cable or a set of Wireless Adapters: up to four consoles run in
//! lockstep, exchanging multi-play words or adapter commands.
//!
//! With a cable, node 0 is the parent, the console that starts every transfer;
//! the others are children in cable order. Node 0 always runs first. Whenever a
//! serial port needs the link (a mode change, a transfer starting or ending, an
//! adapter command) the run loop hands control back here, the other consoles are
//! run up to node 0's clock, and every port is updated together. Between those
//! moments the others trail node 0 by at most one instruction, which is far
//! closer than any game can tell. The adapters' side lives in [`crate::wireless`].
//!
//! This is the in-process link, for one machine: headless tests and, later, local
//! multiplayer. Play over a network will carry the same words with a delay.

use crate::sio::{Mode, Stop};
use crate::Gba;
use pipit_common::snapshot::StateError;
use serde::{Deserialize, Serialize};

/// Everything a `Link` needs to continue exactly: each console plus the cable.
#[derive(Serialize, Deserialize)]
struct LinkState {
    nodes: Vec<Vec<u8>>,
    latched: Option<[u16; 4]>,
}

/// A cable joins at most this many consoles.
pub const MAX_NODES: usize = 4;

pub struct Link {
    pub(crate) nodes: Vec<Gba>,
    /// The words latched when the current transfer started, one per slot.
    latched: Option<[u16; 4]>,
    /// Words of every transfer completed since the last `take_transfers`.
    transfer_log: Vec<[u16; 4]>,
    transfers: u64,
    /// The consoles carry Wireless Adapters rather than a cable.
    wireless: bool,
    /// Adapter traffic, while tracing (see `set_trace`).
    pub(crate) log: Option<Vec<String>>,
}

impl Link {
    /// Joins the consoles with a cable; the first one is the parent.
    ///
    /// # Panics
    /// With fewer than one or more than four consoles.
    pub fn new(nodes: Vec<Gba>) -> Self {
        assert!((1..=MAX_NODES).contains(&nodes.len()), "a link cable joins one to four consoles");
        let count = nodes.len() as u8;
        let mut link = Self::bare(nodes);
        for (i, node) in link.nodes.iter_mut().enumerate() {
            node.bus.sio.attach(i as u8, count);
        }
        link.update_ready();
        link
    }

    /// Gives every console a Wireless Adapter instead of a cable; any of them can
    /// host. A single console is fine too (an adapter with nobody to talk to).
    ///
    /// # Panics
    /// With fewer than one or more than four consoles.
    pub fn wireless(nodes: Vec<Gba>) -> Self {
        assert!(
            (1..=MAX_NODES).contains(&nodes.len()),
            "a wireless link joins one to four consoles"
        );
        let mut link = Self::bare(nodes);
        link.wireless = true;
        for (i, node) in link.nodes.iter_mut().enumerate() {
            node.bus.sio.attach_adapter(crate::wireless::device_id(i));
        }
        let bus = &mut link.nodes[0].bus;
        bus.sio.start_ticks(&mut bus.scheduler);
        link
    }

    fn bare(nodes: Vec<Gba>) -> Self {
        Self {
            nodes,
            latched: None,
            transfer_log: Vec::new(),
            transfers: 0,
            wireless: false,
            log: None,
        }
    }

    /// Whether the consoles are joined by adapters rather than a cable.
    pub fn is_wireless(&self) -> bool {
        self.wireless
    }

    /// Records every adapter command and answer (see `take_log`).
    pub fn set_trace(&mut self, on: bool) {
        self.log = if on { Some(Vec::new()) } else { None };
    }

    /// The adapter traffic recorded since the last call, one line per command.
    pub fn take_log(&mut self) -> Vec<String> {
        self.log.as_mut().map(std::mem::take).unwrap_or_default()
    }

    pub fn nodes(&self) -> &[Gba] {
        &self.nodes
    }

    pub fn nodes_mut(&mut self) -> &mut [Gba] {
        &mut self.nodes
    }

    pub fn into_nodes(self) -> Vec<Gba> {
        self.nodes
    }

    /// A hash of every console's state, to check that two emulations agree.
    pub fn state_hash(&self) -> u64 {
        let mut bytes = Vec::new();
        for node in &self.nodes {
            bytes.extend_from_slice(&node.state_hash().to_le_bytes());
        }
        crate::fnv1a(&bytes)
    }

    /// Serializes every console and the cable's own state (for rolling back).
    pub fn save_state(&self) -> Vec<u8> {
        let state = LinkState {
            nodes: self.nodes.iter().map(Gba::save_state).collect(),
            latched: self.latched,
        };
        bincode::serialize(&state).expect("link state serializes")
    }

    /// Restores a state from `save_state`, made with the same consoles.
    pub fn load_state(&mut self, data: &[u8]) -> Result<(), StateError> {
        let state: LinkState = bincode::deserialize(data).map_err(|_| StateError::Corrupt)?;
        if state.nodes.len() != self.nodes.len() {
            return Err(StateError::Corrupt);
        }
        for (node, bytes) in self.nodes.iter_mut().zip(&state.nodes) {
            node.load_state(bytes)?;
        }
        self.latched = state.latched;
        Ok(())
    }

    /// Transfers completed since the cable was connected.
    pub fn transfers(&self) -> u64 {
        self.transfers
    }

    /// The words of every transfer completed since the last call.
    pub fn take_transfers(&mut self) -> Vec<[u16; 4]> {
        std::mem::take(&mut self.transfer_log)
    }

    /// Runs every console for one of the parent's frames.
    pub fn run_frame(&mut self) {
        loop {
            let stop = self.nodes[0].run_frame_linked();
            self.sync(stop);
            if stop.is_none() {
                break;
            }
        }
    }

    /// Runs every console until the parent's clock reaches `target`.
    pub fn run_until(&mut self, target: u64) {
        loop {
            let stop = self.nodes[0].run_until_linked(target);
            self.sync(stop);
            if stop.is_none() {
                break;
            }
        }
    }

    /// Brings the other consoles up to node 0's clock, then acts on its stop.
    fn sync(&mut self, stop: Option<Stop>) {
        let now = self.nodes[0].bus.scheduler.now();
        for i in 1..self.nodes.len() {
            while let Some(child_stop) = self.nodes[i].run_until_linked(now) {
                self.handle(i, child_stop);
            }
        }
        if let Some(stop) = stop {
            self.handle(0, stop);
        }
    }

    /// Acts on a stop from node `i`.
    fn handle(&mut self, i: usize, stop: Stop) {
        match stop {
            Stop::ModeChange => self.update_ready(),
            Stop::Rfu => self.rfu_request(i),
            Stop::Tick => self.rfu_tick(),
            // Only the cable's parent, node 0, starts transfers.
            Stop::TransferStart => {
                self.update_ready();
                let mut words = [0xFFFF; 4];
                for (i, node) in self.nodes.iter_mut().enumerate() {
                    words[i] = node.bus.sio.send_word();
                    if i > 0 {
                        node.bus.sio.begin_as_child();
                    }
                }
                self.latched = Some(words);
            }
            Stop::TransferEnd => {
                let words = self.latched.take().unwrap_or([0xFFFF; 4]);
                for node in &mut self.nodes {
                    let bus = &mut node.bus;
                    bus.sio.complete(words, &mut bus.irq);
                }
                self.transfers += 1;
                self.transfer_log.push(words);
            }
        }
    }

    /// SD, "all consoles ready": everyone on the cable is in multi-play mode.
    fn update_ready(&mut self) {
        let all = !self.wireless
            && self.nodes.len() > 1
            && self.nodes.iter().all(|node| node.bus.sio.mode() == Mode::MultiPlay);
        for node in &mut self.nodes {
            node.bus.sio.set_ready(all);
        }
    }
}
