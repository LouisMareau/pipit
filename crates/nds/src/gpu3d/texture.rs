//! Texture sampling: the seven formats, their palettes, repeating and
//! flipping. Textures live in the VRAM banks mapped as texture slots,
//! palettes in those mapped as texture palette slots.

use crate::video::vram::{Region, Vram};

/// A texel as 15-bit colour and 0–31 alpha, or `None` when transparent.
pub type Texel = Option<(u16, u8)>;

/// Wraps or clamps a texel coordinate into a texture `size` wide.
#[inline]
fn coord(v: i32, size: i32, repeat: bool, flip: bool) -> i32 {
    if repeat {
        let m = v & (size - 1);
        if flip && v & size != 0 {
            size - 1 - m
        } else {
            m
        }
    } else {
        v.clamp(0, size - 1)
    }
}

/// The texel at texture coordinates `s`, `t` (12.4) of the texture that
/// TEXIMAGE_PARAM and PLTT_BASE describe.
pub fn sample(vram: &Vram, teximage: u32, pltt_base: u32, s: i32, t: i32) -> Texel {
    let format = (teximage >> 26) & 7;
    if format == 0 {
        return None;
    }
    let width = 8 << ((teximage >> 20) & 7);
    let height = 8 << ((teximage >> 23) & 7);
    let x = coord(s >> 4, width, teximage & (1 << 16) != 0, teximage & (1 << 18) != 0) as usize;
    let y = coord(t >> 4, height, teximage & (1 << 17) != 0, teximage & (1 << 19) != 0) as usize;
    let width = width as usize;
    let base = ((teximage & 0xFFFF) << 3) as usize;
    let at = y * width + x;
    let color0_clear = teximage & (1 << 29) != 0;
    let palette = |index: usize, unit: usize| -> u16 {
        vram.read16(Region::TexPalette, (pltt_base as usize) * unit + index * 2)
    };
    match format {
        1 => {
            let byte = vram.read8(Region::Texture, base + at);
            let alpha = byte >> 5;
            Some((palette(usize::from(byte & 0x1F), 16), (alpha << 2) | (alpha >> 1)))
        }
        2 => {
            let byte = vram.read8(Region::Texture, base + at / 4);
            let index = usize::from((byte >> ((x & 3) * 2)) & 3);
            (index != 0 || !color0_clear).then(|| (palette(index, 8), 31))
        }
        3 => {
            let byte = vram.read8(Region::Texture, base + at / 2);
            let index = usize::from(if x & 1 != 0 { byte >> 4 } else { byte & 0xF });
            (index != 0 || !color0_clear).then(|| (palette(index, 16), 31))
        }
        4 => {
            let index = usize::from(vram.read8(Region::Texture, base + at));
            (index != 0 || !color0_clear).then(|| (palette(index, 16), 31))
        }
        5 => {
            // 4 × 4 texel blocks: two bits per texel in slot 0 or 2, and a
            // palette word per block in the matching half of slot 1.
            let block = (y / 4) * (width / 4) + x / 4;
            let texels = vram.read8(Region::Texture, base + block * 4 + (y & 3));
            let index = usize::from((texels >> ((x & 3) * 2)) & 3);
            let mut info = 0x2_0000 + (base + block * 4) / 2;
            if base >= 0x4_0000 {
                info += 0x1_0000;
            }
            let word = vram.read16(Region::Texture, info);
            let offset = (pltt_base as usize) * 16 + usize::from(word & 0x3FFF) * 4;
            let entry = |i: usize| vram.read16(Region::TexPalette, offset + i * 2);
            let mode = word >> 14;
            match (mode, index) {
                (0, 3) | (1, 3) => None,
                (1, 2) => Some((mix(entry(0), entry(1), 1, 1), 31)),
                (3, 2) => Some((mix(entry(0), entry(1), 5, 3), 31)),
                (3, 3) => Some((mix(entry(0), entry(1), 3, 5), 31)),
                _ => Some((entry(index), 31)),
            }
        }
        6 => {
            let byte = vram.read8(Region::Texture, base + at);
            Some((palette(usize::from(byte & 7), 16), byte >> 3))
        }
        _ => {
            let color = vram.read16(Region::Texture, base + at * 2);
            (color & 0x8000 != 0).then_some((color & 0x7FFF, 31))
        }
    }
}

/// `(a × wa + b × wb) / (wa + wb)` per 5-bit channel.
fn mix(a: u16, b: u16, wa: u16, wb: u16) -> u16 {
    let mut out = 0;
    for shift in [0, 5, 10] {
        let ca = (a >> shift) & 0x1F;
        let cb = (b >> shift) & 0x1F;
        out |= ((ca * wa + cb * wb) / (wa + wb)) << shift;
    }
    out
}
