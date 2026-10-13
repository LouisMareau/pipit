//! The ARM9's memory map, its view of the bus, and its system control
//! coprocessor (CP15: the TCMs, the vector base, waiting for an interrupt).

use super::{run_dma, Bus, View, ARM9};
use crate::video::vram::Region;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct Cp15 {
    /// c1,c0,0: bit 13 high vectors, 16-17 DTCM enable/load mode, 18-19 ITCM.
    pub control: u32,
    /// c9,c1,0: DTCM base (bits 12-31) and size (bits 1-5: 512 << n).
    pub dtcm: u32,
    /// c9,c1,1: ITCM size; its base is always 0.
    pub itcm: u32,
    /// c6,c0..7,0: the protection unit's regions.
    pub regions: [u32; 8],
    /// c2, c3, c5, c9,c0 and c13: cacheability, write buffer, permissions,
    /// lockdown, and the process id registers (free scratch space that some
    /// toolchains use: BlocksDS keeps a value in c13,c0,1 across handlers).
    pub misc: [u32; 16],
    dtcm_base: u32,
    dtcm_size: u32,
    itcm_size: u32,
}

impl Default for Cp15 {
    fn default() -> Self {
        let mut cp15 = Self {
            control: 0x0000_2078,
            dtcm: 0,
            itcm: 0,
            regions: [0; 8],
            misc: [0; 16],
            dtcm_base: 0,
            dtcm_size: 0,
            itcm_size: 0,
        };
        cp15.update();
        cp15
    }
}

impl Cp15 {
    fn update(&mut self) {
        self.dtcm_base = self.dtcm & 0xFFFF_F000;
        self.dtcm_size = 512 << ((self.dtcm >> 1) & 0x1F);
        self.itcm_size = 512 << ((self.itcm >> 1) & 0x1F);
    }

    #[inline(always)]
    fn itcm_readable(&self) -> bool {
        self.control & (3 << 18) == 1 << 18
    }

    #[inline(always)]
    fn itcm_writable(&self) -> bool {
        self.control & (1 << 18) != 0
    }

    #[inline(always)]
    fn dtcm_readable(&self) -> bool {
        self.control & (3 << 16) == 1 << 16
    }

    #[inline(always)]
    fn dtcm_writable(&self) -> bool {
        self.control & (1 << 16) != 0
    }

    #[inline(always)]
    fn in_itcm(&self, addr: u32) -> bool {
        addr < self.itcm_size
    }

    #[inline(always)]
    fn in_dtcm(&self, addr: u32) -> bool {
        addr.wrapping_sub(self.dtcm_base) < self.dtcm_size
    }

    pub fn vector_base(&self) -> u32 {
        if self.control & (1 << 13) != 0 {
            0xFFFF_0000
        } else {
            0
        }
    }

    pub fn read(&self, cn: u32, cm: u32, op2: u32) -> u32 {
        match (cn, cm, op2) {
            (0, 0, 0) => 0x4105_9461,
            (0, 0, 1) => 0x0F0D_2112,
            (0, 0, 2) => 0x0014_0180,
            (1, 0, 0) => self.control,
            (2, 0, n) => self.misc[n as usize & 1],
            (3, 0, 0) => self.misc[2],
            (5, 0, n) => self.misc[4 + (n as usize & 3)],
            (6, m, 0) => self.regions[m as usize & 7],
            (9, 0, n) => self.misc[8 + (n as usize & 1)],
            (9, 1, 0) => self.dtcm,
            (9, 1, 1) => self.itcm,
            (13, 0, n) => self.misc[12 + (n as usize & 1)],
            _ => 0,
        }
    }

    /// Writes a register; returns whether the CPU should wait for an interrupt.
    pub fn write(&mut self, cn: u32, cm: u32, op2: u32, value: u32) -> bool {
        match (cn, cm, op2) {
            (1, 0, 0) => self.control = (value & 0x000F_F085) | 0x0000_0078,
            (2, 0, n) => self.misc[n as usize & 1] = value,
            (3, 0, 0) => self.misc[2] = value,
            (5, 0, n) => self.misc[4 + (n as usize & 3)] = value,
            (6, m, 0) => self.regions[m as usize & 7] = value,
            (7, 0, 4) | (7, 8, 2) => return true,
            (9, 0, n) => self.misc[8 + (n as usize & 1)] = value,
            (9, 1, 0) => {
                self.dtcm = value & 0xFFFF_F03E;
                self.update();
            }
            (9, 1, 1) => {
                self.itcm = value & 0x3E;
                self.update();
            }
            (13, 0, n) => self.misc[12 + (n as usize & 1)] = value,
            _ => {}
        }
        false
    }
}

/// Which VRAM region an ARM9 address at 06xxxxxxh falls in, and the offset.
#[inline(always)]
fn vram_at(addr: u32) -> (Region, usize) {
    match (addr >> 20) & 0xF {
        0 | 1 => (Region::BgA, (addr & 0x7_FFFF) as usize),
        2 | 3 => (Region::BgB, (addr & 0x1_FFFF) as usize),
        4 | 5 => (Region::ObjA, (addr & 0x3_FFFF) as usize),
        6 | 7 => (Region::ObjB, (addr & 0x1_FFFF) as usize),
        _ => (Region::Lcdc, (addr & 0xF_FFFF) as usize),
    }
}

impl Bus {
    /// Where an ARM9 address lands in the shared WRAM, if any of it is the ARM9's.
    #[inline(always)]
    fn wram9(&self, addr: u32) -> Option<usize> {
        match self.wramcnt & 3 {
            0 => Some((addr & 0x7FFF) as usize),
            1 => Some(0x4000 + (addr & 0x3FFF) as usize),
            2 => Some((addr & 0x3FFF) as usize),
            _ => None,
        }
    }

    /// Cycles for an ARM9 access outside the TCMs: `halves` 16-bit bus transfers.
    #[inline(always)]
    fn cost9(&mut self, addr: u32, seq: bool, halves: u64) {
        let cycles = match addr >> 24 {
            0x02 | 0x03 | 0xFF => 2 + u64::from(!seq) * 2,
            _ => 2 * halves,
        };
        self.time[ARM9] += cycles;
    }

    pub fn read9_8(&mut self, addr: u32, seq: bool) -> u8 {
        if self.cp15.itcm_readable() && self.cp15.in_itcm(addr) {
            self.time[ARM9] += 1;
            return self.itcm[(addr & 0x7FFF) as usize];
        }
        if self.cp15.dtcm_readable() && self.cp15.in_dtcm(addr) {
            self.time[ARM9] += 1;
            return self.dtcm[(addr & 0x3FFF) as usize];
        }
        self.cost9(addr, seq, 1);
        match addr >> 24 {
            0x02 => self.main_ram[(addr & 0x3F_FFFF) as usize],
            0x03 => self.wram9(addr).map_or(0, |i| self.shared_wram[i]),
            0x04 => {
                let reg = addr & 0x00FF_FFFF;
                (self.io_read16(ARM9, reg & !1) >> ((reg & 1) * 8)) as u8
            }
            0x05 => self.video.palette[(addr & 0x7FF) as usize],
            0x06 => {
                let (region, offset) = vram_at(addr);
                self.video.vram.read8(region, offset)
            }
            0x07 => self.video.oam[(addr & 0x7FF) as usize],
            0x08..=0x0A => self.slot2_read8(ARM9, addr),
            0xFF => self.bios9[(addr & 0xFFF) as usize],
            _ => 0,
        }
    }

    pub fn read9_16(&mut self, addr: u32, seq: bool) -> u16 {
        let addr = addr & !1;
        if self.cp15.itcm_readable() && self.cp15.in_itcm(addr) {
            self.time[ARM9] += 1;
            let i = (addr & 0x7FFF) as usize;
            return u16::from_le_bytes([self.itcm[i], self.itcm[i + 1]]);
        }
        if self.cp15.dtcm_readable() && self.cp15.in_dtcm(addr) {
            self.time[ARM9] += 1;
            let i = (addr & 0x3FFF) as usize;
            return u16::from_le_bytes([self.dtcm[i], self.dtcm[i + 1]]);
        }
        self.cost9(addr, seq, 1);
        match addr >> 24 {
            0x02 => {
                let i = (addr & 0x3F_FFFF) as usize;
                u16::from_le_bytes([self.main_ram[i], self.main_ram[i + 1]])
            }
            0x03 => match self.wram9(addr) {
                Some(i) => u16::from_le_bytes([self.shared_wram[i], self.shared_wram[i + 1]]),
                None => 0,
            },
            0x04 => self.io_read16(ARM9, addr & 0x00FF_FFFF),
            0x05 => {
                let i = (addr & 0x7FF) as usize;
                u16::from_le_bytes([self.video.palette[i], self.video.palette[i + 1]])
            }
            0x06 => {
                let (region, offset) = vram_at(addr);
                self.video.vram.read16(region, offset)
            }
            0x07 => {
                let i = (addr & 0x7FF) as usize;
                u16::from_le_bytes([self.video.oam[i], self.video.oam[i + 1]])
            }
            0x08..=0x0A => self.slot2_read16(ARM9, addr),
            0xFF => {
                let i = (addr & 0xFFF) as usize;
                u16::from_le_bytes([self.bios9[i], self.bios9[i + 1]])
            }
            _ => 0,
        }
    }

    pub fn read9_32(&mut self, addr: u32, seq: bool) -> u32 {
        let addr = addr & !3;
        if self.cp15.itcm_readable() && self.cp15.in_itcm(addr) {
            self.time[ARM9] += 1;
            let i = (addr & 0x7FFF) as usize;
            return u32::from_le_bytes(self.itcm[i..i + 4].try_into().unwrap());
        }
        if self.cp15.dtcm_readable() && self.cp15.in_dtcm(addr) {
            self.time[ARM9] += 1;
            let i = (addr & 0x3FFF) as usize;
            return u32::from_le_bytes(self.dtcm[i..i + 4].try_into().unwrap());
        }
        self.cost9(addr, seq, 2);
        match addr >> 24 {
            0x02 => {
                let i = (addr & 0x3F_FFFF) as usize;
                u32::from_le_bytes(self.main_ram[i..i + 4].try_into().unwrap())
            }
            0x03 => match self.wram9(addr) {
                Some(i) => u32::from_le_bytes(self.shared_wram[i..i + 4].try_into().unwrap()),
                None => 0,
            },
            0x04 => self.io_read32(ARM9, addr & 0x00FF_FFFF),
            0x05 => {
                let i = (addr & 0x7FF) as usize;
                u32::from_le_bytes(self.video.palette[i..i + 4].try_into().unwrap())
            }
            0x06 => {
                let (region, offset) = vram_at(addr);
                self.video.vram.read32(region, offset)
            }
            0x07 => {
                let i = (addr & 0x7FF) as usize;
                u32::from_le_bytes(self.video.oam[i..i + 4].try_into().unwrap())
            }
            0x08..=0x0A => self.slot2_read32(ARM9, addr),
            0xFF => {
                let i = (addr & 0xFFF) as usize;
                u32::from_le_bytes(self.bios9[i..i + 4].try_into().unwrap())
            }
            _ => 0,
        }
    }

    pub fn write9_8(&mut self, addr: u32, value: u8, seq: bool) {
        if self.cp15.itcm_writable() && self.cp15.in_itcm(addr) {
            self.time[ARM9] += 1;
            self.itcm[(addr & 0x7FFF) as usize] = value;
            return;
        }
        if self.cp15.dtcm_writable() && self.cp15.in_dtcm(addr) {
            self.time[ARM9] += 1;
            self.dtcm[(addr & 0x3FFF) as usize] = value;
            return;
        }
        self.cost9(addr, seq, 1);
        match addr >> 24 {
            0x02 => self.main_ram[(addr & 0x3F_FFFF) as usize] = value,
            0x03 => {
                if let Some(i) = self.wram9(addr) {
                    self.shared_wram[i] = value;
                }
            }
            0x04 => {
                let reg = addr & 0x00FF_FFFF;
                let shift = (reg & 1) * 8;
                self.io_write16(ARM9, reg & !1, u16::from(value) << shift, 0xFF << shift);
            }
            0x08..=0x0A => self.slot2_write8(ARM9, addr, value),
            // Palettes, VRAM and OAM take no byte writes.
            _ => {}
        }
    }

    pub fn write9_16(&mut self, addr: u32, value: u16, seq: bool) {
        let addr = addr & !1;
        if self.cp15.itcm_writable() && self.cp15.in_itcm(addr) {
            self.time[ARM9] += 1;
            let i = (addr & 0x7FFF) as usize;
            self.itcm[i..i + 2].copy_from_slice(&value.to_le_bytes());
            return;
        }
        if self.cp15.dtcm_writable() && self.cp15.in_dtcm(addr) {
            self.time[ARM9] += 1;
            let i = (addr & 0x3FFF) as usize;
            self.dtcm[i..i + 2].copy_from_slice(&value.to_le_bytes());
            return;
        }
        self.cost9(addr, seq, 1);
        match addr >> 24 {
            0x02 => {
                let i = (addr & 0x3F_FFFF) as usize;
                self.main_ram[i..i + 2].copy_from_slice(&value.to_le_bytes());
            }
            0x03 => {
                if let Some(i) = self.wram9(addr) {
                    self.shared_wram[i..i + 2].copy_from_slice(&value.to_le_bytes());
                }
            }
            0x04 => self.io_write16(ARM9, addr & 0x00FF_FFFF, value, 0xFFFF),
            0x05 => {
                let i = (addr & 0x7FF) as usize;
                self.video.palette[i..i + 2].copy_from_slice(&value.to_le_bytes());
            }
            0x06 => {
                let (region, offset) = vram_at(addr);
                self.video.vram.write16(region, offset, value);
            }
            0x07 => {
                let i = (addr & 0x7FF) as usize;
                self.video.oam[i..i + 2].copy_from_slice(&value.to_le_bytes());
            }
            0x08..=0x0A => self.slot2_write16(ARM9, addr, value),
            _ => {}
        }
    }

    pub fn write9_32(&mut self, addr: u32, value: u32, seq: bool) {
        let addr = addr & !3;
        if self.cp15.itcm_writable() && self.cp15.in_itcm(addr) {
            self.time[ARM9] += 1;
            let i = (addr & 0x7FFF) as usize;
            self.itcm[i..i + 4].copy_from_slice(&value.to_le_bytes());
            return;
        }
        if self.cp15.dtcm_writable() && self.cp15.in_dtcm(addr) {
            self.time[ARM9] += 1;
            let i = (addr & 0x3FFF) as usize;
            self.dtcm[i..i + 4].copy_from_slice(&value.to_le_bytes());
            return;
        }
        self.cost9(addr, seq, 2);
        match addr >> 24 {
            0x02 => {
                let i = (addr & 0x3F_FFFF) as usize;
                self.main_ram[i..i + 4].copy_from_slice(&value.to_le_bytes());
            }
            0x03 => {
                if let Some(i) = self.wram9(addr) {
                    self.shared_wram[i..i + 4].copy_from_slice(&value.to_le_bytes());
                }
            }
            0x04 => self.io_write32(ARM9, addr & 0x00FF_FFFF, value),
            0x05 => {
                let i = (addr & 0x7FF) as usize;
                self.video.palette[i..i + 4].copy_from_slice(&value.to_le_bytes());
            }
            0x06 => {
                let (region, offset) = vram_at(addr);
                self.video.vram.write32(region, offset, value);
            }
            0x07 => {
                let i = (addr & 0x7FF) as usize;
                self.video.oam[i..i + 4].copy_from_slice(&value.to_le_bytes());
            }
            0x08..=0x0A => self.slot2_write32(ARM9, addr, value),
            _ => {}
        }
    }
}

/// The ARM9's view of the bus.
pub struct View9<'a>(pub &'a mut Bus);

impl View for View9<'_> {
    const CPU: usize = ARM9;
    #[inline(always)]
    fn bus(&mut self) -> &mut Bus {
        self.0
    }
}

impl pipit_arm::Bus for View9<'_> {
    #[inline(always)]
    fn read8(&mut self, addr: u32, seq: bool) -> u8 {
        self.0.read9_8(addr, seq)
    }
    #[inline(always)]
    fn read16(&mut self, addr: u32, seq: bool) -> u16 {
        self.0.read9_16(addr, seq)
    }
    #[inline(always)]
    fn read32(&mut self, addr: u32, seq: bool) -> u32 {
        self.0.read9_32(addr, seq)
    }
    #[inline(always)]
    fn write8(&mut self, addr: u32, value: u8, seq: bool) {
        self.0.write9_8(addr, value, seq)
    }
    #[inline(always)]
    fn write16(&mut self, addr: u32, value: u16, seq: bool) {
        self.0.write9_16(addr, value, seq)
    }
    #[inline(always)]
    fn write32(&mut self, addr: u32, value: u32, seq: bool) {
        self.0.write9_32(addr, value, seq)
    }
    #[inline(always)]
    fn fetch16(&mut self, addr: u32, seq: bool) -> u16 {
        self.0.read9_16(addr, seq)
    }
    #[inline(always)]
    fn fetch32(&mut self, addr: u32, seq: bool) -> u32 {
        self.0.read9_32(addr, seq)
    }
    #[inline(always)]
    fn idle(&mut self, cycles: u32) {
        self.0.time[ARM9] += u64::from(cycles);
    }
    #[inline(always)]
    fn set_pc(&mut self, pc: u32) {
        self.0.pc[ARM9] = pc;
    }
    #[inline(always)]
    fn sleeping(&mut self) -> bool {
        let bus = &mut *self.0;
        if !bus.irq[ARM9].halted {
            return false;
        }
        if bus.irq[ARM9].should_wake() {
            bus.irq[ARM9].halted = false;
            return false;
        }
        bus.time[ARM9] = bus.time[ARM9].max(bus.target);
        true
    }
    #[inline(always)]
    fn irq_pending(&self) -> bool {
        self.0.irq[ARM9].should_interrupt()
    }
    #[inline(always)]
    fn after_instruction(&mut self) {
        if self.0.dma[ARM9].has_pending() {
            run_dma(self);
        }
    }
    fn swi(&mut self, cpu: &mut pipit_arm::Cpu, number: u8) -> bool {
        if self.0.hle[ARM9] {
            crate::bios::swi::call(cpu, self, number);
            true
        } else {
            false
        }
    }
    fn cp15_read(&mut self, cn: u32, cm: u32, op2: u32) -> u32 {
        self.0.cp15.read(cn, cm, op2)
    }
    fn cp15_write(&mut self, cpu: &mut pipit_arm::Cpu, cn: u32, cm: u32, op2: u32, value: u32) {
        if self.0.cp15.write(cn, cm, op2, value) {
            self.0.irq[ARM9].halted = true;
        }
        cpu.vector_base = self.0.cp15.vector_base();
    }
}
