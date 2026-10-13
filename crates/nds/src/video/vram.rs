//! The nine VRAM banks and where each is mapped (VRAMCNT A–I).
//!
//! Every use of VRAM (the LCDC window, each engine's background and object
//! memory, the ARM7's share; palettes and textures later) sees a region made
//! of 16 KB pages, each pointing into one bank. Writing a VRAMCNT rebuilds the
//! page tables. Where banks overlap, the first in letter order wins.

use serde::{Deserialize, Serialize};

pub const PAGE: usize = 0x4000;
const SIZES: [usize; 9] =
    [0x20000, 0x20000, 0x20000, 0x20000, 0x10000, 0x4000, 0x4000, 0x8000, 0x4000];

/// A region of VRAM as some client sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    /// 06800000h–068A3FFFh: every bank in letter order (656 KB).
    Lcdc,
    /// Engine A backgrounds, 512 KB at 06000000h.
    BgA,
    /// Engine A objects, 256 KB at 06400000h.
    ObjA,
    /// Engine B backgrounds, 128 KB at 06200000h.
    BgB,
    /// Engine B objects, 128 KB at 06600000h.
    ObjB,
    /// Banks C and D as the ARM7's work RAM, 256 KB at 06000000h.
    Arm7,
    /// Engine A's extended background palettes: four 8 KB slots.
    BgExtPalA,
    /// Engine A's extended object palette, 8 KB (one page).
    ObjExtPalA,
    BgExtPalB,
    ObjExtPalB,
}

const REGION_PAGES: [usize; 10] = [41, 32, 16, 8, 8, 16, 2, 1, 2, 1];

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
struct Page {
    /// Bank index + 1; 0 is unmapped.
    bank: u8,
    /// Page within the bank.
    index: u8,
}

#[derive(Serialize, Deserialize)]
pub struct Vram {
    #[serde(with = "pipit_common::snapshot::bytes_box")]
    pub a: Box<[u8; 0x20000]>,
    #[serde(with = "pipit_common::snapshot::bytes_box")]
    pub b: Box<[u8; 0x20000]>,
    #[serde(with = "pipit_common::snapshot::bytes_box")]
    pub c: Box<[u8; 0x20000]>,
    #[serde(with = "pipit_common::snapshot::bytes_box")]
    pub d: Box<[u8; 0x20000]>,
    #[serde(with = "pipit_common::snapshot::bytes_box")]
    pub e: Box<[u8; 0x10000]>,
    #[serde(with = "pipit_common::snapshot::bytes_box")]
    pub f: Box<[u8; 0x4000]>,
    #[serde(with = "pipit_common::snapshot::bytes_box")]
    pub g: Box<[u8; 0x4000]>,
    #[serde(with = "pipit_common::snapshot::bytes_box")]
    pub h: Box<[u8; 0x8000]>,
    #[serde(with = "pipit_common::snapshot::bytes_box")]
    pub i: Box<[u8; 0x4000]>,
    /// VRAMCNT A–I as written.
    pub cnt: [u8; 9],
    tables: [Vec<Page>; 10],
}

impl Default for Vram {
    fn default() -> Self {
        Self::new()
    }
}

impl Vram {
    pub fn new() -> Self {
        let mut vram = Self {
            a: crate::boxed(0),
            b: crate::boxed(0),
            c: crate::boxed(0),
            d: crate::boxed(0),
            e: crate::boxed(0),
            f: crate::boxed(0),
            g: crate::boxed(0),
            h: crate::boxed(0),
            i: crate::boxed(0),
            cnt: [0; 9],
            tables: Default::default(),
        };
        vram.remap();
        vram
    }

    fn bank(&self, n: usize) -> &[u8] {
        match n {
            0 => &self.a[..],
            1 => &self.b[..],
            2 => &self.c[..],
            3 => &self.d[..],
            4 => &self.e[..],
            5 => &self.f[..],
            6 => &self.g[..],
            7 => &self.h[..],
            _ => &self.i[..],
        }
    }

    fn bank_mut(&mut self, n: usize) -> &mut [u8] {
        match n {
            0 => &mut self.a[..],
            1 => &mut self.b[..],
            2 => &mut self.c[..],
            3 => &mut self.d[..],
            4 => &mut self.e[..],
            5 => &mut self.f[..],
            6 => &mut self.g[..],
            7 => &mut self.h[..],
            _ => &mut self.i[..],
        }
    }

    /// Whether bank C (bit 0) and D (bit 1) are the ARM7's: VRAMSTAT.
    pub fn arm7_banks(&self) -> u8 {
        let is7 = |n: usize| self.cnt[n] & 0x87 == 0x82;
        u8::from(is7(2)) | u8::from(is7(3)) << 1
    }

    pub fn write_cnt(&mut self, n: usize, value: u8) {
        self.cnt[n] = value & 0x9F;
        self.remap();
    }

    /// Rebuilds every region's page table from the VRAMCNTs.
    fn remap(&mut self) {
        for (r, table) in self.tables.iter_mut().enumerate() {
            table.clear();
            table.resize(REGION_PAGES[r], Page::default());
        }
        let cnts = self.cnt;
        for (n, &cnt) in cnts.iter().enumerate() {
            if cnt & 0x80 == 0 {
                continue;
            }
            let mst = (cnt & 7) as usize;
            let ofs = ((cnt >> 3) & 3) as usize;
            let pages = SIZES[n] / PAGE;
            // Where this bank lands: (region, byte offset within it).
            let target: Option<(Region, usize)> = match n {
                0..=3 => match mst {
                    0 => Some((Region::Lcdc, n * 0x20000)),
                    1 => Some((Region::BgA, ofs * 0x20000)),
                    2 if n < 2 => Some((Region::ObjA, (ofs & 1) * 0x20000)),
                    2 => Some((Region::Arm7, (ofs & 1) * 0x20000)),
                    4 if n == 2 => Some((Region::BgB, 0)),
                    4 if n == 3 => Some((Region::ObjB, 0)),
                    _ => None,
                },
                4 => match mst {
                    0 => Some((Region::Lcdc, 0x80000)),
                    1 => Some((Region::BgA, 0)),
                    2 => Some((Region::ObjA, 0)),
                    4 => Some((Region::BgExtPalA, 0)),
                    _ => None,
                },
                5 | 6 => {
                    let small = (ofs & 1) * 0x4000 + (ofs & 2) * 0x8000;
                    match mst {
                        0 => Some((Region::Lcdc, 0x90000 + (n - 5) * 0x4000)),
                        1 => Some((Region::BgA, small)),
                        2 => Some((Region::ObjA, small)),
                        4 => Some((Region::BgExtPalA, (ofs & 1) * 0x4000)),
                        5 => Some((Region::ObjExtPalA, 0)),
                        _ => None,
                    }
                }
                7 => match mst {
                    0 => Some((Region::Lcdc, 0x98000)),
                    1 => Some((Region::BgB, 0)),
                    2 => Some((Region::BgExtPalB, 0)),
                    _ => None,
                },
                _ => match mst {
                    0 => Some((Region::Lcdc, 0xA0000)),
                    1 => Some((Region::BgB, 0x8000)),
                    2 => Some((Region::ObjB, 0)),
                    3 => Some((Region::ObjExtPalB, 0)),
                    _ => None,
                },
            };
            let Some((region, offset)) = target else { continue };
            let table = &mut self.tables[region as usize];
            for p in 0..pages {
                let slot = offset / PAGE + p;
                if slot < table.len() && table[slot].bank == 0 {
                    table[slot] = Page { bank: n as u8 + 1, index: p as u8 };
                }
            }
        }
    }

    /// The bank and byte offset behind `offset` in `region`, if mapped.
    #[inline]
    fn locate(&self, region: Region, offset: usize) -> Option<(usize, usize)> {
        let table = &self.tables[region as usize];
        let page = table.get(offset / PAGE)?;
        if page.bank == 0 {
            return None;
        }
        Some((usize::from(page.bank) - 1, usize::from(page.index) * PAGE + offset % PAGE))
    }

    pub fn read8(&self, region: Region, offset: usize) -> u8 {
        match self.locate(region, offset) {
            Some((bank, at)) => self.bank(bank)[at],
            None => 0,
        }
    }

    pub fn read16(&self, region: Region, offset: usize) -> u16 {
        let offset = offset & !1;
        match self.locate(region, offset) {
            Some((bank, at)) => {
                let b = self.bank(bank);
                u16::from_le_bytes([b[at], b[at + 1]])
            }
            None => 0,
        }
    }

    pub fn read32(&self, region: Region, offset: usize) -> u32 {
        let offset = offset & !3;
        match self.locate(region, offset) {
            Some((bank, at)) => {
                let b = self.bank(bank);
                u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
            }
            None => 0,
        }
    }

    /// VRAM takes no byte writes from the CPUs; 8-bit stores are dropped.
    pub fn write16(&mut self, region: Region, offset: usize, value: u16) {
        let offset = offset & !1;
        if let Some((bank, at)) = self.locate(region, offset) {
            self.bank_mut(bank)[at..at + 2].copy_from_slice(&value.to_le_bytes());
        }
    }

    pub fn write32(&mut self, region: Region, offset: usize, value: u32) {
        let offset = offset & !3;
        if let Some((bank, at)) = self.locate(region, offset) {
            self.bank_mut(bank)[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
    }

    /// The size of a region, for wrapping addresses.
    pub fn region_size(region: Region) -> usize {
        REGION_PAGES[region as usize] * PAGE
    }

    /// A halfword of bank `n` itself, wherever it is mapped (the VRAM display
    /// mode and display capture address banks directly).
    pub fn read_bank16(&self, n: usize, offset: usize) -> u16 {
        let b = self.bank(n);
        let offset = offset & !1;
        if offset + 2 > b.len() {
            return 0;
        }
        u16::from_le_bytes([b[offset], b[offset + 1]])
    }

    pub fn write_bank16(&mut self, n: usize, offset: usize, value: u16) {
        let b = self.bank_mut(n);
        let offset = offset & !1;
        if offset + 2 <= b.len() {
            b[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        }
    }
}
