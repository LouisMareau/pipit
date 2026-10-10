//! The cartridge: ROM banking through the common mappers, battery RAM and the
//! MBC3 real-time clock (Pan Docs "MBCs").
//!
//! Save data is the RAM, with the clock's 48-byte trailer that other emulators
//! use when the cartridge has one, so saves move between emulators.

use serde::{Deserialize, Serialize};

use crate::CLOCK_HZ;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mbc {
    None,
    Mbc1,
    Mbc2,
    Mbc3,
    Mbc5,
}

/// MBC3's clock: a seconds counter the game reads as S/M/H/days, latched on demand.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Rtc {
    /// Seconds on the clock at `base_cycles`.
    base_seconds: u64,
    base_cycles: u64,
    halted: bool,
    /// The day counter overflowed past 511.
    carry: bool,
    latched: [u8; 5],
    latch_armed: bool,
    /// Wall-clock time the game last knew about, to carry on from after a reload.
    unix_at_base: i64,
}

impl Rtc {
    fn seconds(&self, now: u64) -> u64 {
        if self.halted {
            self.base_seconds
        } else {
            self.base_seconds + (now - self.base_cycles) / u64::from(CLOCK_HZ)
        }
    }

    fn registers(&self, now: u64) -> [u8; 5] {
        let s = self.seconds(now);
        let days = s / 86_400;
        [
            (s % 60) as u8,
            ((s / 60) % 60) as u8,
            ((s / 3600) % 24) as u8,
            (days & 0xFF) as u8,
            ((days >> 8) & 1) as u8
                | (u8::from(self.halted) << 6)
                | (u8::from(self.carry || days > 511) << 7),
        ]
    }

    fn write(&mut self, reg: usize, value: u8, now: u64) {
        let mut regs = self.registers(now);
        regs[reg] = value;
        let days = u64::from(regs[3]) | (u64::from(regs[4] & 1) << 8);
        self.base_seconds = u64::from(regs[0] % 60)
            + u64::from(regs[1] % 60) * 60
            + u64::from(regs[2] % 24) * 3600
            + days * 86_400;
        self.base_cycles = now;
        self.halted = regs[4] & 0x40 != 0;
        self.carry = regs[4] & 0x80 != 0;
    }

    fn latch(&mut self, now: u64) {
        self.latched = self.registers(now);
    }
}

#[derive(Serialize, Deserialize)]
pub struct Cartridge {
    #[serde(skip)]
    rom: Vec<u8>,
    ram: Vec<u8>,
    mbc: Mbc,
    battery: bool,
    cgb: bool,
    title: String,
    ram_enabled: bool,
    rom_bank: u16,
    ram_bank: u8,
    /// MBC1: advanced banking mode (RAM bank bits also select ROM banks ≥ 32).
    mode: bool,
    rtc: Option<Rtc>,
    #[serde(skip)]
    dirty: bool,
}

impl Cartridge {
    pub fn new(rom: Vec<u8>) -> Self {
        let header = |i: usize| rom.get(i).copied().unwrap_or(0);
        let (mbc, battery, rtc) = match header(0x147) {
            0x01 | 0x02 => (Mbc::Mbc1, false, false),
            0x03 => (Mbc::Mbc1, true, false),
            0x05 => (Mbc::Mbc2, false, false),
            0x06 => (Mbc::Mbc2, true, false),
            0x0F | 0x10 => (Mbc::Mbc3, true, true),
            0x11 | 0x12 => (Mbc::Mbc3, false, false),
            0x13 => (Mbc::Mbc3, true, false),
            0x19 | 0x1A | 0x1C | 0x1D => (Mbc::Mbc5, false, false),
            0x1B | 0x1E => (Mbc::Mbc5, true, false),
            _ => (Mbc::None, header(0x147) == 0x09, false),
        };
        let ram_size = match mbc {
            Mbc::Mbc2 => 512,
            _ => match header(0x149) {
                2 => 0x2000,
                3 => 0x8000,
                4 => 0x20000,
                5 => 0x10000,
                _ => 0,
            },
        };
        let title = rom
            .get(0x134..0x143)
            .map(|t| t.iter().take_while(|&&b| b != 0).map(|&b| b as char).collect())
            .unwrap_or_default();
        Self {
            cgb: header(0x143) & 0x80 != 0,
            rom,
            ram: vec![0xFF; ram_size],
            mbc,
            battery,
            title,
            ram_enabled: false,
            rom_bank: 1,
            ram_bank: 0,
            mode: false,
            rtc: rtc.then(Rtc::default),
            dirty: false,
        }
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    /// Whether the game is for the Game Boy Color (or supports it).
    pub fn is_color(&self) -> bool {
        self.cgb
    }

    pub fn mbc(&self) -> Mbc {
        self.mbc
    }

    pub fn take_rom_from(&mut self, other: &mut Cartridge) {
        self.rom = std::mem::take(&mut other.rom);
    }

    fn rom_byte(&self, bank: usize, offset: usize) -> u8 {
        let i = bank * 0x4000 + offset;
        if self.rom.is_empty() {
            0xFF
        } else {
            self.rom[i % self.rom.len()]
        }
    }

    /// Bank at 0x4000: the mapper's register, masked to the ROM's size.
    fn high_bank(&self) -> usize {
        let bank = match self.mbc {
            Mbc::None => 1,
            Mbc::Mbc1 => {
                let low = if self.rom_bank & 0x1F == 0 { 1 } else { self.rom_bank & 0x1F };
                usize::from(low | ((u16::from(self.ram_bank) & 3) << 5))
            }
            Mbc::Mbc2 => {
                usize::from(if self.rom_bank & 0x0F == 0 { 1 } else { self.rom_bank & 0x0F })
            }
            Mbc::Mbc3 => usize::from(if self.rom_bank == 0 { 1 } else { self.rom_bank }),
            Mbc::Mbc5 => usize::from(self.rom_bank),
        };
        bank % (self.rom.len() / 0x4000).max(1)
    }

    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x3FFF => {
                // MBC1's advanced mode banks the low half too.
                let bank = if self.mbc == Mbc::Mbc1 && self.mode {
                    usize::from(self.ram_bank & 3) << 5
                } else {
                    0
                };
                self.rom_byte(bank % (self.rom.len() / 0x4000).max(1), usize::from(addr))
            }
            0x4000..=0x7FFF => self.rom_byte(self.high_bank(), usize::from(addr - 0x4000)),
            0xA000..=0xBFFF => self.read_ram(addr - 0xA000),
            _ => 0xFF,
        }
    }

    fn read_ram(&self, offset: u16) -> u8 {
        if !self.ram_enabled {
            return 0xFF;
        }
        if let (Mbc::Mbc3, 0x08..=0x0C, Some(rtc)) = (self.mbc, self.ram_bank, &self.rtc) {
            return rtc.latched[usize::from(self.ram_bank - 0x08)];
        }
        if self.ram.is_empty() {
            return 0xFF;
        }
        match self.mbc {
            Mbc::Mbc2 => self.ram[usize::from(offset & 0x1FF)] | 0xF0,
            Mbc::Mbc1 if !self.mode => self.ram[usize::from(offset) % self.ram.len()],
            _ => {
                self.ram
                    [(usize::from(self.ram_bank) * 0x2000 + usize::from(offset)) % self.ram.len()]
            }
        }
    }

    /// `now` is the system clock in T-cycles, for the real-time clock.
    pub fn write(&mut self, addr: u16, value: u8, now: u64) {
        match (self.mbc, addr) {
            (Mbc::None, _) => {}
            (Mbc::Mbc2, 0x0000..=0x3FFF) => {
                if addr & 0x100 == 0 {
                    self.ram_enabled = value & 0x0F == 0x0A;
                } else {
                    self.rom_bank = u16::from(value & 0x0F);
                }
            }
            (_, 0x0000..=0x1FFF) => self.ram_enabled = value & 0x0F == 0x0A,
            (Mbc::Mbc1, 0x2000..=0x3FFF) => self.rom_bank = u16::from(value & 0x1F),
            (Mbc::Mbc3, 0x2000..=0x3FFF) => self.rom_bank = u16::from(value),
            (Mbc::Mbc5, 0x2000..=0x2FFF) => {
                self.rom_bank = (self.rom_bank & 0x100) | u16::from(value)
            }
            (Mbc::Mbc5, 0x3000..=0x3FFF) => {
                self.rom_bank = (self.rom_bank & 0xFF) | (u16::from(value & 1) << 8)
            }
            (Mbc::Mbc1, 0x4000..=0x5FFF) => self.ram_bank = value & 3,
            (Mbc::Mbc3, 0x4000..=0x5FFF) => self.ram_bank = value & 0x0F,
            (Mbc::Mbc5, 0x4000..=0x5FFF) => self.ram_bank = value & 0x0F,
            (Mbc::Mbc1, 0x6000..=0x7FFF) => self.mode = value & 1 != 0,
            (Mbc::Mbc3, 0x6000..=0x7FFF) => {
                if let Some(rtc) = &mut self.rtc {
                    if rtc.latch_armed && value == 1 {
                        rtc.latch(now);
                    }
                    rtc.latch_armed = value == 0;
                }
            }
            (_, 0xA000..=0xBFFF) => self.write_ram(addr - 0xA000, value, now),
            _ => {}
        }
    }

    fn write_ram(&mut self, offset: u16, value: u8, now: u64) {
        if !self.ram_enabled {
            return;
        }
        if let (Mbc::Mbc3, 0x08..=0x0C, Some(rtc)) = (self.mbc, self.ram_bank, &mut self.rtc) {
            rtc.write(usize::from(self.ram_bank - 0x08), value, now);
            self.dirty = true;
            return;
        }
        if self.ram.is_empty() {
            return;
        }
        let i = match self.mbc {
            Mbc::Mbc2 => usize::from(offset & 0x1FF),
            Mbc::Mbc1 if !self.mode => usize::from(offset) % self.ram.len(),
            _ => (usize::from(self.ram_bank) * 0x2000 + usize::from(offset)) % self.ram.len(),
        };
        let value = if self.mbc == Mbc::Mbc2 { value | 0xF0 } else { value };
        if self.ram[i] != value {
            self.ram[i] = value;
            self.dirty = true;
        }
    }

    pub fn has_battery(&self) -> bool {
        self.battery && (!self.ram.is_empty() || self.rtc.is_some())
    }

    /// RAM, plus the clock trailer when there is a clock: five current and five
    /// latched registers as 32-bit words, then the Unix time, all little-endian.
    pub fn save_data(&self, now: u64, unix_now: i64) -> Option<Vec<u8>> {
        if !self.has_battery() {
            return None;
        }
        let mut out = self.ram.clone();
        if let Some(rtc) = &self.rtc {
            for r in rtc.registers(now).into_iter().chain(rtc.latched) {
                out.extend_from_slice(&u32::from(r).to_le_bytes());
            }
            out.extend_from_slice(&(unix_now as u64).to_le_bytes());
        }
        Some(out)
    }

    /// Restores a save; a clock trailer puts the clock where it was, and `unix_now`
    /// (if the file says when it was written) moves it forward by the time since.
    pub fn load_save_data(&mut self, data: &[u8], now: u64, unix_now: i64) {
        let n = data.len().min(self.ram.len());
        self.ram[..n].copy_from_slice(&data[..n]);
        if let Some(rtc) = &mut self.rtc {
            let trailer = &data[self.ram.len().min(data.len())..];
            if trailer.len() >= 48 {
                let word = |i: usize| trailer[i * 4];
                for (i, r) in (0..5).enumerate() {
                    rtc.write(i, word(r), now);
                }
                rtc.latched = [word(5), word(6), word(7), word(8), word(9)];
                let saved_at = i64::from_le_bytes(trailer[40..48].try_into().unwrap());
                if saved_at > 0 && unix_now > saved_at && !rtc.halted {
                    rtc.base_seconds += (unix_now - saved_at) as u64;
                }
            }
        }
        self.dirty = false;
    }

    pub fn take_save_dirty(&mut self) -> bool {
        std::mem::replace(&mut self.dirty, false)
    }
}
