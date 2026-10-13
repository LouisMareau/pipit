//! The objects of one engine: tiled (16 or 256 colours, extended palettes),
//! bitmap (direct colour with an alpha of their own), affine, mosaic, and
//! the ones that only shape the object window.

use super::render::{color15, rd16, ObjPixel, TRANSPARENT};
use super::vram::Region;
use super::{Video, SCREEN_WIDTH};

const SIZES: [[(i32, i32); 4]; 3] = [
    [(8, 8), (16, 16), (32, 32), (64, 64)],
    [(16, 8), (32, 8), (32, 16), (64, 32)],
    [(8, 16), (8, 32), (16, 32), (32, 64)],
];

impl Video {
    pub(super) fn render_objs(&mut self, e: usize, y: usize) {
        let engine = &self.engines[e];
        let dispcnt = engine.dispcnt;
        let region = if e == 0 { Region::ObjA } else { Region::ObjB };
        let oam = &self.oam[e * 0x400..(e + 1) * 0x400];
        let vram = &self.vram;
        let palette = &self.palette[e * 0x400 + 0x200..e * 0x400 + 0x400];
        let ext_pal = dispcnt & (1 << 31) != 0;
        let ext_region = if e == 0 { Region::ObjExtPalA } else { Region::ObjExtPalB };
        let mapping_1d = dispcnt & (1 << 4) != 0;
        let tile_boundary = 32usize << ((dispcnt >> 20) & 3);
        let bitmap_1d = dispcnt & (1 << 6) != 0;
        let bitmap_boundary = 128usize << ((dispcnt >> 22) & 1);
        let bitmap_wide = dispcnt & (1 << 5) != 0;
        let mosaic_w = usize::from((engine.mosaic() >> 8) & 0xF) + 1;
        let mosaic_h = usize::from((engine.mosaic() >> 12) & 0xF) + 1;
        let scratch = &mut self.scratch[e];

        for n in 0..128 {
            let attr0 = rd16(oam, n * 8);
            let attr1 = rd16(oam, n * 8 + 2);
            let attr2 = rd16(oam, n * 8 + 4);
            let affine = attr0 & (1 << 8) != 0;
            if !affine && attr0 & (1 << 9) != 0 {
                continue;
            }
            let shape = usize::from(attr0 >> 14);
            if shape == 3 {
                continue;
            }
            let obj_mode = (attr0 >> 10) & 3;
            let (w, h) = SIZES[shape][usize::from(attr1 >> 14)];
            let double = affine && attr0 & (1 << 9) != 0;
            let (box_w, box_h) = if double { (w * 2, h * 2) } else { (w, h) };
            let mut sy = i32::from(attr0 & 0xFF);
            if sy + box_h > 256 {
                sy -= 256;
            }
            let mut sx = i32::from(attr1 & 0x1FF);
            if sx >= 256 {
                sx -= 512;
            }
            let line_y = y as i32;
            if line_y < sy || line_y >= sy + box_h {
                continue;
            }

            let mosaic = attr0 & (1 << 12) != 0;
            let colors256 = attr0 & (1 << 13) != 0;
            let priority = ((attr2 >> 10) & 3) as u8;
            let pal_number = usize::from((attr2 >> 12) & 0xF);
            let tile = usize::from(attr2 & 0x3FF);
            let bitmap = obj_mode == 3;
            let window = obj_mode == 2;
            let alpha = (attr2 >> 12) as u8 & 0xF;
            if bitmap && alpha == 0 {
                continue;
            }

            let mut local_y = line_y - sy;
            if mosaic {
                local_y = (local_y - (y % mosaic_h) as i32).max(0);
            }
            let (pa, pb, pc, pd) = if affine {
                let group = usize::from((attr1 >> 9) & 0x1F) * 32;
                (
                    i32::from(rd16(oam, group + 6) as i16),
                    i32::from(rd16(oam, group + 14) as i16),
                    i32::from(rd16(oam, group + 22) as i16),
                    i32::from(rd16(oam, group + 30) as i16),
                )
            } else {
                (0x100, 0, 0, 0x100)
            };
            let hflip = !affine && attr1 & (1 << 12) != 0;
            let vflip = !affine && attr1 & (1 << 13) != 0;

            // Where the object's pixels are: tiles per row and bytes per tile for
            // tiled objects; the bitmap's base and stride for bitmap ones.
            let tile_bytes = if colors256 { 64 } else { 32 };
            let (base, row_stride) = if bitmap {
                if bitmap_1d {
                    (tile * bitmap_boundary, w as usize * 2)
                } else if bitmap_wide {
                    ((tile & 0x1F) * 0x10 + (tile & 0x3E0) * 0x100, 512)
                } else {
                    ((tile & 0xF) * 0x10 + (tile & 0x3F0) * 0x80, 256)
                }
            } else if mapping_1d {
                (tile * tile_boundary, (w / 8) as usize * tile_bytes)
            } else {
                (tile * 32, 1024)
            };

            let half_w = box_w / 2;
            let half_h = box_h / 2;
            let dy = local_y - half_h;
            let base_tx = (w / 2) * 256 + pb * dy;
            let base_ty = (h / 2) * 256 + pd * dy;

            for bx in 0..box_w {
                let screen_x = sx + bx;
                if !(0..SCREEN_WIDTH as i32).contains(&screen_x) {
                    continue;
                }
                let dx = bx - half_w;
                let tx = (base_tx + pa * dx) >> 8;
                let ty = (base_ty + pc * dx) >> 8;
                if tx < 0 || ty < 0 || tx >= w || ty >= h {
                    continue;
                }
                let tx = if hflip { w - 1 - tx } else { tx } as usize;
                let ty = if vflip { h - 1 - ty } else { ty } as usize;
                let sxu = screen_x as usize;

                let color = if bitmap {
                    let c = vram.read16(region, base + ty * row_stride + tx * 2);
                    if c & 0x8000 == 0 {
                        continue;
                    }
                    color15(c)
                } else {
                    let tile_at = base + (ty / 8) * row_stride + (tx / 8) * tile_bytes;
                    let index = if colors256 {
                        usize::from(vram.read8(region, tile_at + (ty % 8) * 8 + tx % 8))
                    } else {
                        let byte = vram.read8(region, tile_at + (ty % 8) * 4 + (tx % 8) / 2);
                        usize::from(if tx & 1 != 0 { byte >> 4 } else { byte & 0xF })
                    };
                    if index == 0 {
                        continue;
                    }
                    if window {
                        scratch.obj_window[sxu] = true;
                        continue;
                    }
                    if colors256 && ext_pal {
                        color15(vram.read16(ext_region, pal_number * 0x200 + index * 2))
                    } else if colors256 {
                        color15(rd16(palette, index * 2))
                    } else {
                        color15(rd16(palette, (pal_number * 16 + index) * 2))
                    }
                };
                let slot = &mut scratch.obj[sxu];
                // Earlier OAM entries win ties; a better priority always wins.
                if slot.color == TRANSPARENT || priority < slot.priority {
                    *slot = ObjPixel {
                        color,
                        priority,
                        kind: if bitmap { 2 } else { u8::from(obj_mode == 1) },
                        alpha,
                    };
                }
            }
        }

        if mosaic_w > 1 {
            let obj = &mut scratch.obj;
            let mut i = 0;
            while i < SCREEN_WIDTH {
                let src = obj[i];
                let end = (i + mosaic_w).min(SCREEN_WIDTH);
                if src.color != TRANSPARENT {
                    obj[i + 1..end].fill(src);
                }
                i = end;
            }
        }
    }
}
