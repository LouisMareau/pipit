//! The backgrounds of one engine: text (tiled, 16 or 256 colours, extended
//! palettes), affine (8-bit map entries), the DS's extended kinds (affine
//! with 16-bit entries, 8-bit and 16-bit bitmaps) and mode 6's large bitmap.

use super::render::{apply_mosaic, color15, rd16, TRANSPARENT};
use super::vram::{Region, Vram};
use super::{Engine, Video, SCREEN_WIDTH};

/// What each background is in each mode (`None` = not shown).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text,
    Affine,
    Extended,
    Large,
}

fn kinds(mode: u32) -> [Option<Kind>; 4] {
    use Kind::*;
    match mode {
        0 => [Some(Text), Some(Text), Some(Text), Some(Text)],
        1 => [Some(Text), Some(Text), Some(Text), Some(Affine)],
        2 => [Some(Text), Some(Text), Some(Affine), Some(Affine)],
        3 => [Some(Text), Some(Text), Some(Text), Some(Extended)],
        4 => [Some(Text), Some(Text), Some(Affine), Some(Extended)],
        5 => [Some(Text), Some(Text), Some(Extended), Some(Extended)],
        6 => [None, None, Some(Large), None],
        _ => [None; 4],
    }
}

/// The palette a background pixel comes from.
struct Palette<'a> {
    palette: &'a [u8],
    vram: &'a Vram,
    base: usize,
    ext: Option<(Region, usize)>,
}

impl Palette<'_> {
    /// Colour `index` of 256-colour palette `pal` (0 for the standard one).
    #[inline(always)]
    fn color(&self, pal: usize, index: usize) -> u32 {
        if index == 0 {
            return TRANSPARENT;
        }
        match self.ext {
            Some((region, slot)) => {
                color15(self.vram.read16(region, slot + pal * 0x200 + index * 2))
            }
            None => color15(rd16(self.palette, self.base + index * 2)),
        }
    }
}

impl Video {
    pub(super) fn render_bgs(&mut self, e: usize, y: usize) {
        let kinds = kinds(self.engines[e].mode());
        let bg0_is_3d = e == 0 && self.engines[e].dispcnt & (1 << 3) != 0;
        for (n, kind) in kinds.iter().enumerate() {
            if !self.engines[e].bg_enabled(n) || (n == 0 && bg0_is_3d) {
                continue;
            }
            match *kind {
                Some(Kind::Text) => self.text_bg(e, n, y),
                Some(Kind::Affine) => self.affine_bg(e, n, y),
                Some(Kind::Extended) => self.extended_bg(e, n, y),
                Some(Kind::Large) => self.large_bg(e, y),
                None => {}
            }
        }
    }

    fn bg_region(e: usize) -> Region {
        if e == 0 {
            Region::BgA
        } else {
            Region::BgB
        }
    }

    /// Where an engine's tiles and maps start: engine A adds DISPCNT's blocks.
    fn bases(engine: &Engine, e: usize, cnt: u16) -> (usize, usize) {
        let (char_block, screen_block) = if e == 0 {
            (
                ((engine.dispcnt >> 24) & 7) as usize * 0x10000,
                ((engine.dispcnt >> 27) & 7) as usize * 0x10000,
            )
        } else {
            (0, 0)
        };
        (
            char_block + usize::from((cnt >> 2) & 0xF) * 0x4000,
            screen_block + usize::from((cnt >> 8) & 0x1F) * 0x800,
        )
    }

    fn palette_for<'a>(
        engine: &Engine,
        palette: &'a [u8],
        vram: &'a Vram,
        e: usize,
        n: usize,
        cnt: u16,
    ) -> Palette<'a> {
        let ext = engine.dispcnt & (1 << 30) != 0 && cnt & (1 << 7) != 0;
        let slot = if n < 2 && cnt & (1 << 13) != 0 { n + 2 } else { n };
        let region = if e == 0 { Region::BgExtPalA } else { Region::BgExtPalB };
        Palette { palette, vram, base: e * 0x400, ext: ext.then_some((region, slot * 0x2000)) }
    }

    fn mosaic_of(engine: &Engine, cnt: u16) -> (usize, usize) {
        if cnt & (1 << 6) != 0 {
            let m = engine.mosaic();
            (usize::from(m & 0xF) + 1, usize::from((m >> 4) & 0xF) + 1)
        } else {
            (1, 1)
        }
    }

    fn text_bg(&mut self, e: usize, n: usize, y: usize) {
        let engine = &self.engines[e];
        let cnt = engine.bgcnt(n);
        let region = Self::bg_region(e);
        let (char_base, screen_base) = Self::bases(engine, e, cnt);
        let colors256 = cnt & (1 << 7) != 0;
        let size = (cnt >> 14) & 3;
        let width_tiles = if size & 1 != 0 { 64 } else { 32 };
        let height_tiles = if size & 2 != 0 { 64 } else { 32 };
        let (mosaic_w, mosaic_h) = Self::mosaic_of(engine, cnt);
        let y = y - y % mosaic_h;
        let scroll_x = engine.hofs(n);
        let scroll_y = engine.vofs(n);
        let map_y = (y + scroll_y) % (height_tiles * 8);
        let tile_y = map_y / 8;
        let pal = Self::palette_for(engine, &self.palette[..], &self.vram, e, n, cnt);
        let vram = &self.vram;
        let line = &mut self.scratch[e].bg[n];

        let mut x = 0usize;
        while x < SCREEN_WIDTH {
            let map_x = (x + scroll_x) % (width_tiles * 8);
            let tile_x = map_x / 8;
            let mut block = screen_base;
            if tile_x >= 32 {
                block += 0x800;
            }
            if tile_y >= 32 {
                block += if width_tiles == 64 { 0x1000 } else { 0x800 };
            }
            let entry = vram.read16(region, block + ((tile_y & 31) * 32 + (tile_x & 31)) * 2);
            let tile = usize::from(entry & 0x3FF);
            let hflip = entry & (1 << 10) != 0;
            let vflip = entry & (1 << 11) != 0;
            let palette = usize::from(entry >> 12);
            let row = if vflip { 7 - map_y % 8 } else { map_y % 8 };
            let start_px = map_x % 8;
            let count = (8 - start_px).min(SCREEN_WIDTH - x);
            if colors256 {
                let tile_addr = char_base + tile * 64 + row * 8;
                for i in 0..count {
                    let px = start_px + i;
                    let col = if hflip { 7 - px } else { px };
                    line[x + i] =
                        pal.color(palette, usize::from(vram.read8(region, tile_addr + col)));
                }
            } else {
                let tile_addr = char_base + tile * 32 + row * 4;
                for i in 0..count {
                    let px = start_px + i;
                    let col = if hflip { 7 - px } else { px };
                    let byte = vram.read8(region, tile_addr + col / 2);
                    let index = usize::from(if col & 1 != 0 { byte >> 4 } else { byte & 0xF });
                    line[x + i] =
                        if index == 0 { TRANSPARENT } else { pal.color(0, palette * 16 + index) };
                }
            }
            x += count;
        }
        if mosaic_w > 1 {
            apply_mosaic(line, mosaic_w);
        }
    }

    /// Walks an affine background's line: `sample(x, y)` gives the pixel at a
    /// map position, after wrapping or clipping to `width` × `height`.
    #[allow(clippy::too_many_arguments)]
    fn affine_line(
        engine: &Engine,
        a: usize,
        y: usize,
        cnt: u16,
        width: i32,
        height: i32,
        line: &mut [u32; SCREEN_WIDTH],
        sample: impl Fn(usize, usize) -> u32,
    ) {
        let wrap = cnt & (1 << 13) != 0;
        let (mosaic_w, mosaic_h) = Self::mosaic_of(engine, cnt);
        let [pa, pb, pc, pd] = engine.affine(a);
        // Vertical mosaic replays the reference point of the first line in the block.
        let back = (y % mosaic_h) as i32;
        let mut tx = engine.bgx_internal[a].wrapping_sub(pb.wrapping_mul(back));
        let mut ty = engine.bgy_internal[a].wrapping_sub(pd.wrapping_mul(back));
        for px in line.iter_mut() {
            let mut x = tx >> 8;
            let mut y = ty >> 8;
            tx = tx.wrapping_add(pa);
            ty = ty.wrapping_add(pc);
            if wrap {
                x &= width - 1;
                y &= height - 1;
            } else if x < 0 || y < 0 || x >= width || y >= height {
                *px = TRANSPARENT;
                continue;
            }
            *px = sample(x as usize, y as usize);
        }
        if mosaic_w > 1 {
            apply_mosaic(line, mosaic_w);
        }
    }

    /// An affine background with 8-bit map entries (modes 1, 2 and 4).
    fn affine_bg(&mut self, e: usize, n: usize, y: usize) {
        let engine = &self.engines[e];
        let cnt = engine.bgcnt(n);
        let region = Self::bg_region(e);
        let (char_base, screen_base) = Self::bases(engine, e, cnt);
        let size_tiles = 16usize << ((cnt >> 14) & 3);
        let size = (size_tiles * 8) as i32;
        let pal = Self::palette_for(engine, &self.palette[..], &self.vram, e, n, cnt);
        let vram = &self.vram;
        let line = &mut self.scratch[e].bg[n];
        Self::affine_line(engine, n - 2, y, cnt, size, size, line, |x, y| {
            let tile = usize::from(vram.read8(region, screen_base + (y / 8) * size_tiles + x / 8));
            let index = vram.read8(region, char_base + tile * 64 + (y % 8) * 8 + x % 8);
            // Standard palette only: extended palettes need 16-bit entries.
            if index == 0 {
                TRANSPARENT
            } else {
                color15(rd16(pal.palette, pal.base + usize::from(index) * 2))
            }
        });
    }

    /// Modes 3–5's extended backgrounds: BGxCNT bit 7 picks tiles with 16-bit
    /// map entries (clear) or a bitmap (set), bit 2 then 8-bit or direct colour.
    fn extended_bg(&mut self, e: usize, n: usize, y: usize) {
        let engine = &self.engines[e];
        let cnt = engine.bgcnt(n);
        let region = Self::bg_region(e);
        let vram = &self.vram;
        if cnt & (1 << 7) == 0 {
            let (char_base, screen_base) = Self::bases(engine, e, cnt);
            let size_tiles = 16usize << ((cnt >> 14) & 3);
            let size = (size_tiles * 8) as i32;
            let pal = Self::palette_for(engine, &self.palette[..], &self.vram, e, n, cnt | 1 << 7);
            let line = &mut self.scratch[e].bg[n];
            Self::affine_line(engine, n - 2, y, cnt, size, size, line, |x, y| {
                let entry = vram.read16(region, screen_base + ((y / 8) * size_tiles + x / 8) * 2);
                let tile = usize::from(entry & 0x3FF);
                let px = if entry & (1 << 10) != 0 { 7 - x % 8 } else { x % 8 };
                let py = if entry & (1 << 11) != 0 { 7 - y % 8 } else { y % 8 };
                let index = vram.read8(region, char_base + tile * 64 + py * 8 + px);
                pal.color(usize::from(entry >> 12), usize::from(index))
            });
            return;
        }
        let base = usize::from((cnt >> 8) & 0x1F) * 0x4000;
        let (width, height) =
            [(128, 128), (256, 256), (512, 256), (512, 512)][((cnt >> 14) & 3) as usize];
        let direct = cnt & (1 << 2) != 0;
        let pal = Self::palette_for(engine, &self.palette[..], &self.vram, e, n, 0);
        let line = &mut self.scratch[e].bg[n];
        Self::affine_line(engine, n - 2, y, cnt, width, height, line, |x, y| {
            let at = y * width as usize + x;
            if direct {
                let c = vram.read16(region, base + at * 2);
                if c & 0x8000 != 0 {
                    color15(c)
                } else {
                    TRANSPARENT
                }
            } else {
                pal.color(0, usize::from(vram.read8(region, base + at)))
            }
        });
    }

    /// Mode 6: engine A's BG2 is one 8-bit bitmap over the whole of its VRAM.
    fn large_bg(&mut self, e: usize, y: usize) {
        let engine = &self.engines[e];
        let cnt = engine.bgcnt(2);
        let region = Self::bg_region(e);
        let vram = &self.vram;
        let (width, height) = if cnt & (1 << 14) != 0 { (1024, 512) } else { (512, 1024) };
        let pal = Self::palette_for(engine, &self.palette[..], &self.vram, e, 2, 0);
        let line = &mut self.scratch[e].bg[2];
        Self::affine_line(engine, 0, y, cnt, width, height, line, |x, y| {
            pal.color(0, usize::from(vram.read8(region, y * width as usize + x)))
        });
    }
}
