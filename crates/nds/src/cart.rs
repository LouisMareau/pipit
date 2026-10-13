//! The DS cartridge: the ROM header, what direct boot needs from it, and the
//! card interface games read the rest of the ROM through (ROMCTRL, the
//! command register and the data port).
//!
//! The BIOS boot sequence encrypts its commands (KEY1) and the data stream
//! (KEY2); booting a ROM directly skips all of it, and the plain `B7` reads a
//! game issues afterwards come back unscrambled here, as the scrambling
//! cancels out on the way back on real hardware too.

use crate::backup::Backup;
use crate::scheduler::{Event, Scheduler};
use serde::{Deserialize, Serialize};

/// What the header says about the two programs to load.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Program {
    pub rom_offset: u32,
    pub entry: u32,
    pub ram_address: u32,
    pub size: u32,
}

#[derive(Serialize, Deserialize)]
pub struct Cart {
    #[serde(skip)]
    rom: Vec<u8>,
    pub title: String,
    pub game_code: String,
    pub arm9: Program,
    pub arm7: Program,
    /// The chip id the card reports (command B8).
    pub chip_id: u32,

    // The card interface (shared by both CPUs; EXMEMCNT says who owns it).
    pub auxspicnt: u16,
    /// The save chip behind AUXSPI, and the byte it returned last.
    pub backup: Backup,
    pub auxspi_data: u8,
    pub romctrl: u32,
    pub command: [u8; 8],
    /// Bytes still to deliver for the current command.
    remaining: u32,
    /// Where the next word comes from (a ROM offset, or the chip id).
    cursor: u32,
    /// The word waiting in the data port.
    pub data: u32,
    /// A word is ready to be read (ROMCTRL bit 23).
    pub data_ready: bool,
    /// A transfer is in progress (ROMCTRL bit 31).
    pub busy: bool,
    /// A transfer just ended (the bus raises the card interrupt if asked).
    pub finished: bool,
    /// What the current command returns.
    source: Source,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Source {
    Nothing,
    Rom,
    ChipId,
}

/// Cycles between data words: ROMCTRL bit 27 selects the slower clock.
const WORD_CYCLES_FAST: u64 = 8 * 2 * 4;
const WORD_CYCLES_SLOW: u64 = 5 * 2 * 4 * 2;

fn read_u32(bytes: &[u8], at: usize) -> u32 {
    let mut b = [0u8; 4];
    let n = bytes.len().saturating_sub(at).min(4);
    if n > 0 {
        b[..n].copy_from_slice(&bytes[at..at + n]);
    }
    u32::from_le_bytes(b)
}

impl Cart {
    pub fn new(rom: Vec<u8>) -> Self {
        let text = |from: usize, len: usize| -> String {
            rom.get(from..from + len)
                .map(|s| s.iter().take_while(|&&c| c != 0).map(|&c| c as char).collect())
                .unwrap_or_default()
        };
        let program = |at: usize| Program {
            rom_offset: read_u32(&rom, at),
            entry: read_u32(&rom, at + 4),
            ram_address: read_u32(&rom, at + 8),
            size: read_u32(&rom, at + 12),
        };
        let game_code = text(12, 4);
        let chip_id = 0x0000_3FC2 | ((rom.len() as u32 / 0x10_0000).saturating_sub(1) & 0xFF) << 8;
        let mut backup = Backup::new(game_code.starts_with('I'));
        if let Some((kind, size)) = crate::savedb::lookup(&game_code) {
            backup.set_kind_sized(kind, size);
        }
        Self {
            title: text(0, 12),
            backup,
            auxspi_data: 0,
            game_code,
            arm9: program(0x20),
            arm7: program(0x30),
            chip_id,
            rom,
            auxspicnt: 0,
            romctrl: 0,
            command: [0; 8],
            remaining: 0,
            cursor: 0,
            data: 0,
            data_ready: false,
            busy: false,
            finished: false,
            source: Source::Nothing,
        }
    }

    pub fn rom(&self) -> &[u8] {
        &self.rom
    }

    /// Moves the ROM out of `other` (restoring a save state keeps it).
    pub(crate) fn take_rom_from(&mut self, other: &mut Cart) {
        std::mem::swap(&mut self.rom, &mut other.rom);
    }

    /// A byte of the ROM; reads past the end see FFh like an unprogrammed chip.
    pub fn byte(&self, offset: u32) -> u8 {
        self.rom.get(offset as usize).copied().unwrap_or(0xFF)
    }

    /// AUXSPICNT written: disabling the slot deselects the save chip. Clearing
    /// the hold bit does not by itself; the chip is deselected after the next
    /// transfer, which is how drivers send the last byte of a command.
    pub fn write_auxspicnt(&mut self, value: u16, mask: u16) {
        self.auxspicnt = (self.auxspicnt & !mask) | (value & mask & 0xE0C3);
        if self.auxspicnt & (1 << 15) == 0 {
            self.backup.deselect();
        }
    }

    /// AUXSPIDATA written: a byte to the save chip when the slot is in SPI mode.
    pub fn write_auxspi_data(&mut self, byte: u8) {
        if self.auxspicnt & 0xA000 == 0xA000 {
            let hold = self.auxspicnt & (1 << 6) != 0;
            self.auxspi_data = self.backup.transfer(byte, hold);
        }
    }

    /// ROMCTRL (1A4h) as read: the live bits over what was written.
    pub fn read_romctrl(&self) -> u32 {
        (self.romctrl & 0x7F7F_FFFF) | u32::from(self.data_ready) << 23 | u32::from(self.busy) << 31
    }

    /// ROMCTRL written: bit 31 starts the command in `command`.
    pub fn write_romctrl(&mut self, value: u32, mask: u32, scheduler: &mut Scheduler) {
        self.romctrl = (self.romctrl & !mask) | (value & mask & 0x7F7F_FFFF);
        if mask & 0xFF00_0000 != 0 && value & (1 << 31) != 0 && !self.busy {
            self.start(scheduler);
        }
    }

    fn start(&mut self, scheduler: &mut Scheduler) {
        // Block size: 0 = none, 1-6 = 2^(block+8) bytes, 7 = 4 bytes.
        let block = (self.romctrl >> 24) & 7;
        let length = match block {
            0 => 0,
            7 => 4,
            n => 0x100 << n,
        };
        let (source, cursor) = match self.command[0] {
            0xB7 => {
                let addr = u32::from_be_bytes([
                    self.command[1],
                    self.command[2],
                    self.command[3],
                    self.command[4],
                ]);
                (Source::Rom, addr)
            }
            0xB8 | 0x90 => (Source::ChipId, 0),
            _ => (Source::Nothing, 0),
        };
        self.source = source;
        self.cursor = cursor;
        self.remaining = length;
        self.busy = true;
        self.data_ready = false;
        if length == 0 {
            self.busy = false;
            self.finished = true;
        } else {
            scheduler.cancel(Event::CardWord);
            scheduler.schedule(Event::CardWord, self.word_cycles());
        }
    }

    fn word_cycles(&self) -> u64 {
        if self.romctrl & (1 << 27) != 0 {
            WORD_CYCLES_SLOW
        } else {
            WORD_CYCLES_FAST
        }
    }

    /// The next word has arrived in the data port.
    pub fn on_word(&mut self) {
        self.data = match self.source {
            Source::Rom => {
                // Reads wrap within the 4 KB page of the secure area, like the card.
                let a = self.cursor;
                let b = |i: u32| self.byte(a.wrapping_add(i));
                u32::from_le_bytes([b(0), b(1), b(2), b(3)])
            }
            Source::ChipId => self.chip_id,
            Source::Nothing => 0xFFFF_FFFF,
        };
        self.cursor = self.cursor.wrapping_add(4);
        self.data_ready = true;
    }

    /// The data port (100010h) was read: hand out the word and fetch the next.
    pub fn read_data(&mut self, scheduler: &mut Scheduler) -> u32 {
        if !self.data_ready {
            return self.data;
        }
        let word = self.data;
        self.data_ready = false;
        self.remaining = self.remaining.saturating_sub(4);
        if self.remaining == 0 {
            self.busy = false;
            self.finished = true;
        } else {
            scheduler.schedule(Event::CardWord, self.word_cycles());
        }
        word
    }
}
