//! High-level emulation of the BIOS system calls (GBATEK "BIOS Functions").
//!
//! Each call is dispatched by number with the CPU registers as arguments and
//! results, following the documented calling convention. Memory is touched through
//! the bus so timing and side effects (DMA, I/O) stay right.

use crate::cpu::{mode, Cpu};
use crate::memory::Bus;

/// Where the BIOS keeps the interrupt flags that `IntrWait` watches.
const INTR_CHECK_FLAGS: u32 = 0x0300_7FF8;
const SOFT_RESET_FLAG: u32 = 0x0300_7FFA;

pub fn call(cpu: &mut Cpu, bus: &mut Bus, number: u8) {
    // Entering and leaving the BIOS costs a few cycles on top of the work.
    bus.idle(6);
    // Software can observe which BIOS opcode was fetched last through the BIOS
    // read protection; after a call the original leaves `mov r2, #4` there.
    bus.bios_latch = 0xE3A0_2004;
    match number {
        0x00 => soft_reset(cpu, bus),
        0x01 => register_ram_reset(cpu, bus),
        0x02 => bus.irq.halted = true,
        0x03 => bus.irq.stopped = true,
        0x04 => intr_wait(cpu, bus, cpu.regs[0] != 0, cpu.regs[1] as u16),
        0x05 => intr_wait(cpu, bus, true, 1),
        0x06 => div(cpu, bus, cpu.regs[0] as i32, cpu.regs[1] as i32),
        0x07 => div(cpu, bus, cpu.regs[1] as i32, cpu.regs[0] as i32),
        0x08 => {
            cpu.regs[0] = isqrt(cpu.regs[0]);
            bus.idle(60);
        }
        0x09 => {
            cpu.regs[0] = arctan(cpu.regs[0] as i32) as u32;
            bus.idle(90);
        }
        0x0A => {
            cpu.regs[0] = u32::from(arctan2(cpu.regs[0] as i16 as i32, cpu.regs[1] as i16 as i32));
            bus.idle(120);
        }
        0x0B => cpu_set(cpu, bus),
        0x0C => cpu_fast_set(cpu, bus),
        0x0D => cpu.regs[0] = 0xBAAE_187F,
        0x0E => bg_affine_set(cpu, bus),
        0x0F => obj_affine_set(cpu, bus),
        0x10 => bit_unpack(cpu, bus),
        0x11 => lz77_uncomp(cpu, bus, false),
        0x12 => lz77_uncomp(cpu, bus, true),
        0x13 => huff_uncomp(cpu, bus),
        0x14 => rl_uncomp(cpu, bus, false),
        0x15 => rl_uncomp(cpu, bus, true),
        0x16 => diff_8bit_unfilter(cpu, bus, false),
        0x17 => diff_8bit_unfilter(cpu, bus, true),
        0x18 => diff_16bit_unfilter(cpu, bus),
        0x19 => sound_bias(cpu, bus),
        0x1F => midi_key_to_freq(cpu, bus),
        // Sound driver calls (0x1A-0x1E, 0x28-0x29) belong to the legacy BIOS sound
        // library; every retail game ships its own driver in the ROM.
        _ => {}
    }
}

// -------------------------------------------------------------------------------
// System
// -------------------------------------------------------------------------------

fn soft_reset(cpu: &mut Cpu, bus: &mut Bus) {
    let flag = bus.load8(SOFT_RESET_FLAG);
    for addr in (0x0300_7E00..0x0300_8000).step_by(4) {
        bus.store32(addr, 0);
    }
    cpu.regs = [0; 16];
    cpu.switch_mode(mode::SVC);
    cpu.regs[13] = 0x0300_7FE0;
    cpu.switch_mode(mode::IRQ);
    cpu.regs[13] = 0x0300_7FA0;
    cpu.switch_mode(mode::SYS);
    cpu.regs[13] = 0x0300_7F00;
    cpu.cpsr = mode::SYS;
    bus.irq.halted = false;
    let entry = if flag == 0 { 0x0800_0000 } else { 0x0200_0000 };
    cpu.refill_arm(bus, entry);
}

fn register_ram_reset(_cpu: &mut Cpu, bus: &mut Bus) {
    let flags = _cpu.regs[0];
    // The LCD is blanked first so the clears are not visible.
    bus.store16(0x0400_0000, 0x0080);
    let clear = |bus: &mut Bus, start: u32, len: u32| {
        for addr in (start..start + len).step_by(4) {
            bus.store32(addr, 0);
        }
    };
    if flags & 0x01 != 0 {
        clear(bus, 0x0200_0000, 0x40000);
    }
    if flags & 0x02 != 0 {
        clear(bus, 0x0300_0000, 0x7E00);
    }
    if flags & 0x04 != 0 {
        clear(bus, 0x0500_0000, 0x400);
    }
    if flags & 0x08 != 0 {
        clear(bus, 0x0600_0000, 0x18000);
    }
    if flags & 0x10 != 0 {
        clear(bus, 0x0700_0000, 0x400);
    }
    if flags & 0x20 != 0 {
        clear(bus, 0x0400_0120, 0x40);
    }
    if flags & 0x40 != 0 {
        clear(bus, 0x0400_0060, 0x48);
        bus.store16(0x0400_0088, 0x200);
    }
    if flags & 0x80 != 0 {
        clear(bus, 0x0400_0000, 0x60);
        clear(bus, 0x0400_00B0, 0x30);
        clear(bus, 0x0400_0100, 0x10);
        clear(bus, 0x0400_0200, 0x0C);
        bus.store16(0x0400_0000, 0x0080);
    }
}

/// Waits for one of the interrupts in `flags`, as acknowledged by the game's
/// handler in the BIOS flag word. If the wait cannot finish now, the CPU halts
/// *at the SWI*, so the interrupt return re-executes it and checks again.
fn intr_wait(cpu: &mut Cpu, bus: &mut Bus, discard_old: bool, flags: u16) {
    bus.irq.ime = true;
    if !cpu.intr_wait_continuing && discard_old {
        let seen = bus.load16(INTR_CHECK_FLAGS);
        bus.store16(INTR_CHECK_FLAGS, seen & !flags);
    }
    let seen = bus.load16(INTR_CHECK_FLAGS);
    if seen & flags != 0 {
        bus.store16(INTR_CHECK_FLAGS, seen & !flags);
        cpu.intr_wait_continuing = false;
        return;
    }
    cpu.intr_wait_continuing = true;
    let swi_addr = cpu.pc();
    cpu.branch(bus, swi_addr);
    bus.irq.halted = true;
}

// -------------------------------------------------------------------------------
// Arithmetic
// -------------------------------------------------------------------------------

fn div(cpu: &mut Cpu, bus: &mut Bus, num: i32, den: i32) {
    bus.idle(40);
    if den == 0 {
        // Hardware hangs in a loop here; return the values games are known to see.
        cpu.regs[0] = if num < 0 { 1 } else { u32::MAX };
        cpu.regs[1] = num as u32;
        cpu.regs[3] = 1;
        return;
    }
    let q = num.wrapping_div(den);
    cpu.regs[0] = q as u32;
    cpu.regs[1] = num.wrapping_rem(den) as u32;
    cpu.regs[3] = q.unsigned_abs();
}

fn isqrt(v: u32) -> u32 {
    if v < 2 {
        return v;
    }
    let mut x = (v as f64).sqrt() as u32;
    while u64::from(x) * u64::from(x) > u64::from(v) {
        x -= 1;
    }
    while u64::from(x + 1) * u64::from(x + 1) <= u64::from(v) {
        x += 1;
    }
    x
}

/// ArcTan of a 1.14 fixed-point tangent, as the BIOS polynomial computes it.
fn arctan(i: i32) -> i16 {
    let a = -((i.wrapping_mul(i)) >> 14);
    let mut b = ((0xA9 * a) >> 14) + 0x390;
    b = ((b * a) >> 14) + 0x91C;
    b = ((b * a) >> 14) + 0xFB6;
    b = ((b * a) >> 14) + 0x16AA;
    b = ((b * a) >> 14) + 0x2081;
    b = ((b * a) >> 14) + 0x3651;
    b = ((b * a) >> 14) + 0xA2F9;
    ((i.wrapping_mul(b)) >> 16) as i16
}

/// Full-circle arctangent returning 0..0xFFFF, matching the BIOS quadrant logic.
fn arctan2(x: i32, y: i32) -> u16 {
    let at = |n: i32, d: i32| i32::from(arctan((n << 14) / d));
    let r = if y == 0 {
        if x >= 0 {
            0
        } else {
            0x8000
        }
    } else if x == 0 {
        if y >= 0 {
            0x4000
        } else {
            0xC000
        }
    } else if y >= 0 {
        if x >= 0 {
            if x >= y {
                at(y, x)
            } else {
                0x4000 - at(x, y)
            }
        } else if -x >= y {
            at(y, x) + 0x8000
        } else {
            0x4000 - at(x, y)
        }
    } else if x <= 0 {
        if -x > -y {
            at(y, x) + 0x8000
        } else {
            0xC000 - at(x, y)
        }
    } else if x >= -y {
        at(y, x) + 0x10000
    } else {
        0xC000 - at(x, y)
    };
    r as u16
}

// -------------------------------------------------------------------------------
// Memory
// -------------------------------------------------------------------------------

fn cpu_set(cpu: &mut Cpu, bus: &mut Bus) {
    let mut src = cpu.regs[0];
    let mut dst = cpu.regs[1];
    let control = cpu.regs[2];
    let count = control & 0x1F_FFFF;
    let fill = control & (1 << 24) != 0;
    if control & (1 << 26) != 0 {
        src &= !3;
        dst &= !3;
        let value = bus.read32(src, false);
        for i in 0..count {
            let v = if fill || i == 0 { value } else { bus.read32(src, true) };
            bus.write32(dst, v, i != 0);
            src = src.wrapping_add(4);
            dst = dst.wrapping_add(4);
        }
    } else {
        src &= !1;
        dst &= !1;
        let value = bus.read16(src, false);
        for i in 0..count {
            let v = if fill || i == 0 { value } else { bus.read16(src, true) };
            bus.write16(dst, v, i != 0);
            src = src.wrapping_add(2);
            dst = dst.wrapping_add(2);
        }
    }
}

fn cpu_fast_set(cpu: &mut Cpu, bus: &mut Bus) {
    let mut src = cpu.regs[0] & !3;
    let mut dst = cpu.regs[1] & !3;
    let control = cpu.regs[2];
    // Transfers happen in blocks of 8 words; the count is rounded up to match.
    let count = (control & 0x1F_FFFF).div_ceil(8) * 8;
    let fill = control & (1 << 24) != 0;
    let value = bus.read32(src, false);
    for i in 0..count {
        let v = if fill || i == 0 { value } else { bus.read32(src, true) };
        bus.write32(dst, v, i != 0);
        src = src.wrapping_add(4);
        dst = dst.wrapping_add(4);
    }
}

/// Writes decompressed bytes to memory. VRAM only accepts 16-bit writes, so the
/// VRAM variants of the decompressors go through halfwords.
fn write_output(bus: &mut Bus, dst: u32, data: &[u8], halfwords: bool) {
    if halfwords {
        let dst = dst & !1;
        for (i, pair) in data.chunks(2).enumerate() {
            let v = u16::from(pair[0]) | (u16::from(*pair.get(1).unwrap_or(&0)) << 8);
            bus.write16(dst.wrapping_add(i as u32 * 2), v, i != 0);
        }
    } else {
        for (i, &b) in data.iter().enumerate() {
            bus.write8(dst.wrapping_add(i as u32), b, i != 0);
        }
    }
}

fn read_header(bus: &mut Bus, src: u32) -> (u32, usize) {
    let header = bus.read32(src, false);
    (header, (header >> 8) as usize)
}

fn lz77_uncomp(cpu: &mut Cpu, bus: &mut Bus, vram: bool) {
    let mut src = cpu.regs[0];
    let dst = cpu.regs[1];
    let (_, size) = read_header(bus, src);
    src = src.wrapping_add(4);
    let mut out: Vec<u8> = Vec::with_capacity(size);
    while out.len() < size {
        let flags = bus.read8(src, true);
        src = src.wrapping_add(1);
        for bit in (0..8).rev() {
            if out.len() >= size {
                break;
            }
            if flags & (1 << bit) == 0 {
                out.push(bus.read8(src, true));
                src = src.wrapping_add(1);
            } else {
                let b1 = bus.read8(src, true);
                let b2 = bus.read8(src.wrapping_add(1), true);
                src = src.wrapping_add(2);
                let len = usize::from(b1 >> 4) + 3;
                let disp = ((usize::from(b1 & 0xF) << 8) | usize::from(b2)) + 1;
                for _ in 0..len {
                    if out.len() >= size {
                        break;
                    }
                    let b = if disp <= out.len() { out[out.len() - disp] } else { 0 };
                    out.push(b);
                }
            }
        }
    }
    write_output(bus, dst, &out, vram);
}

fn rl_uncomp(cpu: &mut Cpu, bus: &mut Bus, vram: bool) {
    let mut src = cpu.regs[0];
    let dst = cpu.regs[1];
    let (_, size) = read_header(bus, src);
    src = src.wrapping_add(4);
    let mut out: Vec<u8> = Vec::with_capacity(size);
    while out.len() < size {
        let flag = bus.read8(src, true);
        src = src.wrapping_add(1);
        if flag & 0x80 != 0 {
            let len = usize::from(flag & 0x7F) + 3;
            let b = bus.read8(src, true);
            src = src.wrapping_add(1);
            for _ in 0..len.min(size - out.len()) {
                out.push(b);
            }
        } else {
            let len = usize::from(flag & 0x7F) + 1;
            for _ in 0..len.min(size - out.len()) {
                out.push(bus.read8(src, true));
                src = src.wrapping_add(1);
            }
        }
    }
    write_output(bus, dst, &out, vram);
}

fn huff_uncomp(cpu: &mut Cpu, bus: &mut Bus) {
    let src = cpu.regs[0];
    let dst = cpu.regs[1];
    let (header, size) = read_header(bus, src);
    let bits = header & 0xF;
    let tree_size = u32::from(bus.read8(src.wrapping_add(4), true));
    let tree = src.wrapping_add(5);
    let mut data = src.wrapping_add(5).wrapping_add(tree_size * 2 + 1);
    let mut out: Vec<u8> = Vec::with_capacity(size);

    let mut word: u32 = 0;
    let mut word_bits = 0;
    let mut node = tree;
    let mut nibble_pending: Option<u8> = None;
    while out.len() < size {
        if word_bits == 0 {
            word = bus.read32(data, true);
            data = data.wrapping_add(4);
            word_bits = 32;
        }
        let bit = (word >> 31) & 1;
        word <<= 1;
        word_bits -= 1;

        let n = bus.read8(node, true);
        let offset = u32::from(n & 0x3F);
        let child = (node & !1).wrapping_add(offset * 2 + 2 + bit);
        let is_data = if bit == 0 { n & 0x80 != 0 } else { n & 0x40 != 0 };
        if is_data {
            let value = bus.read8(child, true);
            if bits == 8 {
                out.push(value);
            } else {
                match nibble_pending.take() {
                    None => nibble_pending = Some(value & 0xF),
                    Some(lo) => out.push(lo | (value << 4)),
                }
            }
            node = tree;
        } else {
            node = child;
        }
    }
    // Output is written in 32-bit units.
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
    for (i, w) in out.chunks(4).enumerate() {
        let v = u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
        bus.write32(dst.wrapping_add(i as u32 * 4), v, i != 0);
    }
}

fn diff_8bit_unfilter(cpu: &mut Cpu, bus: &mut Bus, vram: bool) {
    let mut src = cpu.regs[0];
    let dst = cpu.regs[1];
    let (_, size) = read_header(bus, src);
    src = src.wrapping_add(4);
    let mut out: Vec<u8> = Vec::with_capacity(size);
    let mut acc: u8 = 0;
    for _ in 0..size {
        acc = acc.wrapping_add(bus.read8(src, true));
        src = src.wrapping_add(1);
        out.push(acc);
    }
    write_output(bus, dst, &out, vram);
}

fn diff_16bit_unfilter(cpu: &mut Cpu, bus: &mut Bus) {
    let mut src = cpu.regs[0];
    let mut dst = cpu.regs[1];
    let (_, size) = read_header(bus, src);
    src = src.wrapping_add(4);
    let mut acc: u16 = 0;
    for i in 0..size / 2 {
        acc = acc.wrapping_add(bus.read16(src, true));
        src = src.wrapping_add(2);
        bus.write16(dst, acc, i != 0);
        dst = dst.wrapping_add(2);
    }
}

fn bit_unpack(cpu: &mut Cpu, bus: &mut Bus) {
    let mut src = cpu.regs[0];
    let mut dst = cpu.regs[1];
    let info = cpu.regs[2];
    let length = u32::from(bus.read16(info, false));
    let src_width = u32::from(bus.read8(info.wrapping_add(2), true));
    let dst_width = u32::from(bus.read8(info.wrapping_add(3), true));
    let offset_word = bus.read32(info.wrapping_add(4), true);
    let offset = offset_word & 0x7FFF_FFFF;
    let zero_offset = offset_word & (1 << 31) != 0;
    if !matches!(src_width, 1 | 2 | 4 | 8) || !matches!(dst_width, 1 | 2 | 4 | 8 | 16 | 32) {
        return;
    }

    let mut out: u32 = 0;
    let mut out_bits = 0;
    for _ in 0..length {
        let byte = u32::from(bus.read8(src, true));
        src = src.wrapping_add(1);
        for i in (0..8).step_by(src_width as usize) {
            let mut v = (byte >> i) & ((1 << src_width) - 1);
            if v != 0 || zero_offset {
                v = v.wrapping_add(offset);
            }
            out |= v << out_bits;
            out_bits += dst_width;
            if out_bits >= 32 {
                bus.write32(dst, out, true);
                dst = dst.wrapping_add(4);
                out = 0;
                out_bits = 0;
            }
        }
    }
}

// -------------------------------------------------------------------------------
// Affine helpers
// -------------------------------------------------------------------------------

/// Sine and cosine in 1.14 fixed point for a BIOS angle (upper byte = 0..255).
fn sin_cos(theta: u16) -> (i32, i32) {
    let angle = f64::from(theta >> 8) * std::f64::consts::TAU / 256.0;
    ((angle.sin() * 16384.0).round() as i32, (angle.cos() * 16384.0).round() as i32)
}

fn bg_affine_set(cpu: &mut Cpu, bus: &mut Bus) {
    let mut src = cpu.regs[0];
    let mut dst = cpu.regs[1];
    for _ in 0..cpu.regs[2] {
        let ox = bus.read32(src, false) as i32;
        let oy = bus.read32(src.wrapping_add(4), true) as i32;
        let cx = i32::from(bus.read16(src.wrapping_add(8), true) as i16);
        let cy = i32::from(bus.read16(src.wrapping_add(10), true) as i16);
        let sx = i32::from(bus.read16(src.wrapping_add(12), true) as i16);
        let sy = i32::from(bus.read16(src.wrapping_add(14), true) as i16);
        let theta = bus.read16(src.wrapping_add(16), true);
        src = src.wrapping_add(20);
        let (sin, cos) = sin_cos(theta);
        let pa = (sx * cos) >> 14;
        let pb = -((sx * sin) >> 14);
        let pc = (sy * sin) >> 14;
        let pd = (sy * cos) >> 14;
        let x = ox - (pa * cx + pb * cy);
        let y = oy - (pc * cx + pd * cy);
        bus.write16(dst, pa as u16, false);
        bus.write16(dst.wrapping_add(2), pb as u16, true);
        bus.write16(dst.wrapping_add(4), pc as u16, true);
        bus.write16(dst.wrapping_add(6), pd as u16, true);
        bus.write32(dst.wrapping_add(8), x as u32, true);
        bus.write32(dst.wrapping_add(12), y as u32, true);
        dst = dst.wrapping_add(16);
    }
}

fn obj_affine_set(cpu: &mut Cpu, bus: &mut Bus) {
    let mut src = cpu.regs[0];
    let mut dst = cpu.regs[1];
    let stride = cpu.regs[3];
    for _ in 0..cpu.regs[2] {
        let sx = i32::from(bus.read16(src, false) as i16);
        let sy = i32::from(bus.read16(src.wrapping_add(2), true) as i16);
        let theta = bus.read16(src.wrapping_add(4), true);
        src = src.wrapping_add(8);
        let (sin, cos) = sin_cos(theta);
        let pa = (sx * cos) >> 14;
        let pb = -((sx * sin) >> 14);
        let pc = (sy * sin) >> 14;
        let pd = (sy * cos) >> 14;
        bus.write16(dst, pa as u16, false);
        bus.write16(dst.wrapping_add(stride), pb as u16, false);
        bus.write16(dst.wrapping_add(stride * 2), pc as u16, false);
        bus.write16(dst.wrapping_add(stride * 3), pd as u16, false);
        dst = dst.wrapping_add(stride * 4);
    }
}

// -------------------------------------------------------------------------------
// Sound
// -------------------------------------------------------------------------------

fn sound_bias(cpu: &mut Cpu, bus: &mut Bus) {
    let bias = if cpu.regs[0] == 0 { 0 } else { 0x200 };
    bus.store16(0x0400_0088, bias);
}

/// Frequency of a MIDI key for a wave sample: `freq * 2^((180 - key - fine/256) / 12)`.
fn midi_key_to_freq(cpu: &mut Cpu, bus: &mut Bus) {
    let base = f64::from(bus.read32(cpu.regs[0].wrapping_add(4), false));
    let key = f64::from(cpu.regs[1] as u8);
    let fine = f64::from(cpu.regs[2] as u8) / 256.0;
    cpu.regs[0] = (base * 2f64.powf((180.0 - key - fine) / 12.0)) as u32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isqrt_matches_floor_sqrt() {
        for v in [0u32, 1, 2, 3, 4, 15, 16, 17, 99, 100, 65535, 65536, u32::MAX] {
            let r = isqrt(v);
            assert!(u64::from(r) * u64::from(r) <= u64::from(v));
            assert!(u64::from(r + 1) * u64::from(r + 1) > u64::from(v));
        }
    }

    #[test]
    fn arctan2_axes() {
        assert_eq!(arctan2(1, 0), 0);
        assert_eq!(arctan2(0, 1), 0x4000);
        assert_eq!(arctan2(-1, 0), 0x8000);
        assert_eq!(arctan2(0, -1), 0xC000);
        // 45 degrees is 0x2000; the polynomial lands within a few units.
        assert!((i32::from(arctan2(100, 100)) - 0x2000).abs() < 8);
    }
}
