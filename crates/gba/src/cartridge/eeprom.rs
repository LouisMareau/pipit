//! EEPROM backup memory, 512 B or 8 KB (GBATEK "GBA Cart Backup EEPROM").
//!
//! The chip is driven one bit at a time through DMA: a request is a stream of
//! 16-bit writes whose low bit is the data, and a read returns 68 bits (4 dummy +
//! 64 data). Address width is 6 bits for 512 B chips and 14 bits for 8 KB chips.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum State {
    Idle,
    /// Receiving the request type bits (2), then the address.
    Request,
    /// Receiving 64 data bits for a write, then one stop bit.
    WriteData,
    /// A write just finished: the stop bit is next.
    WriteStop,
    /// A read was set up: waiting for the stop bit, then streaming data out.
    ReadStop,
    ReadData,
}

#[derive(Serialize, Deserialize)]
pub struct Eeprom {
    data: Vec<u8>,
    /// Address bits, decided from the first DMA length. `None` until then.
    addr_bits: Option<u32>,
    state: State,
    bits_received: u32,
    shift: u64,
    address: u32,
    read_bits_sent: u32,
    read_value: u64,
}

impl Eeprom {
    pub fn new() -> Self {
        Self {
            data: vec![0xFF; 0x2000],
            addr_bits: None,
            state: State::Idle,
            bits_received: 0,
            shift: 0,
            address: 0,
            read_bits_sent: 0,
            read_value: 0,
        }
    }

    pub fn data(&self) -> &[u8] {
        let n = if self.addr_bits == Some(6) { 0x200 } else { 0x2000 };
        &self.data[..n]
    }

    pub fn load(&mut self, data: &[u8]) {
        let n = data.len().min(self.data.len());
        self.data[..n].copy_from_slice(&data[..n]);
        if data.len() <= 0x200 {
            self.addr_bits = Some(6);
        } else {
            self.addr_bits = Some(14);
        }
    }

    /// A read request is 2 + addr_bits + 1 halfwords; a write 2 + addr_bits + 64 + 1.
    /// 9 or 73 means a 512 B chip, 17 or 81 an 8 KB chip.
    pub fn dma_hint(&mut self, count: u32) {
        if self.addr_bits.is_none() {
            self.addr_bits = Some(match count {
                9 | 73 => 6,
                17 | 81 => 14,
                _ => return,
            });
        }
    }

    fn addr_bits(&self) -> u32 {
        self.addr_bits.unwrap_or(6)
    }

    pub fn read(&mut self) -> u16 {
        match self.state {
            State::ReadData => {
                let bit = if self.read_bits_sent < 4 {
                    0
                } else {
                    ((self.read_value >> (63 - (self.read_bits_sent - 4))) & 1) as u16
                };
                self.read_bits_sent += 1;
                if self.read_bits_sent == 68 {
                    self.state = State::Idle;
                }
                bit
            }
            // "Ready" is signalled by reading 1 after a write completes.
            _ => 1,
        }
    }

    /// Returns true when stored data changed.
    pub fn write(&mut self, value: u16) -> bool {
        let bit = u64::from(value & 1);
        match self.state {
            State::Idle => {
                if bit == 1 {
                    self.state = State::Request;
                    self.bits_received = 1;
                    self.shift = 1;
                }
            }
            State::Request => {
                self.shift = (self.shift << 1) | bit;
                self.bits_received += 1;
                if self.bits_received == 2 + self.addr_bits() {
                    // shift = 1, R/W bit, address.
                    let is_read = (self.shift >> self.addr_bits()) & 1 == 1;
                    self.address = (self.shift & ((1 << self.addr_bits()) - 1)) as u32 & 0x3FF;
                    self.shift = 0;
                    self.bits_received = 0;
                    self.state = if is_read { State::ReadStop } else { State::WriteData };
                }
            }
            State::WriteData => {
                self.shift = (self.shift << 1) | bit;
                self.bits_received += 1;
                if self.bits_received == 64 {
                    let base = (self.address as usize * 8) % self.data.len();
                    self.data[base..base + 8].copy_from_slice(&self.shift.to_be_bytes());
                    self.state = State::WriteStop;
                    return true;
                }
            }
            State::WriteStop => self.state = State::Idle,
            State::ReadStop => {
                let base = (self.address as usize * 8) % self.data.len();
                self.read_value = u64::from_be_bytes(self.data[base..base + 8].try_into().unwrap());
                self.read_bits_sent = 0;
                self.state = State::ReadData;
            }
            State::ReadData => {}
        }
        false
    }
}

impl Default for Eeprom {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn send(e: &mut Eeprom, bits: &[u16]) {
        for &b in bits {
            e.write(b);
        }
    }

    #[test]
    fn write_then_read_back_8k() {
        let mut e = Eeprom::new();
        e.dma_hint(81);
        // Write: 1, 0, 14 address bits (address 3), 64 data bits, stop.
        let mut req = vec![1, 0];
        req.extend((0..14).rev().map(|i| (3u16 >> i) & 1));
        let value: u64 = 0x0123_4567_89AB_CDEF;
        req.extend((0..64).rev().map(|i| ((value >> i) & 1) as u16));
        req.push(0);
        send(&mut e, &req);
        assert_eq!(e.read(), 1);

        // Read: 1, 1, address, stop; then 4 dummy + 64 data bits.
        let mut req = vec![1, 1];
        req.extend((0..14).rev().map(|i| (3u16 >> i) & 1));
        req.push(0);
        send(&mut e, &req);
        let mut out = 0u64;
        for _ in 0..4 {
            e.read();
        }
        for _ in 0..64 {
            out = (out << 1) | u64::from(e.read());
        }
        assert_eq!(out, value);
    }
}
