//! ROM prefetch buffer (GBATEK "GBA System Control", WAITCNT bit 14).
//!
//! While the CPU is busy with internal cycles or memory that is not the Game Pak,
//! the cartridge bus keeps reading ahead: up to eight halfwords past the last
//! opcode fetched, each at the sequential wait-state cost. A code fetch that hits
//! the buffer then costs a single cycle, which is why games enable it.
//!
//! The model: `next` is the address the CPU can get cheaply (the oldest buffered
//! halfword, or the one in flight when the buffer is empty), `count` how many are
//! buffered, and `countdown` the cycles left on the fetch in flight. Any data
//! access to the Game Pak aborts the prefetch.

use serde::{Deserialize, Serialize};

const CAPACITY: u32 = 8;

#[derive(Default, Serialize, Deserialize)]
pub struct Prefetch {
    /// WAITCNT bit 14.
    pub enabled: bool,
    /// The prefetcher is running: the last opcode fetch came from the Game Pak.
    pub active: bool,
    next: u32,
    count: u32,
    countdown: i32,
    /// Sequential 16-bit wait of the region being prefetched.
    s: i32,
}

impl Prefetch {
    /// Stops prefetching (Game Pak data access, DMA, jump away from ROM).
    #[inline(always)]
    pub fn reset(&mut self) {
        self.active = false;
        self.count = 0;
    }

    /// The CPU spent `cycles` away from the Game Pak bus: fetch ahead.
    #[inline(always)]
    pub fn advance(&mut self, cycles: u32) {
        if !self.active || self.count >= CAPACITY {
            return;
        }
        self.countdown -= cycles as i32;
        if self.countdown <= 0 {
            self.fill();
        }
    }

    /// Completes the fetch in flight and any that follow within the elapsed time.
    #[inline(never)]
    fn fill(&mut self) {
        while self.countdown <= 0 && self.count < CAPACITY {
            self.count += 1;
            self.countdown += self.s;
        }
        if self.count >= CAPACITY {
            // Nothing in flight while full; the next slot starts fresh.
            self.countdown = self.s;
        }
    }

    /// Code fetch of a halfword at `addr` from the Game Pak. Returns the cycles it
    /// costs. `n`/`s` are the region's non-sequential / sequential 16-bit waits.
    #[inline]
    pub fn fetch(&mut self, addr: u32, n: u32, s: u32) -> u32 {
        if self.active && addr == self.next {
            self.next = addr.wrapping_add(2);
            if self.count > 0 {
                // Buffered: one cycle, during which the fetch in flight continues.
                self.count -= 1;
                self.countdown -= 1;
                if self.countdown <= 0 {
                    self.fill();
                }
                return 1;
            }
            // In flight: wait for it to land, then continue behind it.
            let cost = self.countdown.max(1) as u32;
            self.countdown = self.s;
            return cost;
        }
        if !self.enabled {
            self.active = false;
            return n;
        }
        self.restart(addr, s);
        n
    }

    /// Code fetch of an aligned word (two halfwords) at `addr`.
    #[inline]
    pub fn fetch_word(&mut self, addr: u32, n: u32, s: u32) -> u32 {
        if self.active && self.count >= 2 && addr == self.next {
            self.next = addr.wrapping_add(4);
            self.count -= 2;
            self.countdown -= 2;
            if self.countdown <= 0 {
                self.fill();
            }
            return 2;
        }
        self.fetch(addr, n, s) + self.fetch(addr.wrapping_add(2), n, s)
    }

    /// Miss: a fresh non-sequential access; prefetching restarts behind it.
    #[inline(never)]
    fn restart(&mut self, addr: u32, s: u32) {
        self.active = true;
        self.next = addr.wrapping_add(2);
        self.count = 0;
        self.s = s as i32;
        self.countdown = self.s;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequential_code_hits_after_a_stall() {
        let mut p = Prefetch { enabled: true, ..Default::default() };
        // First fetch misses (N = 5), then the CPU idles 6 cycles: two halfwords
        // (S = 3 each) get buffered.
        assert_eq!(p.fetch(0x0800_0000, 5, 3), 5);
        p.advance(6);
        assert_eq!(p.fetch(0x0800_0002, 5, 3), 1);
        assert_eq!(p.fetch(0x0800_0004, 5, 3), 1);
        // Nothing buffered now: the next one is in flight with 3 - 2 = 1 cycle left.
        assert_eq!(p.fetch(0x0800_0006, 5, 3), 1);
        assert_eq!(p.fetch(0x0800_0008, 5, 3), 3);
    }

    #[test]
    fn word_fetch_uses_two_buffered_halfwords() {
        let mut p = Prefetch { enabled: true, ..Default::default() };
        p.fetch(0x0800_0000, 5, 3);
        p.advance(12); // four halfwords buffered: 0x02, 0x04, 0x06, 0x08
        assert_eq!(p.fetch_word(0x0800_0002, 5, 3), 2);
        assert_eq!(p.fetch_word(0x0800_0006, 5, 3), 2);
        assert_eq!(p.fetch_word(0x0800_0100, 5, 3), 5 + 3);
    }

    #[test]
    fn buffer_stops_at_capacity() {
        let mut p = Prefetch { enabled: true, ..Default::default() };
        p.fetch(0x0800_0000, 5, 3);
        p.advance(1000);
        assert_eq!(p.count, CAPACITY);
        for i in 1..=8 {
            assert_eq!(p.fetch(0x0800_0000 + 2 * i, 5, 3), 1, "halfword {i}");
        }
        // Prefetching carried on during those 8 cycles: two more halfwords landed.
        assert_eq!(p.count, 2);
    }

    #[test]
    fn jump_misses_and_disabled_pays_full_price() {
        let mut p = Prefetch { enabled: true, ..Default::default() };
        p.fetch(0x0800_0000, 5, 3);
        p.advance(20);
        assert_eq!(p.fetch(0x0800_0100, 5, 3), 5);
        let mut off = Prefetch::default();
        assert_eq!(off.fetch(0x0800_0000, 5, 3), 5);
        assert_eq!(off.fetch(0x0800_0002, 5, 3), 5);
    }
}
