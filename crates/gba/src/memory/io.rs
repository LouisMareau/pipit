//! I/O register dispatch (GBATEK "GBA I/O Map").
//!
//! Registers are addressed by their offset from `0x04000000`. All accesses are
//! 16-bit; byte accesses arrive with a `mask` saying which half is written, and
//! 32-bit accesses are two halfword accesses.

use super::Bus;

// Video
pub const DISPCNT: u32 = 0x000;
pub const DISPSTAT: u32 = 0x004;
pub const VCOUNT: u32 = 0x006;
pub const BLDY: u32 = 0x054;
// Audio
pub const SOUND1CNT_L: u32 = 0x060;
pub const FIFO_B_H: u32 = 0x0A6;
// DMA
pub const DMA0SAD: u32 = 0x0B0;
pub const DMA3CNT_H: u32 = 0x0DE;
// Timers
pub const TM0CNT_L: u32 = 0x100;
pub const TM3CNT_H: u32 = 0x10E;
// Serial
pub const SIODATA32: u32 = 0x120;
pub const JOY_STAT: u32 = 0x158;
// Keypad
pub const KEYINPUT: u32 = 0x130;
pub const KEYCNT: u32 = 0x132;
// System
pub const IE: u32 = 0x200;
pub const IF: u32 = 0x202;
pub const WAITCNT: u32 = 0x204;
pub const IME: u32 = 0x208;
pub const POSTFLG: u32 = 0x300;
pub const HALTCNT: u32 = 0x301;
pub const MEMCTRL: u32 = 0x800;

impl Bus {
    /// Reads a 16-bit I/O register. `addr` is a full, halfword-aligned address.
    pub fn read_io(&mut self, addr: u32) -> u16 {
        let reg = addr & 0x00FF_FFFF;
        // The memory control register is mirrored every 64K through the I/O region.
        if reg >= 0x400 {
            return match reg & 0xFFFF {
                0x800 => self.memctrl as u16,
                0x802 => (self.memctrl >> 16) as u16,
                _ => (self.open_bus >> ((addr & 2) * 8)) as u16,
            };
        }
        match reg {
            DISPCNT..=BLDY => self.video.read_io(reg, self.open_bus),
            SOUND1CNT_L..=FIFO_B_H => self.audio.read_io(reg),
            DMA0SAD..=DMA3CNT_H => self.dma.read_io(reg),
            TM0CNT_L..=TM3CNT_H => self.timers.read_io(reg, self.scheduler.now()),
            KEYINPUT | KEYCNT => self.keypad.read_io(reg),
            SIODATA32..=JOY_STAT => self.sio[((reg - SIODATA32) / 2) as usize],
            IE | IF | IME => self.irq.read_io(reg),
            WAITCNT => self.waitcnt,
            POSTFLG => u16::from(self.postflg),
            // Write-only or unmapped registers read back as open bus.
            _ => (self.open_bus >> ((addr & 2) * 8)) as u16,
        }
    }

    /// Writes a 16-bit I/O register. `mask` selects which bytes are written.
    pub fn write_io(&mut self, addr: u32, value: u16, mask: u16) {
        let reg = addr & 0x00FF_FFFF;
        if reg >= 0x400 {
            match reg & 0xFFFF {
                0x800 => {
                    let lo = (self.memctrl as u16 & !mask) | (value & mask);
                    self.memctrl = (self.memctrl & 0xFFFF_0000) | u32::from(lo);
                    self.update_wait_states();
                }
                0x802 => {
                    let hi = ((self.memctrl >> 16) as u16 & !mask) | (value & mask);
                    self.memctrl = (self.memctrl & 0x0000_FFFF) | (u32::from(hi) << 16);
                    self.update_wait_states();
                }
                _ => {}
            }
            return;
        }
        match reg {
            DISPCNT..=BLDY => self.video.write_io(reg, value, mask),
            SOUND1CNT_L..=FIFO_B_H => self.audio.write_io(reg, value, mask),
            DMA0SAD..=DMA3CNT_H => {
                self.dma.write_io(reg, value, mask);
                if self.dma.has_pending() {
                    self.run_dma();
                }
            }
            TM0CNT_L..=TM3CNT_H => self.timers.write_io(reg, value, mask, &mut self.scheduler),
            KEYINPUT => {}
            KEYCNT => self.keypad.write_io(reg, value, mask, &mut self.irq),
            SIODATA32..=JOY_STAT => {
                let slot = &mut self.sio[((reg - SIODATA32) / 2) as usize];
                *slot = (*slot & !mask) | (value & mask);
            }
            IE | IF | IME => self.irq.write_io(reg, value, mask),
            WAITCNT => {
                self.waitcnt = (self.waitcnt & !mask) | (value & mask & 0x5FFF);
                self.update_wait_states();
                self.cart.set_prefetch_enabled(self.waitcnt & (1 << 14) != 0);
            }
            POSTFLG => {
                if mask & 0x00FF != 0 {
                    self.postflg = value as u8 & 1;
                }
                // HALTCNT is the high byte: bit 7 clear = halt, set = stop.
                if mask & 0xFF00 != 0 {
                    if value & 0x8000 != 0 {
                        self.irq.stopped = true;
                    } else {
                        self.irq.halted = true;
                    }
                }
            }
            _ => {}
        }
    }
}
