//! BIOS calls that work the same on every machine: the memory copies and
//! fills, the decompressors and the arithmetic. Each core's BIOS replacement
//! dispatches to these for the numbers it shares with the others.

use crate::{Bus, Cpu};

pub fn div<B: Bus>(cpu: &mut Cpu, bus: &mut B, num: i32, den: i32) {
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

pub fn isqrt(v: u32) -> u32 {
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

pub fn cpu_set<B: Bus>(cpu: &mut Cpu, bus: &mut B) {
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

pub fn cpu_fast_set<B: Bus>(cpu: &mut Cpu, bus: &mut B) {
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
pub fn write_output<B: Bus>(bus: &mut B, dst: u32, data: &[u8], halfwords: bool) {
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

pub fn read_header<B: Bus>(bus: &mut B, src: u32) -> (u32, usize) {
    let header = bus.read32(src, false);
    (header, (header >> 8) as usize)
}

pub fn lz77_uncomp<B: Bus>(cpu: &mut Cpu, bus: &mut B, vram: bool) {
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

pub fn rl_uncomp<B: Bus>(cpu: &mut Cpu, bus: &mut B, vram: bool) {
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

pub fn huff_uncomp<B: Bus>(cpu: &mut Cpu, bus: &mut B) {
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

pub fn diff_8bit_unfilter<B: Bus>(cpu: &mut Cpu, bus: &mut B, vram: bool) {
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

pub fn diff_16bit_unfilter<B: Bus>(cpu: &mut Cpu, bus: &mut B) {
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

pub fn bit_unpack<B: Bus>(cpu: &mut Cpu, bus: &mut B) {
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
