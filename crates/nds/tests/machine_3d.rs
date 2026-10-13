//! The 3D engine through the machine: registers written as the ARM9 would
//! write them, a swap at VBlank, the frame on engine A's BG0.

use pipit_arm::Bus as _;
use pipit_nds::memory::View9;
use pipit_nds::Nds;

/// A card with a header and an ARM9 program that only loops.
fn looping_rom() -> Vec<u8> {
    let mut rom = vec![0u8; 0x1000];
    rom[..12].copy_from_slice(b"3D TEST\0\0\0\0\0");
    rom[12..16].copy_from_slice(b"TEST");
    let word =
        |rom: &mut Vec<u8>, at: usize, v: u32| rom[at..at + 4].copy_from_slice(&v.to_le_bytes());
    word(&mut rom, 0x20, 0x200); // ARM9: offset, entry, address, size
    word(&mut rom, 0x24, 0x0200_0000);
    word(&mut rom, 0x28, 0x0200_0000);
    word(&mut rom, 0x2C, 4);
    word(&mut rom, 0x30, 0x300); // ARM7
    word(&mut rom, 0x34, 0x0380_0000);
    word(&mut rom, 0x38, 0x0380_0000);
    word(&mut rom, 0x3C, 4);
    word(&mut rom, 0x200, 0xEAFF_FFFE); // b .
    word(&mut rom, 0x300, 0xEAFF_FFFE);
    rom
}

#[test]
fn a_triangle_reaches_the_top_screen() {
    let mut nds = Nds::new(looping_rom(), None, None, None);
    {
        let mut v = View9(&mut nds.bus);
        v.write16(0x0400_0304, 0x820F, false); // engines on, A on top
        v.write32(0x0400_0000, 1 << 16 | 1 << 8 | 1 << 3, false); // graphics, BG0 as the 3D layer
        v.write16(0x0400_0060, 1, false); // DISP3DCNT: textures (irrelevant here)
        v.write16(0x0400_0354, 0x7FFF, false); // CLEAR_DEPTH: the rear plane at the back
        let cmd = |v: &mut View9, c: u32, params: &[u32]| {
            let reg = 0x0400_0400 + c * 4;
            if params.is_empty() {
                v.write32(reg, 0, false);
            }
            for &p in params {
                v.write32(reg, p, false);
            }
        };
        cmd(&mut v, 0x60, &[0xBFFF_0000]);
        cmd(&mut v, 0x10, &[0]);
        cmd(&mut v, 0x15, &[]);
        cmd(&mut v, 0x10, &[2]);
        cmd(&mut v, 0x15, &[]);
        cmd(&mut v, 0x29, &[0x1F << 16 | 3 << 6]);
        cmd(&mut v, 0x40, &[0]);
        cmd(&mut v, 0x20, &[0x03E0]); // green
        for (x, y) in [(0xF000u32, 0xF000u32), (0x1000, 0xF000), (0, 0x1000)] {
            cmd(&mut v, 0x23, &[x | y << 16, 0]);
        }
        cmd(&mut v, 0x41, &[]);
        cmd(&mut v, 0x50, &[0]);
    }
    // The swap happens at the next VBlank; the frame after shows it.
    nds.run_frame();
    nds.run_frame();
    let top = nds.screens()[0];
    assert_eq!(top[100 * 256 + 128], 0xFF00_FF00);
    assert_eq!(top[10 * 256 + 10], 0xFF00_0000);
}
