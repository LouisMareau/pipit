//! Flash backup memory, 64 KB and 128 KB (GBATEK "GBA Cart Backup Flash ROM").
//!
//! Games talk to the chip with a command sequence (`AA` to 0x5555, `55` to 0x2AAA,
//! then the command). 128 KB chips expose two 64 KB banks selected by command 0xB0.

use serde::{Deserialize, Serialize};

const SECTOR_SIZE: usize = 0x1000;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum State {
    Ready,
    /// Received 0xAA at 0x5555.
    Unlock1,
    /// Received 0x55 at 0x2AAA; the next write is a command.
    Command,
    /// Command 0x80 (erase) received; waiting for the second unlock sequence.
    EraseUnlock1,
    EraseUnlock2,
    EraseCommand,
    /// Command 0xA0: the next write programs one byte.
    Write,
    /// Command 0xB0: the next write to 0x0000 selects the bank.
    Bank,
}

#[derive(Serialize, Deserialize)]
pub struct Flash {
    data: Vec<u8>,
    bank: usize,
    state: State,
    id_mode: bool,
    manufacturer: u8,
    device: u8,
}

impl Flash {
    pub fn new(large: bool) -> Self {
        // IDs games recognise: Sanyo 128K (Pokémon) and Panasonic 64K. The
        // manufacturer/device pair decides the sector erase timing the game expects.
        let (manufacturer, device) = if large { (0x62, 0x13) } else { (0x32, 0x1B) };
        Self {
            data: vec![0xFF; if large { 0x20000 } else { 0x10000 }],
            bank: 0,
            state: State::Ready,
            id_mode: false,
            manufacturer,
            device,
        }
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn load(&mut self, data: &[u8]) {
        let n = data.len().min(self.data.len());
        self.data[..n].copy_from_slice(&data[..n]);
    }

    pub fn read(&self, addr: u32) -> u8 {
        if self.id_mode {
            return match addr {
                0 => self.manufacturer,
                1 => self.device,
                _ => 0xFF,
            };
        }
        self.data[self.bank * 0x10000 + (addr & 0xFFFF) as usize]
    }

    /// Returns true when the stored data changed.
    pub fn write(&mut self, addr: u32, value: u8) -> bool {
        match self.state {
            State::Ready | State::EraseCommand => {
                if addr == 0x5555 && value == 0xAA {
                    self.state = if self.state == State::EraseCommand {
                        State::EraseUnlock1
                    } else {
                        State::Unlock1
                    };
                }
                // Sector erase: command 0x30 at the sector's base address.
                else if self.state == State::EraseCommand && value == 0x30 {
                    let base = self.bank * 0x10000 + (addr as usize & !(SECTOR_SIZE - 1) & 0xFFFF);
                    self.data[base..base + SECTOR_SIZE].fill(0xFF);
                    self.state = State::Ready;
                    return true;
                }
            }
            State::Unlock1 | State::EraseUnlock1 => {
                if addr == 0x2AAA && value == 0x55 {
                    self.state = if self.state == State::EraseUnlock1 {
                        State::EraseUnlock2
                    } else {
                        State::Command
                    };
                } else {
                    self.state = State::Ready;
                }
            }
            State::Command => {
                self.state = State::Ready;
                if addr != 0x5555 {
                    return false;
                }
                match value {
                    0x90 => self.id_mode = true,
                    0xF0 => self.id_mode = false,
                    0x80 => self.state = State::EraseCommand,
                    0xA0 => self.state = State::Write,
                    0xB0 if self.data.len() > 0x10000 => self.state = State::Bank,
                    _ => {}
                }
            }
            State::EraseUnlock2 => {
                self.state = State::Ready;
                if addr == 0x5555 && value == 0x10 {
                    self.data.fill(0xFF);
                    return true;
                }
                if value == 0x30 {
                    let base = self.bank * 0x10000 + (addr as usize & !(SECTOR_SIZE - 1) & 0xFFFF);
                    self.data[base..base + SECTOR_SIZE].fill(0xFF);
                    return true;
                }
            }
            State::Write => {
                // Programming can only clear bits; erase first to set them.
                let i = self.bank * 0x10000 + (addr & 0xFFFF) as usize;
                self.data[i] &= value;
                self.state = State::Ready;
                return true;
            }
            State::Bank => {
                if addr == 0 {
                    self.bank = usize::from(value & 1);
                }
                self.state = State::Ready;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(f: &mut Flash, cmd: u8) {
        f.write(0x5555, 0xAA);
        f.write(0x2AAA, 0x55);
        f.write(0x5555, cmd);
    }

    #[test]
    fn id_mode_reports_chip() {
        let mut f = Flash::new(true);
        command(&mut f, 0x90);
        assert_eq!((f.read(0), f.read(1)), (0x62, 0x13));
        command(&mut f, 0xF0);
        assert_eq!(f.read(0), 0xFF);
    }

    #[test]
    fn program_and_sector_erase() {
        let mut f = Flash::new(false);
        command(&mut f, 0xA0);
        assert!(f.write(0x1234, 0x42));
        assert_eq!(f.read(0x1234), 0x42);
        command(&mut f, 0x80);
        f.write(0x5555, 0xAA);
        f.write(0x2AAA, 0x55);
        assert!(f.write(0x1000, 0x30));
        assert_eq!(f.read(0x1234), 0xFF);
    }

    #[test]
    fn bank_switch_on_128k() {
        let mut f = Flash::new(true);
        command(&mut f, 0xB0);
        f.write(0, 1);
        command(&mut f, 0xA0);
        f.write(0x10, 0x55);
        assert_eq!(f.data[0x10010], 0x55);
    }
}
