//! The 2D engines line by line: registers and VRAM are set directly, a line
//! is rendered, and the screens are checked pixel by pixel.

use super::vram::Region;
use super::*;

const RED: u16 = 0x001F;
const GREEN: u16 = 0x03E0;
const BLUE: u16 = 0x7C00;
const WHITE15: u16 = 0x7FFF;

fn c6(v: u32) -> u32 {
    (v << 1) | (v >> 4)
}

fn c8(v: u32) -> u32 {
    (v << 2) | (v >> 4)
}

/// A 15-bit colour as the screen shows it.
fn argb(c: u16) -> u32 {
    let ch = |s: u16| c8(c6(u32::from((c >> s) & 0x1F)));
    0xFF00_0000 | ch(0) << 16 | ch(5) << 8 | ch(10)
}

/// Two colours mixed at `eva`/16 and `evb`/16, as the screen shows it.
fn mix(a: u16, b: u16, eva: u32, evb: u32) -> u32 {
    let ch = |s: u16| {
        let ca = c6(u32::from((a >> s) & 0x1F));
        let cb = c6(u32::from((b >> s) & 0x1F));
        c8(((ca * eva + cb * evb) / 16).min(63))
    };
    0xFF00_0000 | ch(0) << 16 | ch(5) << 8 | ch(10)
}

/// A display with its banks laid out for tests: A = engine A's backgrounds,
/// B = its objects, C = engine B's backgrounds, D = its objects, E = engine
/// A's extended background palettes, F = its extended object palette, H and
/// I engine B's.
fn video() -> Video {
    let mut v = Video::new();
    for (bank, cnt) in
        [(0, 0x81), (1, 0x82), (2, 0x84), (3, 0x84), (4, 0x84), (5, 0x85), (7, 0x82), (8, 0x83)]
    {
        v.vram.write_cnt(bank, cnt);
    }
    v
}

fn set_pal(v: &mut Video, e: usize, index: usize, c: u16) {
    let at = e * 0x400 + index * 2;
    v.palette[at..at + 2].copy_from_slice(&c.to_le_bytes());
}

fn set_obj_pal(v: &mut Video, e: usize, index: usize, c: u16) {
    set_pal(v, e, 0x100 + index, c);
}

fn oam(v: &mut Video, e: usize, n: usize, attrs: [u16; 3]) {
    for (i, a) in attrs.iter().enumerate() {
        let at = e * 0x400 + n * 8 + i * 2;
        v.oam[at..at + 2].copy_from_slice(&a.to_le_bytes());
    }
}

fn reg(v: &mut Video, e: usize, reg: u32, value: u16) {
    v.engines[e].write_reg(reg, value, 0xFFFF);
}

/// Display mode 1 with `mode` and the layers in `layers` (bit 0 BG0 … bit 4 OBJ).
fn graphics(v: &mut Video, e: usize, mode: u32, layers: u32) {
    v.engines[e].dispcnt = 1 << 16 | mode | layers << 8;
}

fn px(v: &Video, screen: usize, x: usize, y: usize) -> u32 {
    v.screens[screen][y * SCREEN_WIDTH + x]
}

/// A solid 8×8 tile of colour `index`, 16-colour form, at `at`.
fn tile16(v: &mut Video, region: Region, at: usize, index: u8) {
    for i in 0..16 {
        v.vram.write16(region, at + i * 2, u16::from(index) * 0x1111);
    }
}

/// The same in 256-colour form.
fn tile256(v: &mut Video, region: Region, at: usize, index: u8) {
    for i in 0..32 {
        v.vram.write16(region, at + i * 2, u16::from(index) * 0x0101);
    }
}

/// Fills a 32×32 map at `map` with `entry`.
fn fill_map(v: &mut Video, region: Region, map: usize, entry: u16) {
    for i in 0..1024 {
        v.vram.write16(region, map + i * 2, entry);
    }
}

#[test]
fn text_background_16_colours_flips_and_scrolls() {
    let mut v = video();
    graphics(&mut v, 0, 0, 1);
    // BG0: tiles at 4000h, map at 0, 16 colours.
    reg(&mut v, 0, 0x008, 1 << 2);
    tile16(&mut v, Region::BgA, 0x4000 + 32, 3);
    // Tile 2: only its first column is set.
    for row in 0..8 {
        v.vram.write16(Region::BgA, 0x4000 + 64 + row * 4, 0x0001);
    }
    v.vram.write16(Region::BgA, 0, 1 | 2 << 12); // (0,0): tile 1, palette 2
    v.vram.write16(Region::BgA, 2, 2 | 1 << 10 | 1 << 12); // (1,0): tile 2 flipped, palette 1
    set_pal(&mut v, 0, 2 * 16 + 3, BLUE);
    set_pal(&mut v, 0, 16 + 1, RED);
    set_pal(&mut v, 0, 0, GREEN);
    v.render_line(0);
    assert_eq!(px(&v, 0, 0, 0), argb(BLUE));
    assert_eq!(px(&v, 0, 7, 0), argb(BLUE));
    assert_eq!(px(&v, 0, 8, 0), argb(GREEN));
    assert_eq!(px(&v, 0, 15, 0), argb(RED));
    assert_eq!(px(&v, 0, 16, 0), argb(GREEN));
    // Scrolling by 4 moves everything left.
    reg(&mut v, 0, 0x010, 4);
    v.render_line(0);
    assert_eq!(px(&v, 0, 3, 0), argb(BLUE));
    assert_eq!(px(&v, 0, 4, 0), argb(GREEN));
    assert_eq!(px(&v, 0, 11, 0), argb(RED));
}

#[test]
fn text_background_extended_palette() {
    let mut v = video();
    v.engines[0].dispcnt = 1 << 16 | 1 << 30 | 1 << 9;
    // BG1: 256 colours, slot 3 (bit 13), tiles at 4000h.
    reg(&mut v, 0, 0x00A, 1 << 2 | 1 << 7 | 1 << 13);
    tile256(&mut v, Region::BgA, 0x4000 + 64, 7);
    v.vram.write16(Region::BgA, 0, 1 | 5 << 12);
    v.vram.write16(Region::BgExtPalA, 3 * 0x2000 + 5 * 0x200 + 7 * 2, GREEN);
    set_pal(&mut v, 0, 7, RED);
    v.render_line(0);
    assert_eq!(px(&v, 0, 0, 0), argb(GREEN));
    // Without the extended palettes the standard one applies.
    v.engines[0].dispcnt &= !(1 << 30);
    v.render_line(0);
    assert_eq!(px(&v, 0, 0, 0), argb(RED));
}

#[test]
fn affine_background_transforms() {
    let mut v = video();
    graphics(&mut v, 0, 2, 1 << 2);
    // BG2: 8-bit entries, 128 px, tiles at 4000h, map at 800h.
    reg(&mut v, 0, 0x00C, 1 << 2 | 1 << 8);
    tile256(&mut v, Region::BgA, 0x4000 + 64, 9);
    v.vram.write16(Region::BgA, 0x800, 0x0001);
    set_pal(&mut v, 0, 9, RED);
    set_pal(&mut v, 0, 0, BLUE);
    v.render_line(0);
    assert_eq!(px(&v, 0, 0, 0), argb(RED));
    assert_eq!(px(&v, 0, 7, 0), argb(RED));
    assert_eq!(px(&v, 0, 8, 0), argb(BLUE));
    // Doubling PA samples every other map pixel: the tile ends at x = 4.
    reg(&mut v, 0, 0x020, 0x200);
    v.render_line(0);
    assert_eq!(px(&v, 0, 3, 0), argb(RED));
    assert_eq!(px(&v, 0, 4, 0), argb(BLUE));
    // A reference point at x = 8 scrolls the tile off the left edge; with
    // wraparound it comes back at 120.
    reg(&mut v, 0, 0x020, 0x100);
    reg(&mut v, 0, 0x028, 8 << 8);
    v.render_line(0);
    assert_eq!(px(&v, 0, 0, 0), argb(BLUE));
    assert_eq!(px(&v, 0, 120, 0), argb(BLUE));
    reg(&mut v, 0, 0x00C, 1 << 2 | 1 << 8 | 1 << 13);
    v.render_line(0);
    assert_eq!(px(&v, 0, 120, 0), argb(RED));
}

#[test]
fn extended_backgrounds_bitmaps_and_tiles() {
    let mut v = video();
    graphics(&mut v, 0, 5, 1 << 2 | 1 << 3);
    // BG2: direct-colour bitmap, 128 × 128 at 8000h.
    reg(&mut v, 0, 0x00C, 1 << 7 | 1 << 2 | 2 << 8);
    v.vram.write16(Region::BgA, 0x8000 + 3 * 2, 0x8000 | RED);
    v.vram.write16(Region::BgA, 0x8000 + 4 * 2, GREEN); // alpha bit clear: transparent
                                                        // BG3: 8-bit bitmap at C000h, priority 1 under BG2.
    reg(&mut v, 0, 0x00E, 1 << 7 | 3 << 8 | 1);
    v.vram.write16(Region::BgA, 0xC000 + 4, 0x0006); // pixel (4,0) = colour 6
    set_pal(&mut v, 0, 6, BLUE);
    set_pal(&mut v, 0, 0, WHITE15);
    v.render_line(0);
    assert_eq!(px(&v, 0, 3, 0), argb(RED));
    assert_eq!(px(&v, 0, 4, 0), argb(BLUE));
    assert_eq!(px(&v, 0, 5, 0), argb(WHITE15));
    // BG2 as tiles with 16-bit entries: tile 1, flipped, in the standard palette.
    reg(&mut v, 0, 0x00C, 1 << 2 | 1 << 8);
    tile256(&mut v, Region::BgA, 0x4000 + 64, 6);
    v.vram.write16(Region::BgA, 0x800, 1 | 1 << 10);
    v.render_line(0);
    assert_eq!(px(&v, 0, 0, 0), argb(BLUE));
    assert_eq!(px(&v, 0, 7, 0), argb(BLUE));
    assert_eq!(px(&v, 0, 8, 0), argb(WHITE15));
}

#[test]
fn large_bitmap_mode_6() {
    let mut v = video();
    graphics(&mut v, 0, 6, 1 << 2);
    reg(&mut v, 0, 0x00C, 0);
    v.vram.write16(Region::BgA, 6, 0x0002); // pixel (6, 0) of a 512-wide bitmap
    set_pal(&mut v, 0, 2, GREEN);
    v.render_line(0);
    assert_eq!(px(&v, 0, 6, 0), argb(GREEN));
    assert_eq!(px(&v, 0, 7, 0), argb(0));
}

#[test]
fn objects_tiles_and_priorities() {
    let mut v = video();
    graphics(&mut v, 0, 0, 1 << 4 | 1);
    v.engines[0].dispcnt |= 1 << 4; // 1D mapping, 32-byte boundary
                                    // BG0 covers the screen in blue at priority 1.
    reg(&mut v, 0, 0x008, 1 << 2 | 1);
    tile16(&mut v, Region::BgA, 0x4000 + 32, 1);
    fill_map(&mut v, Region::BgA, 0, 1);
    set_pal(&mut v, 0, 1, BLUE);
    // Object 0: 8 × 8, 16 colours, tile 4, palette 1, priority 0, at x = 10.
    tile16(&mut v, Region::ObjA, 4 * 32, 5);
    oam(&mut v, 0, 0, [0, 10, 4 | 1 << 12]);
    set_obj_pal(&mut v, 0, 16 + 5, RED);
    // Object 1: 256 colours at x = 30, priority 2: under the background.
    tile256(&mut v, Region::ObjA, 8 * 32, 9);
    oam(&mut v, 0, 1, [1 << 13, 30, 8 | 2 << 10]);
    set_obj_pal(&mut v, 0, 9, GREEN);
    v.render_line(0);
    assert_eq!(px(&v, 0, 9, 0), argb(BLUE));
    assert_eq!(px(&v, 0, 10, 0), argb(RED));
    assert_eq!(px(&v, 0, 17, 0), argb(RED));
    assert_eq!(px(&v, 0, 18, 0), argb(BLUE));
    assert_eq!(px(&v, 0, 30, 0), argb(BLUE));
    // Turning BG0 off reveals the second object.
    v.engines[0].dispcnt &= !(1 << 8);
    v.render_line(0);
    assert_eq!(px(&v, 0, 30, 0), argb(GREEN));
}

#[test]
fn object_extended_palette() {
    let mut v = video();
    graphics(&mut v, 0, 0, 1 << 4);
    v.engines[0].dispcnt |= 1 << 31;
    tile256(&mut v, Region::ObjA, 64, 3); // tile 2 in 2D mapping
    oam(&mut v, 0, 0, [1 << 13, 0, 2 | 4 << 12]);
    v.vram.write16(Region::ObjExtPalA, 4 * 0x200 + 3 * 2, GREEN);
    set_obj_pal(&mut v, 0, 3, RED);
    v.render_line(0);
    assert_eq!(px(&v, 0, 0, 0), argb(GREEN));
}

#[test]
fn bitmap_object_blends_with_its_alpha() {
    let mut v = video();
    graphics(&mut v, 0, 0, 1 << 4);
    v.engines[0].dispcnt |= 1 << 6; // 1D bitmaps, 128-byte boundary
                                    // A direct-colour 8 × 8 object at tile 1 (byte 128), alpha 7, over a blue backdrop.
    for i in 0..64 {
        v.vram.write16(Region::ObjA, 128 + i * 2, 0x8000 | RED);
    }
    v.vram.write16(Region::ObjA, 128 + 2, RED); // pixel (1,0): alpha bit clear
    oam(&mut v, 0, 0, [3 << 10, 0, 1 | 7 << 12]);
    set_pal(&mut v, 0, 0, BLUE);
    // Blending with the backdrop as second target: EVA = alpha + 1.
    reg(&mut v, 0, 0x050, 1 << 13);
    v.render_line(0);
    assert_eq!(px(&v, 0, 0, 0), mix(RED, BLUE, 8, 8));
    assert_eq!(px(&v, 0, 1, 0), argb(BLUE));
    // Without a second target the object is drawn as is.
    reg(&mut v, 0, 0x050, 0);
    v.render_line(0);
    assert_eq!(px(&v, 0, 0, 0), argb(RED));
}

#[test]
fn windows_and_object_window() {
    let mut v = video();
    graphics(&mut v, 0, 0, 1 << 4 | 1);
    v.engines[0].dispcnt |= 1 << 13 | 1 << 15; // window 0 and the object window
    reg(&mut v, 0, 0x008, 1 << 2);
    tile16(&mut v, Region::BgA, 0x4000 + 32, 1);
    fill_map(&mut v, Region::BgA, 0, 1);
    set_pal(&mut v, 0, 1, RED);
    set_pal(&mut v, 0, 0, WHITE15);
    // Window 0 covers x 4..8 on line 0 and shows BG0; outside and the object
    // window show nothing.
    reg(&mut v, 0, 0x040, 4 << 8 | 8);
    reg(&mut v, 0, 0x044, 1);
    reg(&mut v, 0, 0x048, 1);
    reg(&mut v, 0, 0x04A, 0);
    // A window object (mode 2) over x 6..14.
    tile16(&mut v, Region::ObjA, 32, 1);
    oam(&mut v, 0, 0, [2 << 10, 6, 1]);
    v.render_line(0);
    assert_eq!(px(&v, 0, 3, 0), argb(WHITE15));
    assert_eq!(px(&v, 0, 4, 0), argb(RED));
    assert_eq!(px(&v, 0, 7, 0), argb(RED)); // window 0 beats the object window
    assert_eq!(px(&v, 0, 8, 0), argb(WHITE15));
    // Letting the object window show BG0 reveals x 8..14.
    reg(&mut v, 0, 0x04A, 1 << 8);
    v.render_line(0);
    assert_eq!(px(&v, 0, 8, 0), argb(RED));
    assert_eq!(px(&v, 0, 13, 0), argb(RED));
    assert_eq!(px(&v, 0, 14, 0), argb(WHITE15));
}

#[test]
fn colour_effects_between_backgrounds() {
    let mut v = video();
    graphics(&mut v, 0, 0, 0b11);
    reg(&mut v, 0, 0x008, 1 << 2); // BG0: priority 0, red
    reg(&mut v, 0, 0x00A, 1 << 2 | 1 << 8 | 1); // BG1: map at 800h, priority 1, blue
    tile16(&mut v, Region::BgA, 0x4000 + 32, 1);
    tile16(&mut v, Region::BgA, 0x4000 + 64, 2);
    fill_map(&mut v, Region::BgA, 0, 1);
    fill_map(&mut v, Region::BgA, 0x800, 2);
    set_pal(&mut v, 0, 1, RED);
    set_pal(&mut v, 0, 2, BLUE);
    // Alpha blending BG0 over BG1 at 8/16 each.
    reg(&mut v, 0, 0x050, 1 << 6 | 1 | 1 << 9);
    reg(&mut v, 0, 0x052, 8 | 8 << 8);
    v.render_line(0);
    assert_eq!(px(&v, 0, 0, 0), mix(RED, BLUE, 8, 8));
    // Brightness up on BG0: red gains half of what it lacks.
    reg(&mut v, 0, 0x050, 2 << 6 | 1);
    reg(&mut v, 0, 0x054, 8);
    v.render_line(0);
    assert_eq!(px(&v, 0, 0, 0), mix(RED, WHITE15, 8, 8));
    // Brightness down to black.
    reg(&mut v, 0, 0x050, 3 << 6 | 1);
    reg(&mut v, 0, 0x054, 16);
    v.render_line(0);
    assert_eq!(px(&v, 0, 0, 0), 0xFF00_0000);
}

#[test]
fn master_brightness_fades_the_line() {
    let mut v = video();
    graphics(&mut v, 0, 0, 0);
    set_pal(&mut v, 0, 0, RED);
    reg(&mut v, 0, 0x06C, 2 << 14 | 16);
    v.render_line(0);
    assert_eq!(px(&v, 0, 0, 0), 0xFF00_0000);
    reg(&mut v, 0, 0x06C, 1 << 14 | 16);
    v.render_line(0);
    assert_eq!(px(&v, 0, 0, 0), 0xFFFF_FFFF);
    reg(&mut v, 0, 0x06C, 1 << 14 | 8);
    v.render_line(0);
    assert_eq!(px(&v, 0, 0, 0), mix(RED, WHITE15, 8, 8));
}

#[test]
fn vram_display_and_fifo_modes() {
    let mut v = video();
    // Display mode 2 shows bank C (block 2) as a bitmap.
    v.engines[0].dispcnt = 2 << 16 | 2 << 18;
    v.vram.write_bank16(2, 512 + 10 * 2, GREEN); // pixel (10, 1)
    v.render_line(1);
    assert_eq!(px(&v, 0, 10, 1), argb(GREEN));
    assert_eq!(px(&v, 0, 11, 1), argb(0));
    // Display mode 3 shows what the FIFO received for the line.
    v.engines[0].dispcnt = 3 << 16;
    v.push_fifo(u32::from(RED) | u32::from(BLUE) << 16);
    v.render_line(0);
    assert_eq!(px(&v, 0, 0, 0), argb(RED));
    assert_eq!(px(&v, 0, 1, 0), argb(BLUE));
}

#[test]
fn capture_writes_the_graphics_into_vram() {
    let mut v = video();
    graphics(&mut v, 0, 0, 0);
    set_pal(&mut v, 0, 0, RED);
    // Source A (the graphics) into bank B at offset 8000h, 256 × 192.
    v.dispcapcnt = 1 << 31 | 3 << 20 | 1 << 16 | 1 << 18;
    v.render_line(0);
    assert_eq!(v.vram.read_bank16(1, 0x8000 + 4), 0x8000 | RED);
    // Source B (bank A, DISPCNT's block 0) blended with A: half red, half blue.
    v.vram.write_bank16(0, 4, 0x8000 | BLUE);
    v.dispcapcnt = 1 << 31 | 3 << 20 | 1 << 16 | 1 << 18 | 2 << 29 | 8 | 8 << 8;
    v.render_line(0);
    assert_eq!(v.vram.read_bank16(1, 0x8000 + 4), 0x8000 | 0x0F | 0x3C00);
    // The capture ends with the frame.
    let mut scheduler = crate::scheduler::Scheduler::new();
    v.vcount = 191;
    v.on_hblank_end(0, &mut scheduler);
    assert_eq!(v.dispcapcnt & (1 << 31), 0);
}

#[test]
fn engine_b_draws_on_the_other_screen() {
    let mut v = video();
    graphics(&mut v, 1, 0, 1);
    reg(&mut v, 1, 0x008, 1 << 2);
    tile16(&mut v, Region::BgB, 0x4000 + 32, 1);
    v.vram.write16(Region::BgB, 0, 1);
    set_pal(&mut v, 1, 1, GREEN);
    set_pal(&mut v, 1, 0, BLUE);
    v.render_line(0);
    assert_eq!(px(&v, 1, 0, 0), argb(GREEN));
    assert_eq!(px(&v, 1, 8, 0), argb(BLUE));
    // Swapping the screens puts engine B on top.
    v.powcnt1 &= !(1 << 15);
    v.render_line(0);
    assert_eq!(px(&v, 0, 0, 0), argb(GREEN));
}
