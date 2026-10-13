//! The I/O registers (04000000h–04FFFFFFh) as each CPU sees them. Both CPUs
//! share most addresses but not most registers: the engines and VRAM control
//! are the ARM9's, the sound, SPI and extra keys the ARM7's.

use super::{Bus, ARM7, ARM9};

impl Bus {
    /// A 2D engine's registers: `e` is 0 for A (000h), 1 for B (1000h).
    fn engine_read(&self, e: usize, reg: u32) -> u16 {
        let engine = &self.video.engines[e];
        match reg {
            0x000 => engine.dispcnt as u16,
            0x002 => (engine.dispcnt >> 16) as u16,
            0x008..=0x05F => engine.regs[((reg - 8) / 2) as usize],
            0x06C => engine.master_bright,
            _ => 0,
        }
    }

    fn engine_write(&mut self, e: usize, reg: u32, value: u16, mask: u16) {
        let engine = &mut self.video.engines[e];
        let v = u32::from(value & mask);
        let m = u32::from(mask);
        match reg {
            0x000 => engine.dispcnt = (engine.dispcnt & !m) | v,
            0x002 => engine.dispcnt = (engine.dispcnt & !(m << 16)) | (v << 16),
            0x008..=0x05F => {
                let r = &mut engine.regs[((reg - 8) / 2) as usize];
                *r = (*r & !mask) | (value & mask);
            }
            0x06C => engine.master_bright = (engine.master_bright & !mask) | (value & mask),
            _ => {}
        }
    }

    pub fn io_read16(&mut self, cpu: usize, reg: u32) -> u16 {
        match reg {
            0x004 => self.video.read_dispstat(cpu),
            0x006 => self.video.vcount,
            0x000..=0x06F if cpu == ARM9 => self.engine_read(0, reg),
            0x1000..=0x106F if cpu == ARM9 => self.engine_read(1, reg - 0x1000),
            0x0B0..=0x0DF => self.dma[cpu].read_io(reg),
            0x0E0..=0x0EF if cpu == ARM9 => {
                (self.dma[ARM9].fill[((reg - 0xE0) / 4) as usize] >> ((reg & 2) * 8)) as u16
            }
            0x100..=0x10F => self.timers[cpu].read_io(reg, self.time[cpu]),
            0x130 => self.keys,
            0x132 => self.keycnt[cpu],
            0x136 if cpu == ARM7 => u16::from(self.ext_keys),
            0x138 if cpu == ARM7 => self.rtc,
            0x180 => self.ipc.read_sync(cpu),
            0x184 => self.ipc.read_cnt(cpu),
            0x1A0 => self.cart.auxspicnt,
            0x1A2 => 0,
            0x1A4 => self.cart.read_romctrl() as u16,
            0x1A6 => (self.cart.read_romctrl() >> 16) as u16,
            0x1A8..=0x1AE => {
                let i = (reg - 0x1A8) as usize;
                u16::from_le_bytes([self.cart.command[i], self.cart.command[i + 1]])
            }
            0x1C0 if cpu == ARM7 => self.spicnt,
            0x1C2 if cpu == ARM7 => 0,
            0x204 => {
                if cpu == ARM9 {
                    self.exmemcnt
                } else {
                    (self.exmemcnt & 0xFF80) | (self.exmemstat7 & 0x7F)
                }
            }
            0x206 if cpu == ARM7 => self.wifiwaitcnt,
            0x208 | 0x210 | 0x212 | 0x214 | 0x216 => self.irq[cpu].read_io(reg),
            0x240 => {
                if cpu == ARM9 {
                    u16::from_le_bytes([self.video.vram.cnt[0], self.video.vram.cnt[1]])
                } else {
                    u16::from(self.video.vram.arm7_banks()) | u16::from(self.wramcnt) << 8
                }
            }
            0x242 if cpu == ARM9 => {
                u16::from_le_bytes([self.video.vram.cnt[2], self.video.vram.cnt[3]])
            }
            0x244 if cpu == ARM9 => {
                u16::from_le_bytes([self.video.vram.cnt[4], self.video.vram.cnt[5]])
            }
            0x246 if cpu == ARM9 => u16::from_le_bytes([self.video.vram.cnt[6], self.wramcnt]),
            0x248 if cpu == ARM9 => {
                u16::from_le_bytes([self.video.vram.cnt[7], self.video.vram.cnt[8]])
            }
            0x280..=0x2BF if cpu == ARM9 => self.math.read_io(reg),
            0x300 => u16::from(self.postflg[cpu]),
            0x304 => {
                if cpu == ARM9 {
                    self.video.powcnt1
                } else {
                    self.powcnt2
                }
            }
            0x400..=0x51F if cpu == ARM7 => {
                let i = (reg - 0x400) as usize;
                u16::from_le_bytes([self.sound[i], self.sound[i + 1]])
            }
            _ => 0,
        }
    }

    pub fn io_read32(&mut self, cpu: usize, reg: u32) -> u32 {
        match reg {
            0x10_0000 => {
                let (word, signals) = self.ipc.receive(cpu);
                self.apply_ipc(cpu, signals);
                word
            }
            0x10_0010 => {
                let word = self.cart.read_data(&mut self.scheduler);
                self.card_check();
                word
            }
            0x0E0..=0x0EC if cpu == ARM9 => self.dma[ARM9].fill[((reg - 0xE0) / 4) as usize],
            0x1A4 => self.cart.read_romctrl(),
            _ => {
                u32::from(self.io_read16(cpu, reg)) | u32::from(self.io_read16(cpu, reg + 2)) << 16
            }
        }
    }

    pub fn io_write16(&mut self, cpu: usize, reg: u32, value: u16, mask: u16) {
        let v = value & mask;
        match reg {
            0x004 => self.video.write_dispstat(cpu, value, mask),
            0x000..=0x06F if cpu == ARM9 => self.engine_write(0, reg, value, mask),
            0x1000..=0x106F if cpu == ARM9 => self.engine_write(1, reg - 0x1000, value, mask),
            0x0B0..=0x0DF => self.dma[cpu].write_io(reg, value, mask),
            0x0E0..=0x0EF if cpu == ARM9 => {
                let fill = &mut self.dma[ARM9].fill[((reg - 0xE0) / 4) as usize];
                let shift = (reg & 2) * 8;
                *fill = (*fill & !(u32::from(mask) << shift)) | (u32::from(v) << shift);
            }
            0x100..=0x10F => {
                let now = self.time[cpu];
                self.timers[cpu].write_io(reg, value, mask, now, &mut self.scheduler);
            }
            0x132 => self.keycnt[cpu] = (self.keycnt[cpu] & !mask) | v,
            0x138 if cpu == ARM7 => self.rtc = (self.rtc & !mask) | v,
            0x180 => {
                let signals = self.ipc.write_sync(cpu, value, mask);
                self.apply_ipc(cpu, signals);
            }
            0x184 => {
                let signals = self.ipc.write_cnt(cpu, value, mask);
                self.apply_ipc(cpu, signals);
            }
            0x1A0 => self.cart.auxspicnt = (self.cart.auxspicnt & !mask) | v,
            0x1A4 => {
                self.cart.write_romctrl(u32::from(value), u32::from(mask), &mut self.scheduler);
                self.card_check();
            }
            0x1A6 => {
                self.cart.write_romctrl(
                    u32::from(value) << 16,
                    u32::from(mask) << 16,
                    &mut self.scheduler,
                );
                self.card_check();
            }
            0x1A8..=0x1AE => {
                let i = (reg - 0x1A8) as usize;
                if mask & 0x00FF != 0 {
                    self.cart.command[i] = value as u8;
                }
                if mask & 0xFF00 != 0 {
                    self.cart.command[i + 1] = (value >> 8) as u8;
                }
            }
            0x1C0 if cpu == ARM7 => self.spicnt = (self.spicnt & !mask) | v,
            0x204 => {
                if cpu == ARM9 {
                    self.exmemcnt = (self.exmemcnt & !mask) | v;
                } else {
                    self.exmemstat7 = (self.exmemstat7 & !mask) | (v & 0x7F);
                }
            }
            0x206 if cpu == ARM7 => self.wifiwaitcnt = (self.wifiwaitcnt & !mask) | v,
            0x208 | 0x210 | 0x212 | 0x214 | 0x216 => self.irq[cpu].write_io(reg, value, mask),
            0x240 | 0x242 | 0x244 | 0x246 | 0x248 if cpu == ARM9 => {
                let first = ((reg - 0x240) / 2 * 2) as usize;
                for (k, byte) in [(first, value as u8), (first + 1, (value >> 8) as u8)] {
                    if mask & (0xFF << ((k & 1) * 8)) == 0 {
                        continue;
                    }
                    match k {
                        7 => self.wramcnt = byte & 3,
                        n => self.video.vram.write_cnt(if n > 7 { n - 1 } else { n }, byte),
                    }
                }
            }
            0x280..=0x2BF if cpu == ARM9 => self.math.write_io(reg, value, mask),
            0x300 => {
                if mask & 0x00FF != 0 {
                    self.postflg[cpu] |= value as u8 & if cpu == ARM9 { 3 } else { 1 };
                }
                if cpu == ARM7 && mask & 0xFF00 != 0 && (value >> 8) & 0xC0 != 0 {
                    self.irq[ARM7].halted = true;
                }
            }
            0x304 => {
                if cpu == ARM9 {
                    self.video.powcnt1 = (self.video.powcnt1 & !mask) | (v & 0x820F);
                } else {
                    self.powcnt2 = (self.powcnt2 & !mask) | (v & 3);
                }
            }
            0x400..=0x51F if cpu == ARM7 => {
                let i = (reg - 0x400) as usize;
                if mask & 0x00FF != 0 {
                    self.sound[i] = value as u8;
                }
                if mask & 0xFF00 != 0 {
                    self.sound[i + 1] = (value >> 8) as u8;
                }
            }
            _ => {}
        }
    }

    pub fn io_write32(&mut self, cpu: usize, reg: u32, value: u32) {
        match reg {
            0x188 => {
                let signals = self.ipc.send(cpu, value);
                self.apply_ipc(cpu, signals);
            }
            0x0E0..=0x0EC if cpu == ARM9 => {
                self.dma[ARM9].fill[((reg - 0xE0) / 4) as usize] = value
            }
            0x1A4 => {
                self.cart.write_romctrl(value, 0xFFFF_FFFF, &mut self.scheduler);
                self.card_check();
            }
            _ => {
                self.io_write16(cpu, reg, value as u16, 0xFFFF);
                self.io_write16(cpu, reg + 2, (value >> 16) as u16, 0xFFFF);
            }
        }
    }
}
