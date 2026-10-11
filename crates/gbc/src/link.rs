//! A link cable between two Game Boys, run in lockstep: whichever console is
//! behind in time takes the next step, so neither gets more than one
//! instruction ahead of the other, and a transfer finds the partner's byte
//! exactly where it is at that moment. Either console may clock a transfer;
//! the games take turns.

use serde::{Deserialize, Serialize};

use crate::serial::Stop;
use crate::snapshot::StateError;
use crate::Gbc;

#[derive(Serialize, Deserialize)]
struct LinkState {
    nodes: Vec<Vec<u8>>,
    master: Option<usize>,
    latched: [u8; 2],
}

pub struct Link {
    nodes: Vec<Gbc>,
    /// The console clocking the transfer in flight.
    master: Option<usize>,
    /// Both bytes as they were when the transfer started: the master's, then the other's.
    latched: [u8; 2],
    log: Vec<[u8; 2]>,
    transfers: u64,
}

impl Link {
    /// One console alone, or two on a cable.
    ///
    /// # Panics
    /// With no console, or more than two.
    pub fn new(nodes: Vec<Gbc>) -> Self {
        assert!((1..=2).contains(&nodes.len()), "a Game Boy link cable joins two consoles");
        let mut link =
            Self { nodes, master: None, latched: [0xFF; 2], log: Vec::new(), transfers: 0 };
        let linked = link.nodes.len() == 2;
        for node in &mut link.nodes {
            node.bus.serial.attach(linked);
        }
        link
    }

    pub fn nodes(&self) -> &[Gbc] {
        &self.nodes
    }

    pub fn nodes_mut(&mut self) -> &mut [Gbc] {
        &mut self.nodes
    }

    pub fn transfers(&self) -> u64 {
        self.transfers
    }

    /// The bytes of every transfer completed since the last call: the master's, then the other's.
    pub fn take_transfers(&mut self) -> Vec<[u8; 2]> {
        std::mem::take(&mut self.log)
    }

    /// Runs until the first console has finished a frame; the second stays within an instruction.
    pub fn run_frame(&mut self) {
        if self.nodes.len() == 1 {
            self.nodes[0].run_frame();
            return;
        }
        loop {
            let i = usize::from(self.nodes[1].bus.cycles < self.nodes[0].bus.cycles);
            self.nodes[i].step();
            self.handle_stop(i);
            if self.nodes[i].bus.video.take_frame_ready() && i == 0 {
                break;
            }
        }
    }

    fn handle_stop(&mut self, i: usize) {
        let other = 1 - i;
        match self.nodes[i].bus.serial.take_stop() {
            Some(Stop::TransferStart) => {
                // A second console clocking at the same time hears nothing useful.
                if self.master.is_none() {
                    self.master = Some(i);
                    self.latched =
                        [self.nodes[i].bus.serial.byte(), self.nodes[other].bus.serial.byte()];
                }
            }
            Some(Stop::TransferEnd) => {
                if self.master == Some(i) {
                    self.master = None;
                    let [mine, theirs] = self.latched;
                    let bus = &mut self.nodes[i].bus;
                    bus.serial.complete(theirs, &mut bus.if_);
                    let bus = &mut self.nodes[other].bus;
                    bus.serial.receive(mine, &mut bus.if_);
                    self.transfers += 1;
                    self.log.push([mine, theirs]);
                } else {
                    let bus = &mut self.nodes[i].bus;
                    bus.serial.complete(0xFF, &mut bus.if_);
                }
            }
            None => {}
        }
    }

    /// Serializes both consoles and the cable (for rolling back).
    pub fn save_state(&self) -> Vec<u8> {
        let state = LinkState {
            nodes: self.nodes.iter().map(Gbc::save_state).collect(),
            master: self.master,
            latched: self.latched,
        };
        bincode::serialize(&state).expect("link state serializes")
    }

    pub fn load_state(&mut self, data: &[u8]) -> Result<(), StateError> {
        let state: LinkState = bincode::deserialize(data).map_err(|_| StateError::Corrupt)?;
        if state.nodes.len() != self.nodes.len() {
            return Err(StateError::Corrupt);
        }
        for (node, bytes) in self.nodes.iter_mut().zip(&state.nodes) {
            node.load_state(bytes)?;
        }
        self.master = state.master;
        self.latched = state.latched;
        Ok(())
    }

    /// A hash of every console's state, to check that two emulations agree.
    pub fn state_hash(&self) -> u64 {
        self.nodes
            .iter()
            .flat_map(|n| n.state_hash().to_le_bytes())
            .fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3))
    }
}
