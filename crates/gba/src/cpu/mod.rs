//! ARM7TDMI core: registers, mode banking, pipeline and exceptions.
//!
//! The two instruction sets are in `arm.rs` and `thumb.rs`. Both see the same
//! model: `regs[15]` is the address of the executing instruction plus 8 (ARM) or
//! plus 4 (Thumb), exactly as software observes it, and `pipeline` holds the two
//! opcodes already fetched behind it.

mod arm;
mod thumb;

use crate::memory::Bus;

pub const FLAG_N: u32 = 1 << 31;
pub const FLAG_Z: u32 = 1 << 30;
pub const FLAG_C: u32 = 1 << 29;
pub const FLAG_V: u32 = 1 << 28;
pub const FLAG_I: u32 = 1 << 7;
pub const FLAG_F: u32 = 1 << 6;
pub const FLAG_T: u32 = 1 << 5;
const MODE_MASK: u32 = 0x1F;

/// Processor modes (CPSR bits 0-4).
pub mod mode {
    pub const USER: u32 = 0x10;
    pub const FIQ: u32 = 0x11;
    pub const IRQ: u32 = 0x12;
    pub const SVC: u32 = 0x13;
    pub const ABT: u32 = 0x17;
    pub const UND: u32 = 0x1B;
    pub const SYS: u32 = 0x1F;
}

/// Exception vectors.
pub mod vector {
    pub const RESET: u32 = 0x00;
    pub const UNDEFINED: u32 = 0x04;
    pub const SWI: u32 = 0x08;
    pub const IRQ: u32 = 0x18;
}

const BANK_USR: usize = 0;
const BANK_FIQ: usize = 1;

fn bank_of(mode: u32) -> usize {
    match mode {
        mode::FIQ => BANK_FIQ,
        mode::SVC => 2,
        mode::ABT => 3,
        mode::IRQ => 4,
        mode::UND => 5,
        _ => BANK_USR,
    }
}

pub struct Cpu {
    pub regs: [u32; 16],
    pub cpsr: u32,
    /// SPSR of the current mode (meaningless in User/System).
    pub spsr: u32,
    /// r8-r14 for each bank. Only FIQ uses all seven; the others keep r13-r14 in
    /// slots 5-6 and share r8-r12 with the user bank.
    banks: [[u32; 7]; 6],
    spsr_banks: [u32; 6],
    /// Opcodes in the decode and fetch stages.
    pipeline: [u32; 2],
    /// Whether the next opcode fetch is sequential to the previous bus access.
    next_seq: bool,
    /// Set by any branch so `step` does not also advance the PC.
    flushed: bool,
    /// An HLE IntrWait is in progress: the SWI re-executes after each interrupt.
    pub(crate) intr_wait_continuing: bool,
}

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}

impl Cpu {
    pub fn new() -> Self {
        Self {
            regs: [0; 16],
            cpsr: mode::SVC | FLAG_I | FLAG_F,
            spsr: 0,
            banks: [[0; 7]; 6],
            spsr_banks: [0; 6],
            pipeline: [0; 2],
            next_seq: false,
            flushed: false,
            intr_wait_continuing: false,
        }
    }

    /// Puts the CPU in the state the BIOS leaves it in before jumping to the
    /// cartridge, or at the real reset vector if a BIOS image is loaded.
    pub fn reset(&mut self, bus: &mut Bus) {
        self.regs = [0; 16];
        self.banks = [[0; 7]; 6];
        self.spsr_banks = [0; 6];
        self.intr_wait_continuing = false;
        // Start in Supervisor mode with its stack, then let `switch_mode` bank it
        // away when entering System mode for the cartridge.
        self.cpsr = mode::SVC | FLAG_I | FLAG_F;
        self.regs[13] = 0x0300_7FE0;
        self.banks[bank_of(mode::IRQ)][5] = 0x0300_7FA0;
        self.banks[BANK_USR][5] = 0x0300_7F00;
        if bus.bios.hle {
            self.switch_mode(mode::SYS);
            self.cpsr = mode::SYS;
            self.refill_arm(bus, 0x0800_0000);
        } else {
            self.refill_arm(bus, vector::RESET);
        }
    }

    // ---------------------------------------------------------------------------
    // State helpers
    // ---------------------------------------------------------------------------

    #[inline(always)]
    pub fn is_thumb(&self) -> bool {
        self.cpsr & FLAG_T != 0
    }

    /// The opcode that the next `step` will execute (for tracing and debuggers).
    pub fn next_opcode(&self) -> u32 {
        self.pipeline[0]
    }

    /// Address of the instruction currently executing.
    #[inline(always)]
    pub fn pc(&self) -> u32 {
        if self.is_thumb() {
            self.regs[15].wrapping_sub(4)
        } else {
            self.regs[15].wrapping_sub(8)
        }
    }

    #[inline(always)]
    fn flag(&self, f: u32) -> bool {
        self.cpsr & f != 0
    }

    #[inline(always)]
    fn set_flag(&mut self, f: u32, on: bool) {
        if on {
            self.cpsr |= f;
        } else {
            self.cpsr &= !f;
        }
    }

    #[inline(always)]
    fn set_nz(&mut self, result: u32) {
        self.cpsr = (self.cpsr & !(FLAG_N | FLAG_Z))
            | (result & FLAG_N)
            | if result == 0 { FLAG_Z } else { 0 };
    }

    #[inline(always)]
    fn set_nzc(&mut self, result: u32, carry: bool) {
        self.set_nz(result);
        self.set_flag(FLAG_C, carry);
    }

    /// `a + b + carry_in` with N, Z, C, V updated.
    #[inline(always)]
    fn add_flags(&mut self, a: u32, b: u32, carry_in: bool) -> u32 {
        let (r1, c1) = a.overflowing_add(b);
        let (r, c2) = r1.overflowing_add(u32::from(carry_in));
        let v = ((a ^ r) & (b ^ r)) & FLAG_N != 0;
        self.set_nz(r);
        self.set_flag(FLAG_C, c1 | c2);
        self.set_flag(FLAG_V, v);
        r
    }

    /// `a - b - !carry_in` (plain SUB when `carry_in` is true) with flags updated.
    /// C is set when no borrow occurred.
    #[inline(always)]
    fn sub_flags(&mut self, a: u32, b: u32, carry_in: bool) -> u32 {
        let (r1, b1) = a.overflowing_sub(b);
        let (r, b2) = r1.overflowing_sub(u32::from(!carry_in));
        let v = ((a ^ b) & (a ^ r)) & FLAG_N != 0;
        self.set_nz(r);
        self.set_flag(FLAG_C, !(b1 | b2));
        self.set_flag(FLAG_V, v);
        r
    }

    /// Switches mode, swapping banked registers. Does not touch other CPSR bits.
    pub fn switch_mode(&mut self, new_mode: u32) {
        let old_bank = bank_of(self.cpsr & MODE_MASK);
        let new_bank = bank_of(new_mode);
        self.cpsr = (self.cpsr & !MODE_MASK) | new_mode;
        if old_bank == new_bank {
            return;
        }
        if old_bank == BANK_FIQ {
            self.banks[BANK_FIQ].copy_from_slice(&self.regs[8..15]);
        } else {
            self.banks[BANK_USR][..5].copy_from_slice(&self.regs[8..13]);
            self.banks[old_bank][5..].copy_from_slice(&self.regs[13..15]);
        }
        self.spsr_banks[old_bank] = self.spsr;
        if new_bank == BANK_FIQ {
            self.regs[8..15].copy_from_slice(&self.banks[BANK_FIQ]);
        } else {
            self.regs[8..13].copy_from_slice(&self.banks[BANK_USR][..5]);
            self.regs[13..15].copy_from_slice(&self.banks[new_bank][5..]);
        }
        self.spsr = self.spsr_banks[new_bank];
    }

    /// Full CPSR write (MSR, SPSR restore): switches mode if needed.
    pub fn set_cpsr(&mut self, value: u32) {
        if value & MODE_MASK != self.cpsr & MODE_MASK {
            self.switch_mode(value & MODE_MASK);
        }
        self.cpsr = value;
    }

    /// Reads a register from the user bank regardless of the current mode
    /// (LDM/STM with the S bit).
    fn user_reg(&self, r: usize) -> u32 {
        let bank = bank_of(self.cpsr & MODE_MASK);
        match r {
            8..=12 if bank == BANK_FIQ => self.banks[BANK_USR][r - 8],
            13 | 14 if bank != BANK_USR => self.banks[BANK_USR][r - 8],
            _ => self.regs[r],
        }
    }

    fn set_user_reg(&mut self, r: usize, value: u32) {
        let bank = bank_of(self.cpsr & MODE_MASK);
        match r {
            8..=12 if bank == BANK_FIQ => self.banks[BANK_USR][r - 8] = value,
            13 | 14 if bank != BANK_USR => self.banks[BANK_USR][r - 8] = value,
            _ => self.regs[r] = value,
        }
    }

    // ---------------------------------------------------------------------------
    // Pipeline
    // ---------------------------------------------------------------------------

    /// Restarts execution at `addr` in ARM state: refetches the pipeline (N + S)
    /// and leaves `regs[15]` at `addr + 8`.
    pub fn refill_arm(&mut self, bus: &mut Bus, addr: u32) {
        let addr = addr & !3;
        self.pipeline[0] = bus.fetch32(addr, false);
        self.pipeline[1] = bus.fetch32(addr.wrapping_add(4), true);
        self.regs[15] = addr.wrapping_add(8);
        self.next_seq = true;
        self.flushed = true;
    }

    /// Restarts execution at `addr` in Thumb state; `regs[15]` becomes `addr + 4`.
    pub fn refill_thumb(&mut self, bus: &mut Bus, addr: u32) {
        let addr = addr & !1;
        self.pipeline[0] = u32::from(bus.fetch16(addr, false));
        self.pipeline[1] = u32::from(bus.fetch16(addr.wrapping_add(2), true));
        self.regs[15] = addr.wrapping_add(4);
        self.next_seq = true;
        self.flushed = true;
    }

    /// Branches within the current instruction set.
    #[inline]
    pub fn branch(&mut self, bus: &mut Bus, addr: u32) {
        if self.is_thumb() {
            self.refill_thumb(bus, addr);
        } else {
            self.refill_arm(bus, addr);
        }
    }

    /// BX semantics: bit 0 of the address selects Thumb.
    #[inline]
    pub fn branch_exchange(&mut self, bus: &mut Bus, addr: u32) {
        if addr & 1 != 0 {
            self.cpsr |= FLAG_T;
            self.refill_thumb(bus, addr);
        } else {
            self.cpsr &= !FLAG_T;
            self.refill_arm(bus, addr);
        }
    }

    /// Enters an exception: banks SPSR/LR, switches mode, disables IRQs, jumps to
    /// the vector in ARM state.
    pub fn exception(&mut self, bus: &mut Bus, vec: u32, new_mode: u32, lr: u32) {
        let old_cpsr = self.cpsr;
        self.switch_mode(new_mode);
        self.spsr = old_cpsr;
        self.regs[14] = lr;
        self.cpsr = (self.cpsr & !FLAG_T) | FLAG_I;
        self.refill_arm(bus, vec);
    }

    fn take_irq(&mut self, bus: &mut Bus) {
        // LR must point one instruction past the one about to execute, so that the
        // handler's `subs pc, lr, #4` returns to it.
        let lr = if self.is_thumb() { self.regs[15] } else { self.regs[15].wrapping_sub(4) };
        self.exception(bus, vector::IRQ, mode::IRQ, lr);
    }

    /// Executes one instruction, or sleeps until the next event when halted.
    pub fn step(&mut self, bus: &mut Bus) {
        if bus.irq.halted || bus.irq.stopped {
            if bus.irq.should_wake() {
                bus.irq.halted = false;
                bus.irq.stopped = false;
            } else {
                bus.scheduler.skip_to_next_event();
                bus.run_events();
                return;
            }
        }
        if bus.irq.should_interrupt() && !self.flag(FLAG_I) {
            self.take_irq(bus);
        }

        let seq = self.next_seq;
        self.next_seq = true;
        self.flushed = false;
        let opcode = self.pipeline[0];
        self.pipeline[0] = self.pipeline[1];
        if self.is_thumb() {
            bus.pc = self.regs[15].wrapping_sub(4);
            self.pipeline[1] = u32::from(bus.fetch16(self.regs[15], seq));
            self.execute_thumb(bus, opcode as u16);
            if !self.flushed {
                self.regs[15] = self.regs[15].wrapping_add(2);
            }
        } else {
            bus.pc = self.regs[15].wrapping_sub(8);
            self.pipeline[1] = bus.fetch32(self.regs[15], seq);
            self.execute_arm(bus, opcode);
            if !self.flushed {
                self.regs[15] = self.regs[15].wrapping_add(4);
            }
        }

        if bus.scheduler.is_due() {
            bus.run_events();
        }
    }

    // ---------------------------------------------------------------------------
    // Shared instruction helpers
    // ---------------------------------------------------------------------------

    /// Evaluates a condition code (bits 31-28 of an ARM opcode, or a Thumb branch).
    #[inline]
    pub fn condition(&self, cond: u32) -> bool {
        let n = self.flag(FLAG_N);
        let z = self.flag(FLAG_Z);
        let c = self.flag(FLAG_C);
        let v = self.flag(FLAG_V);
        match cond {
            0x0 => z,
            0x1 => !z,
            0x2 => c,
            0x3 => !c,
            0x4 => n,
            0x5 => !n,
            0x6 => v,
            0x7 => !v,
            0x8 => c && !z,
            0x9 => !c || z,
            0xA => n == v,
            0xB => n != v,
            0xC => !z && n == v,
            0xD => z || n != v,
            0xE => true,
            _ => false,
        }
    }

    /// Word load with the ARM7's rotation of misaligned addresses.
    #[inline]
    fn load_word(&mut self, bus: &mut Bus, addr: u32, seq: bool) -> u32 {
        let value = bus.read32(addr, seq);
        value.rotate_right((addr & 3) * 8)
    }

    /// Halfword load: misaligned addresses rotate the halfword by 8 bits.
    #[inline]
    fn load_half(&mut self, bus: &mut Bus, addr: u32, seq: bool) -> u32 {
        let value = u32::from(bus.read16(addr, seq));
        value.rotate_right((addr & 1) * 8)
    }

    /// Signed halfword load: a misaligned address loads a signed byte instead.
    #[inline]
    fn load_signed_half(&mut self, bus: &mut Bus, addr: u32, seq: bool) -> u32 {
        if addr & 1 != 0 {
            bus.read8(addr, seq) as i8 as i32 as u32
        } else {
            bus.read16(addr, seq) as i16 as i32 as u32
        }
    }

    /// Internal cycles a multiply needs for the given multiplier operand.
    #[inline]
    fn multiply_cycles(rs: u32, signed: bool) -> u32 {
        let top = rs & 0xFFFF_FF00;
        if top == 0 || (signed && top == 0xFFFF_FF00) {
            1
        } else if rs & 0xFFFF_0000 == 0 || (signed && rs & 0xFFFF_0000 == 0xFFFF_0000) {
            2
        } else if rs & 0xFF00_0000 == 0 || (signed && rs & 0xFF00_0000 == 0xFF00_0000) {
            3
        } else {
            4
        }
    }
}

/// Barrel shifter. `imm` says the amount came from an immediate field, which gives
/// `0` the special meanings (LSR/ASR #32, RRX) that register amounts do not have.
/// Returns the value and the carry out.
#[inline]
pub fn barrel_shift(kind: u32, value: u32, amount: u32, imm: bool, carry: bool) -> (u32, bool) {
    match kind {
        // LSL
        0 => match amount {
            0 => (value, carry),
            1..=31 => (value << amount, (value >> (32 - amount)) & 1 != 0),
            32 => (0, value & 1 != 0),
            _ => (0, false),
        },
        // LSR
        1 => {
            let amount = if imm && amount == 0 { 32 } else { amount };
            match amount {
                0 => (value, carry),
                1..=31 => (value >> amount, (value >> (amount - 1)) & 1 != 0),
                32 => (0, value >> 31 != 0),
                _ => (0, false),
            }
        }
        // ASR
        2 => {
            let amount = if imm && amount == 0 { 32 } else { amount };
            match amount {
                0 => (value, carry),
                1..=31 => (((value as i32) >> amount) as u32, (value >> (amount - 1)) & 1 != 0),
                _ => (((value as i32) >> 31) as u32, value >> 31 != 0),
            }
        }
        // ROR
        _ => {
            if imm && amount == 0 {
                // RRX: rotate right through carry by one.
                ((u32::from(carry) << 31) | (value >> 1), value & 1 != 0)
            } else if amount == 0 {
                (value, carry)
            } else {
                let amount = amount & 31;
                if amount == 0 {
                    (value, value >> 31 != 0)
                } else {
                    (value.rotate_right(amount), (value >> (amount - 1)) & 1 != 0)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shifter_special_cases() {
        assert_eq!(barrel_shift(1, 0x8000_0000, 0, true, false), (0, true)); // LSR #32
        assert_eq!(barrel_shift(2, 0x8000_0000, 0, true, false), (0xFFFF_FFFF, true)); // ASR #32
        assert_eq!(barrel_shift(3, 0x3, 0, true, true), (0x8000_0001, true)); // RRX
        assert_eq!(barrel_shift(0, 0x1, 32, false, false), (0, true)); // LSL #32 by register
        assert_eq!(barrel_shift(0, 0x1, 33, false, false), (0, false));
        assert_eq!(barrel_shift(3, 0x8000_0001, 32, false, false), (0x8000_0001, true));
    }

    #[test]
    fn mode_switch_banks_sp_and_lr() {
        let mut cpu = Cpu::new();
        cpu.cpsr = mode::SYS;
        cpu.regs[13] = 0x1000;
        cpu.regs[14] = 0x2000;
        cpu.switch_mode(mode::IRQ);
        assert_eq!(cpu.regs[13], 0);
        cpu.regs[13] = 0x3000;
        cpu.switch_mode(mode::FIQ);
        cpu.regs[8] = 0xF8;
        cpu.switch_mode(mode::SYS);
        assert_eq!((cpu.regs[13], cpu.regs[14], cpu.regs[8]), (0x1000, 0x2000, 0));
        cpu.switch_mode(mode::IRQ);
        assert_eq!(cpu.regs[13], 0x3000);
        assert_eq!(cpu.user_reg(13), 0x1000);
    }
}
