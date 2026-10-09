//! Event scheduler: the shared clock every hardware block runs on.
//!
//! Only the CPU "runs". Everything else (video timing, timers, FIFO drains, DMA
//! starts) registers an event at an absolute cycle time. After each instruction the
//! CPU asks `is_due()` and, if so, the bus dispatches whatever has come due.

use serde::{Deserialize, Serialize};

/// Hardware events that can be scheduled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Event {
    /// Visible part of a scanline finished; HBlank starts (cycle 960 of 1232).
    HBlankStart,
    /// Scanline finished; VCOUNT advances (cycle 1232).
    HBlankEnd,
    /// Timer `n` overflows.
    TimerOverflow(u8),
    /// Advance the PSG frame sequencer (envelopes, sweep, length counters).
    AudioSequencer,
    /// Produce one output audio sample.
    AudioSample,
    /// A serial (multi-play) transfer finishes.
    SioTransfer,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct Entry {
    at: u64,
    event: Event,
}

/// Sorted list of pending events. Small enough that a `Vec` beats a heap.
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

    /// Current time in CPU cycles since power-on.
    #[inline(always)]
    pub fn now(&self) -> u64 {
        self.now
    }

    /// Advances the clock. Called by the bus for every memory access and idle cycle.
    #[inline(always)]
    pub fn advance(&mut self, cycles: u32) {
        self.now += u64::from(cycles);
    }

    /// Whether at least one event is due at the current time.
    #[inline(always)]
    pub fn is_due(&self) -> bool {
        matches!(self.queue.last(), Some(e) if e.at <= self.now)
    }

    /// Time of the next event, if any.
    pub fn next_at(&self) -> Option<u64> {
        self.queue.last().map(|e| e.at)
    }

    /// Jumps the clock forward to the next event (used while the CPU is halted).
    pub fn skip_to_next_event(&mut self) {
        if let Some(at) = self.next_at() {
            self.now = self.now.max(at);
        }
    }

    /// Schedules `event` to fire `delay` cycles from now.
    pub fn schedule(&mut self, event: Event, delay: u64) {
        self.schedule_at(event, self.now + delay);
    }

    /// Schedules `event` at an absolute time. The queue is kept sorted with the
    /// earliest event last so popping is O(1).
    pub fn schedule_at(&mut self, event: Event, at: u64) {
        // Inserting before equal timestamps means they pop first: insertion order.
        let idx = self.queue.partition_point(|e| e.at > at);
        self.queue.insert(idx, Entry { at, event });
    }

    /// Removes every pending occurrence of `event`.
    pub fn cancel(&mut self, event: Event) {
        self.queue.retain(|e| e.event != event);
    }

    /// Pops the next due event, if any.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fires_in_time_order() {
        let mut s = Scheduler::new();
        s.schedule(Event::HBlankEnd, 1232);
        s.schedule(Event::HBlankStart, 960);
        s.schedule(Event::TimerOverflow(0), 1000);
        assert!(!s.is_due());
        s.advance(1000);
        assert_eq!(s.pop_due().map(|e| e.0), Some(Event::HBlankStart));
        assert_eq!(s.pop_due().map(|e| e.0), Some(Event::TimerOverflow(0)));
        assert_eq!(s.pop_due(), None);
        s.advance(232);
        assert_eq!(s.pop_due().map(|e| e.0), Some(Event::HBlankEnd));
    }

    #[test]
    fn cancel_removes_event() {
        let mut s = Scheduler::new();
        s.schedule(Event::TimerOverflow(1), 10);
        s.cancel(Event::TimerOverflow(1));
        s.advance(10);
        assert_eq!(s.pop_due(), None);
    }
}
