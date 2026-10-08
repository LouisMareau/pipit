//! Cartridge GPIO port and the real-time clock behind it
//! (GBATEK "GBA Cart I/O Port (GPIO)" and "GBA Cart Real-Time Clock").
//!
//! Three 16-bit registers sit in the ROM address space at 0x080000C4-C8. Games
//! bit-bang the S-3511A clock chip over them: chip select, clock and a
//! bidirectional data line.

use serde::{Deserialize, Serialize};

/// GBA clock rate, for advancing the clock with emulated time.
const CYCLES_PER_SECOND: u64 = crate::CLOCK_HZ as u64;

const PIN_SCK: u8 = 1 << 0;
const PIN_SIO: u8 = 1 << 1;
const PIN_CS: u8 = 1 << 2;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Phase {
    /// Chip select low: nothing happening.
    Idle,
    /// Receiving the command byte (MSB first).
    Command,
    /// Transferring the command's parameter bytes (LSB first).
    Data,
}

#[derive(Serialize, Deserialize)]
pub struct Gpio {
    data: u8,
    /// Bit set = pin driven by the GBA.
    direction: u8,
    /// Register reads are enabled (bit 0 of the control register).
    readable: bool,
    rtc: Rtc,
}

impl Default for Gpio {
    fn default() -> Self {
        Self::new()
    }
}

impl Gpio {
    pub fn new() -> Self {
        Self { data: 0, direction: 0, readable: false, rtc: Rtc::new() }
    }

    /// Reads one of the three registers. `None` when the port is not readable, in
    /// which case the ROM shows through.
    pub fn read(&self, addr: u32, now: u64) -> Option<u16> {
        if !self.readable {
            return None;
        }
        Some(match addr & 0xF {
            0x4 => {
                // Pins the GBA does not drive come from the device.
                let device = self.rtc.output(now);
                u16::from((self.data & self.direction) | (device & !self.direction))
            }
            0x6 => u16::from(self.direction),
            0x8 => u16::from(self.readable),
            _ => 0,
        })
    }

    pub fn write(&mut self, addr: u32, value: u16, now: u64) {
        match addr & 0xF {
            0x4 => {
                self.data = value as u8 & 0xF;
                let pins = self.data & self.direction;
                self.rtc.update_pins(pins, self.direction, now);
            }
            0x6 => self.direction = value as u8 & 0xF,
            0x8 => self.readable = value & 1 != 0,
            _ => {}
        }
    }

    /// Sets the clock from a Unix timestamp (seconds). The front-end calls this
    /// when a game starts; the clock then runs on emulated time.
    pub fn set_time(&mut self, unix_seconds: i64, now: u64) {
        self.rtc.set_time(unix_seconds, now);
    }
}

/// Seiko S-3511A real-time clock.
#[derive(Serialize, Deserialize)]
struct Rtc {
    phase: Phase,
    last_sck: bool,
    bits: u8,
    bit_count: u8,
    command: u8,
    reading: bool,
    buffer: [u8; 8],
    buffer_len: usize,
    buffer_pos: usize,
    /// Control/status register: bit 6 = 24-hour mode.
    control: u8,
    /// Unix time the clock was set to, and the cycle count at that moment.
    base_unix: i64,
    base_cycles: u64,
    output_bit: u8,
}

impl Rtc {
    fn new() -> Self {
        Self {
            phase: Phase::Idle,
            last_sck: false,
            bits: 0,
            bit_count: 0,
            command: 0,
            reading: false,
            buffer: [0; 8],
            buffer_len: 0,
            buffer_pos: 0,
            control: 0x40,
            // A fixed default keeps headless runs deterministic: 2026-01-01 00:00:00.
            base_unix: 1_767_225_600,
            base_cycles: 0,
            output_bit: 0,
        }
    }

    fn set_time(&mut self, unix_seconds: i64, now: u64) {
        self.base_unix = unix_seconds;
        self.base_cycles = now;
    }

    fn output(&self, _now: u64) -> u8 {
        self.output_bit << 1
    }

    fn update_pins(&mut self, pins: u8, direction: u8, now: u64) {
        let cs = pins & PIN_CS != 0;
        let sck = pins & PIN_SCK != 0;
        let sio = pins & PIN_SIO != 0;

        if !cs {
            self.phase = Phase::Idle;
            self.bits = 0;
            self.bit_count = 0;
            self.last_sck = sck;
            return;
        }
        if self.phase == Phase::Idle {
            self.phase = Phase::Command;
            self.bits = 0;
            self.bit_count = 0;
        }

        let rising = sck && !self.last_sck;
        let falling = !sck && self.last_sck;
        self.last_sck = sck;

        match self.phase {
            Phase::Command => {
                if rising {
                    // Command bytes arrive MSB first.
                    self.bits = (self.bits << 1) | u8::from(sio);
                    self.bit_count += 1;
                    if self.bit_count == 8 {
                        self.start_command(self.bits, now);
                    }
                }
            }
            Phase::Data => {
                if self.reading {
                    // Present the next bit on the falling edge; the game samples it
                    // on the rising edge.
                    if falling || self.bit_count == 0 && !sck {
                        let byte = self.buffer[self.buffer_pos.min(7)];
                        self.output_bit = (byte >> self.bit_count) & 1;
                    }
                    if rising && direction & PIN_SIO == 0 {
                        self.bit_count += 1;
                        if self.bit_count == 8 {
                            self.bit_count = 0;
                            self.buffer_pos += 1;
                            if self.buffer_pos >= self.buffer_len {
                                self.phase = Phase::Idle;
                            }
                        }
                    }
                } else if rising {
                    // Parameter bytes arrive LSB first.
                    self.bits |= u8::from(sio) << self.bit_count;
                    self.bit_count += 1;
                    if self.bit_count == 8 {
                        self.buffer[self.buffer_pos] = self.bits;
                        self.buffer_pos += 1;
                        self.bits = 0;
                        self.bit_count = 0;
                        if self.buffer_pos >= self.buffer_len {
                            self.finish_write(now);
                            self.phase = Phase::Idle;
                        }
                    }
                }
            }
            Phase::Idle => {}
        }
    }

    fn start_command(&mut self, byte: u8, now: u64) {
        // 0110 cccr: fixed pattern, command, read flag.
        if byte >> 4 != 0b0110 {
            self.phase = Phase::Idle;
            return;
        }
        self.command = (byte >> 1) & 7;
        self.reading = byte & 1 != 0;
        self.buffer_pos = 0;
        self.bit_count = 0;
        self.bits = 0;
        self.buffer_len = match self.command {
            0 => {
                // Reset.
                self.control = 0;
                self.set_time(0, now);
                0
            }
            1 => 1, // control
            2 => 7, // date and time
            3 => 3, // time
            _ => 0, // alarm / IRQ: unused by games
        };
        if self.buffer_len == 0 {
            self.phase = Phase::Idle;
            return;
        }
        if self.reading {
            let fields = self.date_time(now);
            match self.command {
                1 => self.buffer[0] = self.control,
                2 => self.buffer[..7].copy_from_slice(&fields),
                3 => self.buffer[..3].copy_from_slice(&fields[4..7]),
                _ => {}
            }
            self.output_bit = self.buffer[0] & 1;
        }
        self.phase = Phase::Data;
    }

    fn finish_write(&mut self, now: u64) {
        match self.command {
            1 => self.control = self.buffer[0] & 0x6A | 0x40,
            2 => {
                let fields: [u8; 7] = self.buffer[..7].try_into().unwrap();
                if let Some(unix) = unix_from_fields(&fields) {
                    self.set_time(unix, now);
                }
            }
            3 => {
                let mut fields = self.date_time(now);
                fields[4..7].copy_from_slice(&self.buffer[..3]);
                if let Some(unix) = unix_from_fields(&fields) {
                    self.set_time(unix, now);
                }
            }
            _ => {}
        }
    }

    /// Current date/time as the 7 BCD bytes the chip reports:
    /// year, month, day, weekday, hour, minute, second.
    fn date_time(&self, now: u64) -> [u8; 7] {
        let elapsed = (now.saturating_sub(self.base_cycles) / CYCLES_PER_SECOND) as i64;
        let unix = self.base_unix + elapsed;
        let days = unix.div_euclid(86_400);
        let secs = unix.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        let weekday = (days + 4).rem_euclid(7) as u8; // 1970-01-01 was a Thursday (4)
        let hour = (secs / 3600) as u8;
        let hour_field = if self.control & 0x40 != 0 {
            bcd(hour)
        } else {
            // 12-hour mode: bit 7 flags PM.
            let pm = hour >= 12;
            bcd(hour % 12) | if pm { 0x80 } else { 0 }
        };
        [
            bcd((year % 100) as u8),
            bcd(month),
            bcd(day),
            weekday,
            hour_field,
            bcd(((secs % 3600) / 60) as u8),
            bcd((secs % 60) as u8),
        ]
    }
}

fn bcd(v: u8) -> u8 {
    ((v / 10) << 4) | (v % 10)
}

fn from_bcd(v: u8) -> Option<u8> {
    let (hi, lo) = (v >> 4, v & 0xF);
    if hi > 9 || lo > 9 {
        None
    } else {
        Some(hi * 10 + lo)
    }
}

fn unix_from_fields(f: &[u8; 7]) -> Option<i64> {
    let year = 2000 + i64::from(from_bcd(f[0])?);
    let month = from_bcd(f[1])?;
    let day = from_bcd(f[2])?;
    let hour = from_bcd(f[4] & 0x3F)? + if f[4] & 0x80 != 0 { 12 } else { 0 };
    let minute = from_bcd(f[5])?;
    let second = from_bcd(f[6])?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let days = days_from_civil(year, month, day);
    Some(days * 86_400 + i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second))
}

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian.
fn civil_from_days(z: i64) -> (i64, u8, u8) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn days_from_civil(y: i64, m: u8, d: u8) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = i64::from(if m > 2 { m - 3 } else { m + 9 });
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_round_trip() {
        for days in [-1, 0, 1, 10_957, 19_723, 20_454] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
        assert_eq!(civil_from_days(20_454), (2026, 1, 1));
    }

    /// Drives the pins like `siirtc.c` in the Pokémon decomp does.
    struct Driver {
        gpio: Gpio,
    }

    impl Driver {
        fn new() -> Self {
            let mut gpio = Gpio::new();
            gpio.write(0x080000C8, 1, 0);
            gpio.write(0x080000C6, 7, 0); // all pins output
            gpio.write(0x080000C4, 1, 0); // CS low, SCK high
            gpio.write(0x080000C4, 5, 0); // CS high
            Self { gpio }
        }

        fn command(&mut self, byte: u8) {
            for i in (0..8).rev() {
                let bit = ((byte >> i) & 1) << 1;
                self.gpio.write(0x080000C4, u16::from(4 | bit), 0);
                self.gpio.write(0x080000C4, u16::from(5 | bit), 0);
            }
        }

        fn read_byte(&mut self) -> u8 {
            self.gpio.write(0x080000C6, 5, 0); // SIO becomes input
            let mut v = 0;
            for i in 0..8 {
                self.gpio.write(0x080000C4, 4, 0);
                self.gpio.write(0x080000C4, 5, 0);
                let pins = self.gpio.read(0x080000C4, 0).unwrap();
                v |= ((pins >> 1) & 1) << i;
            }
            v as u8
        }
    }

    #[test]
    fn reads_status_and_date() {
        let mut d = Driver::new();
        d.command(0x63); // status, read
        assert_eq!(d.read_byte() & 0x40, 0x40);

        let mut d = Driver::new();
        d.command(0x65); // date/time, read
        let fields: Vec<u8> = (0..7).map(|_| d.read_byte()).collect();
        assert_eq!(&fields[..3], &[0x26, 0x01, 0x01]);
        assert_eq!(fields[3], 4); // Thursday
        assert_eq!(&fields[4..], &[0, 0, 0]);
    }
}
