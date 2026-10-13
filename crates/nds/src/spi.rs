//! The ARM7's SPI bus (SPICNT, SPIDATA) and the three devices on it: the
//! power management chip, the firmware flash and the touch screen
//! controller. A transfer completes at once; the chip stays selected across
//! bytes while SPICNT's hold bit is set, which is how commands span bytes.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Spi {
    /// SPICNT: baud rate (0-1), device (8-9), 16-bit (10), hold (11), IRQ (14), enable (15).
    pub cnt: u16,
    /// What the selected device returned last: SPIDATA.
    data: u8,
    /// Bytes transferred so far in the current transaction.
    pos: u32,
    command: u8,
    /// Power management registers: control, battery, mic amplifier, mic gain, backlights.
    pm: [u8; 5],
    /// The chip asked to power the console down (control bit 6).
    pub powered_off: bool,
    #[serde(skip)]
    pub firmware: Vec<u8>,
    fw_addr: u32,
    fw_write_enabled: bool,
    /// The touch controller's last control byte, its conversion, and how
    /// many bytes of the result went out.
    tsc_control: u8,
    tsc_result: u16,
    tsc_pos: u32,
    /// The pen's ADC coordinates while it is down.
    pub touch: Option<(u16, u16)>,
}

impl Spi {
    pub fn new(firmware: Vec<u8>) -> Self {
        Self {
            cnt: 0,
            data: 0,
            pos: 0,
            command: 0,
            pm: [0; 5],
            powered_off: false,
            firmware,
            fw_addr: 0,
            fw_write_enabled: false,
            tsc_control: 0,
            tsc_result: 0,
            tsc_pos: 0,
            touch: None,
        }
    }

    pub fn write_cnt(&mut self, value: u16, mask: u16) {
        self.cnt = (self.cnt & !mask) | (value & mask & 0xCF03);
        // Disabling the bus deselects the chip. Clearing the hold bit does
        // not by itself: the chip is deselected after the next transfer,
        // which is how drivers send the last byte of a command.
        if self.cnt & (1 << 15) == 0 {
            self.pos = 0;
        }
    }

    pub fn read_data(&self) -> u8 {
        self.data
    }

    /// Sends a byte to the selected device; returns whether to raise the SPI
    /// interrupt.
    pub fn write_data(&mut self, byte: u8) -> bool {
        if self.cnt & (1 << 15) == 0 {
            return false;
        }
        self.data = match (self.cnt >> 8) & 3 {
            0 => self.pm_transfer(byte),
            1 => self.fw_transfer(byte),
            2 => self.tsc_transfer(byte),
            _ => 0,
        };
        self.pos += 1;
        if self.cnt & (1 << 11) == 0 {
            self.pos = 0;
        }
        self.cnt & (1 << 14) != 0
    }

    /// The power management chip: a command byte (bit 7 read, bits 0-6 the
    /// register), then the register's byte.
    fn pm_transfer(&mut self, byte: u8) -> u8 {
        if self.pos == 0 {
            self.command = byte;
            return 0;
        }
        let reg = usize::from(self.command & 0x7F);
        if self.command & 0x80 != 0 {
            self.pm.get(reg).copied().unwrap_or(0)
        } else {
            match reg {
                0 => {
                    self.pm[0] = byte & 0x7F;
                    if byte & (1 << 6) != 0 {
                        self.powered_off = true;
                    }
                }
                2 => self.pm[2] = byte & 1,
                3 => self.pm[3] = byte & 3,
                4 => self.pm[4] = byte & 7,
                _ => {}
            }
            0
        }
    }

    /// The firmware flash: a serial flash with the usual command set. Writes
    /// change the image in memory only.
    fn fw_transfer(&mut self, byte: u8) -> u8 {
        if self.pos == 0 {
            self.command = byte;
            self.fw_addr = 0;
            match byte {
                0x06 => self.fw_write_enabled = true,
                0x04 => self.fw_write_enabled = false,
                _ => {}
            }
            return 0;
        }
        let len = self.firmware.len().max(1);
        match self.command {
            0x03 | 0x0B if self.pos <= 3 => {
                self.fw_addr = (self.fw_addr << 8) | u32::from(byte);
                0
            }
            0x0B if self.pos == 4 => 0,
            0x03 | 0x0B => {
                let value = self.firmware[self.fw_addr as usize % len];
                self.fw_addr = self.fw_addr.wrapping_add(1);
                value
            }
            0x05 => u8::from(self.fw_write_enabled) << 1,
            0x9F => [0x20, 0x40, 0x12][(self.pos as usize - 1) % 3],
            0x02 | 0x0A if self.pos <= 3 => {
                self.fw_addr = (self.fw_addr << 8) | u32::from(byte);
                0
            }
            0x02 | 0x0A => {
                if self.fw_write_enabled && !self.firmware.is_empty() {
                    let at = self.fw_addr as usize % len;
                    self.firmware[at] =
                        if self.command == 0x02 { self.firmware[at] & byte } else { byte };
                }
                // Writes stay within their 256-byte page.
                self.fw_addr = (self.fw_addr & !0xFF) | (self.fw_addr.wrapping_add(1) & 0xFF);
                0
            }
            0xDB | 0xD8 if self.pos <= 3 => {
                self.fw_addr = (self.fw_addr << 8) | u32::from(byte);
                if self.pos == 3 && self.fw_write_enabled && !self.firmware.is_empty() {
                    let (start, size) = if self.command == 0xDB {
                        (self.fw_addr as usize & !0xFF, 0x100)
                    } else {
                        (self.fw_addr as usize & !0xFFFF, 0x1_0000)
                    };
                    for i in 0..size {
                        self.firmware[(start + i) % len] = 0xFF;
                    }
                }
                0
            }
            _ => 0,
        }
    }

    /// The touch screen controller (a TSC2046): a control byte with bit 7
    /// set picks a channel; the 12-bit conversion then comes out over the
    /// next two bytes, bits 11-5 first (behind a zero), then bits 4-0 high.
    fn tsc_transfer(&mut self, byte: u8) -> u8 {
        let out = match self.tsc_pos {
            1 => (self.tsc_result >> 5) as u8 & 0x7F,
            2 => (self.tsc_result << 3) as u8,
            _ => 0,
        };
        if byte & 0x80 != 0 {
            self.tsc_control = byte;
            self.tsc_pos = 0;
            let (x, y) = self.touch.unwrap_or((0, 0xFFF));
            let down = self.touch.is_some();
            self.tsc_result = match (byte >> 4) & 7 {
                1 => y,
                5 => x,
                3 => {
                    if down {
                        0x200
                    } else {
                        0
                    }
                }
                4 => {
                    if down {
                        0x100
                    } else {
                        0
                    }
                }
                0 => 0x300,
                7 => 0x3A0,
                2 => 0x400,
                _ => 0,
            };
            if byte & (1 << 3) != 0 {
                // 8-bit conversions: the top eight bits, in the same position.
                self.tsc_result &= 0xFF0;
            }
        }
        self.tsc_pos += 1;
        out
    }

    /// Restoring a state keeps the running machine's firmware image.
    pub(crate) fn take_firmware_from(&mut self, other: &mut Spi) {
        std::mem::swap(&mut self.firmware, &mut other.firmware);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spi() -> Spi {
        let mut spi = Spi::new(crate::firmware::default_image());
        spi.write_cnt(0x8800 | 1 << 8, 0xFFFF); // enabled, firmware, hold
        spi
    }

    #[test]
    fn firmware_reads_the_image() {
        let mut spi = spi();
        for b in [0x03, 0x00, 0x00, 0x08] {
            spi.write_data(b);
        }
        let id: Vec<u8> = (0..4)
            .map(|_| {
                spi.write_data(0);
                spi.read_data()
            })
            .collect();
        assert_eq!(id, b"MACP");
        // Releasing the hold ends the command; the next byte is a new one.
        spi.write_cnt(0x8100, 0xFFFF);
        spi.write_data(0x9F);
        spi.write_cnt(0x8900, 0xFFFF);
        spi.write_data(0x9F);
        spi.write_data(0);
        assert_eq!(spi.read_data(), 0x20);
    }

    #[test]
    fn touch_controller_reports_the_pen() {
        let mut spi = spi();
        spi.write_cnt(0x8A00, 0xFFFF); // touch screen, hold
        spi.touch = Some((0x7D0, 0x3E8));
        spi.write_data(0xD0); // read X, 12 bits
        spi.write_data(0);
        let hi = spi.read_data();
        spi.write_data(0);
        let lo = spi.read_data();
        assert_eq!(u16::from(hi) << 5 | u16::from(lo) >> 3, 0x7D0);
        spi.write_data(0x90); // Y
        spi.write_data(0);
        assert_eq!(spi.read_data(), (0x3E8 >> 5) as u8);
        spi.touch = None;
        spi.write_data(0x90);
        spi.write_data(0);
        assert_eq!(spi.read_data(), 0x7F);
    }

    #[test]
    fn power_management_registers() {
        let mut spi = spi();
        spi.write_cnt(0x8800, 0xFFFF); // power management, hold
        spi.write_data(0x00);
        spi.write_cnt(0x8000, 0xFFFF); // the next byte is the last of the command
        spi.write_data(0x0D); // amplifier on, both backlights
        spi.write_cnt(0x8800, 0xFFFF);
        spi.write_data(0x80);
        spi.write_cnt(0x8000, 0xFFFF);
        spi.write_data(0);
        assert_eq!(spi.read_data(), 0x0D);
        assert!(!spi.powered_off);
    }
}
