//! ARM instruction set (ARM7TDMI data sheet, chapter 4).
//!
//! Decoding keys on bits 27-20 and 7-4, which is enough to tell every class apart.
//! Handlers never write `regs[15]` directly: branches go through `Cpu::branch`.

use super::{barrel_shift, mode, vector, Cpu, FLAG_C, FLAG_T};
use crate::memory::Bus;

impl Cpu {
    pub(super) fn execute_arm(&mut self, bus: &mut Bus, op: u32) {
        if !self.condition(op >> 28) {
            return;
        }
        let key = ((op >> 16) & 0xFF0) | ((op >> 4) & 0xF);
        match key {
            // Branch and exchange: 0001 0010 .... .... .... 0001
            0x121 if op & 0x0FFF_FF00 == 0x012F_FF00 => self.arm_bx(bus, op),
            // Multiply / multiply long: 0000 00xx 1001 and 0000 1xxx 1001
            0x009 | 0x019 | 0x029 | 0x039 => self.arm_multiply(bus, op),
            0x089 | 0x099 | 0x0A9 | 0x0B9 | 0x0C9 | 0x0D9 | 0x0E9 | 0x0F9 => {
                self.arm_multiply_long(bus, op)
            }
            // Single data swap: 0001 0x00 1001
            0x109 | 0x149 => self.arm_swap(bus, op),
            // Halfword and signed transfers: 000x xxxx 1xx1 with bits 6-5 != 00
            k if k & 0xE09 == 0x009 && k & 0x6 != 0 => self.arm_halfword_transfer(bus, op),
            // PSR transfers: 0001 0x00 0000 (MRS) / 0001 0x10 .... and 0011 0x10 (MSR)
            0x100 | 0x140 => self.arm_mrs(op),
            k if k & 0xFB0 == 0x120 || k & 0xFB0 == 0x320 => self.arm_msr(op),
            // Data processing: 00xx xxxx xxxx
            k if k & 0xC00 == 0x000 => self.arm_data_processing(bus, op),
            // Single data transfer: 01xx xxxx xxxx (register form must have bit 4 clear)
            k if k & 0xC00 == 0x400 => {
                if k & 0x201 == 0x201 {
                    self.arm_undefined(bus);
                } else {
                    self.arm_single_transfer(bus, op);
                }
            }
            // Block data transfer: 100x xxxx
            k if k & 0xE00 == 0x800 => self.arm_block_transfer(bus, op),
            // Branch: 101x xxxx
            k if k & 0xE00 == 0xA00 => self.arm_branch(bus, op),
            // Software interrupt: 1111 xxxx
            k if k & 0xF00 == 0xF00 => self.arm_swi(bus, op),
            // Coprocessor instructions: the GBA has none.
            _ => self.arm_undefined(bus),
        }
    }

    fn arm_undefined(&mut self, bus: &mut Bus) {
        let lr = self.regs[15].wrapping_sub(4);
        bus.idle(1);
        self.exception(bus, vector::UNDEFINED, mode::UND, lr);
    }

    fn arm_swi(&mut self, bus: &mut Bus, op: u32) {
        if bus.bios.hle {
            crate::bios::swi::call(self, bus, (op >> 16) as u8);
        } else {
            let lr = self.regs[15].wrapping_sub(4);
            self.exception(bus, vector::SWI, mode::SVC, lr);
        }
    }

    fn arm_branch(&mut self, bus: &mut Bus, op: u32) {
        let offset = ((op << 8) as i32 >> 6) as u32;
        if op & (1 << 24) != 0 {
            self.regs[14] = self.regs[15].wrapping_sub(4);
        }
        let target = self.regs[15].wrapping_add(offset);
        self.refill_arm(bus, target);
    }

    fn arm_bx(&mut self, bus: &mut Bus, op: u32) {
        let target = self.regs[(op & 0xF) as usize];
        self.branch_exchange(bus, target);
    }

    // ---------------------------------------------------------------------------
    // Data processing
    // ---------------------------------------------------------------------------

    fn arm_data_processing(&mut self, bus: &mut Bus, op: u32) {
        let opcode = (op >> 21) & 0xF;
        let set_flags = op & (1 << 20) != 0;
        let rn = ((op >> 16) & 0xF) as usize;
        let rd = ((op >> 12) & 0xF) as usize;
        let carry_in = self.cpsr & FLAG_C != 0;

        // Operand 2 and the shifter carry.
        let (op2, shifter_carry, rn_value) = if op & (1 << 25) != 0 {
            let imm = op & 0xFF;
            let rot = (op >> 7) & 0x1E;
            let value = imm.rotate_right(rot);
            let carry = if rot == 0 { carry_in } else { value >> 31 != 0 };
            (value, carry, self.regs[rn])
        } else {
            let rm = (op & 0xF) as usize;
            let kind = (op >> 5) & 3;
            if op & (1 << 4) != 0 {
                // Shift amount from a register: one internal cycle, and PC reads as +12.
                bus.idle(1);
                let amount = self.regs[((op >> 8) & 0xF) as usize] & 0xFF;
                let rm_value = if rm == 15 { self.regs[15].wrapping_add(4) } else { self.regs[rm] };
                let rn_value = if rn == 15 { self.regs[15].wrapping_add(4) } else { self.regs[rn] };
                let (v, c) = barrel_shift(kind, rm_value, amount, false, carry_in);
                (v, c, rn_value)
            } else {
                let amount = (op >> 7) & 0x1F;
                let (v, c) = barrel_shift(kind, self.regs[rm], amount, true, carry_in);
                (v, c, self.regs[rn])
            }
        };

        let result = match opcode {
            0x0 | 0x8 => rn_value & op2, // AND, TST
            0x1 | 0x9 => rn_value ^ op2, // EOR, TEQ
            0x2 | 0xA => {
                // SUB, CMP
                if set_flags {
                    self.sub_flags(rn_value, op2, true)
                } else {
                    rn_value.wrapping_sub(op2)
                }
            }
            0x3 => {
                if set_flags {
                    self.sub_flags(op2, rn_value, true)
                } else {
                    op2.wrapping_sub(rn_value)
                }
            }
            0x4 | 0xB => {
                // ADD, CMN
                if set_flags {
                    self.add_flags(rn_value, op2, false)
                } else {
                    rn_value.wrapping_add(op2)
                }
            }
            0x5 => {
                if set_flags {
                    self.add_flags(rn_value, op2, carry_in)
                } else {
                    rn_value.wrapping_add(op2).wrapping_add(u32::from(carry_in))
                }
            }
            0x6 => {
                if set_flags {
                    self.sub_flags(rn_value, op2, carry_in)
                } else {
                    rn_value.wrapping_sub(op2).wrapping_sub(u32::from(!carry_in))
                }
            }
            0x7 => {
                if set_flags {
                    self.sub_flags(op2, rn_value, carry_in)
                } else {
                    op2.wrapping_sub(rn_value).wrapping_sub(u32::from(!carry_in))
                }
            }
            0xC => rn_value | op2,
            0xD => op2,
            0xE => rn_value & !op2,
            _ => !op2,
        };

        let logical = matches!(opcode, 0x0 | 0x1 | 0x8 | 0x9 | 0xC | 0xD | 0xE | 0xF);
        let test_only = matches!(opcode, 0x8..=0xB);

        if rd == 15 && set_flags && test_only {
            // CMP/CMN/TST/TEQ with Rd = 15 (the "P" forms): CPSR = SPSR, no result,
            // no pipeline flush.
            let spsr = self.spsr;
            self.set_cpsr(spsr);
            return;
        }
        if rd == 15 && set_flags {
            // Return from exception: restore CPSR (possibly into Thumb) and branch.
            let spsr = self.spsr;
            self.set_cpsr(spsr);
            if self.cpsr & FLAG_T != 0 {
                self.refill_thumb(bus, result);
            } else {
                self.refill_arm(bus, result);
            }
            return;
        }
        if set_flags && logical {
            self.set_nzc(result, shifter_carry);
        }
        if test_only {
            return;
        }
        if rd == 15 {
            self.refill_arm(bus, result);
        } else {
            self.regs[rd] = result;
        }
    }

    fn arm_mrs(&mut self, op: u32) {
        let rd = ((op >> 12) & 0xF) as usize;
        self.regs[rd] = if op & (1 << 22) != 0 { self.spsr } else { self.cpsr };
    }

    fn arm_msr(&mut self, op: u32) {
        let value = if op & (1 << 25) != 0 {
            (op & 0xFF).rotate_right((op >> 7) & 0x1E)
        } else {
            self.regs[(op & 0xF) as usize]
        };
        let mut mask = 0u32;
        if op & (1 << 16) != 0 {
            mask |= 0x0000_00FF;
        }
        if op & (1 << 17) != 0 {
            mask |= 0x0000_FF00;
        }
        if op & (1 << 18) != 0 {
            mask |= 0x00FF_0000;
        }
        if op & (1 << 19) != 0 {
            mask |= 0xFF00_0000;
        }
        if op & (1 << 22) != 0 {
            self.spsr = (self.spsr & !mask) | (value & mask);
        } else {
            // User mode may only change the flags.
            if self.cpsr & 0x1F == mode::USER {
                mask &= 0xFF00_0000;
            }
            // Changing T through MSR is unpredictable; keep the current state.
            mask &= !FLAG_T;
            let new = (self.cpsr & !mask) | (value & mask);
            self.set_cpsr(new);
        }
    }

    // ---------------------------------------------------------------------------
    // Multiply
    // ---------------------------------------------------------------------------

    fn arm_multiply(&mut self, bus: &mut Bus, op: u32) {
        let rd = ((op >> 16) & 0xF) as usize;
        let rn = ((op >> 12) & 0xF) as usize;
        let rs = ((op >> 8) & 0xF) as usize;
        let rm = (op & 0xF) as usize;
        let accumulate = op & (1 << 21) != 0;
        let rs_value = self.regs[rs];
        let mut cycles = Self::multiply_cycles(rs_value, true);
        let mut result = self.regs[rm].wrapping_mul(rs_value);
        if accumulate {
            result = result.wrapping_add(self.regs[rn]);
            cycles += 1;
        }
        bus.idle(cycles);
        self.regs[rd] = result;
        if op & (1 << 20) != 0 {
            self.set_nz(result);
        }
    }

    fn arm_multiply_long(&mut self, bus: &mut Bus, op: u32) {
        let rd_hi = ((op >> 16) & 0xF) as usize;
        let rd_lo = ((op >> 12) & 0xF) as usize;
        let rs = ((op >> 8) & 0xF) as usize;
        let rm = (op & 0xF) as usize;
        let signed = op & (1 << 22) != 0;
        let accumulate = op & (1 << 21) != 0;
        let rs_value = self.regs[rs];
        let mut cycles = Self::multiply_cycles(rs_value, signed) + 1;
        let mut result: u64 = if signed {
            (i64::from(self.regs[rm] as i32) * i64::from(rs_value as i32)) as u64
        } else {
            u64::from(self.regs[rm]) * u64::from(rs_value)
        };
        if accumulate {
            let acc = (u64::from(self.regs[rd_hi]) << 32) | u64::from(self.regs[rd_lo]);
            result = result.wrapping_add(acc);
            cycles += 1;
        }
        bus.idle(cycles);
        self.regs[rd_lo] = result as u32;
        self.regs[rd_hi] = (result >> 32) as u32;
        if op & (1 << 20) != 0 {
            self.cpsr = (self.cpsr & !(super::FLAG_N | super::FLAG_Z))
                | ((result >> 32) as u32 & super::FLAG_N)
                | if result == 0 { super::FLAG_Z } else { 0 };
        }
    }

    // ---------------------------------------------------------------------------
    // Loads and stores
    // ---------------------------------------------------------------------------

    fn arm_swap(&mut self, bus: &mut Bus, op: u32) {
        let rn = ((op >> 16) & 0xF) as usize;
        let rd = ((op >> 12) & 0xF) as usize;
        let rm = (op & 0xF) as usize;
        let addr = self.regs[rn];
        let value = if op & (1 << 22) != 0 {
            let old = u32::from(bus.read8(addr, false));
            bus.write8(addr, self.regs[rm] as u8, false);
            old
        } else {
            let old = self.load_word(bus, addr, false);
            bus.write32(addr, self.regs[rm], false);
            old
        };
        bus.idle(1);
        self.regs[rd] = value;
        self.next_seq = false;
    }

    fn arm_single_transfer(&mut self, bus: &mut Bus, op: u32) {
        let pre = op & (1 << 24) != 0;
        let up = op & (1 << 23) != 0;
        let byte = op & (1 << 22) != 0;
        let writeback = op & (1 << 21) != 0 || !pre;
        let load = op & (1 << 20) != 0;
        let rn = ((op >> 16) & 0xF) as usize;
        let rd = ((op >> 12) & 0xF) as usize;

        let offset = if op & (1 << 25) != 0 {
            let rm = (op & 0xF) as usize;
            let carry = self.cpsr & FLAG_C != 0;
            barrel_shift((op >> 5) & 3, self.regs[rm], (op >> 7) & 0x1F, true, carry).0
        } else {
            op & 0xFFF
        };

        let base = self.regs[rn];
        let offset_base = if up { base.wrapping_add(offset) } else { base.wrapping_sub(offset) };
        let addr = if pre { offset_base } else { base };

        if load {
            let value = if byte {
                u32::from(bus.read8(addr, false))
            } else {
                self.load_word(bus, addr, false)
            };
            bus.idle(1);
            if writeback {
                self.regs[rn] = offset_base;
            }
            // A loaded value overrides the writeback when Rd == Rn.
            if rd == 15 {
                self.refill_arm(bus, value);
            } else {
                self.regs[rd] = value;
            }
        } else {
            // Stores of PC see it 12 bytes ahead.
            let value = if rd == 15 { self.regs[15].wrapping_add(4) } else { self.regs[rd] };
            if byte {
                bus.write8(addr, value as u8, false);
            } else {
                bus.write32(addr, value, false);
            }
            if writeback {
                self.regs[rn] = offset_base;
            }
        }
        self.next_seq = false;
    }

    fn arm_halfword_transfer(&mut self, bus: &mut Bus, op: u32) {
        let pre = op & (1 << 24) != 0;
        let up = op & (1 << 23) != 0;
        let writeback = op & (1 << 21) != 0 || !pre;
        let load = op & (1 << 20) != 0;
        let rn = ((op >> 16) & 0xF) as usize;
        let rd = ((op >> 12) & 0xF) as usize;
        let kind = (op >> 5) & 3;

        let offset = if op & (1 << 22) != 0 {
            ((op >> 4) & 0xF0) | (op & 0xF)
        } else {
            self.regs[(op & 0xF) as usize]
        };

        let base = self.regs[rn];
        let offset_base = if up { base.wrapping_add(offset) } else { base.wrapping_sub(offset) };
        let addr = if pre { offset_base } else { base };

        if load {
            let value = match kind {
                1 => self.load_half(bus, addr, false),
                2 => bus.read8(addr, false) as i8 as i32 as u32,
                _ => self.load_signed_half(bus, addr, false),
            };
            bus.idle(1);
            if writeback {
                self.regs[rn] = offset_base;
            }
            if rd == 15 {
                self.refill_arm(bus, value);
            } else {
                self.regs[rd] = value;
            }
        } else {
            // Only STRH exists on ARMv4; the signed forms are undefined when storing.
            if kind != 1 {
                self.arm_undefined(bus);
                return;
            }
            let value = if rd == 15 { self.regs[15].wrapping_add(4) } else { self.regs[rd] };
            bus.write16(addr, value as u16, false);
            if writeback {
                self.regs[rn] = offset_base;
            }
        }
        self.next_seq = false;
    }

    fn arm_block_transfer(&mut self, bus: &mut Bus, op: u32) {
        let pre = op & (1 << 24) != 0;
        let up = op & (1 << 23) != 0;
        let psr_or_user = op & (1 << 22) != 0;
        let writeback = op & (1 << 21) != 0;
        let load = op & (1 << 20) != 0;
        let rn = ((op >> 16) & 0xF) as usize;
        let mut list = op & 0xFFFF;

        // An empty list transfers PC alone and moves the base by 16 words.
        let empty = list == 0;
        if empty {
            list = 1 << 15;
        }
        let count = if empty { 16 } else { list.count_ones() };
        let base = self.regs[rn];
        let span = count * 4;

        let (mut addr, final_base) = if up {
            (if pre { base.wrapping_add(4) } else { base }, base.wrapping_add(span))
        } else {
            (
                if pre { base.wrapping_sub(span) } else { base.wrapping_sub(span).wrapping_add(4) },
                base.wrapping_sub(span),
            )
        };
        addr &= !3;

        let user_bank = psr_or_user && !(load && list & (1 << 15) != 0);
        let mut seq = false;

        if load {
            // Writeback first: a base register that is also loaded ends up with
            // the loaded value, not the written-back one.
            if writeback {
                self.regs[rn] = final_base;
            }
            for r in 0..16 {
                if list & (1 << r) == 0 {
                    continue;
                }
                let value = bus.read32(addr, seq);
                addr = addr.wrapping_add(4);
                seq = true;
                if r == 15 {
                    if psr_or_user {
                        let spsr = self.spsr;
                        self.set_cpsr(spsr);
                        if self.cpsr & FLAG_T != 0 {
                            self.refill_thumb(bus, value);
                        } else {
                            self.refill_arm(bus, value);
                        }
                    } else {
                        self.refill_arm(bus, value);
                    }
                } else if user_bank {
                    self.set_user_reg(r, value);
                } else {
                    self.regs[r] = value;
                }
            }
            bus.idle(1);
        } else {
            let mut first = true;
            for r in 0..16 {
                if list & (1 << r) == 0 {
                    continue;
                }
                let value = if r == 15 {
                    self.regs[15].wrapping_add(4)
                } else if user_bank {
                    self.user_reg(r)
                } else {
                    self.regs[r]
                };
                bus.write32(addr, value, seq);
                addr = addr.wrapping_add(4);
                seq = true;
                // The base is written back after the first store, so a base that
                // comes later in the list stores its new value.
                if first && writeback {
                    self.regs[rn] = final_base;
                    first = false;
                }
            }
        }
        self.next_seq = false;
    }
}
