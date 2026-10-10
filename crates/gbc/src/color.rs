//! The Game Boy Color's screen look: its LCD mixes the channels and mutes
//! saturation, so games tuned on it look garish on a modern display. The
//! transform here (the one Gambatte and others use) brings that back; the
//! front-end applies it through a table indexed by the 15-bit colour.

/// The 15-bit colour behind a framebuffer pixel, as the table index.
pub fn lut_index(px: u32) -> usize {
    let r = ((px >> 16) & 0xFF) >> 3;
    let g = ((px >> 8) & 0xFF) >> 3;
    let b = (px & 0xFF) >> 3;
    (r | (g << 5) | (b << 10)) as usize
}

/// One RGBA entry per 15-bit colour, at `strength` (0 = raw, 1 = the full look).
pub fn lcd_lut_with_strength(strength: f64) -> Vec<[u8; 4]> {
    let strength = strength.clamp(0.0, 1.0);
    (0..0x8000u32)
        .map(|c| {
            let r = f64::from(c & 0x1F) * 255.0 / 31.0;
            let g = f64::from((c >> 5) & 0x1F) * 255.0 / 31.0;
            let b = f64::from((c >> 10) & 0x1F) * 255.0 / 31.0;
            let lr = (r * 13.0 + g * 2.0 + b) / 16.0;
            let lg = (g * 3.0 + b) / 4.0;
            let lb = (r * 3.0 + g * 2.0 + b * 11.0) / 16.0;
            let mix =
                |raw: f64, lcd: f64| (raw + (lcd - raw) * strength).round().clamp(0.0, 255.0) as u8;
            [mix(r, lr), mix(g, lg), mix(b, lb), 0xFF]
        })
        .collect()
}
