//! The sound unit (Pan Docs "Audio"): two pulse channels, the wave channel
//! and the noise channel, clocked by a 512 Hz frame sequencer for lengths,
//! envelopes and the sweep, mixed to stereo 16-bit samples at 32768 Hz.

use serde::{Deserialize, Serialize};

/// Clocks per output sample: 4194304 / 32768.
const SAMPLE_DIV: u32 = 128;
const SEQUENCER_DIV: u32 = 8192;
const DUTY: [u8; 4] = [0b0000_0001, 0b1000_0001, 0b1000_0111, 0b0111_1110];

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
struct Envelope {
    initial: u8,
    increase: bool,
    period: u8,
    timer: u8,
    volume: u8,
}

impl Envelope {
    fn write(&mut self, value: u8) {
        self.initial = value >> 4;
        self.increase = value & 0x08 != 0;
        self.period = value & 7;
    }

    fn read(&self) -> u8 {
        (self.initial << 4) | (u8::from(self.increase) << 3) | self.period
    }

    fn dac_on(&self) -> bool {
        self.initial != 0 || self.increase
    }

    fn trigger(&mut self) {
        self.volume = self.initial;
        self.timer = if self.period == 0 { 8 } else { self.period };
    }

    fn clock(&mut self) {
        if self.period == 0 {
            return;
        }
        self.timer = self.timer.saturating_sub(1);
        if self.timer == 0 {
            self.timer = self.period;
            if self.increase && self.volume < 15 {
                self.volume += 1;
            } else if !self.increase && self.volume > 0 {
                self.volume -= 1;
            }
        }
    }
}

/// Counts `timer` down by `t` clocks, calling `wrap` every time it reaches zero.
fn count_down(timer: &mut u32, period: u32, mut t: u32, mut wrap: impl FnMut()) {
    while t > 0 {
        let step = t.min(*timer);
        *timer -= step;
        t -= step;
        if *timer == 0 {
            *timer = period.max(1);
            wrap();
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Square {
    enabled: bool,
    duty: u8,
    length: u16,
    length_enable: bool,
    frequency: u16,
    timer: u32,
    phase: u8,
    envelope: Envelope,
    sweep_period: u8,
    sweep_negate: bool,
    sweep_shift: u8,
    sweep_timer: u8,
    sweep_shadow: u16,
    sweep_enabled: bool,
}

impl Square {
    fn period(&self) -> u32 {
        (2048 - u32::from(self.frequency)) * 4
    }

    fn step(&mut self, t: u32) {
        let period = self.period();
        let phase = &mut self.phase;
        count_down(&mut self.timer, period, t, || *phase = (*phase + 1) & 7);
    }

    fn output(&self) -> Option<u8> {
        if !self.enabled || !self.envelope.dac_on() {
            return None;
        }
        Some(if (DUTY[usize::from(self.duty)] >> self.phase) & 1 != 0 {
            self.envelope.volume
        } else {
            0
        })
    }

    fn trigger(&mut self) {
        self.enabled = self.envelope.dac_on();
        if self.length == 0 {
            self.length = 64;
        }
        self.timer = self.period();
        self.envelope.trigger();
        self.sweep_shadow = self.frequency;
        self.sweep_timer = if self.sweep_period == 0 { 8 } else { self.sweep_period };
        self.sweep_enabled = self.sweep_period != 0 || self.sweep_shift != 0;
        if self.sweep_shift != 0 && self.swept() > 2047 {
            self.enabled = false;
        }
    }

    fn swept(&self) -> u16 {
        let delta = self.sweep_shadow >> self.sweep_shift;
        if self.sweep_negate {
            self.sweep_shadow.wrapping_sub(delta)
        } else {
            self.sweep_shadow + delta
        }
    }

    fn clock_sweep(&mut self) {
        if !self.sweep_enabled || self.sweep_period == 0 {
            return;
        }
        self.sweep_timer -= 1;
        if self.sweep_timer == 0 {
            self.sweep_timer = self.sweep_period;
            let next = self.swept();
            if next > 2047 {
                self.enabled = false;
            } else if self.sweep_shift != 0 {
                self.sweep_shadow = next;
                self.frequency = next;
                if self.swept() > 2047 {
                    self.enabled = false;
                }
            }
        }
    }

    fn clock_length(&mut self) {
        if self.length_enable && self.length > 0 {
            self.length -= 1;
            if self.length == 0 {
                self.enabled = false;
            }
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Wave {
    enabled: bool,
    dac_on: bool,
    length: u16,
    length_enable: bool,
    frequency: u16,
    timer: u32,
    position: u8,
    volume_code: u8,
    ram: [u8; 16],
    sample: u8,
}

impl Wave {
    fn period(&self) -> u32 {
        (2048 - u32::from(self.frequency)) * 2
    }

    fn step(&mut self, t: u32) {
        let period = self.period();
        let (position, ram, sample) = (&mut self.position, &self.ram, &mut self.sample);
        count_down(&mut self.timer, period, t, || {
            *position = (*position + 1) & 31;
            let byte = ram[usize::from(*position / 2)];
            *sample = if *position & 1 == 0 { byte >> 4 } else { byte & 0x0F };
        });
    }

    fn output(&self) -> Option<u8> {
        if !self.enabled || !self.dac_on {
            return None;
        }
        Some(self.sample >> [4, 0, 1, 2][usize::from(self.volume_code)])
    }

    fn trigger(&mut self) {
        self.enabled = self.dac_on;
        if self.length == 0 {
            self.length = 256;
        }
        self.timer = self.period();
        self.position = 0;
    }

    fn clock_length(&mut self) {
        if self.length_enable && self.length > 0 {
            self.length -= 1;
            if self.length == 0 {
                self.enabled = false;
            }
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Noise {
    enabled: bool,
    length: u16,
    length_enable: bool,
    envelope: Envelope,
    clock_shift: u8,
    width7: bool,
    divisor_code: u8,
    timer: u32,
    lfsr: u16,
}

impl Noise {
    fn period(&self) -> u32 {
        let divisor = if self.divisor_code == 0 { 8 } else { u32::from(self.divisor_code) * 16 };
        divisor << self.clock_shift
    }

    fn step(&mut self, t: u32) {
        let period = self.period();
        let (lfsr, width7) = (&mut self.lfsr, self.width7);
        count_down(&mut self.timer, period, t, || {
            let bit = (*lfsr ^ (*lfsr >> 1)) & 1;
            *lfsr = (*lfsr >> 1) | (bit << 14);
            if width7 {
                *lfsr = (*lfsr & !0x40) | (bit << 6);
            }
        });
    }

    fn output(&self) -> Option<u8> {
        if !self.enabled || !self.envelope.dac_on() {
            return None;
        }
        Some(if self.lfsr & 1 == 0 { self.envelope.volume } else { 0 })
    }

    fn trigger(&mut self) {
        self.enabled = self.envelope.dac_on();
        if self.length == 0 {
            self.length = 64;
        }
        self.timer = self.period();
        self.lfsr = 0x7FFF;
        self.envelope.trigger();
    }

    fn clock_length(&mut self) {
        if self.length_enable && self.length > 0 {
            self.length -= 1;
            if self.length == 0 {
                self.enabled = false;
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct Audio {
    ch1: Square,
    ch2: Square,
    ch3: Wave,
    ch4: Noise,
    nr50: u8,
    nr51: u8,
    power: bool,
    sequencer_counter: u32,
    sequencer_step: u8,
    sample_counter: u32,
    #[serde(skip)]
    samples: Vec<i16>,
}

impl Default for Audio {
    fn default() -> Self {
        Self::new()
    }
}

impl Audio {
    /// Registers as the boot ROM leaves them: on, with the first pulse channel playing.
    pub fn new() -> Self {
        let mut audio = Self {
            ch1: Square::default(),
            ch2: Square::default(),
            ch3: Wave::default(),
            ch4: Noise::default(),
            nr50: 0,
            nr51: 0,
            power: true,
            sequencer_counter: 0,
            sequencer_step: 0,
            sample_counter: 0,
            samples: Vec::with_capacity(4096),
        };
        for (addr, value) in [
            (0xFF10, 0x80),
            (0xFF11, 0xBF),
            (0xFF12, 0xF3),
            (0xFF14, 0xBF),
            (0xFF21, 0x3F),
            (0xFF24, 0xBF),
            (0xFF30, 0x7F),
            (0xFF31, 0xFF),
            (0xFF32, 0x9F),
            (0xFF34, 0xBF),
            (0xFF41, 0xFF),
            (0xFF44, 0xBF),
            (0xFF50, 0x77),
            (0xFF51, 0xF3),
        ] {
            audio.write(addr, value);
        }
        audio
    }

    pub fn drain_samples(&mut self) -> Vec<i16> {
        std::mem::take(&mut self.samples)
    }

    /// `t` clocks pass.
    pub fn step(&mut self, t: u32) {
        if self.power {
            self.ch1.step(t);
            self.ch2.step(t);
            self.ch3.step(t);
            self.ch4.step(t);
            self.sequencer_counter += t;
            if self.sequencer_counter >= SEQUENCER_DIV {
                self.sequencer_counter -= SEQUENCER_DIV;
                self.sequencer_step = (self.sequencer_step + 1) & 7;
                if self.sequencer_step.is_multiple_of(2) {
                    self.ch1.clock_length();
                    self.ch2.clock_length();
                    self.ch3.clock_length();
                    self.ch4.clock_length();
                }
                if self.sequencer_step == 2 || self.sequencer_step == 6 {
                    self.ch1.clock_sweep();
                }
                if self.sequencer_step == 7 {
                    self.ch1.envelope.clock();
                    self.ch2.envelope.clock();
                    self.ch4.envelope.clock();
                }
            }
        }
        self.sample_counter += t;
        if self.sample_counter >= SAMPLE_DIV {
            self.sample_counter -= SAMPLE_DIV;
            let (left, right) = self.mix();
            self.samples.push(left);
            self.samples.push(right);
        }
    }

    /// Each playing channel's DAC swings −15..15 around silence; NR51 routes, NR50 scales.
    fn mix(&self) -> (i16, i16) {
        if !self.power {
            return (0, 0);
        }
        let outputs = [self.ch1.output(), self.ch2.output(), self.ch3.output(), self.ch4.output()];
        let (mut left, mut right) = (0i32, 0i32);
        for (i, out) in outputs.iter().enumerate() {
            let Some(level) = out else { continue };
            let level = i32::from(*level) * 2 - 15;
            if self.nr51 & (0x10 << i) != 0 {
                left += level;
            }
            if self.nr51 & (1 << i) != 0 {
                right += level;
            }
        }
        left *= i32::from((self.nr50 >> 4) & 7) + 1;
        right *= i32::from(self.nr50 & 7) + 1;
        ((left * 48) as i16, (right * 48) as i16)
    }

    pub fn read(&self, addr: u16) -> u8 {
        let (ch1, ch2, ch3, ch4) = (&self.ch1, &self.ch2, &self.ch3, &self.ch4);
        match addr {
            0xFF10 => {
                0x80 | (ch1.sweep_period << 4) | (u8::from(ch1.sweep_negate) << 3) | ch1.sweep_shift
            }
            0xFF11 => (ch1.duty << 6) | 0x3F,
            0xFF12 => ch1.envelope.read(),
            0xFF14 => 0xBF | (u8::from(ch1.length_enable) << 6),
            0xFF16 => (ch2.duty << 6) | 0x3F,
            0xFF17 => ch2.envelope.read(),
            0xFF19 => 0xBF | (u8::from(ch2.length_enable) << 6),
            0xFF1A => 0x7F | (u8::from(ch3.dac_on) << 7),
            0xFF1C => 0x9F | (ch3.volume_code << 5),
            0xFF1E => 0xBF | (u8::from(ch3.length_enable) << 6),
            0xFF21 => ch4.envelope.read(),
            0xFF22 => (ch4.clock_shift << 4) | (u8::from(ch4.width7) << 3) | ch4.divisor_code,
            0xFF23 => 0xBF | (u8::from(ch4.length_enable) << 6),
            0xFF24 => self.nr50,
            0xFF25 => self.nr51,
            0xFF26 => {
                0x70 | (u8::from(self.power) << 7)
                    | u8::from(ch1.enabled)
                    | (u8::from(ch2.enabled) << 1)
                    | (u8::from(ch3.enabled) << 2)
                    | (u8::from(ch4.enabled) << 3)
            }
            0xFF30..=0xFF3F => ch3.ram[usize::from(addr - 0xFF30)],
            _ => 0xFF,
        }
    }

    pub fn write(&mut self, addr: u16, value: u8) {
        if let 0xFF30..=0xFF3F = addr {
            self.ch3.ram[usize::from(addr - 0xFF30)] = value;
            return;
        }
        if addr == 0xFF26 {
            let on = value & 0x80 != 0;
            if self.power && !on {
                // Powering off clears every register but keeps the wave RAM.
                let ram = self.ch3.ram;
                *self = Self {
                    ch3: Wave { ram, ..Wave::default() },
                    power: false,
                    samples: std::mem::take(&mut self.samples),
                    ..Self::new()
                };
                self.power = false;
                self.ch1 = Square::default();
                self.ch2 = Square::default();
                self.ch4 = Noise::default();
                self.nr50 = 0;
                self.nr51 = 0;
            } else if !self.power && on {
                self.power = true;
                self.sequencer_step = 0;
            }
            return;
        }
        if !self.power {
            return;
        }
        let (ch1, ch2, ch3, ch4) = (&mut self.ch1, &mut self.ch2, &mut self.ch3, &mut self.ch4);
        match addr {
            0xFF10 => {
                ch1.sweep_period = (value >> 4) & 7;
                ch1.sweep_negate = value & 0x08 != 0;
                ch1.sweep_shift = value & 7;
            }
            0xFF11 | 0xFF16 => {
                let ch = if addr == 0xFF11 { ch1 } else { ch2 };
                ch.duty = value >> 6;
                ch.length = 64 - u16::from(value & 0x3F);
            }
            0xFF12 | 0xFF17 => {
                let ch = if addr == 0xFF12 { ch1 } else { ch2 };
                ch.envelope.write(value);
                if !ch.envelope.dac_on() {
                    ch.enabled = false;
                }
            }
            0xFF13 | 0xFF18 => {
                let ch = if addr == 0xFF13 { ch1 } else { ch2 };
                ch.frequency = (ch.frequency & 0x700) | u16::from(value);
            }
            0xFF14 | 0xFF19 => {
                let ch = if addr == 0xFF14 { ch1 } else { ch2 };
                ch.frequency = (ch.frequency & 0xFF) | (u16::from(value & 7) << 8);
                ch.length_enable = value & 0x40 != 0;
                if value & 0x80 != 0 {
                    ch.trigger();
                }
            }
            0xFF1A => {
                ch3.dac_on = value & 0x80 != 0;
                if !ch3.dac_on {
                    ch3.enabled = false;
                }
            }
            0xFF1B => ch3.length = 256 - u16::from(value),
            0xFF1C => ch3.volume_code = (value >> 5) & 3,
            0xFF1D => ch3.frequency = (ch3.frequency & 0x700) | u16::from(value),
            0xFF1E => {
                ch3.frequency = (ch3.frequency & 0xFF) | (u16::from(value & 7) << 8);
                ch3.length_enable = value & 0x40 != 0;
                if value & 0x80 != 0 {
                    ch3.trigger();
                }
            }
            0xFF20 => ch4.length = 64 - u16::from(value & 0x3F),
            0xFF21 => {
                ch4.envelope.write(value);
                if !ch4.envelope.dac_on() {
                    ch4.enabled = false;
                }
            }
            0xFF22 => {
                ch4.clock_shift = value >> 4;
                ch4.width7 = value & 0x08 != 0;
                ch4.divisor_code = value & 7;
            }
            0xFF23 => {
                ch4.length_enable = value & 0x40 != 0;
                if value & 0x80 != 0 {
                    ch4.trigger();
                }
            }
            0xFF24 => self.nr50 = value,
            0xFF25 => self.nr51 = value,
            _ => {}
        }
    }
}
