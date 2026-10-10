//! The SM83 CPU (Pan Docs "CPU Instruction Set"). Every memory access takes
//! one M-cycle through the bus, which advances the rest of the machine; the
//! few instructions with internal cycles ask the bus for those explicitly, so
//! timing falls out of the access pattern rather than a table.

use serde::{Deserialize, Serialize};

use crate::memory::Bus;

const Z: u8 = 0x80;
const N: u8 = 0x40;
const H: u8 = 0x20;
const C: u8 = 0x10;

#[derive(Serialize, Deserialize)]
pub struct Cpu {
    pub a: u8,
    pub f: u8,
    pub b: u8,
    pub c: u8,
    pub d: u8,
    pub e: u8,
    pub h: u8,
    pub l: u8,
    pub sp: u16,
    pub pc: u16,
    pub ime: bool,
    /// EI enables interrupts after the instruction that follows it.
    ei_delay: u8,
    pub halted: bool,
    /// HALT with interrupts disabled but pending: the next byte is read twice.
    halt_bug: bool,
    pub instructions: u64,
}

impl Cpu {
    /// Registers as the boot ROM leaves them, for a colour or a classic game.
    pub fn new(cgb: bool) -> Self {
        let (a, b, c, d, e, h, l) = if cgb {
            (0x11, 0, 0, 0xFF, 0x56, 0, 0x0D)
        } else {
            (0x01, 0, 0x13, 0, 0xD8, 0x01, 0x4D)
        };
        Self {
            a,
            f: if cgb { 0x80 } else { 0xB0 },
            b,
            c,
            d,
            e,
            h,
            l,
            sp: 0xFFFE,
            pc: 0x100,
            ime: false,
            ei_delay: 0,
            halted: false,
            halt_bug: false,
            instructions: 0,
        }
    }

    fn flag(&self, mask: u8) -> bool {
        self.f & mask != 0
    }

    fn set_flags(&mut self, z: bool, n: bool, h: bool, c: bool) {
        self.f = (u8::from(z) << 7) | (u8::from(n) << 6) | (u8::from(h) << 5) | (u8::from(c) << 4);
    }

    pub fn af(&self) -> u16 {
        u16::from_be_bytes([self.a, self.f])
    }
    pub fn bc(&self) -> u16 {
        u16::from_be_bytes([self.b, self.c])
    }
    pub fn de(&self) -> u16 {
        u16::from_be_bytes([self.d, self.e])
    }
    pub fn hl(&self) -> u16 {
        u16::from_be_bytes([self.h, self.l])
    }

    fn set_hl(&mut self, v: u16) {
        [self.h, self.l] = v.to_be_bytes();
    }

    fn fetch(&mut self, bus: &mut Bus) -> u8 {
        let v = bus.read(self.pc);
        if self.halt_bug {
            self.halt_bug = false;
        } else {
            self.pc = self.pc.wrapping_add(1);
        }
        v
    }

    fn fetch16(&mut self, bus: &mut Bus) -> u16 {
        let lo = self.fetch(bus);
        let hi = self.fetch(bus);
        u16::from_le_bytes([lo, hi])
    }

    /// One instruction, or one M-cycle of halting; interrupts are taken first.
    pub fn step(&mut self, bus: &mut Bus) {
        if self.halted {
            bus.tick();
            if bus.interrupts_pending() == 0 {
                return;
            }
            self.halted = false;
        }
        if self.ime && bus.interrupts_pending() != 0 {
            self.dispatch(bus);
            return;
        }
        if self.ei_delay > 0 {
            self.ei_delay -= 1;
            if self.ei_delay == 0 {
                self.ime = true;
            }
        }
        self.instructions += 1;
        let opcode = self.fetch(bus);
        self.execute(opcode, bus);
    }

    /// Five M-cycles: two idle, the push, and the jump to the vector.
    fn dispatch(&mut self, bus: &mut Bus) {
        self.ime = false;
        self.ei_delay = 0;
        bus.tick();
        bus.tick();
        let [hi, lo] = self.pc.to_be_bytes();
        self.sp = self.sp.wrapping_sub(1);
        bus.write(self.sp, hi);
        // What is pending can change while the high byte is pushed (IE at 0xFFFF).
        let pending = bus.interrupts_pending();
        self.sp = self.sp.wrapping_sub(1);
        bus.write(self.sp, lo);
        bus.tick();
        if pending == 0 {
            self.pc = 0;
            return;
        }
        let bit = pending.trailing_zeros();
        bus.if_ &= !(1 << bit);
        self.pc = 0x40 + 8 * bit as u16;
    }

    // -- operands -----------------------------------------------------------

    /// r: B C D E H L (HL) A.
    fn r8(&mut self, i: u8, bus: &mut Bus) -> u8 {
        match i {
            0 => self.b,
            1 => self.c,
            2 => self.d,
            3 => self.e,
            4 => self.h,
            5 => self.l,
            6 => bus.read(self.hl()),
            _ => self.a,
        }
    }

    fn set_r8(&mut self, i: u8, v: u8, bus: &mut Bus) {
        match i {
            0 => self.b = v,
            1 => self.c = v,
            2 => self.d = v,
            3 => self.e = v,
            4 => self.h = v,
            5 => self.l = v,
            6 => bus.write(self.hl(), v),
            _ => self.a = v,
        }
    }

    /// rp: BC DE HL SP.
    fn rp(&self, i: u8) -> u16 {
        match i {
            0 => self.bc(),
            1 => self.de(),
            2 => self.hl(),
            _ => self.sp,
        }
    }

    fn set_rp(&mut self, i: u8, v: u16) {
        let [hi, lo] = v.to_be_bytes();
        match i {
            0 => [self.b, self.c] = [hi, lo],
            1 => [self.d, self.e] = [hi, lo],
            2 => [self.h, self.l] = [hi, lo],
            _ => self.sp = v,
        }
    }

    fn condition(&self, cc: u8) -> bool {
        match cc {
            0 => !self.flag(Z),
            1 => self.flag(Z),
            2 => !self.flag(C),
            _ => self.flag(C),
        }
    }

    // -- arithmetic -----------------------------------------------------------

    fn alu(&mut self, op: u8, v: u8) {
        let a = self.a;
        let carry = u8::from(self.flag(C));
        match op {
            0 | 1 => {
                let c_in = if op == 1 { carry } else { 0 };
                let r = u16::from(a) + u16::from(v) + u16::from(c_in);
                self.set_flags(r as u8 == 0, false, (a & 0xF) + (v & 0xF) + c_in > 0xF, r > 0xFF);
                self.a = r as u8;
            }
            2 | 3 | 7 => {
                let c_in = if op == 3 { carry } else { 0 };
                let r = i32::from(a) - i32::from(v) - i32::from(c_in);
                self.set_flags(r as u8 == 0, true, (a & 0xF) < (v & 0xF) + c_in, r < 0);
                if op != 7 {
                    self.a = r as u8;
                }
            }
            4 => {
                self.a &= v;
                self.set_flags(self.a == 0, false, true, false);
            }
            5 => {
                self.a ^= v;
                self.set_flags(self.a == 0, false, false, false);
            }
            _ => {
                self.a |= v;
                self.set_flags(self.a == 0, false, false, false);
            }
        }
    }

    fn inc8(&mut self, v: u8) -> u8 {
        let r = v.wrapping_add(1);
        self.f = (self.f & C) | (u8::from(r == 0) << 7) | (u8::from(v & 0xF == 0xF) << 5);
        r
    }

    fn dec8(&mut self, v: u8) -> u8 {
        let r = v.wrapping_sub(1);
        self.f = (self.f & C) | (u8::from(r == 0) << 7) | N | (u8::from(v & 0xF == 0) << 5);
        r
    }

    fn add_hl(&mut self, v: u16) {
        let hl = self.hl();
        let r = u32::from(hl) + u32::from(v);
        self.f = (self.f & Z)
            | (u8::from((hl & 0xFFF) + (v & 0xFFF) > 0xFFF) << 5)
            | (u8::from(r > 0xFFFF) << 4);
        self.set_hl(r as u16);
    }

    /// SP + signed byte, as ADD SP,e and LD HL,SP+e compute it (flags from the low byte).
    fn sp_offset(&mut self, e: i8) -> u16 {
        let sp = self.sp;
        let v = e as u16;
        self.set_flags(false, false, (sp & 0xF) + (v & 0xF) > 0xF, (sp & 0xFF) + (v & 0xFF) > 0xFF);
        sp.wrapping_add(v)
    }

    /// Rotates and shifts (the CB prefix's first group); `set_z` is false for the
    /// A-register forms, whose Z is always clear.
    fn rotate(&mut self, op: u8, v: u8, set_z: bool) -> u8 {
        let carry = u8::from(self.flag(C));
        let (r, c) = match op {
            0 => (v.rotate_left(1), v >> 7),
            1 => (v.rotate_right(1), v & 1),
            2 => ((v << 1) | carry, v >> 7),
            3 => ((v >> 1) | (carry << 7), v & 1),
            4 => (v << 1, v >> 7),
            5 => ((v >> 1) | (v & 0x80), v & 1),
            6 => (v.rotate_left(4), 0),
            _ => (v >> 1, v & 1),
        };
        self.set_flags(set_z && r == 0, false, false, c != 0);
        r
    }

    fn daa(&mut self) {
        let mut a = self.a;
        let mut carry = self.flag(C);
        if !self.flag(N) {
            if self.flag(H) || a & 0xF > 9 {
                a = a.wrapping_add(6);
            }
            if carry || self.a > 0x99 {
                a = a.wrapping_add(0x60);
                carry = true;
            }
        } else {
            if self.flag(H) {
                a = a.wrapping_sub(6);
            }
            if carry {
                a = a.wrapping_sub(0x60);
            }
        }
        self.a = a;
        self.f = (self.f & N) | (u8::from(a == 0) << 7) | (u8::from(carry) << 4);
    }

    fn push(&mut self, v: u16, bus: &mut Bus) {
        let [hi, lo] = v.to_be_bytes();
        self.sp = self.sp.wrapping_sub(1);
        bus.write(self.sp, hi);
        self.sp = self.sp.wrapping_sub(1);
        bus.write(self.sp, lo);
    }

    fn pop(&mut self, bus: &mut Bus) -> u16 {
        let lo = bus.read(self.sp);
        self.sp = self.sp.wrapping_add(1);
        let hi = bus.read(self.sp);
        self.sp = self.sp.wrapping_add(1);
        u16::from_le_bytes([lo, hi])
    }

    fn call(&mut self, target: u16, bus: &mut Bus) {
        bus.tick();
        self.push(self.pc, bus);
        self.pc = target;
    }

    // -- execution ------------------------------------------------------------

    fn execute(&mut self, op: u8, bus: &mut Bus) {
        let (x, y, z) = (op >> 6, (op >> 3) & 7, op & 7);
        let (p, q) = (y >> 1, y & 1);
        match x {
            0 => match z {
                0 => match y {
                    0 => {}
                    1 => {
                        let addr = self.fetch16(bus);
                        let [hi, lo] = self.sp.to_be_bytes();
                        bus.write(addr, lo);
                        bus.write(addr.wrapping_add(1), hi);
                    }
                    2 => {
                        self.fetch(bus);
                        if !bus.switch_speed() {
                            self.halted = true;
                        }
                    }
                    _ => {
                        let e = self.fetch(bus) as i8;
                        if y == 3 || self.condition(y - 4) {
                            bus.tick();
                            self.pc = self.pc.wrapping_add(e as u16);
                        }
                    }
                },
                1 if q == 0 => {
                    let v = self.fetch16(bus);
                    self.set_rp(p, v);
                }
                1 => {
                    bus.tick();
                    self.add_hl(self.rp(p));
                }
                2 => {
                    let addr = match p {
                        0 => self.bc(),
                        1 => self.de(),
                        _ => self.hl(),
                    };
                    if q == 0 {
                        bus.write(addr, self.a);
                    } else {
                        self.a = bus.read(addr);
                    }
                    match p {
                        2 => self.set_hl(addr.wrapping_add(1)),
                        3 => self.set_hl(addr.wrapping_sub(1)),
                        _ => {}
                    }
                }
                3 => {
                    bus.tick();
                    let v = self.rp(p);
                    self.set_rp(p, if q == 0 { v.wrapping_add(1) } else { v.wrapping_sub(1) });
                }
                4 | 5 => {
                    let v = self.r8(y, bus);
                    let r = if z == 4 { self.inc8(v) } else { self.dec8(v) };
                    self.set_r8(y, r, bus);
                }
                6 => {
                    let v = self.fetch(bus);
                    self.set_r8(y, v, bus);
                }
                _ => match y {
                    0..=3 => self.a = self.rotate(y, self.a, false),
                    4 => self.daa(),
                    5 => {
                        self.a = !self.a;
                        self.f |= N | H;
                    }
                    6 => self.f = (self.f & Z) | C,
                    _ => self.f = (self.f & Z) | ((self.f & C) ^ C),
                },
            },
            1 if op == 0x76 => {
                if !self.ime && bus.interrupts_pending() != 0 {
                    self.halt_bug = true;
                } else {
                    self.halted = true;
                }
            }
            1 => {
                let v = self.r8(z, bus);
                self.set_r8(y, v, bus);
            }
            2 => {
                let v = self.r8(z, bus);
                self.alu(y, v);
            }
            _ => self.execute_x3(op, bus),
        }
    }

    /// Opcodes 0xC0-0xFF: jumps, calls, returns, the stack, the prefix.
    fn execute_x3(&mut self, op: u8, bus: &mut Bus) {
        let (y, z) = ((op >> 3) & 7, op & 7);
        let (p, q) = (y >> 1, y & 1);
        match z {
            0 => match y {
                0..=3 => {
                    bus.tick();
                    if self.condition(y) {
                        self.pc = self.pop(bus);
                        bus.tick();
                    }
                }
                4 => {
                    let n = self.fetch(bus);
                    bus.write(0xFF00 | u16::from(n), self.a);
                }
                5 => {
                    let e = self.fetch(bus) as i8;
                    bus.tick();
                    bus.tick();
                    self.sp = self.sp_offset(e);
                }
                6 => {
                    let n = self.fetch(bus);
                    self.a = bus.read(0xFF00 | u16::from(n));
                }
                _ => {
                    let e = self.fetch(bus) as i8;
                    bus.tick();
                    let v = self.sp_offset(e);
                    self.set_hl(v);
                }
            },
            1 if q == 0 => {
                let v = self.pop(bus);
                if p == 3 {
                    [self.a, self.f] = v.to_be_bytes();
                    self.f &= 0xF0;
                } else {
                    self.set_rp(p, v);
                }
            }
            1 => match p {
                0 | 1 => {
                    self.pc = self.pop(bus);
                    bus.tick();
                    if p == 1 {
                        self.ime = true;
                    }
                }
                2 => self.pc = self.hl(),
                _ => {
                    bus.tick();
                    self.sp = self.hl();
                }
            },
            2 => match y {
                0..=3 => {
                    let addr = self.fetch16(bus);
                    if self.condition(y) {
                        bus.tick();
                        self.pc = addr;
                    }
                }
                4 => bus.write(0xFF00 | u16::from(self.c), self.a),
                5 => {
                    let addr = self.fetch16(bus);
                    bus.write(addr, self.a);
                }
                6 => self.a = bus.read(0xFF00 | u16::from(self.c)),
                _ => {
                    let addr = self.fetch16(bus);
                    self.a = bus.read(addr);
                }
            },
            3 => match y {
                0 => {
                    let addr = self.fetch16(bus);
                    bus.tick();
                    self.pc = addr;
                }
                1 => self.prefixed(bus),
                6 => {
                    self.ime = false;
                    self.ei_delay = 0;
                }
                7 => self.ei_delay = 2,
                _ => {}
            },
            4 => {
                let addr = self.fetch16(bus);
                if y < 4 && self.condition(y) {
                    self.call(addr, bus);
                }
            }
            5 if q == 0 => {
                bus.tick();
                let v = if p == 3 { self.af() } else { self.rp(p) };
                self.push(v, bus);
            }
            5 => {
                let addr = self.fetch16(bus);
                self.call(addr, bus);
            }
            6 => {
                let v = self.fetch(bus);
                self.alu(y, v);
            }
            _ => self.call(u16::from(y) * 8, bus),
        }
    }

    fn prefixed(&mut self, bus: &mut Bus) {
        let op = self.fetch(bus);
        let (x, y, z) = (op >> 6, (op >> 3) & 7, op & 7);
        let v = self.r8(z, bus);
        match x {
            0 => {
                let r = self.rotate(y, v, true);
                self.set_r8(z, r, bus);
            }
            1 => self.f = (self.f & C) | H | (u8::from(v & (1 << y) == 0) << 7),
            2 => self.set_r8(z, v & !(1 << y), bus),
            _ => self.set_r8(z, v | (1 << y), bus),
        }
    }
}
