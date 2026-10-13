//! The sound controller (GBATEK "DS Sound"): sixteen channels playing PCM8,
//! PCM16 or IMA-ADPCM from memory, or square waves (8–13) and noise (14–15);
//! the mixer with each channel's volume and pan and the master volume; and
//! the two capture units that write the mix back to memory, which games use
//! for echo and reverb.
//!
//! The mix runs at the hardware's rate: one stereo sample every 1024 bus
//! cycles (32.7 kHz, what the front-ends play as 32768 Hz), into an
//! interleaved `i16` buffer drained once per frame. Each channel's timer
//! advances 512 ticks per mix sample and yields a source sample whenever it
//! wraps, so channels at any rate are read at the right pace.

use serde::{Deserialize, Serialize};

/// ARM9 cycles between mix samples: 1024 bus cycles.
pub const CYCLES_PER_SAMPLE: u64 = 2048;
/// Channel timer ticks per mix sample: the timers run at half the bus clock.
const TICKS_PER_SAMPLE: u32 = 512;

/// Where the channels read their samples and the captures write theirs.
pub trait Memory {
    fn read8(&self, addr: u32) -> u8;
    fn read16(&self, addr: u32) -> u16;
    fn read32(&self, addr: u32) -> u32;
    fn write8(&mut self, addr: u32, value: u8);
    fn write16(&mut self, addr: u32, value: u16);
}

const ADPCM_STEPS: [u16; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449,
    494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272,
    2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493,
    10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];
const ADPCM_INDEX: [i8; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];

/// An IMA-ADPCM decoder's state: the last sample and the step table index.
#[derive(Clone, Copy, Default, Serialize, Deserialize)]
struct Adpcm {
    value: i32,
    index: u8,
}

impl Adpcm {
    fn decode(&mut self, nibble: u8) -> i16 {
        let step = i32::from(ADPCM_STEPS[usize::from(self.index)]);
        let mut diff = step >> 3;
        if nibble & 1 != 0 {
            diff += step >> 2;
        }
        if nibble & 2 != 0 {
            diff += step >> 1;
        }
        if nibble & 4 != 0 {
            diff += step;
        }
        self.value = if nibble & 8 != 0 {
            (self.value - diff).max(-0x7FFF)
        } else {
            (self.value + diff).min(0x7FFF)
        };
        self.index = (i32::from(self.index) + i32::from(ADPCM_INDEX[usize::from(nibble & 7)]))
            .clamp(0, 88) as u8;
        self.value as i16
    }
}

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
pub struct Channel {
    /// SOUNDxCNT: volume (0-6), volume shift (8-9), hold (15), pan (16-22),
    /// duty (24-26), repeat mode (27-28), format (29-30), start/busy (31).
    pub cnt: u32,
    pub sad: u32,
    pub tmr: u16,
    pub pnt: u16,
    pub len: u32,
    /// Ticks towards the next source sample.
    timer: u32,
    /// The next source sample: a byte, halfword or nibble index, or the
    /// square wave's step.
    pos: u32,
    /// The source sample being output, before volume.
    current: i16,
    adpcm: Adpcm,
    /// The decoder's state at the loop point, restored on each repeat.
    adpcm_loop: Option<Adpcm>,
    lfsr: u16,
}

impl Channel {
    #[inline(always)]
    pub fn running(&self) -> bool {
        self.cnt & (1 << 31) != 0
    }

    fn format(&self) -> u32 {
        (self.cnt >> 29) & 3
    }

    /// Timer ticks per source sample.
    fn period(&self) -> u32 {
        0x1_0000 - u32::from(self.tmr)
    }

    /// Where the loop starts and where the data ends, in source samples.
    fn bounds(&self) -> (u32, u32) {
        let pnt = u32::from(self.pnt);
        let end = pnt + (self.len & 0x3F_FFFF);
        match self.format() {
            0 => (pnt * 4, end * 4),
            1 => (pnt * 2, end * 2),
            2 => ((pnt * 8).saturating_sub(8), (end * 8).saturating_sub(8)),
            _ => (0, u32::MAX),
        }
    }

    fn start(&mut self, mem: &impl Memory) {
        self.timer = 0;
        self.pos = 0;
        self.current = 0;
        self.adpcm_loop = None;
        self.lfsr = 0x7FFF;
        if self.format() == 2 {
            let header = mem.read32(self.sad);
            self.adpcm = Adpcm {
                value: i32::from(header as u16 as i16),
                index: ((header >> 16) & 0x7F).min(88) as u8,
            };
        }
    }

    /// The timer wrapped: fetch the next source sample. Past the end of the
    /// data a looping channel goes back to its loop point, a one-shot one
    /// stops (keeping its last sample only with the hold bit).
    fn fetch(&mut self, n: usize, mem: &impl Memory) {
        let format = self.format();
        if format == 3 {
            if n >= 14 {
                // Noise: a 15-bit shift register, low on each feedback.
                if self.lfsr & 1 != 0 {
                    self.lfsr = (self.lfsr >> 1) ^ 0x6000;
                    self.current = -0x7FFF;
                } else {
                    self.lfsr >>= 1;
                    self.current = 0x7FFF;
                }
            } else if n >= 8 {
                // Square wave: `duty` + 1 eighths high, except 7, always low.
                let duty = (self.cnt >> 24) & 7;
                self.pos = (self.pos + 1) & 7;
                self.current = if duty < 7 && self.pos >= 7 - duty { 0x7FFF } else { -0x7FFF };
            } else {
                self.current = 0;
            }
            return;
        }
        let (loop_start, end) = self.bounds();
        if self.pos >= end {
            if (self.cnt >> 27) & 3 == 2 {
                self.cnt &= !(1 << 31);
                if self.cnt & (1 << 15) == 0 {
                    self.current = 0;
                }
                return;
            }
            self.pos = loop_start;
            if let Some(state) = self.adpcm_loop {
                self.adpcm = state;
            }
        }
        match format {
            0 => self.current = i16::from(mem.read8(self.sad.wrapping_add(self.pos)) as i8) << 8,
            1 => self.current = mem.read16(self.sad.wrapping_add(self.pos * 2)) as i16,
            _ => {
                if self.pos == loop_start && self.adpcm_loop.is_none() {
                    self.adpcm_loop = Some(self.adpcm);
                }
                let byte = mem.read8(self.sad.wrapping_add(4 + self.pos / 2));
                let nibble = if self.pos & 1 != 0 { byte >> 4 } else { byte & 0xF };
                self.current = self.adpcm.decode(nibble);
            }
        }
        self.pos += 1;
    }

    /// The channel's output after its volume: what the mixer and captures see.
    fn output(&self) -> i32 {
        let volume = (self.cnt & 0x7F) as i32;
        let shift = [0, 1, 2, 4][((self.cnt >> 8) & 3) as usize];
        ((i32::from(self.current) * volume) >> 7) >> shift
    }

    fn pan(&self) -> i32 {
        ((self.cnt >> 16) & 0x7F) as i32
    }
}

/// A capture unit: writes the mix (or a channel) to memory at the rate of
/// channel 1 (capture 0) or channel 3 (capture 1).
#[derive(Clone, Copy, Default, Serialize, Deserialize)]
pub struct Capture {
    /// SNDCAPxCNT: bit 0 add the channel, 1 source (0 mixer, 1 channel 0/2),
    /// 2 one-shot, 3 PCM8, 7 start/busy.
    pub cnt: u8,
    pub dad: u32,
    pub len: u32,
    timer: u32,
    /// Bytes written so far.
    pos: u32,
}

#[derive(Serialize, Deserialize)]
pub struct Sound {
    pub channels: [Channel; 16],
    pub captures: [Capture; 2],
    /// SOUNDCNT: master volume (0-6), left and right output source (8-11),
    /// channels 1 and 3 kept out of the mixer (12-13), enable (15).
    pub soundcnt: u16,
    pub soundbias: u16,
    /// Interleaved stereo output since the last drain.
    #[serde(skip)]
    samples: Vec<i16>,
}

impl Default for Sound {
    fn default() -> Self {
        Self::new()
    }
}

impl Sound {
    pub fn new() -> Self {
        Self {
            channels: [Channel::default(); 16],
            captures: [Capture::default(); 2],
            soundcnt: 0,
            soundbias: 0x200,
            samples: Vec::with_capacity(2048),
        }
    }

    pub fn start(&self, scheduler: &mut crate::scheduler::Scheduler) {
        scheduler.schedule_at(crate::scheduler::Event::AudioSample, CYCLES_PER_SAMPLE);
    }

    /// Takes every sample produced since the last call.
    pub fn drain_samples(&mut self) -> Vec<i16> {
        std::mem::take(&mut self.samples)
    }

    /// One mix sample: every running channel advances and the mix is written
    /// to the output and the captures.
    pub fn sample(&mut self, mem: &mut impl Memory) {
        let enabled = self.soundcnt & (1 << 15) != 0;
        let (mut left, mut right) = (0i32, 0i32);
        // Each channel's output as (left, right) after its pan.
        let mut panned = [(0i32, 0i32); 16];
        for (n, ch) in self.channels.iter_mut().enumerate() {
            if !ch.running() {
                continue;
            }
            ch.timer += TICKS_PER_SAMPLE;
            let period = ch.period();
            while ch.timer >= period && ch.running() {
                ch.timer -= period;
                ch.fetch(n, mem);
            }
            let out = ch.output();
            let pan = ch.pan();
            panned[n] = ((out * (128 - pan)) >> 7, (out * pan) >> 7);
            let excluded = (n == 1 && self.soundcnt & (1 << 12) != 0)
                || (n == 3 && self.soundcnt & (1 << 13) != 0);
            if !excluded {
                left += panned[n].0;
                right += panned[n].1;
            }
        }

        if enabled {
            for i in 0..2 {
                let (mix, source, extra, period) = if i == 0 {
                    (left, panned[0].0, panned[1].0, self.channels[1].period())
                } else {
                    (right, panned[2].1, panned[3].1, self.channels[3].period())
                };
                self.capture(i, mem, mix, source, extra, period);
            }
        }

        let select = |bits: u16, mix: i32, ch1: i32, ch3: i32| match bits & 3 {
            0 => mix,
            1 => ch1,
            2 => ch3,
            _ => ch1 + ch3,
        };
        let (mut out_l, mut out_r) = if enabled {
            (
                select(self.soundcnt >> 8, left, panned[1].0, panned[3].0),
                select(self.soundcnt >> 10, right, panned[1].1, panned[3].1),
            )
        } else {
            (0, 0)
        };
        let master = i32::from(self.soundcnt & 0x7F);
        out_l = (out_l * master) >> 7;
        out_r = (out_r * master) >> 7;
        self.samples.push(out_l.clamp(-0x8000, 0x7FFF) as i16);
        self.samples.push(out_r.clamp(-0x8000, 0x7FFF) as i16);
    }

    fn capture(
        &mut self,
        i: usize,
        mem: &mut impl Memory,
        mix: i32,
        source: i32,
        extra: i32,
        period: u32,
    ) {
        let cap = &mut self.captures[i];
        if cap.cnt & 0x80 == 0 {
            return;
        }
        cap.timer += TICKS_PER_SAMPLE;
        while cap.timer >= period && cap.cnt & 0x80 != 0 {
            cap.timer -= period;
            // Bit 0 adds channel 1 (or 3) to what is captured; whether that is
            // exactly the hardware's meaning of the bit is not documented.
            let mut value = if cap.cnt & 2 != 0 { source } else { mix };
            if cap.cnt & 1 != 0 {
                value += extra;
            }
            let value = value.clamp(-0x8000, 0x7FFF);
            let bytes = (cap.len & 0xFFFF).max(1) * 4;
            if cap.cnt & 8 != 0 {
                mem.write8(cap.dad.wrapping_add(cap.pos), (value >> 8) as u8);
                cap.pos += 1;
            } else {
                mem.write16(cap.dad.wrapping_add(cap.pos), value as u16);
                cap.pos += 2;
            }
            if cap.pos >= bytes {
                cap.pos = 0;
                if cap.cnt & 4 != 0 {
                    cap.cnt &= !0x80;
                }
            }
        }
    }

    /// The registers at 400h–51Fh as 16-bit halves; only the controls read back.
    pub fn read_io(&self, reg: u32) -> u16 {
        match reg {
            0x400..=0x4FF => {
                let ch = &self.channels[((reg - 0x400) / 16) as usize];
                match reg & 0xF {
                    0x0 => ch.cnt as u16,
                    0x2 => (ch.cnt >> 16) as u16,
                    _ => 0,
                }
            }
            0x500 => self.soundcnt,
            0x504 => self.soundbias,
            0x508 => u16::from(self.captures[0].cnt) | u16::from(self.captures[1].cnt) << 8,
            _ => 0,
        }
    }

    pub fn write_io(&mut self, reg: u32, value: u16, mask: u16, mem: &impl Memory) {
        let v = u32::from(value & mask);
        let m = u32::from(mask);
        match reg {
            0x400..=0x4FF => {
                let ch = &mut self.channels[((reg - 0x400) / 16) as usize];
                match reg & 0xF {
                    0x0 => ch.cnt = (ch.cnt & !m) | (v & 0xFF7F),
                    0x2 => {
                        let was_running = ch.running();
                        ch.cnt = (ch.cnt & !(m << 16)) | ((v & 0xFF7F) << 16);
                        if ch.running() && !was_running {
                            ch.start(mem);
                        }
                    }
                    0x4 => ch.sad = (ch.sad & !m) | (v & 0xFFFC),
                    0x6 => ch.sad = (ch.sad & !(m << 16)) | ((v & 0x07FF) << 16),
                    0x8 => ch.tmr = (ch.tmr & !mask) | (value & mask),
                    0xA => ch.pnt = (ch.pnt & !mask) | (value & mask),
                    0xC => ch.len = (ch.len & !m) | v,
                    0xE => ch.len = (ch.len & !(m << 16)) | ((v & 0x3F) << 16),
                    _ => {}
                }
            }
            0x500 => self.soundcnt = (self.soundcnt & !mask) | (value & mask & 0xBF7F),
            0x504 => self.soundbias = (self.soundbias & !mask) | (value & mask & 0x3FF),
            0x508 => {
                for (i, shift) in [(0, 0), (1, 8)] {
                    if mask & (0xFF << shift) == 0 {
                        continue;
                    }
                    let cap = &mut self.captures[i];
                    let was_running = cap.cnt & 0x80 != 0;
                    cap.cnt = (value >> shift) as u8 & 0x8F;
                    if cap.cnt & 0x80 != 0 && !was_running {
                        cap.timer = 0;
                        cap.pos = 0;
                    }
                }
            }
            0x510 | 0x518 => {
                let cap = &mut self.captures[usize::from(reg == 0x518)];
                cap.dad = (cap.dad & !m) | (v & 0xFFFC);
            }
            0x512 | 0x51A => {
                let cap = &mut self.captures[usize::from(reg == 0x51A)];
                cap.dad = (cap.dad & !(m << 16)) | ((v & 0x07FF) << 16);
            }
            0x514 | 0x51C => {
                let cap = &mut self.captures[usize::from(reg == 0x51C)];
                cap.len = (cap.len & !m) | v;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Memory as a flat buffer at 02000000h.
    struct Ram(Vec<u8>);

    impl Memory for Ram {
        fn read8(&self, addr: u32) -> u8 {
            self.0[(addr - 0x0200_0000) as usize]
        }
        fn read16(&self, addr: u32) -> u16 {
            u16::from_le_bytes([self.read8(addr), self.read8(addr + 1)])
        }
        fn read32(&self, addr: u32) -> u32 {
            u32::from(self.read16(addr)) | u32::from(self.read16(addr + 2)) << 16
        }
        fn write8(&mut self, addr: u32, value: u8) {
            self.0[(addr - 0x0200_0000) as usize] = value;
        }
        fn write16(&mut self, addr: u32, value: u16) {
            self.write8(addr, value as u8);
            self.write8(addr + 1, (value >> 8) as u8);
        }
    }

    fn ram() -> Ram {
        Ram(vec![0; 0x1000])
    }

    /// Channel `n` reading from 02000000h, one source sample per mix sample,
    /// full volume, centred, with the master volume at its maximum.
    fn setup(sound: &mut Sound, mem: &Ram, n: usize, format: u32, repeat: u32, pnt: u16, len: u32) {
        let base = 0x400 + n as u32 * 16;
        sound.write_io(0x500, 0x807F, 0xFFFF, mem);
        sound.write_io(base + 4, 0x0000, 0xFFFF, mem);
        sound.write_io(base + 6, 0x0200, 0xFFFF, mem);
        sound.write_io(base + 8, (0x1_0000 - 512) as u16, 0xFFFF, mem);
        sound.write_io(base + 0xA, pnt, 0xFFFF, mem);
        sound.write_io(base + 0xC, len as u16, 0xFFFF, mem);
        sound.write_io(base, 0x007F, 0xFFFF, mem);
        sound.write_io(base + 2, (0x8000 | format << 13 | repeat << 11 | 64) as u16, 0xFFFF, mem);
    }

    /// `(left, right)` of the next mix sample.
    fn next(sound: &mut Sound, mem: &mut Ram) -> (i32, i32) {
        sound.sample(mem);
        let s = sound.drain_samples();
        (i32::from(s[0]), i32::from(s[1]))
    }

    /// A full-scale source sample after a channel volume of 127, a centre
    /// pan and the master volume of 127.
    fn scaled(source: i32) -> i32 {
        (((((source * 127) >> 7) * 64) >> 7) * 127) >> 7
    }

    #[test]
    fn pcm8_plays_bytes_and_loops() {
        let mut mem = ram();
        mem.0[..4].copy_from_slice(&[0x7F, 0x80, 0x10, 0x00]);
        let mut sound = Sound::new();
        setup(&mut sound, &mem, 0, 0, 1, 0, 1);
        let samples: Vec<i32> = (0..6).map(|_| next(&mut sound, &mut mem).0).collect();
        let expect = |b: i8| scaled(i32::from(b) << 8);
        assert_eq!(
            samples,
            [expect(0x7F), expect(-0x80), expect(0x10), 0, expect(0x7F), expect(-0x80)]
        );
        assert!(sound.channels[0].running());
    }

    #[test]
    fn pcm16_one_shot_stops_at_the_end() {
        let mut mem = ram();
        for (i, v) in [1000i16, -1000, 2000, -2000].iter().enumerate() {
            mem.write16(0x0200_0000 + i as u32 * 2, *v as u16);
        }
        let mut sound = Sound::new();
        setup(&mut sound, &mem, 5, 1, 2, 0, 2);
        let samples: Vec<i32> = (0..6).map(|_| next(&mut sound, &mut mem).1).collect();
        assert_eq!(samples, [scaled(1000), scaled(-1000), scaled(2000), scaled(-2000), 0, 0]);
        assert!(!sound.channels[5].running());
        assert_eq!(sound.read_io(0x450 + 2) & 0x8000, 0);
    }

    #[test]
    fn adpcm_decodes_and_restores_the_loop_state() {
        let mut mem = ram();
        // Header: sample 0, step index 0; then nibbles 7, 7 (up), 8 (down), 0.
        mem.write16(0x0200_0004, 0x0877);
        let mut sound = Sound::new();
        setup(&mut sound, &mem, 2, 2, 1, 1, 1);
        // Step 7: +(0 + 1 + 3 + 7) = 11, index to 8 (step 16); then +(2 + 4 + 8 + 16) = 30.
        let first: Vec<i32> = (0..8).map(|_| next(&mut sound, &mut mem).0).collect();
        assert_eq!(first[0], scaled(11));
        assert_eq!(first[1], scaled(41));
        assert!(first[2] < first[1]);
        // The loop restarts from the header's state: the same values again.
        let again: Vec<i32> = (0..2).map(|_| next(&mut sound, &mut mem).0).collect();
        assert_eq!(again, first[..2]);
    }

    #[test]
    fn square_wave_and_noise() {
        let mut mem = ram();
        let mut sound = Sound::new();
        setup(&mut sound, &mem, 8, 3, 1, 0, 0);
        sound.write_io(0x480 + 2, 0x8000 | 3 << 13 | 3 << 8 | 64, 0xFFFF, &mem); // duty 3: half high
        let wave: Vec<bool> = (0..8).map(|_| next(&mut sound, &mut mem).0 > 0).collect();
        assert_eq!(wave, [false, false, false, true, true, true, true, false]);
        setup(&mut sound, &mem, 14, 3, 1, 0, 0);
        let noise: Vec<i32> = (0..4).map(|_| next(&mut sound, &mut mem).0).collect();
        // Channel 8 is low again for its next three steps; the noise starts low.
        assert_eq!(noise[0], 2 * scaled(-0x7FFF));
        assert!(noise.iter().any(|&v| v != noise[0]));
    }

    #[test]
    fn pan_master_and_output_selection() {
        let mut mem = ram();
        mem.0[..4].fill(0x40);
        let mut sound = Sound::new();
        setup(&mut sound, &mem, 1, 0, 1, 0, 1);
        sound.write_io(0x412, 0x8000 | 1 << 11 | 0, 0xFFFF, &mem); // pan 0: all left
        let (l, r) = next(&mut sound, &mut mem);
        assert!(l > 0 && r == 0);
        // Half the master volume halves the output.
        sound.write_io(0x500, 0x8040, 0xFFFF, &mem);
        let (half, _) = next(&mut sound, &mut mem);
        assert_eq!(half, ((l * 128 / 127) * 64) >> 7);
        // The left output can take channel 1 directly while the mixer drops it.
        sound.write_io(0x500, 0x907F | 1 << 8, 0xFFFF, &mem);
        let (direct, _) = next(&mut sound, &mut mem);
        assert_eq!(direct, l);
        sound.write_io(0x500, 0x907F, 0xFFFF, &mem);
        assert_eq!(next(&mut sound, &mut mem), (0, 0));
        // Disabled: silence.
        sound.write_io(0x500, 0x007F, 0xFFFF, &mem);
        assert_eq!(next(&mut sound, &mut mem), (0, 0));
    }

    #[test]
    fn capture_records_the_left_mix() {
        let mut mem = ram();
        mem.0[..4].fill(0x20);
        let mut sound = Sound::new();
        setup(&mut sound, &mem, 0, 0, 1, 0, 1);
        // Channel 1's timer paces the capture: one write per mix sample.
        sound.write_io(0x418, (0x1_0000 - 512) as u16, 0xFFFF, &mem);
        // Capture 0: the mixer, PCM16, looping over 2 words at 02000100h.
        sound.write_io(0x510, 0x0100, 0xFFFF, &mem);
        sound.write_io(0x512, 0x0200, 0xFFFF, &mem);
        sound.write_io(0x514, 2, 0xFFFF, &mem);
        sound.write_io(0x508, 0x80, 0x00FF, &mem);
        for _ in 0..5 {
            next(&mut sound, &mut mem);
        }
        let left = (((0x2000 * 127) >> 7) * 64) >> 7;
        assert_eq!(mem.read16(0x0200_0100) as i16 as i32, left);
        assert_eq!(mem.read16(0x0200_0106) as i16 as i32, left);
        assert_eq!(sound.read_io(0x508) & 0x80, 0x80);
        // One-shot captures stop after their length.
        sound.write_io(0x508, 0x84, 0x00FF, &mem);
        for _ in 0..5 {
            next(&mut sound, &mut mem);
        }
        assert_eq!(sound.read_io(0x508) & 0x80, 0);
    }
}
