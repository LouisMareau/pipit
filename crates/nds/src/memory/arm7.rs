//! The ARM7's memory map and its view of the bus.

use super::{run_dma, Bus, View, ARM7};
use crate::video::vram::Region;

impl Bus {
    /// Where an ARM7 address at 03xxxxxxh lands: the shared WRAM (if any of
    /// it is the ARM7's and the address is below 03800000h) or its own.
    #[inline(always)]
    fn wram7(&self, addr: u32) -> (bool, usize) {
        if addr & 0x0080_0000 == 0 {
            match self.wramcnt & 3 {
                1 => return (true, (addr & 0x3FFF) as usize),
                2 => return (true, 0x4000 + (addr & 0x3FFF) as usize),
                3 => return (true, (addr & 0x7FFF) as usize),
                _ => {}
            }
        }
        (false, (addr & 0xFFFF) as usize)
    }

    /// Cycles for an ARM7 access: `halves` 16-bit transfers on the narrow buses.
    #[inline(always)]
    fn cost7(&mut self, addr: u32, seq: bool, halves: u64) {
        let cycles = match addr >> 24 {
            0x00 | 0x03 => 2,
            0x02 => 2 * halves + u64::from(!seq) * 2,
            _ => 2 * halves,
        };
        self.time[ARM7] += cycles;
    }

    pub fn read7_8(&mut self, addr: u32, seq: bool) -> u8 {
        self.cost7(addr, seq, 1);
        match addr >> 24 {
            0x00 => self.bios7.get(addr as usize).copied().unwrap_or(0),
            0x02 => self.main_ram[(addr & 0x3F_FFFF) as usize],
            0x03 => match self.wram7(addr) {
                (true, i) => self.shared_wram[i],
                (false, i) => self.arm7_wram[i],
            },
            0x04 if addr & 0x0080_0000 == 0 => {
                let reg = addr & 0x00FF_FFFF;
                (self.io_read16(ARM7, reg & !1) >> ((reg & 1) * 8)) as u8
            }
            0x06 => self.video.vram.read8(Region::Arm7, (addr & 0x3_FFFF) as usize),
            0x08..=0x0A => 0xFF,
            _ => 0,
        }
    }

    pub fn read7_16(&mut self, addr: u32, seq: bool) -> u16 {
        let addr = addr & !1;
        self.cost7(addr, seq, 1);
        match addr >> 24 {
            0x00 => match self.bios7.get(addr as usize..addr as usize + 2) {
                Some(b) => u16::from_le_bytes([b[0], b[1]]),
                None => 0,
            },
            0x02 => {
                let i = (addr & 0x3F_FFFF) as usize;
                u16::from_le_bytes([self.main_ram[i], self.main_ram[i + 1]])
            }
            0x03 => match self.wram7(addr) {
                (true, i) => u16::from_le_bytes([self.shared_wram[i], self.shared_wram[i + 1]]),
                (false, i) => u16::from_le_bytes([self.arm7_wram[i], self.arm7_wram[i + 1]]),
            },
            0x04 if addr & 0x0080_0000 == 0 => self.io_read16(ARM7, addr & 0x00FF_FFFF),
            0x06 => self.video.vram.read16(Region::Arm7, (addr & 0x3_FFFF) as usize),
            0x08..=0x0A => 0xFFFF,
            _ => 0,
        }
    }

    pub fn read7_32(&mut self, addr: u32, seq: bool) -> u32 {
        let addr = addr & !3;
        self.cost7(addr, seq, 2);
        match addr >> 24 {
            0x00 => match self.bios7.get(addr as usize..addr as usize + 4) {
                Some(b) => u32::from_le_bytes(b.try_into().unwrap()),
                None => 0,
            },
            0x02 => {
                let i = (addr & 0x3F_FFFF) as usize;
                u32::from_le_bytes(self.main_ram[i..i + 4].try_into().unwrap())
            }
            0x03 => match self.wram7(addr) {
                (true, i) => u32::from_le_bytes(self.shared_wram[i..i + 4].try_into().unwrap()),
                (false, i) => u32::from_le_bytes(self.arm7_wram[i..i + 4].try_into().unwrap()),
            },
            0x04 if addr & 0x0080_0000 == 0 => self.io_read32(ARM7, addr & 0x00FF_FFFF),
            0x06 => self.video.vram.read32(Region::Arm7, (addr & 0x3_FFFF) as usize),
            0x08..=0x0A => 0xFFFF_FFFF,
            _ => 0,
        }
    }

    pub fn write7_8(&mut self, addr: u32, value: u8, seq: bool) {
        self.cost7(addr, seq, 1);
        match addr >> 24 {
            0x02 => self.main_ram[(addr & 0x3F_FFFF) as usize] = value,
            0x03 => match self.wram7(addr) {
                (true, i) => self.shared_wram[i] = value,
                (false, i) => self.arm7_wram[i] = value,
            },
            0x04 if addr & 0x0080_0000 == 0 => {
                let reg = addr & 0x00FF_FFFF;
                let shift = (reg & 1) * 8;
                self.io_write16(ARM7, reg & !1, u16::from(value) << shift, 0xFF << shift);
            }
            _ => {}
        }
    }

    pub fn write7_16(&mut self, addr: u32, value: u16, seq: bool) {
        let addr = addr & !1;
        self.cost7(addr, seq, 1);
        match addr >> 24 {
            0x02 => {
                let i = (addr & 0x3F_FFFF) as usize;
                self.main_ram[i..i + 2].copy_from_slice(&value.to_le_bytes());
            }
            0x03 => match self.wram7(addr) {
                (true, i) => self.shared_wram[i..i + 2].copy_from_slice(&value.to_le_bytes()),
                (false, i) => self.arm7_wram[i..i + 2].copy_from_slice(&value.to_le_bytes()),
            },
            0x04 if addr & 0x0080_0000 == 0 => {
                self.io_write16(ARM7, addr & 0x00FF_FFFF, value, 0xFFFF)
            }
            0x06 => self.video.vram.write16(Region::Arm7, (addr & 0x3_FFFF) as usize, value),
            _ => {}
        }
    }

    pub fn write7_32(&mut self, addr: u32, value: u32, seq: bool) {
        let addr = addr & !3;
        self.cost7(addr, seq, 2);
        match addr >> 24 {
            0x02 => {
                let i = (addr & 0x3F_FFFF) as usize;
                self.main_ram[i..i + 4].copy_from_slice(&value.to_le_bytes());
            }
            0x03 => match self.wram7(addr) {
                (true, i) => self.shared_wram[i..i + 4].copy_from_slice(&value.to_le_bytes()),
                (false, i) => self.arm7_wram[i..i + 4].copy_from_slice(&value.to_le_bytes()),
            },
            0x04 if addr & 0x0080_0000 == 0 => self.io_write32(ARM7, addr & 0x00FF_FFFF, value),
            0x06 => self.video.vram.write32(Region::Arm7, (addr & 0x3_FFFF) as usize, value),
            _ => {}
        }
    }
}

/// The ARM7's view of the bus.
pub struct View7<'a>(pub &'a mut Bus);

impl View for View7<'_> {
    const CPU: usize = ARM7;
    #[inline(always)]
    fn bus(&mut self) -> &mut Bus {
        self.0
    }
}

impl pipit_arm::Bus for View7<'_> {
    #[inline(always)]
    fn read8(&mut self, addr: u32, seq: bool) -> u8 {
        self.0.read7_8(addr, seq)
    }
    #[inline(always)]
    fn read16(&mut self, addr: u32, seq: bool) -> u16 {
        self.0.read7_16(addr, seq)
    }
    #[inline(always)]
    fn read32(&mut self, addr: u32, seq: bool) -> u32 {
        self.0.read7_32(addr, seq)
    }
    #[inline(always)]
    fn write8(&mut self, addr: u32, value: u8, seq: bool) {
        self.0.write7_8(addr, value, seq)
    }
    #[inline(always)]
    fn write16(&mut self, addr: u32, value: u16, seq: bool) {
        self.0.write7_16(addr, value, seq)
    }
    #[inline(always)]
    fn write32(&mut self, addr: u32, value: u32, seq: bool) {
        self.0.write7_32(addr, value, seq)
    }
    #[inline(always)]
    fn fetch16(&mut self, addr: u32, seq: bool) -> u16 {
        self.0.read7_16(addr, seq)
    }
    #[inline(always)]
    fn fetch32(&mut self, addr: u32, seq: bool) -> u32 {
        self.0.read7_32(addr, seq)
    }
    #[inline(always)]
    fn idle(&mut self, cycles: u32) {
        self.0.time[ARM7] += u64::from(cycles) * 2;
    }
    #[inline(always)]
    fn set_pc(&mut self, pc: u32) {
        self.0.pc[ARM7] = pc;
    }
    #[inline(always)]
    fn sleeping(&mut self) -> bool {
        let bus = &mut *self.0;
        if !bus.irq[ARM7].halted {
            return false;
        }
        if bus.irq[ARM7].should_wake() {
            bus.irq[ARM7].halted = false;
            return false;
        }
        bus.time[ARM7] = bus.time[ARM7].max(bus.target);
        true
    }
    #[inline(always)]
    fn irq_pending(&self) -> bool {
        self.0.irq[ARM7].should_interrupt()
    }
    #[inline(always)]
    fn after_instruction(&mut self) {
        if self.0.dma[ARM7].has_pending() {
            run_dma(self);
        }
    }
    fn swi(&mut self, cpu: &mut pipit_arm::Cpu, number: u8) -> bool {
        if self.0.hle[ARM7] {
            crate::bios::swi::call(cpu, self, number);
            true
        } else {
            false
        }
    }
}
