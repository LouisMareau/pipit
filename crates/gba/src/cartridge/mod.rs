//! Game Pak: ROM, backup memory and the GPIO devices behind it
//! (GBATEK "GBA Cartridges").

mod eeprom;
mod flash;
mod gpio;

use self::eeprom::Eeprom;
use self::flash::Flash;
use self::gpio::Gpio;
use serde::{Deserialize, Serialize};

pub const MAX_ROM_SIZE: usize = 32 * 1024 * 1024;

/// Kind of backup memory on the cartridge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SaveType {
    None,
    Sram,
    Flash64K,
    Flash128K,
    Eeprom,
}

#[derive(Serialize, Deserialize)]
enum Backup {
    None,
    Sram(#[serde(with = "crate::snapshot::bytes_box")] Box<[u8; 0x8000]>),
    Flash(Flash),
    Eeprom(Eeprom),
}

#[derive(Serialize, Deserialize)]
pub struct Cartridge {
    #[serde(skip)]
    rom: Vec<u8>,
    backup: Backup,
    save_type: SaveType,
    /// Backup memory changed since the front-end last looked: bookkeeping, not
    /// machine state (linked players must hash identical states).
    #[serde(skip)]
    dirty: bool,
    /// GPIO port (RTC). Always present: a game that never enables it just sees ROM.
    gpio: Gpio,
    /// Scheduler time of the latest access, for the clock.
    now: u64,
}

impl Cartridge {
    pub fn new(mut rom: Vec<u8>) -> Self {
        rom.truncate(MAX_ROM_SIZE);
        // Pad to a multiple of 4 so word reads never straddle the end.
        while !rom.len().is_multiple_of(4) {
            rom.push(0);
        }
        let save_type = detect_save_type(&rom);
        let backup = match save_type {
            SaveType::None => Backup::None,
            SaveType::Sram => Backup::Sram(Box::new([0xFF; 0x8000])),
            SaveType::Flash64K => Backup::Flash(Flash::new(false)),
            SaveType::Flash128K => Backup::Flash(Flash::new(true)),
            SaveType::Eeprom => Backup::Eeprom(Eeprom::new()),
        };
        Self { rom, backup, save_type, dirty: false, gpio: Gpio::new(), now: 0 }
    }

    /// Moves the ROM out of `other` (restoring a save state keeps the loaded ROM).
    pub(crate) fn take_rom_from(&mut self, other: &mut Cartridge) {
        self.rom = std::mem::take(&mut other.rom);
    }

    pub fn save_type(&self) -> SaveType {
        self.save_type
    }

    pub fn rom(&self) -> &[u8] {
        &self.rom
    }

    /// The bus passes the scheduler clock along so the RTC can keep time.
    #[inline(always)]
    pub fn set_now(&mut self, now: u64) {
        self.now = now;
    }

    /// Sets the real-time clock from a Unix timestamp.
    pub fn set_time(&mut self, unix_seconds: i64) {
        self.gpio.set_time(unix_seconds, self.now);
    }

    pub fn title(&self) -> String {
        header_string(&self.rom, 0xA0, 12)
    }

    pub fn game_code(&self) -> String {
        header_string(&self.rom, 0xAC, 4)
    }

    // ---------------------------------------------------------------------------
    // ROM
    // ---------------------------------------------------------------------------

    /// EEPROM is mapped over the top of the ROM region 0x0D. Carts of 32 MB keep it
    /// to the last 256 bytes; smaller ones answer anywhere in that region.
    #[inline]
    pub fn is_eeprom_addr(&self, addr: u32) -> bool {
        matches!(self.backup, Backup::Eeprom(_))
            && addr >> 24 == 0xD
            && (self.rom.len() <= 16 * 1024 * 1024 || addr >= 0x0DFF_FF00)
    }

    #[inline]
    pub fn read_rom16(&mut self, addr: u32) -> u16 {
        if self.is_eeprom_addr(addr) {
            if let Backup::Eeprom(e) = &mut self.backup {
                return e.read();
            }
        }
        if (0x0800_00C4..=0x0800_00C8).contains(&addr) {
            if let Some(v) = self.gpio.read(addr, self.now) {
                return v;
            }
        }
        let off = (addr & 0x01FF_FFFE) as usize;
        if off < self.rom.len() {
            u16::from_le_bytes([self.rom[off], self.rom[off + 1]])
        } else {
            // Past the end of the ROM the bus floats to the address itself.
            ((addr >> 1) & 0xFFFF) as u16
        }
    }

    #[inline]
    pub fn read_rom32(&mut self, addr: u32) -> u32 {
        let off = (addr & 0x01FF_FFFC) as usize;
        if off < self.rom.len() && !self.is_eeprom_addr(addr) {
            u32::from_le_bytes([
                self.rom[off],
                self.rom[off + 1],
                self.rom[off + 2],
                self.rom[off + 3],
            ])
        } else {
            u32::from(self.read_rom16(addr)) | (u32::from(self.read_rom16(addr + 2)) << 16)
        }
    }

    #[inline]
    pub fn read_rom8(&mut self, addr: u32) -> u8 {
        (self.read_rom16(addr & !1) >> ((addr & 1) * 8)) as u8
    }

    pub fn write_rom8(&mut self, addr: u32, value: u8) {
        self.write_rom16(addr & !1, u16::from(value) * 0x0101);
    }

    pub fn write_rom16(&mut self, addr: u32, value: u16) {
        if self.is_eeprom_addr(addr) {
            if let Backup::Eeprom(e) = &mut self.backup {
                if e.write(value) {
                    self.dirty = true;
                }
            }
        }
        if (0x0800_00C4..=0x0800_00C8).contains(&addr) {
            self.gpio.write(addr, value, self.now);
        }
    }

    pub fn write_rom32(&mut self, addr: u32, value: u32) {
        self.write_rom16(addr, value as u16);
        self.write_rom16(addr + 2, (value >> 16) as u16);
    }

    /// Tells an EEPROM how long the DMA driving it is, so it can tell the 512-byte
    /// protocol (6-bit addresses) from the 8 KB one (14-bit) on first contact.
    pub fn eeprom_dma_hint(&mut self, count: u32) {
        if let Backup::Eeprom(e) = &mut self.backup {
            e.dma_hint(count);
        }
    }

    // ---------------------------------------------------------------------------
    // Backup memory (0x0E000000 region)
    // ---------------------------------------------------------------------------

    pub fn read_backup(&mut self, addr: u32) -> u8 {
        match &mut self.backup {
            Backup::Sram(sram) => sram[(addr & 0x7FFF) as usize],
            Backup::Flash(flash) => flash.read(addr & 0xFFFF),
            _ => 0xFF,
        }
    }

    pub fn write_backup(&mut self, addr: u32, value: u8) {
        match &mut self.backup {
            Backup::Sram(sram) => {
                sram[(addr & 0x7FFF) as usize] = value;
                self.dirty = true;
            }
            Backup::Flash(flash) => self.dirty |= flash.write(addr & 0xFFFF, value),
            _ => {}
        }
    }

    pub fn save_data(&self) -> Option<&[u8]> {
        match &self.backup {
            Backup::None => None,
            Backup::Sram(sram) => Some(&sram[..]),
            Backup::Flash(flash) => Some(flash.data()),
            Backup::Eeprom(e) => Some(e.data()),
        }
    }

    pub fn load_save_data(&mut self, data: &[u8]) {
        match &mut self.backup {
            Backup::None => {}
            Backup::Sram(sram) => {
                let n = data.len().min(sram.len());
                sram[..n].copy_from_slice(&data[..n]);
            }
            Backup::Flash(flash) => flash.load(data),
            Backup::Eeprom(e) => e.load(data),
        }
        self.dirty = false;
    }

    pub fn take_save_dirty(&mut self) -> bool {
        std::mem::replace(&mut self.dirty, false)
    }
}

fn header_string(rom: &[u8], offset: usize, len: usize) -> String {
    rom.get(offset..offset + len)
        .map(|b| b.iter().take_while(|&&c| c != 0).map(|&c| c as char).collect())
        .unwrap_or_default()
}

/// Finds the backup type from the library ID strings every official SDK leaves in
/// the ROM (GBATEK "GBA Cart Backup IDs"). Falls back to SRAM, which is harmless.
pub fn detect_save_type(rom: &[u8]) -> SaveType {
    const IDS: [(&[u8], SaveType); 6] = [
        (b"EEPROM_V", SaveType::Eeprom),
        (b"SRAM_V", SaveType::Sram),
        (b"SRAM_F_V", SaveType::Sram),
        (b"FLASH_V", SaveType::Flash64K),
        (b"FLASH512_V", SaveType::Flash64K),
        (b"FLASH1M_V", SaveType::Flash128K),
    ];
    for chunk in (0..rom.len().saturating_sub(16)).step_by(4) {
        let window = &rom[chunk..];
        for (id, kind) in IDS {
            if window.starts_with(id) {
                return kind;
            }
        }
    }
    SaveType::Sram
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_flash1m_from_id_string() {
        let mut rom = vec![0u8; 0x200];
        rom[0x100..0x109].copy_from_slice(b"FLASH1M_V");
        assert_eq!(detect_save_type(&rom), SaveType::Flash128K);
    }

    #[test]
    fn reads_past_rom_end_return_address_pattern() {
        let mut cart = Cartridge::new(vec![0u8; 0x100]);
        assert_eq!(cart.read_rom16(0x0800_1000), 0x0800);
        assert_eq!(cart.read_rom32(0x0800_1000), 0x0801_0800);
    }
}
