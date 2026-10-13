//! Thumb instruction set (ARM7TDMI data sheet, chapter 5).
//!
//! Formats are numbered as in the data sheet. `regs[15]` reads as the instruction
//! address plus 4 throughout.

use super::{barrel_shift, mode, vector, Arch, Cpu, FLAG_C, FLAG_T};
use crate::Bus;

impl Cpu {
    pub(super) fn execute_thumb<B: Bus>(&mut self, bus: &mut B, op: u16) {
        let op = u32::from(op);
        match op >> 13 {
            0b000 => {
                if (op >> 11) & 3 == 3 {
                    self.thumb_add_sub(op)
                } else {
                    self.thumb_shift(bus, op)
                }
            }
            0b001 => self.thumb_immediate(op),
            0b010 => match (op >> 10) & 7 {
                0b000 => self.thumb_alu(bus, op),
                0b001 => self.thumb_hi_reg(bus, op),
                0b010 | 0b011 => self.thumb_load_pc_relative(bus, op),
                _ => {
                    if op & (1 << 9) != 0 {
                        self.thumb_load_store_signed(bus, op)
                    } else {
                        self.thumb_load_store_register(bus, op)
                    }
                }
            },
            0b011 => self.thumb_load_store_immediate(bus, op),
            0b100 => {
                if op & (1 << 12) != 0 {
                    self.thumb_load_store_sp(bus, op)
                } else {
                    self.thumb_load_store_half(bus, op)
                }
            }
            0b101 => {
                if op & (1 << 12) == 0 {
                    self.thumb_load_address(op)
                } else if (op >> 8) & 0xF == 0 {
                    self.thumb_add_sp(op)
                } else if (op >> 9) & 3 == 2 {
                    self.thumb_push_pop(bus, op)
                } else {
                    self.thumb_undefined(bus)
                }
            }
            0b110 => {
                if op & (1 << 12) == 0 {
                    self.thumb_multiple(bus, op)
                } else if (op >> 8) & 0xF == 0xF {
                    self.thumb_swi(bus, op)
                } else {
                    self.thumb_conditional_branch(bus, op)
                }
            }
            _ => {
                if op & (1 << 12) != 0 {
                    self.thumb_long_branch(bus, op)
                } else if op & (1 << 11) == 0 {
                    self.thumb_branch(bus, op)
                } else if self.arch == Arch::V5TE {
                    self.thumb_blx_suffix(bus, op)
                } else {
                    self.thumb_undefined(bus)
                }
            }
        }
    }

    fn thumb_undefined<B: Bus>(&mut self, bus: &mut B) {
        let lr = self.regs[15].wrapping_sub(2);
        bus.idle(1);
        self.exception(bus, vector::UNDEFINED, mode::UND, lr);
    }

    fn thumb_swi<B: Bus>(&mut self, bus: &mut B, op: u32) {
        if !bus.swi(self, op as u8) {
            let lr = self.regs[15].wrapping_sub(2);
            self.exception(bus, vector::SWI, mode::SVC, lr);
        }
    }

    // Format 1: move shifted register.
    fn thumb_shift<B: Bus>(&mut self, _bus: &mut B, op: u32) {
        let kind = (op >> 11) & 3;
        let amount = (op >> 6) & 0x1F;
        let rs = ((op >> 3) & 7) as usize;
        let rd = (op & 7) as usize;
        let carry = self.cpsr & FLAG_C != 0;
        let (value, c) = barrel_shift(kind, self.regs[rs], amount, true, carry);
        self.regs[rd] = value;
        self.set_nzc(value, c);
    }

    // Format 2: add / subtract.
    fn thumb_add_sub(&mut self, op: u32) {
        let rs = ((op >> 3) & 7) as usize;
        let rd = (op & 7) as usize;
        let operand =
            if op & (1 << 10) != 0 { (op >> 6) & 7 } else { self.regs[((op >> 6) & 7) as usize] };
        let a = self.regs[rs];
        self.regs[rd] = if op & (1 << 9) != 0 {
            self.sub_flags(a, operand, true)
        } else {
            self.add_flags(a, operand, false)
        };
    }

    // Format 3: move / compare / add / subtract immediate.
    fn thumb_immediate(&mut self, op: u32) {
        let rd = ((op >> 8) & 7) as usize;
        let imm = op & 0xFF;
        match (op >> 11) & 3 {
            0 => {
                self.regs[rd] = imm;
                self.set_nz(imm);
            }
            1 => {
                self.sub_flags(self.regs[rd], imm, true);
            }
            2 => self.regs[rd] = self.add_flags(self.regs[rd], imm, false),
            _ => self.regs[rd] = self.sub_flags(self.regs[rd], imm, true),
        }
    }

    // Format 4: ALU operations.
    fn thumb_alu<B: Bus>(&mut self, bus: &mut B, op: u32) {
        let rs = ((op >> 3) & 7) as usize;
        let rd = (op & 7) as usize;
        let a = self.regs[rd];
        let b = self.regs[rs];
        let carry = self.cpsr & FLAG_C != 0;
        match (op >> 6) & 0xF {
            0x0 => {
                self.regs[rd] = a & b;
                self.set_nz(a & b);
            }
            0x1 => {
                self.regs[rd] = a ^ b;
                self.set_nz(a ^ b);
            }
            0x2..=0x4 | 0x7 => {
                // LSL, LSR, ASR, ROR by register: one extra internal cycle.
                let kind = match (op >> 6) & 0xF {
                    0x2 => 0,
                    0x3 => 1,
                    0x4 => 2,
                    _ => 3,
                };
                bus.idle(1);
                let (value, c) = barrel_shift(kind, a, b & 0xFF, false, carry);
                self.regs[rd] = value;
                self.set_nzc(value, c);
            }
            0x5 => self.regs[rd] = self.add_flags(a, b, carry),
            0x6 => self.regs[rd] = self.sub_flags(a, b, carry),
            0x8 => self.set_nz(a & b),
            0x9 => self.regs[rd] = self.sub_flags(0, b, true),
            0xA => {
                self.sub_flags(a, b, true);
            }
            0xB => {
                self.add_flags(a, b, false);
            }
            0xC => {
                self.regs[rd] = a | b;
                self.set_nz(a | b);
            }
            0xD => {
                bus.idle(Self::multiply_cycles(a, true));
                let value = a.wrapping_mul(b);
                self.regs[rd] = value;
                self.set_nz(value);
            }
            0xE => {
                self.regs[rd] = a & !b;
                self.set_nz(a & !b);
            }
            _ => {
                self.regs[rd] = !b;
                self.set_nz(!b);
            }
        }
    }

    // Format 5: hi register operations / branch exchange.
    fn thumb_hi_reg<B: Bus>(&mut self, bus: &mut B, op: u32) {
        let rs = (((op >> 3) & 7) | ((op >> 3) & 8)) as usize;
        let rd = ((op & 7) | ((op >> 4) & 8)) as usize;
        let b = self.regs[rs];
        match (op >> 8) & 3 {
            0 => {
                let value = self.regs[rd].wrapping_add(b);
                if rd == 15 {
                    self.refill_thumb(bus, value);
                } else {
                    self.regs[rd] = value;
                }
            }
            1 => {
                self.sub_flags(self.regs[rd], b, true);
            }
            2 => {
                if rd == 15 {
                    self.refill_thumb(bus, b);
                } else {
                    self.regs[rd] = b;
                }
            }
            _ => {
                // Bit 7 makes it BLX on ARMv5.
                if op & (1 << 7) != 0 && self.arch == Arch::V5TE {
                    self.regs[14] = self.regs[15].wrapping_sub(2) | 1;
                }
                self.branch_exchange(bus, b)
            }
        }
    }

    // Format 6: PC-relative load.
    fn thumb_load_pc_relative<B: Bus>(&mut self, bus: &mut B, op: u32) {
        let rd = ((op >> 8) & 7) as usize;
        let addr = (self.regs[15] & !2).wrapping_add((op & 0xFF) * 4);
        self.regs[rd] = bus.read32(addr, false);
        bus.idle(1);
        self.next_seq = false;
    }

    // Format 7: load / store with register offset.
    fn thumb_load_store_register<B: Bus>(&mut self, bus: &mut B, op: u32) {
        let ro = ((op >> 6) & 7) as usize;
        let rb = ((op >> 3) & 7) as usize;
        let rd = (op & 7) as usize;
        let addr = self.regs[rb].wrapping_add(self.regs[ro]);
        match (op >> 10) & 3 {
            0 => bus.write32(addr, self.regs[rd], false),
            1 => bus.write8(addr, self.regs[rd] as u8, false),
            2 => {
                self.regs[rd] = self.load_word(bus, addr, false);
                bus.idle(1);
            }
            _ => {
                self.regs[rd] = u32::from(bus.read8(addr, false));
                bus.idle(1);
            }
        }
        self.next_seq = false;
    }

    // Format 8: load / store sign-extended byte / halfword.
    fn thumb_load_store_signed<B: Bus>(&mut self, bus: &mut B, op: u32) {
        let ro = ((op >> 6) & 7) as usize;
        let rb = ((op >> 3) & 7) as usize;
        let rd = (op & 7) as usize;
        let addr = self.regs[rb].wrapping_add(self.regs[ro]);
        match (op >> 10) & 3 {
            0 => bus.write16(addr, self.regs[rd] as u16, false),
            1 => {
                self.regs[rd] = bus.read8(addr, false) as i8 as i32 as u32;
                bus.idle(1);
            }
            2 => {
                self.regs[rd] = self.load_half(bus, addr, false);
                bus.idle(1);
            }
            _ => {
                self.regs[rd] = self.load_signed_half(bus, addr, false);
                bus.idle(1);
            }
        }
        self.next_seq = false;
    }

    // Format 9: load / store with immediate offset.
    fn thumb_load_store_immediate<B: Bus>(&mut self, bus: &mut B, op: u32) {
        let rb = ((op >> 3) & 7) as usize;
        let rd = (op & 7) as usize;
        let offset = (op >> 6) & 0x1F;
        match (op >> 11) & 3 {
            0 => bus.write32(self.regs[rb].wrapping_add(offset * 4), self.regs[rd], false),
            1 => {
                self.regs[rd] = self.load_word(bus, self.regs[rb].wrapping_add(offset * 4), false);
                bus.idle(1);
            }
            2 => bus.write8(self.regs[rb].wrapping_add(offset), self.regs[rd] as u8, false),
            _ => {
                self.regs[rd] = u32::from(bus.read8(self.regs[rb].wrapping_add(offset), false));
                bus.idle(1);
            }
        }
        self.next_seq = false;
    }

    // Format 10: load / store halfword.
    fn thumb_load_store_half<B: Bus>(&mut self, bus: &mut B, op: u32) {
        let rb = ((op >> 3) & 7) as usize;
        let rd = (op & 7) as usize;
        let addr = self.regs[rb].wrapping_add(((op >> 6) & 0x1F) * 2);
        if op & (1 << 11) != 0 {
            self.regs[rd] = self.load_half(bus, addr, false);
            bus.idle(1);
        } else {
            bus.write16(addr, self.regs[rd] as u16, false);
        }
        self.next_seq = false;
    }

    // Format 11: SP-relative load / store.
    fn thumb_load_store_sp<B: Bus>(&mut self, bus: &mut B, op: u32) {
        let rd = ((op >> 8) & 7) as usize;
        let addr = self.regs[13].wrapping_add((op & 0xFF) * 4);
        if op & (1 << 11) != 0 {
            self.regs[rd] = self.load_word(bus, addr, false);
            bus.idle(1);
        } else {
            bus.write32(addr, self.regs[rd], false);
        }
        self.next_seq = false;
    }

    // Format 12: load address.
    fn thumb_load_address(&mut self, op: u32) {
        let rd = ((op >> 8) & 7) as usize;
        let offset = (op & 0xFF) * 4;
        self.regs[rd] = if op & (1 << 11) != 0 {
            self.regs[13].wrapping_add(offset)
        } else {
            (self.regs[15] & !2).wrapping_add(offset)
        };
    }

    // Format 13: add offset to stack pointer.
    fn thumb_add_sp(&mut self, op: u32) {
        let offset = (op & 0x7F) * 4;
        self.regs[13] = if op & (1 << 7) != 0 {
            self.regs[13].wrapping_sub(offset)
        } else {
            self.regs[13].wrapping_add(offset)
        };
    }

    // Format 14: push / pop registers.
    fn thumb_push_pop<B: Bus>(&mut self, bus: &mut B, op: u32) {
        let list = op & 0xFF;
        let extra = op & (1 << 8) != 0;
        let count = list.count_ones() + u32::from(extra);
        let mut seq = false;
        if op & (1 << 11) != 0 {
            // POP
            let mut addr = self.regs[13];
            for r in 0..8 {
                if list & (1 << r) != 0 {
                    self.regs[r] = bus.read32(addr, seq);
                    addr = addr.wrapping_add(4);
                    seq = true;
                }
            }
            if extra {
                let value = bus.read32(addr, seq);
                addr = addr.wrapping_add(4);
                self.regs[13] = addr;
                bus.idle(1);
                self.load_pc(bus, value);
            } else {
                self.regs[13] = addr;
                bus.idle(1);
            }
        } else {
            // PUSH
            let mut addr = self.regs[13].wrapping_sub(count * 4);
            self.regs[13] = addr;
            for r in 0..8 {
                if list & (1 << r) != 0 {
                    bus.write32(addr, self.regs[r], seq);
                    addr = addr.wrapping_add(4);
                    seq = true;
                }
            }
            if extra {
                bus.write32(addr, self.regs[14], seq);
            }
        }
        self.next_seq = false;
    }

    // Format 15: multiple load / store.
    fn thumb_multiple<B: Bus>(&mut self, bus: &mut B, op: u32) {
        let rb = ((op >> 8) & 7) as usize;
        let list = op & 0xFF;
        let mut addr = self.regs[rb];
        let mut seq = false;
        if list == 0 {
            // Empty list: PC is transferred and the base moves by 16 words.
            if op & (1 << 11) != 0 {
                let value = bus.read32(addr, false);
                self.regs[rb] = addr.wrapping_add(0x40);
                self.refill_thumb(bus, value);
            } else {
                bus.write32(addr, self.regs[15].wrapping_add(2), false);
                self.regs[rb] = addr.wrapping_add(0x40);
            }
            self.next_seq = false;
            return;
        }
        if op & (1 << 11) != 0 {
            // LDMIA: writeback unless the base is in the list.
            for r in 0..8 {
                if list & (1 << r) != 0 {
                    self.regs[r] = bus.read32(addr, seq);
                    addr = addr.wrapping_add(4);
                    seq = true;
                }
            }
            if list & (1 << rb) == 0 {
                self.regs[rb] = addr;
            }
            bus.idle(1);
        } else {
            // STMIA: the base is written back after the first store.
            let final_base = addr.wrapping_add(list.count_ones() * 4);
            let mut first = true;
            for r in 0..8 {
                if list & (1 << r) != 0 {
                    bus.write32(addr, self.regs[r], seq);
                    addr = addr.wrapping_add(4);
                    seq = true;
                    if first {
                        self.regs[rb] = final_base;
                        first = false;
                    }
                }
            }
        }
        self.next_seq = false;
    }

    // Format 16: conditional branch.
    fn thumb_conditional_branch<B: Bus>(&mut self, bus: &mut B, op: u32) {
        if self.condition((op >> 8) & 0xF) {
            let offset = ((op & 0xFF) as u8 as i8 as i32 * 2) as u32;
            let target = self.regs[15].wrapping_add(offset);
            self.refill_thumb(bus, target);
        }
    }

    // Format 18: unconditional branch.
    fn thumb_branch<B: Bus>(&mut self, bus: &mut B, op: u32) {
        let offset = (((op & 0x7FF) << 21) as i32 >> 20) as u32;
        let target = self.regs[15].wrapping_add(offset);
        self.refill_thumb(bus, target);
    }

    /// ARMv5: the second half of a BLX (11101), which lands in ARM state.
    fn thumb_blx_suffix<B: Bus>(&mut self, bus: &mut B, op: u32) {
        let next = self.regs[15].wrapping_sub(2);
        let target = self.regs[14].wrapping_add((op & 0x7FF) << 1) & !3;
        self.regs[14] = next | 1;
        self.cpsr &= !FLAG_T;
        self.refill_arm(bus, target);
    }

    // Format 19: long branch with link, in two halves.
    fn thumb_long_branch<B: Bus>(&mut self, bus: &mut B, op: u32) {
        let offset = op & 0x7FF;
        if op & (1 << 11) == 0 {
            let high = ((offset << 21) as i32 >> 9) as u32;
            self.regs[14] = self.regs[15].wrapping_add(high);
        } else {
            let next = self.regs[15].wrapping_sub(2);
            let target = self.regs[14].wrapping_add(offset << 1);
            self.regs[14] = next | 1;
            self.refill_thumb(bus, target);
        }
    }
}
