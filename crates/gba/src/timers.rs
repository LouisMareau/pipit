//! The four 16-bit timers (GBATEK "GBA Timers").
//!
//! Counters are not ticked; each running timer stores the counter value it had at a
//! known cycle and computes the current value on demand. Overflows are scheduler
//! events. Timers 0 and 1 also clock the Direct Sound FIFOs.

use crate::audio::Audio;
use crate::dma::Dma;
use crate::irq::{Interrupt, Irq};
use crate::scheduler::{Event, Scheduler};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
struct Timer {
    /// TMxCNT_L write value: loaded into the counter on start and overflow.
    reload: u16,
    /// TMxCNT_H: bits 0-1 prescaler, 2 count-up (cascade), 6 IRQ enable, 7 start.
    control: u16,
    /// Counter value at `latched_at`.
    counter: u16,
    /// Cycle at which `counter` was valid.
    latched_at: u64,
}

impl Timer {
    fn running(&self) -> bool {
        self.control & (1 << 7) != 0
    }

    fn cascade(&self) -> bool {
        self.control & (1 << 2) != 0
    }

    fn prescale(&self) -> u64 {
        [1, 64, 256, 1024][(self.control & 3) as usize]
    }

    /// Ticks elapsed between `latched_at` and `now`. The prescaler is a free-running
    /// divider, so ticks land on multiples of the prescale value.
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

    /// When the counter overflows, if it keeps counting from `latched_at`.
    fn overflow_at(&self) -> u64 {
        let p = self.prescale();
        let ticks_needed = 0x1_0000 - u64::from(self.counter);
        let first_tick = (self.latched_at / p + 1) * p;
        first_tick + (ticks_needed - 1) * p
    }
}

#[derive(Default, Serialize, Deserialize)]
pub struct Timers {
    timers: [Timer; 4],
}

impl Timers {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn read_io(&self, reg: u32, now: u64) -> u16 {
        let n = ((reg - 0x100) / 4) as usize;
        if reg & 2 == 0 {
            self.timers[n].current(now)
        } else {
            self.timers[n].control
        }
    }

    pub fn write_io(&mut self, reg: u32, value: u16, mask: u16, scheduler: &mut Scheduler) {
        let n = ((reg - 0x100) / 4) as usize;
        let now = scheduler.now();
        let t = &mut self.timers[n];
        if reg & 2 == 0 {
            t.reload = (t.reload & !mask) | (value & mask);
            return;
        }
        let was_running = t.running();
        // Latch the current count before the prescaler can change under it.
        t.counter = t.current(now);
        t.latched_at = now;
        t.control = (t.control & !mask) | (value & mask & 0xC7);
        if t.running() && !was_running {
            t.counter = t.reload;
        }
        self.reschedule(n, scheduler);
    }

    fn reschedule(&mut self, n: usize, scheduler: &mut Scheduler) {
        scheduler.cancel(Event::TimerOverflow(n as u8));
        let t = &self.timers[n];
        // Timer 0 ignores the cascade bit: there is nothing above it to count.
        if t.running() && (!t.cascade() || n == 0) {
            scheduler.schedule_at(Event::TimerOverflow(n as u8), t.overflow_at());
        }
    }

    /// Handles the overflow of timer `n` at cycle `at`.
    pub fn on_overflow(
        &mut self,
        n: usize,
        at: u64,
        scheduler: &mut Scheduler,
        irq: &mut Irq,
        audio: &mut Audio,
        dma: &mut Dma,
    ) {
        let mut n = n;
        loop {
            let t = &mut self.timers[n];
            t.counter = t.reload;
            t.latched_at = at;
            if t.control & (1 << 6) != 0 {
                irq.raise(Interrupt::timer(n));
            }
            if n < 2 {
                audio.on_timer_overflow(n, dma);
            }
            if !t.cascade() || n == 0 {
                self.reschedule(n, scheduler);
            }
            // Count-up timing: the next timer ticks once per overflow of this one.
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
