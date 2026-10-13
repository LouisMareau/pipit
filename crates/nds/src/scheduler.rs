//! Event scheduler: the shared clock of the whole machine, in ARM9 cycles
//! (two per bus cycle; the ARM7 and everything else run at the bus clock).
//!
//! The two CPUs each keep their own timestamp and run in short slices; between
//! slices the system brings the clock up to the slice's end and dispatches the
//! events that have come due.

use serde::{Deserialize, Serialize};

/// The bus (ARM7) clock, in Hz.
pub const BUS_HZ: u32 = 33_513_982;
/// One scanline, in ARM9 cycles: 355 dots of 6 bus cycles.
pub const LINE_CYCLES: u64 = 355 * 6 * 2;
/// Where HBlank starts within a scanline (after 256 dots), in ARM9 cycles.
pub const HBLANK_START: u64 = 256 * 6 * 2;
/// Scanlines per frame: 192 shown, 71 blank.
pub const LINES: u32 = 263;
/// One frame, in ARM9 cycles.
pub const FRAME_CYCLES: u64 = LINE_CYCLES * LINES as u64;

/// Hardware events that can be scheduled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Event {
    /// The visible part of a scanline finished.
    HBlankStart,
    /// A scanline finished; VCOUNT advances.
    HBlankEnd,
    /// Timer `n` overflows: 0–3 are the ARM9's, 4–7 the ARM7's.
    Timer(u8),
    /// A cartridge data word has arrived.
    CardWord,
    /// A division or square root result is ready.
    MathDone,
    /// One output audio sample is due.
    AudioSample,
    /// The real-time clock's second ticks.
    RtcSecond,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct Entry {
    at: u64,
    event: Event,
}

/// Sorted list of pending events (soonest last, so popping is cheap).
#[derive(Serialize, Deserialize)]
pub struct Scheduler {
    now: u64,
    queue: Vec<Entry>,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheduler {
    pub fn new() -> Self {
        Self { now: 0, queue: Vec::with_capacity(16) }
    }

    /// The machine's clock: where the last slice ended, in ARM9 cycles.
    #[inline(always)]
    pub fn now(&self) -> u64 {
        self.now
    }

    /// Moves the clock forward (never back).
    #[inline(always)]
    pub fn set_now(&mut self, now: u64) {
        if now > self.now {
            self.now = now;
        }
    }

    /// When the next event is due, if any.
    pub fn next_at(&self) -> Option<u64> {
        self.queue.last().map(|e| e.at)
    }

    pub fn schedule(&mut self, event: Event, delay: u64) {
        self.schedule_at(event, self.now + delay);
    }

    pub fn schedule_at(&mut self, event: Event, at: u64) {
        // Inserting before equal timestamps means they pop first: insertion order.
        let idx = self.queue.partition_point(|e| e.at > at);
        self.queue.insert(idx, Entry { at, event });
    }

    pub fn cancel(&mut self, event: Event) {
        self.queue.retain(|e| e.event != event);
    }

    /// The next event whose time has come, with that time.
    pub fn pop_due(&mut self) -> Option<(Event, u64)> {
        match self.queue.last() {
            Some(e) if e.at <= self.now => {
                let e = self.queue.pop().unwrap();
                Some((e.event, e.at))
            }
            _ => None,
        }
    }
}
