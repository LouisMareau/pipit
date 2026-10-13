//! The ARM9's hardware divider and square root unit (280h–2BFh). Results are
//! ready at once; the busy bits never read as set.

use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize)]
pub struct Math {
    pub divcnt: u16,
    pub numer: u64,
    pub denom: u64,
    pub quotient: u64,
    pub remainder: u64,
    pub sqrtcnt: u16,
    pub sqrt_param: u64,
    pub sqrt_result: u32,
}

impl Math {
    fn divide(&mut self) {
        let mode = self.divcnt & 3;
        let (n, d): (i64, i64) = match mode {
            0 => (i64::from(self.numer as i32), i64::from(self.denom as i32)),
            1 => (self.numer as i64, i64::from(self.denom as i32)),
            _ => (self.numer as i64, self.denom as i64),
        };
        let by_zero = d == 0;
        if by_zero {
            // The sign of the result is wrong on purpose, as on the hardware.
            let q: i64 = if n < 0 { 1 } else { -1 };
            self.quotient = if mode == 0 { q as u64 ^ 0xFFFF_FFFF_0000_0000 } else { q as u64 };
            self.remainder = n as u64;
        } else {
            self.quotient = n.wrapping_div(d) as u64;
            self.remainder = n.wrapping_rem(d) as u64;
        }
        self.divcnt = (self.divcnt & 3) | u16::from(self.denom == 0) << 14;
    }

    fn root(&mut self) {
        let v = if self.sqrtcnt & 1 != 0 { self.sqrt_param } else { self.sqrt_param & 0xFFFF_FFFF };
        self.sqrt_result = isqrt64(v);
    }

    pub fn read_io(&self, reg: u32) -> u16 {
        let half = |v: u64| (v >> ((reg & 6) * 8)) as u16;
        match reg {
            0x280 => self.divcnt,
            0x290..=0x296 => half(self.numer),
            0x298..=0x29E => half(self.denom),
            0x2A0..=0x2A6 => half(self.quotient),
            0x2A8..=0x2AE => half(self.remainder),
            0x2B0 => self.sqrtcnt,
            0x2B4 => self.sqrt_result as u16,
            0x2B6 => (self.sqrt_result >> 16) as u16,
            0x2B8..=0x2BE => half(self.sqrt_param),
            _ => 0,
        }
    }

    pub fn write_io(&mut self, reg: u32, value: u16, mask: u16) {
        let put = |v: &mut u64| {
            let shift = (reg & 6) * 8;
            *v = (*v & !(u64::from(mask) << shift)) | (u64::from(value & mask) << shift);
        };
        match reg {
            0x280 => {
                self.divcnt = (self.divcnt & !mask) | (value & mask & 3);
                self.divide();
            }
            0x290..=0x296 => {
                put(&mut self.numer);
                self.divide();
            }
            0x298..=0x29E => {
                put(&mut self.denom);
                self.divide();
            }
            0x2B0 => {
                self.sqrtcnt = (self.sqrtcnt & !mask) | (value & mask & 1);
                self.root();
            }
            0x2B8..=0x2BE => {
                put(&mut self.sqrt_param);
                self.root();
            }
            _ => {}
        }
    }
}

/// The integer square root of a 64-bit value (fits in 32 bits).
pub fn isqrt64(v: u64) -> u32 {
    let v = u128::from(v);
    let mut r = (v as f64).sqrt() as u128;
    while r * r > v {
        r -= 1;
    }
    while (r + 1) * (r + 1) <= v {
        r += 1;
    }
    r.min(0xFFFF_FFFF) as u32
}
