//! Colour correction: the GBA's reflective LCD showed games far more muted than
//! their raw 15-bit palettes, and artists tuned their colours for that screen.
//! This reproduces the look with the widely used "gba-color" transform
//! (Pokefan531 and hunterk): linearise, dim, mix the channels, re-gamma.
//!
//! The GBA has only 32768 colours, so the transform is precomputed into a table
//! indexed by the BGR555 value; applying it costs one lookup per pixel.

/// Number of 15-bit colours.
pub const LUT_SIZE: usize = 0x8000;

/// The standard "gba-color" parameters.
const TARGET_GAMMA: f64 = 2.2;
const DISPLAY_GAMMA: f64 = 2.2;
const LUMINANCE: f64 = 0.94;
/// Output channel = row · (R, G, B) in linear light. Rows sum to 1 so white
/// stays neutral.
const MATRIX: [[f64; 3]; 3] = [[0.82, 0.24, -0.06], [0.125, 0.665, 0.21], [0.195, 0.075, 0.73]];

/// Builds the lookup table: `lut[bgr555]` is the corrected colour as RGBA bytes.
pub fn lcd_lut() -> Vec<[u8; 4]> {
    // 5-bit → linear light, through the same 8-bit expansion the display uses.
    let linear: Vec<f64> = (0..32u32)
        .map(|v| (f64::from(v << 3 | v >> 2) / 255.0).powf(TARGET_GAMMA) * LUMINANCE)
        .collect();
    let mut lut = Vec::with_capacity(LUT_SIZE);
    for c in 0..LUT_SIZE {
        let r = linear[c & 0x1F];
        let g = linear[(c >> 5) & 0x1F];
        let b = linear[(c >> 10) & 0x1F];
        let mut out = [0u8; 4];
        for (i, row) in MATRIX.iter().enumerate() {
            let v = (row[0] * r + row[1] * g + row[2] * b).clamp(0.0, 1.0);
            out[i] = (v.powf(1.0 / DISPLAY_GAMMA) * 255.0).round() as u8;
        }
        out[3] = 0xFF;
        lut.push(out);
    }
    lut
}

/// Index into the table for a framebuffer pixel (`0xFFRRGGBB` from `bgr555_to_argb`).
#[inline(always)]
pub fn lut_index(argb: u32) -> usize {
    let r = (argb >> 19) & 0x1F;
    let g = (argb >> 11) & 0x1F;
    let b = (argb >> 3) & 0x1F;
    ((b << 10) | (g << 5) | r) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_dims_and_black_stays_black() {
        let lut = lcd_lut();
        assert_eq!(lut[0], [0, 0, 0, 255]);
        let white = lut[0x7FFF];
        assert_eq!(white[0], white[1]);
        assert_eq!(white[1], white[2]);
        assert!((240..=250).contains(&white[0]), "white became {:?}", white);
    }

    #[test]
    fn pure_red_loses_saturation() {
        let lut = lcd_lut();
        let red = lut[0x001F];
        assert!(red[0] > 180 && red[1] > 60 && red[2] > 80, "red became {:?}", red);
    }

    #[test]
    fn index_round_trips_through_the_framebuffer_format() {
        for c in [0u32, 0x7FFF, 0x001F, 0x03E0, 0x7C00, 0x1234] {
            assert_eq!(lut_index(crate::video::render::bgr555_to_argb(c as u16)), c as usize);
        }
    }
}
