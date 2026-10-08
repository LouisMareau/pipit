//! Sound controller (GBATEK "GBA Sound Controller").
//!
//! Two Direct Sound FIFOs fed by DMA plus the four PSG channels in `psg.rs`. Output
//! is mixed at a fixed 32768 Hz into an interleaved stereo `i16` buffer that the
//! front-end drains once per frame.

mod psg;

use crate::dma::Dma;
use crate::scheduler::{Event, Scheduler};
use serde::{Deserialize, Serialize};

/// Output sample rate in Hz.
pub const SAMPLE_RATE: u32 = 32_768;
const CYCLES_PER_SAMPLE: u64 = (crate::CLOCK_HZ / SAMPLE_RATE) as u64;
/// The PSG frame sequencer runs at 512 Hz.
const CYCLES_PER_SEQUENCER_STEP: u64 = (crate::CLOCK_HZ / 512) as u64;

/// A 32-byte Direct Sound FIFO of signed 8-bit samples.
#[derive(Default, Serialize, Deserialize)]
struct Fifo {
    data: [i8; 32],
    read: usize,
    len: usize,
    /// Sample currently being output.
    current: i8,
}

impl Fifo {
    fn push(&mut self, byte: u8) {
        if self.len < 32 {
            self.data[(self.read + self.len) % 32] = byte as i8;
            self.len += 1;
        }
    }

    fn pop(&mut self) {
        if self.len > 0 {
            self.current = self.data[self.read];
            self.read = (self.read + 1) % 32;
            self.len -= 1;
        }
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

#[derive(Serialize, Deserialize)]
pub struct Audio {
    square1: psg::Square,
    square2: psg::Square,
    wave: psg::Wave,
    noise: psg::Noise,
    /// Raw register images for read-back (0x60-0x8E), masked to readable bits.
    regs: [u16; 0x18],
    fifo: [Fifo; 2],
    sequencer_step: u8,
    #[serde(skip)]
    samples: Vec<i16>,
}

impl Default for Audio {
    fn default() -> Self {
        Self::new()
    }
}

/// Bits that read back for each register from 0x60 to 0x8E (GBATEK).
const READ_MASK: [u16; 0x18] = [
    0x007F, 0x0000, 0xFFC0, 0x0000, 0x4000, 0x0000, 0xFFC0, 0x0000, // 0x60-0x6E
    0x4000, 0x0000, 0x00E0, 0x0000, 0xE000, 0x0000, 0x4000, 0x0000, // 0x70-0x7E
    0xFF77, 0x770F, 0x0080, 0x0000, 0xC3FE, 0x0000, 0x0000, 0x0000, // 0x80-0x8E
];

impl Audio {
    pub fn new() -> Self {
        let mut audio = Self {
            square1: psg::Square::new(),
            square2: psg::Square::new(),
            wave: psg::Wave::new(),
            noise: psg::Noise::new(),
            regs: [0; 0x18],
            fifo: Default::default(),
            sequencer_step: 0,
            samples: Vec::with_capacity(4096),
        };
        audio.regs[(0x88 - 0x60) / 2] = 0x200; // SOUNDBIAS default
        audio
    }

    pub fn schedule_first(&self, scheduler: &mut Scheduler) {
        scheduler.schedule_at(Event::AudioSample, CYCLES_PER_SAMPLE);
        scheduler.schedule_at(Event::AudioSequencer, CYCLES_PER_SEQUENCER_STEP);
    }

    /// Takes every sample produced since the last call.
    pub fn drain_samples(&mut self) -> Vec<i16> {
        std::mem::take(&mut self.samples)
    }

    fn reg(&self, addr: u32) -> u16 {
        self.regs[((addr - 0x60) / 2) as usize]
    }

    fn master_enabled(&self) -> bool {
        self.reg(0x84) & (1 << 7) != 0
    }

    /// A timer overflowed: FIFOs clocked by it advance one sample, and ask DMA for
    /// more data once they are half empty.
    pub fn on_timer_overflow(&mut self, timer: usize, dma: &mut Dma) {
        let cnt = self.reg(0x82);
        for i in 0..2 {
            let timer_select = (cnt >> (10 + 4 * i)) & 1;
            if usize::from(timer_select) == timer {
                self.fifo[i].pop();
                if self.fifo[i].len <= 16 {
                    dma.request_fifo(i);
                }
            }
        }
    }

    /// Frame sequencer: lengths at 256 Hz, sweep at 128 Hz, envelopes at 64 Hz.
    pub fn on_sequencer(&mut self, at: u64, scheduler: &mut Scheduler) {
        scheduler.schedule_at(Event::AudioSequencer, at + CYCLES_PER_SEQUENCER_STEP);
        if !self.master_enabled() {
            return;
        }
        let step = self.sequencer_step;
        self.sequencer_step = (step + 1) & 7;
        if step & 1 == 0 {
            self.square1.tick_length();
            self.square2.tick_length();
            self.wave.tick_length();
            self.noise.tick_length();
        }
        if step == 2 || step == 6 {
            self.square1.tick_sweep();
        }
        if step == 7 {
            self.square1.tick_envelope();
            self.square2.tick_envelope();
            self.noise.tick_envelope();
        }
    }

    pub fn on_sample(&mut self, at: u64, scheduler: &mut Scheduler) {
        scheduler.schedule_at(Event::AudioSample, at + CYCLES_PER_SAMPLE);
        let (mut left, mut right) = (0i32, 0i32);
        if self.master_enabled() {
            let cnt_l = self.reg(0x80);
            let cnt_h = self.reg(0x82);
            let cycles = CYCLES_PER_SAMPLE as i32;

            // PSG: each channel is a 4-bit DAC centred on zero; the master volume
            // (0-7) and the SOUNDCNT_H ratio (25/50/100 %) scale the sum.
            let outputs = [
                self.square1.sample(cycles),
                self.square2.sample(cycles),
                self.wave.sample(cycles),
                self.noise.sample(cycles),
            ];
            let (mut psg_l, mut psg_r) = (0i32, 0i32);
            for (i, out) in outputs.iter().enumerate() {
                if let Some(v) = out {
                    let centred = i32::from(*v) * 2 - 15;
                    if cnt_l & (1 << (12 + i)) != 0 {
                        psg_l += centred;
                    }
                    if cnt_l & (1 << (8 + i)) != 0 {
                        psg_r += centred;
                    }
                }
            }
            psg_l *= i32::from((cnt_l >> 4) & 7);
            psg_r *= i32::from(cnt_l & 7);
            let ratio_shift = match cnt_h & 3 {
                0 => 2,
                1 => 1,
                _ => 0,
            };
            left += psg_l >> ratio_shift;
            right += psg_r >> ratio_shift;

            // Direct Sound: 8-bit samples at 50 % or 100 %, into the 10-bit range.
            for i in 0..2 {
                let shift = if cnt_h & (1 << (2 + i)) != 0 { 2 } else { 1 };
                let sample = i32::from(self.fifo[i].current) << shift;
                if cnt_h & (1 << (9 + 4 * i)) != 0 {
                    left += sample;
                }
                if cnt_h & (1 << (8 + 4 * i)) != 0 {
                    right += sample;
                }
            }
        }
        // 10-bit output around the bias level, scaled to 16-bit.
        let scale = |v: i32| (v.clamp(-0x200, 0x1FF) << 6) as i16;
        self.samples.push(scale(left));
        self.samples.push(scale(right));
    }

    pub fn read_io(&self, reg: u32) -> u16 {
        match reg {
            0x84 => {
                let mut v = self.reg(0x84) & 0x80;
                v |= u16::from(self.square1.is_enabled());
                v |= u16::from(self.square2.is_enabled()) << 1;
                v |= u16::from(self.wave.is_enabled()) << 2;
                v |= u16::from(self.noise.is_enabled()) << 3;
                v
            }
            0x60..=0x8E => self.reg(reg) & READ_MASK[((reg - 0x60) / 2) as usize],
            0x90..=0x9E => {
                let bank = &self.wave.ram[self.wave.ram_bank_for_cpu()];
                let i = ((reg - 0x90) & 0xF) as usize;
                u16::from(bank[i]) | (u16::from(bank[i + 1]) << 8)
            }
            _ => 0, // FIFOs are write-only
        }
    }

    pub fn write_io(&mut self, reg: u32, value: u16, mask: u16) {
        match reg {
            0xA0 | 0xA2 | 0xA4 | 0xA6 => {
                let fifo = if reg < 0xA4 { 0 } else { 1 };
                for b in (value & mask).to_le_bytes() {
                    self.fifo[fifo].push(b);
                }
                return;
            }
            0x90..=0x9E => {
                let bank = self.wave.ram_bank_for_cpu();
                let i = ((reg - 0x90) & 0xF) as usize;
                if mask & 0x00FF != 0 {
                    self.wave.ram[bank][i] = value as u8;
                }
                if mask & 0xFF00 != 0 {
                    self.wave.ram[bank][i + 1] = (value >> 8) as u8;
                }
                return;
            }
            0x60..=0x8E => {}
            _ => return,
        }

        let i = ((reg - 0x60) / 2) as usize;
        // With the master switch off, only SOUNDCNT_X and SOUNDBIAS respond.
        if !self.master_enabled() && reg != 0x84 && reg != 0x88 {
            return;
        }
        let v = (self.regs[i] & !mask) | (value & mask);
        self.regs[i] = v;
        match reg {
            0x60 => self.square1.write_sweep(v),
            0x62 => self.square1.write_control(v),
            0x64 => self.square1.write_frequency(v),
            0x68 => self.square2.write_control(v),
            0x6C => self.square2.write_frequency(v),
            0x70 => self.wave.write_select(v),
            0x72 => self.wave.write_control(v),
            0x74 => self.wave.write_frequency(v),
            0x78 => self.noise.write_control(v),
            0x7C => self.noise.write_frequency(v),
            0x82 => {
                // Bits 11 and 15 reset the FIFOs; they always read back as 0.
                if v & (1 << 11) != 0 {
                    self.fifo[0].reset();
                }
                if v & (1 << 15) != 0 {
                    self.fifo[1].reset();
                }
                self.regs[i] &= !((1 << 11) | (1 << 15));
            }
            0x84 if v & (1 << 7) == 0 => {
                // Powering the PSG down clears its registers and state.
                for r in 0..(0x82 - 0x60) / 2 {
                    self.regs[r] = 0;
                }
                self.square1.power_off();
                self.square2.power_off();
                self.wave.power_off();
                self.noise.power_off();
                self.sequencer_step = 0;
            }
            _ => {}
        }
    }
}
