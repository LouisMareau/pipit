//! The real-time clock (an S-35180), bit-banged through RTC (0138h): the
//! ARM7 toggles chip select, clock and data lines. Data travels least
//! significant bit first; the command byte is accepted in either order. The
//! clock runs on emulated time from a Unix time the front-end sets.

use serde::{Deserialize, Serialize};

/// ARM9 cycles per second: twice the bus clock.
pub const CYCLES_PER_SECOND: u64 = crate::scheduler::BUS_HZ as u64 * 2;

#[derive(Serialize, Deserialize)]
pub struct Rtc {
    /// The register as written: data (0), clock (1), select (2), directions (4-6).
    reg: u8,
    /// Bits received or sent in the current byte, and how many.
    bits: u8,
    count: u8,
    /// The command once received: register (0-7) and whether it is a read.
    command: Option<(u8, bool)>,
    /// The bytes going out (reads) or coming in (writes), and the index.
    buffer: [u8; 8],
    len: usize,
    index: usize,
    /// The bit the chip presents on the data line.
    out: bool,
    /// Status 1: bit 1 24-hour mode; status 2: INT1 mode (0-3), INT2 enable (6).
    status1: u8,
    status2: u8,
    alarm1: [u8; 3],
    alarm2: [u8; 3],
    adjust: u8,
    free: u8,
    base_unix: i64,
    base_cycles: u64,
}

impl Default for Rtc {
    fn default() -> Self {
        Self::new()
    }
}

impl Rtc {
    pub fn new() -> Self {
        Self {
            reg: 0,
            bits: 0,
            count: 0,
            command: None,
            buffer: [0; 8],
            len: 0,
            index: 0,
            out: false,
            status1: 0,
            status2: 0,
            alarm1: [0; 3],
            alarm2: [0; 3],
            adjust: 0,
            free: 0,
            // 2026-01-01 00:00:00 until the front-end sets the time.
            base_unix: 1_767_225_600,
            base_cycles: 0,
        }
    }

    /// Sets the clock from a Unix time; it then runs on emulated time.
    pub fn set_time(&mut self, unix_seconds: i64, now: u64) {
        self.base_unix = unix_seconds;
        self.base_cycles = now;
    }

    fn unix(&self, now: u64) -> i64 {
        self.base_unix + (now.saturating_sub(self.base_cycles) / CYCLES_PER_SECOND) as i64
    }

    pub fn read(&self) -> u8 {
        let data = if self.reg & 0x10 != 0 { self.reg & 1 } else { u8::from(self.out) };
        (self.reg & 0x76) | data
    }

    pub fn write(&mut self, value: u8, now: u64) {
        let old = self.reg;
        self.reg = value & 0x77;
        let cs = value & 4 != 0;
        if !cs || old & 4 == 0 {
            // Deselected, or just selected: a transaction starts from the command.
            if old & 4 != 0 && !cs {
                self.finish(now);
            }
            self.bits = 0;
            self.count = 0;
            self.command = None;
            self.len = 0;
            self.index = 0;
            if !cs {
                return;
            }
        }
        if value & 2 != 0 && old & 2 == 0 {
            self.clock(value & 1 != 0, now);
        }
    }

    /// A rising clock edge: one bit in, or one bit out.
    fn clock(&mut self, data: bool, now: u64) {
        if let Some((_, true)) = self.command {
            if self.index < self.len {
                self.out = (self.buffer[self.index] >> self.count) & 1 != 0;
                self.count += 1;
                if self.count == 8 {
                    self.count = 0;
                    self.index += 1;
                }
            }
            return;
        }
        self.bits |= u8::from(data) << self.count;
        self.count += 1;
        if self.count < 8 {
            return;
        }
        let byte = self.bits;
        self.bits = 0;
        self.count = 0;
        match self.command {
            None => {
                // On the wire: 0110, the register number high bit first, the
                // read flag. Received low bit first, the number comes reversed.
                let reg = ((byte >> 4) & 1) << 2 | ((byte >> 5) & 1) << 1 | ((byte >> 6) & 1);
                let read = byte & 0x80 != 0;
                self.command = Some((reg, read));
                if read {
                    self.prepare(reg, now);
                }
            }
            Some(_) => {
                if self.len < 8 {
                    self.buffer[self.len] = byte;
                    self.len += 1;
                }
            }
        }
    }

    /// Loads the bytes a read command returns.
    fn prepare(&mut self, reg: u8, now: u64) {
        let f = self.date_time(now);
        let mut bytes = [0u8; 8];
        let len = match reg {
            0 => {
                bytes[0] = self.status1;
                1
            }
            1 => {
                bytes[0] = self.status2;
                1
            }
            2 => {
                bytes[..7].copy_from_slice(&f);
                7
            }
            3 => {
                bytes[..3].copy_from_slice(&f[4..7]);
                3
            }
            4 => {
                bytes[..3].copy_from_slice(&self.alarm1);
                3
            }
            5 => {
                bytes[..3].copy_from_slice(&self.alarm2);
                3
            }
            6 => {
                bytes[0] = self.adjust;
                1
            }
            _ => {
                bytes[0] = self.free;
                1
            }
        };
        self.buffer = bytes;
        self.len = len;
        self.index = 0;
    }

    /// Chip select dropped after a write command: apply its bytes.
    fn finish(&mut self, now: u64) {
        let Some((reg, false)) = self.command else { return };
        let b = self.buffer;
        match reg {
            0 => {
                if b[0] & 1 != 0 {
                    self.status1 = 0;
                    self.status2 = 0;
                } else {
                    self.status1 = b[0] & 0x0E;
                }
            }
            1 => self.status2 = b[0] & 0x7F,
            2 if self.len >= 7 => {
                if let Some(unix) = unix_from_fields(&[b[0], b[1], b[2], b[4], b[5], b[6]]) {
                    self.set_time(unix, now);
                }
            }
            3 if self.len >= 3 => {
                let f = self.date_time(now);
                if let Some(unix) = unix_from_fields(&[f[0], f[1], f[2], b[0], b[1], b[2]]) {
                    self.set_time(unix, now);
                }
            }
            4 => self.alarm1 = [b[0], b[1], b[2]],
            5 => self.alarm2 = [b[0], b[1], b[2]],
            6 => self.adjust = b[0],
            _ => self.free = b[0],
        }
    }

    /// Once a second: whether the interrupt line fires, per INT1's mode
    /// (1: every second, 2–3: on the minute, 4: alarm 1) and INT2 (alarm 2).
    pub fn on_second(&self, now: u64) -> bool {
        let f = self.date_time(now);
        let on_minute = f[6] == 0;
        let alarm = |a: &[u8; 3]| on_minute && a[1] & 0x7F == f[4] & 0x7F && a[2] == f[5];
        let int1 = match self.status2 & 0xF {
            1 => true,
            2 | 3 => on_minute,
            4 => alarm(&self.alarm1),
            _ => false,
        };
        int1 || (self.status2 & (1 << 6) != 0 && alarm(&self.alarm2))
    }

    /// The date and time as the chip reports them: year, month, day,
    /// weekday, hour (bit 6 set from noon), minute, second, in BCD.
    fn date_time(&self, now: u64) -> [u8; 7] {
        let unix = self.unix(now);
        let days = unix.div_euclid(86_400);
        let secs = unix.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        let weekday = (days + 4).rem_euclid(7) as u8;
        let hour = (secs / 3600) as u8;
        let digits = if self.status1 & 2 != 0 { bcd(hour) } else { bcd(hour % 12) };
        let hour_field = digits | if hour >= 12 { 0x40 } else { 0 };
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
    (hi <= 9 && lo <= 9).then_some(hi * 10 + lo)
}

/// Year, month, day, hour, minute, second fields (BCD) to a Unix time.
fn unix_from_fields(f: &[u8; 6]) -> Option<i64> {
    let year = 2000 + i64::from(from_bcd(f[0])?);
    let month = from_bcd(f[1])?;
    let day = from_bcd(f[2])?;
    let hour = from_bcd(f[3] & 0x3F)?;
    let hour = if f[3] & 0x40 != 0 && hour < 12 { hour + 12 } else { hour };
    let minute = from_bcd(f[4])?;
    let second = from_bcd(f[5])?;
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

    /// Runs a transaction the way libnds does: the command byte high bit
    /// first, then parameters out or results in low bit first.
    fn transaction(rtc: &mut Rtc, command: u8, params: &[u8], results: usize) -> Vec<u8> {
        let now = 0;
        rtc.write(0x60 | 0x02, now);
        rtc.write(0x70 | 0x06, now);
        let mut cmd = command;
        for _ in 0..8 {
            rtc.write(0x70 | 0x04 | (cmd >> 7), now);
            rtc.write(0x70 | 0x06 | (cmd >> 7), now);
            cmd <<= 1;
        }
        for &p in params {
            for bit in 0..8 {
                let b = (p >> bit) & 1;
                rtc.write(0x70 | 0x04 | b, now);
                rtc.write(0x70 | 0x06 | b, now);
            }
        }
        let mut out = Vec::new();
        for _ in 0..results {
            let mut byte = 0u8;
            for bit in 0..8 {
                rtc.write(0x60 | 0x04, now);
                rtc.write(0x60 | 0x06, now);
                byte |= (rtc.read() & 1) << bit;
            }
            out.push(byte);
        }
        rtc.write(0x60 | 0x02, now);
        out
    }

    #[test]
    fn reads_the_date_and_time_in_bcd() {
        let mut rtc = Rtc::new();
        // 2026-10-13 14:05:09 UTC, a Tuesday.
        rtc.set_time(1_791_900_309, 0);
        transaction(&mut rtc, 0x60, &[0x02], 0); // 24-hour mode
        let date = transaction(&mut rtc, 0x65, &[], 7);
        assert_eq!(date, [0x26, 0x10, 0x13, 2, 0x54, 0x05, 0x09]);
        let time = transaction(&mut rtc, 0x67, &[], 3);
        assert_eq!(time, [0x54, 0x05, 0x09]);
        // 12-hour mode: 2 PM.
        transaction(&mut rtc, 0x60, &[0x00], 0);
        assert_eq!(transaction(&mut rtc, 0x67, &[], 3)[0], 0x42);
    }

    #[test]
    fn writes_set_the_clock_and_time_passes() {
        let mut rtc = Rtc::new();
        transaction(&mut rtc, 0x60, &[0x02], 0);
        transaction(&mut rtc, 0x64, &[0x20, 0x03, 0x01, 0x06, 0x23, 0x59, 0x58], 0);
        assert_eq!(transaction(&mut rtc, 0x65, &[], 7), [0x20, 0x03, 0x01, 0, 0x63, 0x59, 0x58]);
        // Two seconds later it is 2 March 2020, a Monday.
        let later = 2 * CYCLES_PER_SECOND;
        assert_eq!(rtc.date_time(later), [0x20, 0x03, 0x02, 1, 0x00, 0x00, 0x00]);
        assert_eq!(transaction(&mut rtc, 0x63, &[], 1), [0]);
        transaction(&mut rtc, 0x62, &[0x01], 0);
        assert!(rtc.on_second(0));
    }
}
