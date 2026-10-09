//! A link cable: up to four consoles run in lockstep, exchanging multi-play words.
//!
//! Node 0 is the parent, the console that starts every transfer; the others are
//! children in cable order. The parent runs first. Whenever its serial port needs
//! the cable (a mode change, a transfer starting or ending) the run loop hands
//! control back here, the children are run up to the parent's clock, and every
//! port is updated together. Between those moments the children trail the parent
//! by at most one instruction, which is far closer than any game can tell.
//!
//! This is the in-process link, for one machine: headless tests and, later, local
//! multiplayer. Play over a network will carry the same words with a delay.

use crate::sio::{Mode, Stop};
use crate::Gba;

/// A cable joins at most this many consoles.
pub const MAX_NODES: usize = 4;

pub struct Link {
    nodes: Vec<Gba>,
    /// The words latched when the current transfer started, one per slot.
    latched: Option<[u16; 4]>,
    /// Words of every transfer completed since the last `take_transfers`.
    log: Vec<[u16; 4]>,
    transfers: u64,
}

impl Link {
    /// Joins the consoles with a cable; the first one is the parent.
    ///
    /// # Panics
    /// With fewer than one or more than four consoles.
    pub fn new(nodes: Vec<Gba>) -> Self {
        assert!((1..=MAX_NODES).contains(&nodes.len()), "a link cable joins one to four consoles");
        let count = nodes.len() as u8;
        let mut link = Self { nodes, latched: None, log: Vec::new(), transfers: 0 };
        for (i, node) in link.nodes.iter_mut().enumerate() {
            node.bus.sio.attach(i as u8, count);
        }
        link.update_ready();
        link
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

    /// Transfers completed since the cable was connected.
    pub fn transfers(&self) -> u64 {
        self.transfers
    }

    /// The words of every transfer completed since the last call.
    pub fn take_transfers(&mut self) -> Vec<[u16; 4]> {
        std::mem::take(&mut self.log)
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

    /// Brings the children up to the parent's clock, then acts on the parent's stop.
    fn sync(&mut self, stop: Option<Stop>) {
        let now = self.nodes[0].bus.scheduler.now();
        for i in 1..self.nodes.len() {
            while let Some(child_stop) = self.nodes[i].run_until_linked(now) {
                // Children cannot start transfers, so they only report mode changes.
                debug_assert_eq!(child_stop, Stop::ModeChange);
                self.update_ready();
            }
        }
        match stop {
            None => {}
            Some(Stop::ModeChange) => self.update_ready(),
            Some(Stop::TransferStart) => {
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
            Some(Stop::TransferEnd) => {
                let words = self.latched.take().unwrap_or([0xFFFF; 4]);
                for node in &mut self.nodes {
                    let bus = &mut node.bus;
                    bus.sio.complete(words, &mut bus.irq);
                }
                self.transfers += 1;
                self.log.push(words);
            }
        }
    }

    /// SD, "all consoles ready": everyone on the cable is in multi-play mode.
    fn update_ready(&mut self) {
        let all = self.nodes.len() > 1
            && self.nodes.iter().all(|node| node.bus.sio.mode() == Mode::MultiPlay);
        for node in &mut self.nodes {
            node.bus.sio.set_ready(all);
        }
    }
}
