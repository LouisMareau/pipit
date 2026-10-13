//! The SWIs of both BIOSes, in Rust. Each CPU's view handles the call when it
//! has no user BIOS; the generic routines (CpuSet, the decompressors…) come
//! from `pipit_arm::swi`.

use crate::memory::{View, ARM7, ARM9};
use pipit_arm::swi as generic;
use pipit_arm::Cpu;

pub fn call<V: View>(cpu: &mut Cpu, bus: &mut V, number: u8) {
    let me = V::CPU;
    match number {
        // WaitByLoop: r0 iterations of a 4-cycle loop.
        0x03 => {
            let loops = cpu.regs[0];
            bus.idle(loops.saturating_mul(4));
            cpu.regs[0] = 0;
        }
        0x04 => intr_wait(cpu, bus, cpu.regs[0] != 0, cpu.regs[1]),
        0x05 => intr_wait(cpu, bus, true, 1),
        0x06 | 0x07 => bus.bus().irq[me].halted = true,
        0x08 => {}
        0x09 => generic::div(cpu, bus, cpu.regs[0] as i32, cpu.regs[1] as i32),
        0x0B => generic::cpu_set(cpu, bus),
        0x0C => generic::cpu_fast_set(cpu, bus),
        0x0D => cpu.regs[0] = generic::isqrt(cpu.regs[0]),
        0x0E => crc16(cpu, bus),
        0x0F => cpu.regs[0] = 0,
        0x10 => generic::bit_unpack(cpu, bus),
        0x11 => generic::lz77_uncomp(cpu, bus, false),
        0x12 => generic::lz77_uncomp(cpu, bus, true),
        0x13 => generic::huff_uncomp(cpu, bus),
        0x14 => generic::rl_uncomp(cpu, bus, false),
        0x15 => generic::rl_uncomp(cpu, bus, true),
        0x16 if me == ARM9 => generic::diff_8bit_unfilter(cpu, bus, false),
        0x18 if me == ARM9 => generic::diff_16bit_unfilter(cpu, bus),
        0x1A if me == ARM7 => cpu.regs[0] = sine_table(cpu.regs[0]),
        0x1B if me == ARM7 => cpu.regs[0] = pitch_table(cpu.regs[0]),
        0x1C if me == ARM7 => cpu.regs[0] = volume_table(cpu.regs[0]),
        _ => {}
    }
}

/// Where the game's interrupt handler records what it serviced, for IntrWait:
/// the last words of DTCM on the ARM9, of the ARM7's work RAM on the ARM7.
fn flags_address<V: View>(bus: &mut V) -> u32 {
    if V::CPU == ARM9 {
        (bus.bus().cp15.dtcm & 0xFFFF_F000) + 0x3FF8
    } else {
        0x0380_FFF8
    }
}

/// Waits for one of the interrupts in `flags`. If it cannot finish now, the
/// CPU halts *at the SWI*, so returning from the interrupt re-executes it
/// and checks again (as `intr_wait_continuing` records).
fn intr_wait<V: View>(cpu: &mut Cpu, bus: &mut V, discard_old: bool, flags: u32) {
    let me = V::CPU;
    let at = flags_address(bus);
    bus.bus().irq[me].ime = true;
    let continuing = bus.bus().intr_wait_continuing[me];
    if !continuing && discard_old {
        let seen = bus.read32(at, false);
        bus.write32(at, seen & !flags, false);
    }
    let seen = bus.read32(at, false);
    if seen & flags != 0 {
        bus.write32(at, seen & !flags, false);
        bus.bus().intr_wait_continuing[me] = false;
        return;
    }
    bus.bus().intr_wait_continuing[me] = true;
    let swi_addr = cpu.pc();
    cpu.branch(bus, swi_addr);
    bus.bus().irq[me].halted = true;
}

/// GetCRC16: r0 = initial value, r1 = address, r2 = length; CRC-16/MODBUS style.
fn crc16<V: View>(cpu: &mut Cpu, bus: &mut V) {
    let mut crc = cpu.regs[0] & 0xFFFF;
    let (addr, len) = (cpu.regs[1], cpu.regs[2]);
    for i in 0..len {
        crc ^= u32::from(bus.read8(addr.wrapping_add(i), i != 0));
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xA001 } else { crc >> 1 };
        }
    }
    cpu.regs[0] = crc;
}

/// GetSineTable: a quarter wave in 64 steps, scaled to 7FFFh.
fn sine_table(index: u32) -> u32 {
    let x = f64::from(index & 0x3F) * std::f64::consts::PI / 128.0;
    (x.sin() * 32767.0).round() as i32 as u32
}

/// GetPitchTable: 768 steps per octave, as the fractional part in 1/65536.
fn pitch_table(index: u32) -> u32 {
    let i = f64::from(index % 768);
    ((2f64.powf(i / 768.0) - 1.0) * 65536.0).round() as u32
}

/// GetVolumeTable: an exponential volume curve over 724 steps (approximated).
fn volume_table(index: u32) -> u32 {
    let i = f64::from(index.min(723));
    (127.0 * 2f64.powf((i - 723.0) / 64.0)).round() as u32
}
