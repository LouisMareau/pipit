//! The four programmable sound channels inherited from the Game Boy
//! (GBATEK "GBA Sound Channel 1-4").
//!
//! Channels keep a phase countdown in CPU cycles and are advanced on demand when
//! the mixer takes a sample, so nothing ticks per cycle. The 512 Hz frame
//! sequencer drives length counters, envelopes and the sweep unit.

/// Common envelope: 4-bit volume stepping up or down every `period` 64 Hz ticks.
#[derive(Clone, Copy, Default)]
struct Envelope {
    initial: u8,
    increase: bool,
    period: u8,
    volume: u8,
    counter: u8,
}

impl Envelope {
    fn write(&mut self, bits: u16) {
        self.initial = (bits >> 12) as u8 & 0xF;
        self.increase = bits & (1 << 11) != 0;
        self.period = (bits >> 8) as u8 & 7;
    }

    fn trigger(&mut self) {
        self.volume = self.initial;
        self.counter = self.period;
    }

    fn tick(&mut self) {
        if self.period == 0 {
            return;
        }
        self.counter = self.counter.saturating_sub(1);
        if self.counter == 0 {
            self.counter = self.period;
            if self.increase && self.volume < 15 {
                self.volume += 1;
            } else if !self.increase && self.volume > 0 {
                self.volume -= 1;
            }
        }
    }

    /// A channel whose DAC is off (volume 0, decreasing) produces no sound at all.
    fn dac_on(&self) -> bool {
        self.initial != 0 || self.increase
    }
}

/// Length counter shared by all channels: silences the channel when it expires.
#[derive(Clone, Copy, Default)]
struct Length {
    counter: u16,
    enabled: bool,
    max: u16,
}

impl Length {
    fn load(&mut self, value: u16) {
        self.counter = self.max - value;
    }

    /// Returns true when the channel should stop.
    fn tick(&mut self) -> bool {
        if self.enabled && self.counter > 0 {
            self.counter -= 1;
            return self.counter == 0;
        }
        false
    }

    fn trigger(&mut self) {
        if self.counter == 0 {
            self.counter = self.max;
        }
    }
}

const DUTY: [[u8; 8]; 4] = [
    [0, 0, 0, 0, 0, 0, 0, 1],
    [1, 0, 0, 0, 0, 0, 0, 1],
    [1, 0, 0, 0, 0, 1, 1, 1],
    [0, 1, 1, 1, 1, 1, 1, 0],
];

#[derive(Default)]
pub struct Square {
    enabled: bool,
    duty: u8,
    frequency: u16,
    envelope: Envelope,
    length: Length,
    phase: u8,
    countdown: i32,
    // Sweep (channel 1 only)
    sweep_shift: u8,
    sweep_decrease: bool,
    sweep_period: u8,
    sweep_counter: u8,
    sweep_shadow: u16,
    sweep_enabled: bool,
}

impl Square {
    pub fn new() -> Self {
        Self { length: Length { max: 64, ..Default::default() }, ..Default::default() }
    }

    fn period(&self) -> i32 {
        16 * (2048 - i32::from(self.frequency))
    }

    pub fn write_sweep(&mut self, bits: u16) {
        self.sweep_shift = bits as u8 & 7;
        self.sweep_decrease = bits & (1 << 3) != 0;
        self.sweep_period = (bits >> 4) as u8 & 7;
    }

    pub fn write_control(&mut self, bits: u16) {
        self.length.load(bits & 0x3F);
        self.duty = (bits >> 6) as u8 & 3;
        self.envelope.write(bits);
        if !self.envelope.dac_on() {
            self.enabled = false;
        }
    }

    pub fn write_frequency(&mut self, bits: u16) {
        self.frequency = bits & 0x7FF;
        self.length.enabled = bits & (1 << 14) != 0;
        if bits & (1 << 15) != 0 {
            self.trigger();
        }
    }

    fn trigger(&mut self) {
        self.enabled = self.envelope.dac_on();
        self.length.trigger();
        self.envelope.trigger();
        self.countdown = self.period();
        self.sweep_shadow = self.frequency;
        self.sweep_counter = if self.sweep_period == 0 { 8 } else { self.sweep_period };
        self.sweep_enabled = self.sweep_period != 0 || self.sweep_shift != 0;
        if self.sweep_shift != 0 && self.sweep_next() > 2047 {
            self.enabled = false;
        }
    }

    fn sweep_next(&self) -> u16 {
        let delta = self.sweep_shadow >> self.sweep_shift;
        if self.sweep_decrease {
            self.sweep_shadow.wrapping_sub(delta)
        } else {
            self.sweep_shadow + delta
        }
    }

    pub fn tick_sweep(&mut self) {
        if !self.sweep_enabled || !self.enabled {
            return;
        }
        self.sweep_counter -= 1;
        if self.sweep_counter == 0 {
            self.sweep_counter = if self.sweep_period == 0 { 8 } else { self.sweep_period };
            if self.sweep_period != 0 {
                let next = self.sweep_next();
                if next > 2047 {
                    self.enabled = false;
                } else if self.sweep_shift != 0 {
                    self.sweep_shadow = next;
                    self.frequency = next;
                    if self.sweep_next() > 2047 {
                        self.enabled = false;
                    }
                }
            }
        }
    }

    pub fn tick_length(&mut self) {
        if self.length.tick() {
            self.enabled = false;
        }
    }

    pub fn tick_envelope(&mut self) {
        self.envelope.tick();
    }

    /// Advances by `cycles` and returns the output (0-15), or `None` when the DAC is off.
    pub fn sample(&mut self, cycles: i32) -> Option<u8> {
        if !self.envelope.dac_on() {
            return None;
        }
        if !self.enabled {
            return Some(0);
        }
        self.countdown -= cycles;
        let period = self.period();
        while self.countdown <= 0 {
            self.countdown += period;
            self.phase = (self.phase + 1) & 7;
        }
        Some(DUTY[self.duty as usize][self.phase as usize] * self.envelope.volume)
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn power_off(&mut self) {
        *self = Self::new();
    }
}

#[derive(Default)]
pub struct Wave {
    enabled: bool,
    playing: bool,
    two_banks: bool,
    bank: usize,
    volume: u8,
    force_75: bool,
    frequency: u16,
    length: Length,
    /// Both 16-byte banks, 32 4-bit samples each.
    pub ram: [[u8; 16]; 2],
    position: usize,
    countdown: i32,
}

impl Wave {
    pub fn new() -> Self {
        Self { length: Length { max: 256, ..Default::default() }, ..Default::default() }
    }

    fn period(&self) -> i32 {
        8 * (2048 - i32::from(self.frequency))
    }

    pub fn write_select(&mut self, bits: u16) {
        self.two_banks = bits & (1 << 5) != 0;
        self.bank = usize::from((bits >> 6) & 1);
        self.playing = bits & (1 << 7) != 0;
        if !self.playing {
            self.enabled = false;
        }
    }

    pub fn write_control(&mut self, bits: u16) {
        self.length.load(bits & 0xFF);
        self.volume = (bits >> 13) as u8 & 3;
        self.force_75 = bits & (1 << 15) != 0;
    }

    pub fn write_frequency(&mut self, bits: u16) {
        self.frequency = bits & 0x7FF;
        self.length.enabled = bits & (1 << 14) != 0;
        if bits & (1 << 15) != 0 {
            self.enabled = self.playing;
            self.length.trigger();
            self.position = 0;
            self.countdown = self.period();
        }
    }

    /// Wave RAM accesses go to the bank that is *not* selected for playback.
    pub fn ram_bank_for_cpu(&self) -> usize {
        self.bank ^ 1
    }

    pub fn tick_length(&mut self) {
        if self.length.tick() {
            self.enabled = false;
        }
    }

    pub fn sample(&mut self, cycles: i32) -> Option<u8> {
        if !self.playing {
            return None;
        }
        if !self.enabled {
            return Some(0);
        }
        self.countdown -= cycles;
        let period = self.period();
        let samples = if self.two_banks { 64 } else { 32 };
        while self.countdown <= 0 {
            self.countdown += period;
            self.position = (self.position + 1) % samples;
        }
        let bank = if self.two_banks { (self.bank + self.position / 32) & 1 } else { self.bank };
        let byte = self.ram[bank][(self.position % 32) / 2];
        let raw = if self.position & 1 == 0 { byte >> 4 } else { byte & 0xF };
        Some(if self.force_75 {
            raw * 3 / 4
        } else {
            match self.volume {
                0 => 0,
                1 => raw,
                2 => raw >> 1,
                _ => raw >> 2,
            }
        })
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn power_off(&mut self) {
        let ram = self.ram;
        *self = Self::new();
        self.ram = ram;
    }
}

#[derive(Default)]
pub struct Noise {
    enabled: bool,
    envelope: Envelope,
    length: Length,
    divisor: u8,
    shift: u8,
    narrow: bool,
    lfsr: u16,
    countdown: i32,
}

impl Noise {
    pub fn new() -> Self {
        Self {
            length: Length { max: 64, ..Default::default() },
            lfsr: 0x7FFF,
            ..Default::default()
        }
    }

    fn period(&self) -> i32 {
        let r = if self.divisor == 0 { 8 } else { 16 * i32::from(self.divisor) };
        r << (self.shift + 1)
    }

    pub fn write_control(&mut self, bits: u16) {
        self.length.load(bits & 0x3F);
        self.envelope.write(bits);
        if !self.envelope.dac_on() {
            self.enabled = false;
        }
    }

    pub fn write_frequency(&mut self, bits: u16) {
        self.divisor = bits as u8 & 7;
        self.narrow = bits & (1 << 3) != 0;
        self.shift = (bits >> 4) as u8 & 0xF;
        self.length.enabled = bits & (1 << 14) != 0;
        if bits & (1 << 15) != 0 {
            self.enabled = self.envelope.dac_on();
            self.length.trigger();
            self.envelope.trigger();
            self.lfsr = if self.narrow { 0x7F } else { 0x7FFF };
            self.countdown = self.period();
        }
    }

    pub fn tick_length(&mut self) {
        if self.length.tick() {
            self.enabled = false;
        }
    }

    pub fn tick_envelope(&mut self) {
        self.envelope.tick();
    }

    pub fn sample(&mut self, cycles: i32) -> Option<u8> {
        if !self.envelope.dac_on() {
            return None;
        }
        if !self.enabled {
            return Some(0);
        }
        self.countdown -= cycles;
        let period = self.period();
        while self.countdown <= 0 {
            self.countdown += period;
            let bit = (self.lfsr ^ (self.lfsr >> 1)) & 1;
            self.lfsr >>= 1;
            if self.narrow {
                self.lfsr = (self.lfsr & !0x40) | (bit << 6);
            } else {
                self.lfsr = (self.lfsr & !0x4000) | (bit << 14);
            }
        }
        Some(if self.lfsr & 1 == 0 { self.envelope.volume } else { 0 })
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn power_off(&mut self) {
        *self = Self::new();
    }
}
