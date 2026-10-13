//! The GBA slot (slot 2): a GBA cartridge, read through the GBA core's own
//! ROM and save-chip logic; the Rumble Pak; or nothing (GBATEK "DS Memory
//! Control", "DS Cart Rumble Pak").
//!
//! The slot belongs to one CPU at a time (EXMEMCNT bit 7) and the other
//! reads zeros. An empty slot's ROM region reads back the halfword address,
//! its RAM region FFh. The Rumble Pak holds one data line low, so it reads
//! as the address with bit 1 clear, and its actuator moves whenever a write
//! toggles bit 1: games rumble by toggling it quickly, which `take_rumble`
//! reports.

use crate::memory::{Bus, ARM7, ARM9};
use pipit_gba::cartridge::Cartridge;
use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize)]
pub enum Slot2 {
    #[default]
    Empty,
    RumblePak {
        /// Bit 1 of the last write.
        state: bool,
        /// Toggles since `take_rumble` last looked.
        toggles: u32,
    },
    Cartridge(Cartridge),
}

impl Slot2 {
    pub fn cartridge(rom: Vec<u8>) -> Self {
        Self::Cartridge(Cartridge::new(rom))
    }

    pub fn rumble_pak() -> Self {
        Self::RumblePak { state: false, toggles: 0 }
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
    }

    /// Whether the actuator moved since the last call.
    pub fn take_rumble(&mut self) -> bool {
        match self {
            Self::RumblePak { toggles, .. } => std::mem::take(toggles) > 0,
            _ => false,
        }
    }

    pub fn read16(&mut self, addr: u32) -> u16 {
        let rom = addr >> 24 < 0x0A;
        match self {
            Self::Cartridge(cart) if rom => cart.read_rom16(addr),
            Self::Cartridge(cart) => u16::from(cart.read_backup(addr)) * 0x0101,
            Self::RumblePak { .. } if rom => (addr >> 1) as u16 & 0xFFFD,
            Self::Empty if rom => (addr >> 1) as u16,
            _ => 0xFFFF,
        }
    }

    pub fn read8(&mut self, addr: u32) -> u8 {
        match self {
            Self::Cartridge(cart) if addr >> 24 >= 0x0A => cart.read_backup(addr),
            _ => (self.read16(addr & !1) >> ((addr & 1) * 8)) as u8,
        }
    }

    pub fn read32(&mut self, addr: u32) -> u32 {
        u32::from(self.read16(addr)) | u32::from(self.read16(addr + 2)) << 16
    }

    pub fn write16(&mut self, addr: u32, value: u16) {
        let rom = addr >> 24 < 0x0A;
        match self {
            Self::Cartridge(cart) if rom => cart.write_rom16(addr, value),
            // The RAM bus is eight bits wide: the low byte lands.
            Self::Cartridge(cart) => cart.write_backup(addr, value as u8),
            Self::RumblePak { state, toggles } if rom => {
                let on = value & 2 != 0;
                if on != *state {
                    *state = on;
                    *toggles += 1;
                }
            }
            _ => {}
        }
    }

    pub fn write8(&mut self, addr: u32, value: u8) {
        match self {
            Self::Cartridge(cart) if addr >> 24 >= 0x0A => cart.write_backup(addr, value),
            _ => self.write16(addr & !1, u16::from(value) * 0x0101),
        }
    }

    pub fn write32(&mut self, addr: u32, value: u32) {
        self.write16(addr, value as u16);
        self.write16(addr + 2, (value >> 16) as u16);
    }

    pub fn save_data(&self) -> Option<&[u8]> {
        match self {
            Self::Cartridge(cart) => cart.save_data(),
            _ => None,
        }
    }

    pub fn load_save_data(&mut self, data: &[u8]) {
        if let Self::Cartridge(cart) = self {
            cart.load_save_data(data);
        }
    }

    pub fn take_save_dirty(&mut self) -> bool {
        match self {
            Self::Cartridge(cart) => cart.take_save_dirty(),
            _ => false,
        }
    }

    /// Moves the ROM out of `other` (restoring a save state keeps the loaded one).
    pub(crate) fn take_rom_from(&mut self, other: &mut Slot2) {
        if let (Self::Cartridge(cart), Self::Cartridge(old)) = (self, other) {
            cart.take_rom_from(old);
        }
    }
}

impl Bus {
    #[inline(always)]
    fn slot2_owner(&self) -> usize {
        if self.exmemcnt & 0x80 == 0 {
            ARM9
        } else {
            ARM7
        }
    }

    pub fn slot2_read8(&mut self, cpu: usize, addr: u32) -> u8 {
        if self.slot2_owner() != cpu {
            return 0;
        }
        self.slot2.read8(addr)
    }

    pub fn slot2_read16(&mut self, cpu: usize, addr: u32) -> u16 {
        if self.slot2_owner() != cpu {
            return 0;
        }
        self.slot2.read16(addr)
    }

    pub fn slot2_read32(&mut self, cpu: usize, addr: u32) -> u32 {
        if self.slot2_owner() != cpu {
            return 0;
        }
        self.slot2.read32(addr)
    }

    pub fn slot2_write8(&mut self, cpu: usize, addr: u32, value: u8) {
        if self.slot2_owner() == cpu {
            self.slot2.write8(addr, value);
        }
    }

    pub fn slot2_write16(&mut self, cpu: usize, addr: u32, value: u16) {
        if self.slot2_owner() == cpu {
            self.slot2.write16(addr, value);
        }
    }

    pub fn slot2_write32(&mut self, cpu: usize, addr: u32, value: u32) {
        if self.slot2_owner() == cpu {
            self.slot2.write32(addr, value);
        }
    }
}
