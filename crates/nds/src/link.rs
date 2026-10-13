//! Local wireless: consoles within range of each other, in one process.
//!
//! The consoles run in lockstep in short steps. A frame one of them sends
//! during a step reaches all the others at the end of it, a few microseconds
//! later at most: close enough to the air for the hardware's multiplayer
//! exchange, whose replies are collected well before the host's command times
//! out. Play over a network carries the same frames with a delay: every peer
//! emulates every console (see the web app's netplay).

use crate::firmware;
use crate::wifi::Frame;
use crate::{Nds, StateError};
use serde::{Deserialize, Serialize};

/// The hardware's multiplayer protocol addresses up to sixteen consoles.
pub const MAX_NODES: usize = 16;
/// How long each console runs between exchanges, in ARM9 cycles (about 15 µs).
pub const STEP: u64 = 1024;

/// Everything a `Link` needs to continue exactly: each console. Frames never
/// wait between steps, so the air itself has no state.
#[derive(Serialize, Deserialize)]
struct LinkState {
    nodes: Vec<Vec<u8>>,
}

pub struct Link {
    nodes: Vec<Nds>,
    /// Frames carried since the link was made.
    frames: u64,
    /// Every frame carried, while tracing (see `set_trace`).
    log: Option<Vec<String>>,
}

impl Link {
    /// Puts the consoles within range of each other. Each gets its own MAC
    /// address, derived from the first one's, as games tell players apart by it.
    ///
    /// # Panics
    /// With no consoles, or more than sixteen.
    pub fn new(mut nodes: Vec<Nds>) -> Self {
        assert!(
            (1..=MAX_NODES).contains(&nodes.len()),
            "a wireless link joins one to sixteen consoles"
        );
        let base = firmware::mac_of(&nodes[0].bus.spi.firmware);
        for (i, node) in nodes.iter_mut().enumerate().skip(1) {
            let mut mac = base;
            mac[5] = mac[5].wrapping_add(i as u8);
            firmware::set_mac(&mut node.bus.spi.firmware, mac);
        }
        Self { nodes, frames: 0, log: None }
    }

    pub fn nodes(&self) -> &[Nds] {
        &self.nodes
    }

    pub fn nodes_mut(&mut self) -> &mut [Nds] {
        &mut self.nodes
    }

    pub fn into_nodes(self) -> Vec<Nds> {
        self.nodes
    }

    /// Frames carried since the link was made: flat while a game's link is stuck.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Records every frame carried (see `take_log`).
    pub fn set_trace(&mut self, on: bool) {
        self.log = on.then(Vec::new);
    }

    pub fn take_log(&mut self) -> Vec<String> {
        self.log.as_mut().map(std::mem::take).unwrap_or_default()
    }

    /// A hash of every console's state, to check that two emulations agree.
    pub fn state_hash(&self) -> u64 {
        let mut bytes = Vec::new();
        for node in &self.nodes {
            bytes.extend_from_slice(&node.state_hash().to_le_bytes());
        }
        bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
            (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
        })
    }

    /// Serializes every console (for rolling back).
    pub fn save_state(&self) -> Vec<u8> {
        let state = LinkState { nodes: self.nodes.iter().map(Nds::save_state).collect() };
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
        Ok(())
    }

    /// Runs every console for one of the first one's frames.
    pub fn run_frame(&mut self) {
        loop {
            let target = self.nodes[0].bus.scheduler.now() + STEP;
            let mut done = false;
            for (i, node) in self.nodes.iter_mut().enumerate() {
                if node.run_until(target) && i == 0 {
                    done = true;
                }
            }
            self.exchange();
            if done {
                return;
            }
        }
    }

    /// Runs every console until the first one's clock reaches `target`.
    pub fn run_until(&mut self, target: u64) {
        while self.nodes[0].bus.scheduler.now() < target {
            let step = (self.nodes[0].bus.scheduler.now() + STEP).min(target);
            for node in &mut self.nodes {
                node.run_until(step);
            }
            self.exchange();
        }
    }

    /// Delivers every frame sent during the step to the other consoles.
    fn exchange(&mut self) {
        for i in 0..self.nodes.len() {
            let frames = self.nodes[i].bus.wifi.take_outbox();
            if frames.is_empty() {
                continue;
            }
            self.frames += frames.len() as u64;
            if let Some(log) = &mut self.log {
                log.extend(frames.iter().map(|frame| describe(i, frame)));
            }
            for (j, node) in self.nodes.iter_mut().enumerate() {
                if j == i {
                    continue;
                }
                for frame in &frames {
                    node.bus.wifi.receive(frame);
                }
                node.bus.wifi_irq();
            }
        }
    }
}

/// One line about a frame: who sent it, its kind and length, and the start of
/// its body.
fn describe(from: usize, frame: &Frame) -> String {
    let body: Vec<String> =
        frame.data.iter().skip(24).take(16).map(|b| format!("{b:02X}")).collect();
    format!("{from}: {:?} {} bytes {}", frame.kind, frame.data.len(), body.join(" "))
}
