//! One CPU's four 16-bit timers, clocked at the bus clock (half the ARM9's).
//!
//! Counters are not ticked: a running timer stores the count it had at a known
//! time and computes the current count on demand; overflows are scheduler events.

use crate::irq::{Interrupt, Irq};
use crate::scheduler::{Event, Scheduler};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
struct Timer {
    reload: u16,
    /// Bits 0-1 prescaler, 2 count-up (cascade), 6 IRQ enable, 7 start.
    control: u16,
    /// Counter value at `latched_at`.
    counter: u16,
    latched_at: u64,
}

impl Timer {
    fn running(&self) -> bool {
        self.control & (1 << 7) != 0
    }

    fn cascade(&self) -> bool {
        self.control & (1 << 2) != 0
    }

    /// ARM9 cycles per tick: bus cycles ×1, ×64, ×256 or ×1024, and two ARM9
    /// cycles per bus cycle.
    fn prescale(&self) -> u64 {
        [2, 128, 512, 2048][(self.control & 3) as usize]
    }

    fn ticks_since(&self, now: u64) -> u64 {
        let p = self.prescale();
        now / p - self.latched_at / p
    }

    fn current(&self, now: u64) -> u16 {
        if self.running() && !self.cascade() {
            self.counter.wrapping_add(self.ticks_since(now) as u16)
        } else {
            self.counter
        }
    }

    fn overflow_at(&self) -> u64 {
        let p = self.prescale();
        let ticks_needed = 0x1_0000 - u64::from(self.counter);
        let first_tick = (self.latched_at / p + 1) * p;
        first_tick + (ticks_needed - 1) * p
    }
}

#[derive(Serialize, Deserialize)]
pub struct Timers {
    timers: [Timer; 4],
    /// Scheduler event index of timer 0 (0 for the ARM9's set, 4 for the ARM7's).
    base: u8,
}

impl Timers {
    pub fn new(base: u8) -> Self {
        Self { timers: [Timer::default(); 4], base }
    }

    /// Which set an overflow event belongs to, and the timer within it.
    pub fn owns(&self, event_index: u8) -> Option<usize> {
        let n = event_index.wrapping_sub(self.base);
        (n < 4).then_some(n as usize)
    }

    pub fn read_io(&self, reg: u32, now: u64) -> u16 {
        let n = ((reg - 0x100) / 4) as usize;
        if reg & 2 == 0 {
            self.timers[n].current(now)
        } else {
            self.timers[n].control
        }
    }

    pub fn write_io(
        &mut self,
        reg: u32,
        value: u16,
        mask: u16,
        now: u64,
        scheduler: &mut Scheduler,
    ) {
        let n = ((reg - 0x100) / 4) as usize;
        let t = &mut self.timers[n];
        if reg & 2 == 0 {
            t.reload = (t.reload & !mask) | (value & mask);
            return;
        }
        let was_running = t.running();
        t.counter = t.current(now);
        t.latched_at = now;
        t.control = (t.control & !mask) | (value & mask & 0xC7);
        if t.running() && !was_running {
            t.counter = t.reload;
        }
        self.reschedule(n, scheduler);
    }

    fn reschedule(&mut self, n: usize, scheduler: &mut Scheduler) {
        let event = Event::Timer(self.base + n as u8);
        scheduler.cancel(event);
        let t = &self.timers[n];
        if t.running() && (!t.cascade() || n == 0) {
            scheduler.schedule_at(event, t.overflow_at());
        }
    }

    /// Timer `n` overflowed at `at`: reload, interrupt, cascade upwards.
    pub fn on_overflow(&mut self, n: usize, at: u64, scheduler: &mut Scheduler, irq: &mut Irq) {
        let mut n = n;
        loop {
            let t = &mut self.timers[n];
            t.counter = t.reload;
            t.latched_at = at;
            if t.control & (1 << 6) != 0 {
                irq.raise(Interrupt::timer(n));
            }
            if !t.cascade() || n == 0 {
                self.reschedule(n, scheduler);
            }
            n += 1;
            if n == 4 {
                break;
            }
            let next = &mut self.timers[n];
            if !(next.running() && next.cascade()) {
                break;
            }
            next.counter = next.counter.wrapping_add(1);
            if next.counter != 0 {
                break;
            }
        }
    }
}
