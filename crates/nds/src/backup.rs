//! The save memory behind the card's AUXSPI: an EEPROM, FRAM or Flash chip
//! the game never names. Pipit knows some games' chips (`crate::savedb`),
//! takes the user's word for others, and otherwise tells them apart by the
//! first write (how many address bytes come before the data) or by the size
//! of a save file loaded before the game runs; the data is kept at the
//! chip's size.
//!
//! Cards with an infrared port (game codes starting with I) put a small
//! controller in front of the chip: its first byte is a command, 00h
//! passing the rest through to the save chip.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Unknown,
    /// 4 Kbit EEPROM: one address byte (plus bit 3 of the command), 16-byte pages.
    Eeprom512,
    /// 64 Kbit EEPROM: two address bytes, 32-byte pages.
    Eeprom8K,
    /// 512 Kbit EEPROM: two address bytes, 128-byte pages.
    Eeprom64K,
    /// 256 Kbit FRAM: two address bytes, writes of any length.
    Fram32K,
    /// 1 Mbit EEPROM: three address bytes, 256-byte pages, no erasing.
    Eeprom128K,
    /// Flash, 2 to 64 Mbit: three address bytes, 256-byte pages, erase commands.
    Flash,
}

impl Kind {
    fn address_bytes(self) -> u32 {
        match self {
            Kind::Eeprom512 => 1,
            Kind::Eeprom8K | Kind::Eeprom64K | Kind::Fram32K => 2,
            _ => 3,
        }
    }

    /// Writes wrap within a page; the FRAM has none.
    fn page(self) -> u32 {
        match self {
            Kind::Eeprom512 => 16,
            Kind::Eeprom8K => 32,
            Kind::Eeprom64K => 128,
            Kind::Fram32K => 0x8000,
            _ => 256,
        }
    }

    /// The chip's size, or the smallest Flash chip's.
    pub fn size(self) -> usize {
        match self {
            Kind::Eeprom512 => 512,
            Kind::Eeprom8K => 0x2000,
            Kind::Eeprom64K => 0x1_0000,
            Kind::Fram32K => 0x8000,
            Kind::Eeprom128K => 0x2_0000,
            Kind::Flash => 0x4_0000,
            Kind::Unknown => 0,
        }
    }

    fn from_size(len: usize) -> Kind {
        match len {
            0 => Kind::Unknown,
            1..=512 => Kind::Eeprom512,
            513..=0x2000 => Kind::Eeprom8K,
            0x2001..=0x8000 => Kind::Fram32K,
            0x8001..=0x1_0000 => Kind::Eeprom64K,
            0x1_0001..=0x2_0000 => Kind::Eeprom128K,
            _ => Kind::Flash,
        }
    }

    /// The names the command line and the settings use, with the chip size.
    pub const NAMES: &[(&str, Kind, usize)] = &[
        ("eeprom4k", Kind::Eeprom512, 512),
        ("eeprom64k", Kind::Eeprom8K, 0x2000),
        ("eeprom512k", Kind::Eeprom64K, 0x1_0000),
        ("fram256k", Kind::Fram32K, 0x8000),
        ("eeprom1m", Kind::Eeprom128K, 0x2_0000),
        ("flash2m", Kind::Flash, 0x4_0000),
        ("flash4m", Kind::Flash, 0x8_0000),
        ("flash8m", Kind::Flash, 0x10_0000),
        ("flash16m", Kind::Flash, 0x20_0000),
        ("flash32m", Kind::Flash, 0x40_0000),
        ("flash64m", Kind::Flash, 0x80_0000),
    ];

    /// A chip by name (`eeprom4k`, `eeprom64k`, `eeprom512k`, `fram256k`,
    /// `eeprom1m`, `flash2m` … `flash64m`), with its size.
    pub fn parse(name: &str) -> Option<(Kind, usize)> {
        Self::NAMES.iter().find(|(n, _, _)| *n == name).map(|&(_, kind, size)| (kind, size))
    }
}

#[derive(Serialize, Deserialize)]
pub struct Backup {
    pub kind: Kind,
    data: Vec<u8>,
    /// The size is known for sure (a database entry or the user's choice):
    /// a Flash chip then wraps instead of growing.
    size_fixed: bool,
    #[serde(skip)]
    dirty: bool,
    ir: bool,
    /// Bytes transferred in the current transaction, the command, the address.
    pos: u32,
    command: u8,
    addr: u32,
    write_enabled: bool,
    status: u8,
    /// A write's bytes while the kind is still unknown.
    pending: Vec<u8>,
    /// The infrared controller's command for this transaction.
    ir_command: u8,
}

impl Backup {
    pub fn new(ir: bool) -> Self {
        Self {
            kind: Kind::Unknown,
            data: Vec::new(),
            size_fixed: false,
            dirty: false,
            ir,
            pos: 0,
            command: 0,
            addr: 0,
            write_enabled: false,
            status: 0,
            pending: Vec::new(),
            ir_command: 0,
        }
    }

    /// A save file. Its size says what chip it is, unless the chip is
    /// already known: the file then fills what it can of it.
    pub fn load(&mut self, data: &[u8]) {
        if self.kind == Kind::Unknown {
            self.kind = Kind::from_size(data.len());
            if self.kind == Kind::Unknown {
                self.data.clear();
                return;
            }
            let size = self.kind.size().max(data.len().next_power_of_two());
            self.data = vec![0xFF; size];
        }
        let n = data.len().min(self.data.len());
        self.data[..n].copy_from_slice(&data[..n]);
        self.dirty = false;
    }

    /// The chip a database or the user names, of `size` bytes, blank.
    pub fn set_kind_sized(&mut self, kind: Kind, size: usize) {
        self.kind = kind;
        self.data = vec![0xFF; size.max(1)];
        self.size_fixed = true;
        self.dirty = false;
        self.pending.clear();
    }

    /// The chip's contents, once it is known what chip there is.
    pub fn data(&self) -> Option<&[u8]> {
        (self.kind != Kind::Unknown).then_some(&self.data[..])
    }

    pub fn take_dirty(&mut self) -> bool {
        std::mem::replace(&mut self.dirty, false)
    }

    /// One byte each way; `hold` keeps the chip selected for the next byte.
    pub fn transfer(&mut self, byte: u8, hold: bool) -> u8 {
        let out = self.step(byte);
        self.pos += 1;
        if !hold {
            self.deselect();
        }
        out
    }

    /// Chip select released: the command completes.
    pub fn deselect(&mut self) {
        if self.pos > 0 {
            self.finish();
        }
        self.pos = 0;
    }

    fn step(&mut self, byte: u8) -> u8 {
        let mut pos = self.pos;
        if self.ir {
            if pos == 0 {
                self.ir_command = byte;
                return 0;
            }
            match self.ir_command {
                0x00 => pos -= 1,
                0x08 => return 0xAA,
                _ => return 0,
            }
        }
        if pos == 0 {
            self.command = byte;
            self.addr = 0;
            self.pending.clear();
            match byte {
                0x06 => self.write_enabled = true,
                0x04 => self.write_enabled = false,
                // Only Flash answers an identification request; asking is a tell.
                0x9F if self.kind == Kind::Unknown => self.set_kind(Kind::Flash),
                _ => {}
            }
            return 0;
        }
        let n = self.kind.address_bytes();
        match self.command {
            0x05 => {
                let fixed = if self.kind == Kind::Eeprom512 { 0xF0 } else { 0 };
                u8::from(self.write_enabled) << 1 | self.status | fixed
            }
            0x01 => {
                if pos == 1 {
                    self.status = byte & 0x8C;
                }
                0
            }
            0x9F => {
                if self.kind == Kind::Flash {
                    [0x20, 0x40, 0x12][(pos as usize - 1) % 3]
                } else {
                    0xFF
                }
            }
            0x03 | 0x0B => {
                if pos <= n {
                    self.push_addr(byte, pos);
                    0
                } else if self.command == 0x0B && pos == n + 1 {
                    0
                } else {
                    self.read_byte()
                }
            }
            0x02 | 0x0A => {
                if self.kind == Kind::Unknown {
                    self.pending.push(byte);
                } else if pos <= n {
                    self.push_addr(byte, pos);
                } else if self.write_enabled {
                    self.write_byte(byte);
                }
                0
            }
            0xDB | 0xD8 => {
                if pos <= 3 {
                    self.push_addr(byte, pos);
                }
                0
            }
            _ => 0,
        }
    }

    fn push_addr(&mut self, byte: u8, pos: u32) {
        if self.kind == Kind::Eeprom512 {
            self.addr = u32::from((self.command >> 3) & 1) << 8 | u32::from(byte);
        } else if pos == 1 {
            self.addr = u32::from(byte);
        } else {
            self.addr = (self.addr << 8) | u32::from(byte);
        }
    }

    /// Flash chips come in sizes up to 8 MB: the data grows to cover the
    /// addresses a game uses.
    fn grow_to(&mut self, addr: u32) {
        if self.kind == Kind::Flash && !self.size_fixed && addr as usize >= self.data.len() {
            let size = (addr as usize + 1).next_power_of_two().clamp(0x4_0000, 0x80_0000);
            self.data.resize(size, 0xFF);
        }
    }

    fn read_byte(&mut self) -> u8 {
        if self.data.is_empty() {
            return 0xFF;
        }
        self.grow_to(self.addr);
        let value = self.data[self.addr as usize % self.data.len()];
        self.addr = self.addr.wrapping_add(1);
        value
    }

    fn write_byte(&mut self, byte: u8) {
        if self.data.is_empty() {
            return;
        }
        self.grow_to(self.addr);
        let i = self.addr as usize % self.data.len();
        // A Flash page program can only clear bits; a page write replaces.
        self.data[i] = if self.command == 0x02 && self.kind == Kind::Flash {
            self.data[i] & byte
        } else {
            byte
        };
        self.dirty = true;
        let page = self.kind.page();
        self.addr = (self.addr & !(page - 1)) | ((self.addr + 1) & (page - 1));
    }

    fn set_kind(&mut self, kind: Kind) {
        self.kind = kind;
        self.data = vec![0xFF; kind.size()];
        self.size_fixed = false;
        self.dirty = true;
    }

    fn finish(&mut self) {
        match self.command {
            0x02 | 0x0A => {
                if self.kind == Kind::Unknown && !self.pending.is_empty() {
                    // The first write says how many address bytes the chip takes.
                    let kind = match self.pending.len() {
                        0..=17 => Kind::Eeprom512,
                        18..=34 => Kind::Eeprom8K,
                        35..=130 => Kind::Eeprom64K,
                        _ => Kind::Flash,
                    };
                    self.set_kind(kind);
                    let pending = std::mem::take(&mut self.pending);
                    let n = kind.address_bytes() as usize;
                    for (i, &b) in pending.iter().enumerate() {
                        if i < n {
                            self.push_addr(b, i as u32 + 1);
                        } else if self.write_enabled {
                            self.write_byte(b);
                        }
                    }
                }
                self.write_enabled = false;
            }
            0xDB | 0xD8 if self.kind == Kind::Flash && self.write_enabled => {
                let (start, size) = if self.command == 0xDB {
                    (self.addr & !0xFF, 0x100)
                } else {
                    (self.addr & !0xFFFF, 0x1_0000)
                };
                self.grow_to(start + size - 1);
                let len = self.data.len();
                for i in 0..size as usize {
                    self.data[(start as usize + i) % len] = 0xFF;
                }
                self.dirty = true;
                self.write_enabled = false;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One command with the chip held through `bytes`, released at the end.
    fn command(b: &mut Backup, bytes: &[u8]) -> Vec<u8> {
        let n = bytes.len();
        bytes.iter().enumerate().map(|(i, &byte)| b.transfer(byte, i + 1 < n)).collect()
    }

    #[test]
    fn unknown_chip_reads_blank_then_becomes_a_small_eeprom() {
        let mut b = Backup::new(false);
        assert_eq!(command(&mut b, &[0x03, 0x00, 0x00, 0x00, 0x00])[4], 0xFF);
        assert!(b.data().is_none());
        command(&mut b, &[0x06]);
        let mut write = vec![0x02, 0x10];
        write.extend((1..=16).map(|i| i as u8));
        command(&mut b, &write);
        assert_eq!(b.kind, Kind::Eeprom512);
        assert_eq!(b.data().unwrap().len(), 512);
        assert_eq!(
            &b.data().unwrap()[0x10..0x20],
            &(1..=16).map(|i| i as u8).collect::<Vec<_>>()[..]
        );
        // Bit 3 of the command addresses the upper half; the status has its fixed bits.
        command(&mut b, &[0x06]);
        command(&mut b, &[0x0A, 0x00, 0x55]);
        assert_eq!(b.data().unwrap()[0x100], 0x55);
        assert_eq!(command(&mut b, &[0x05, 0x00])[1], 0xF0);
        assert_eq!(command(&mut b, &[0x0B, 0x00, 0x00, 0x00])[3], 0x55);
        assert!(b.take_dirty());
    }

    #[test]
    fn three_address_bytes_mean_flash() {
        let mut b = Backup::new(false);
        command(&mut b, &[0x06]);
        let mut write = vec![0x0A, 0x01, 0x02, 0x00];
        write.extend(std::iter::repeat_n(0xAB, 256));
        command(&mut b, &write);
        assert_eq!(b.kind, Kind::Flash);
        assert_eq!(b.data().unwrap().len(), 0x4_0000);
        assert_eq!(b.data().unwrap()[0x10200], 0xAB);
        assert_eq!(command(&mut b, &[0x03, 0x01, 0x02, 0xFF, 0x00])[4], 0xAB);
        // A page program only clears bits; an erase sets them back.
        command(&mut b, &[0x06]);
        command(&mut b, &[0x02, 0x01, 0x02, 0x00, 0x0F]);
        assert_eq!(b.data().unwrap()[0x10200], 0x0B);
        command(&mut b, &[0x06]);
        command(&mut b, &[0xDB, 0x01, 0x02, 0x00]);
        assert_eq!(b.data().unwrap()[0x10200], 0xFF);
        assert_eq!(command(&mut b, &[0x9F, 0, 0, 0])[1..], [0x20, 0x40, 0x12]);
        // Addresses beyond the first 256 KB grow the chip.
        command(&mut b, &[0x03, 0x07, 0x00, 0x00, 0x00]);
        assert_eq!(b.data().unwrap().len(), 0x8_0000);
    }

    #[test]
    fn a_save_file_sets_the_kind_and_infrared_cards_pass_through() {
        let mut b = Backup::new(true);
        b.load(&vec![0x5A; 0x2000]);
        assert_eq!(b.kind, Kind::Eeprom8K);
        assert_eq!(command(&mut b, &[0x00, 0x03, 0x00, 0x20, 0x00])[4], 0x5A);
        assert_eq!(command(&mut b, &[0x08, 0x00])[1], 0xAA);
    }

    #[test]
    fn a_named_chip_starts_blank_and_a_fixed_flash_wraps() {
        let mut b = Backup::new(false);
        b.set_kind_sized(Kind::Flash, 0x4_0000);
        assert_eq!(b.data().unwrap().len(), 0x4_0000);
        assert!(!b.take_dirty(), "nothing to keep yet");
        // A read past the end wraps instead of growing the chip.
        command(&mut b, &[0x03, 0x04, 0x00, 0x00, 0x00]);
        assert_eq!(b.data().unwrap().len(), 0x4_0000);
        // A save file of another size fills what it can.
        b.load(&vec![0x11; 0x2000]);
        assert_eq!(b.kind, Kind::Flash);
        assert_eq!(b.data().unwrap().len(), 0x4_0000);
        assert_eq!(b.data().unwrap()[0x1FFF], 0x11);
        assert_eq!(b.data().unwrap()[0x2000], 0xFF);
    }

    #[test]
    fn fram_writes_any_length_and_the_large_eeprom_takes_three_address_bytes() {
        let mut b = Backup::new(false);
        b.set_kind_sized(Kind::Fram32K, 0x8000);
        command(&mut b, &[0x06]);
        let mut write = vec![0x02, 0x7F, 0xF0];
        write.extend((0..40).map(|i| i as u8));
        command(&mut b, &write);
        // No page boundary: the write runs to the chip's end and round.
        let bytes = |from: u8, to: u8| (from..to).collect::<Vec<u8>>();
        assert_eq!(b.data().unwrap()[0x7FF0..0x8000], bytes(0, 16)[..]);
        assert_eq!(b.data().unwrap()[..24], bytes(16, 40)[..]);
        let mut b = Backup::new(false);
        b.set_kind_sized(Kind::Eeprom128K, 0x2_0000);
        command(&mut b, &[0x06]);
        command(&mut b, &[0x02, 0x01, 0xFF, 0x00, 0x0F]);
        assert_eq!(b.data().unwrap()[0x1FF00], 0x0F);
        // Writing again replaces: no erase needed, unlike Flash.
        command(&mut b, &[0x06]);
        command(&mut b, &[0x02, 0x01, 0xFF, 0x00, 0xF0]);
        assert_eq!(b.data().unwrap()[0x1FF00], 0xF0);
        assert_eq!(command(&mut b, &[0x03, 0x01, 0xFF, 0x00, 0x00])[4], 0xF0);
        assert_eq!(Kind::parse("eeprom1m"), Some((Kind::Eeprom128K, 0x2_0000)));
        assert_eq!(Kind::parse("sram"), None);
    }
}
