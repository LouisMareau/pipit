//! WebAssembly bindings. The web app talks to `Emulator` from a Web Worker.
//!
//! An `Emulator` is one or more consoles of one kind playing together (made with
//! `linked`; every console is emulated here and `local` is the one this player
//! sees and controls): Game Boy Advances or Game Boys on a cable, or DSs within
//! wireless range of each other. Frames are exposed as a
//! pointer into WASM memory (RGBA bytes, ready for an `ImageData`) so the
//! worker can copy them out without an intermediate `Vec`.

use pipit_gba::{Gba, Keys};
use pipit_gbc::Gbc;
use pipit_nds::{Nds, Slot2};
use wasm_bindgen::prelude::*;

/// `system` values the front-end passes: 0 is a Game Boy Advance.
const SYSTEM_GBC: u8 = 1;
const SYSTEM_NDS: u8 = 2;
/// Bits of a DS's input number that carry the pen (see `set_player_keys`).
const PEN_INCLUDED: u32 = 1 << 30;
const PEN_DOWN: u32 = 1 << 28;

/// One long-lived object either way: the size difference does not matter.
#[allow(clippy::large_enum_variant)]
enum Core {
    Gba(pipit_gba::Link),
    Gbc(pipit_gbc::Link),
    Nds(pipit_nds::Link),
}

/// A console's picture (the last console's, for a player index past the end).
fn framebuffer(core: &Core, player: usize) -> &[u32] {
    match core {
        Core::Gba(link) => link.nodes()[player.min(link.nodes().len() - 1)].framebuffer(),
        Core::Gbc(link) => link.nodes()[player.min(link.nodes().len() - 1)].framebuffer(),
        Core::Nds(link) => link.nodes()[player.min(link.nodes().len() - 1)].framebuffer(),
    }
}

/// 0xFFRRGGBB pixels to RGBA bytes, through the colour table when there is one.
fn convert(framebuffer: &[u32], rgba: &mut [u8], lut: Option<&[[u8; 4]]>) {
    let (pixels, _) = rgba.as_chunks_mut::<4>();
    if let Some(lut) = lut {
        for (px, out) in framebuffer.iter().zip(pixels) {
            *out = lut[pipit_gbc::color::lut_index(*px)];
        }
    } else {
        for (px, out) in framebuffer.iter().zip(pixels) {
            *out = [(px >> 16) as u8, (px >> 8) as u8, *px as u8, 0xFF];
        }
    }
}

#[wasm_bindgen]
pub struct Emulator {
    core: Core,
    local: usize,
    /// The console whose picture and sound are shown: the local one, or a partner's.
    view: usize,
    width: usize,
    height: usize,
    rgba: Vec<u8>,
    /// A second picture, of whichever console `render_side` was last asked for.
    side: Vec<u8>,
    /// Colour correction table, when the LCD look is on.
    color_lut: Option<Vec<[u8; 4]>>,
}

#[wasm_bindgen]
impl Emulator {
    /// Creates an emulator for the ROM bytes: `system` 0 is a Game Boy Advance
    /// (with an optional BIOS image), 1 a Game Boy (Color).
    #[wasm_bindgen(constructor)]
    pub fn new(rom: &[u8], bios: Option<Vec<u8>>, system: u8) -> Emulator {
        Self::linked(rom, 1, 0, bios, system, false)
    }

    /// `players` consoles on a link cable, all running this ROM; `local` is the
    /// one this player sees and controls. A Game Boy cable joins two.
    pub fn linked(
        rom: &[u8],
        players: usize,
        local: usize,
        bios: Option<Vec<u8>>,
        system: u8,
        wireless: bool,
    ) -> Emulator {
        let (core, width, height) = if system == SYSTEM_NDS {
            // Both screens, the top one above the bottom one.
            let nodes = (0..players).map(|_| Nds::new(rom.to_vec(), None, None, None)).collect();
            let link = pipit_nds::Link::new(nodes);
            (Core::Nds(link), pipit_nds::SCREEN_WIDTH, pipit_nds::SCREEN_HEIGHT * 2)
        } else if system == SYSTEM_GBC {
            let nodes = (0..players.min(2)).map(|_| Gbc::new(rom.to_vec())).collect();
            (
                Core::Gbc(pipit_gbc::Link::new(nodes)),
                pipit_gbc::SCREEN_WIDTH,
                pipit_gbc::SCREEN_HEIGHT,
            )
        } else {
            let nodes = (0..players).map(|_| Gba::new(rom.to_vec(), bios.clone())).collect();
            let link = if wireless {
                pipit_gba::Link::wireless(nodes)
            } else {
                pipit_gba::Link::new(nodes)
            };
            (Core::Gba(link), pipit_gba::SCREEN_WIDTH, pipit_gba::SCREEN_HEIGHT)
        };
        let local = local.min(players.max(1) - 1);
        Emulator {
            core,
            local,
            view: local,
            width,
            height,
            rgba: vec![0; width * height * 4],
            side: Vec::new(),
            color_lut: None,
        }
    }

    pub fn players(&self) -> usize {
        match &self.core {
            Core::Gba(link) => link.nodes().len(),
            Core::Gbc(link) => link.nodes().len(),
            Core::Nds(link) => link.nodes().len(),
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// Whether the game is a colour one (false for a classic Game Boy game).
    pub fn is_color(&self) -> bool {
        match &self.core {
            Core::Gba(_) => true,
            Core::Gbc(link) => link.nodes()[self.local].is_color(),
            Core::Nds(_) => true,
        }
    }

    /// Emulates until the next frame is complete and converts it to RGBA.
    pub fn run_frame(&mut self) {
        self.advance();
        self.refresh_frame();
    }

    fn advance(&mut self) {
        match &mut self.core {
            Core::Gba(link) if link.nodes().len() == 1 => link.nodes_mut()[0].run_frame(),
            Core::Gba(link) => link.run_frame(),
            Core::Gbc(link) => link.run_frame(),
            Core::Nds(link) if link.nodes().len() == 1 => link.nodes_mut()[0].run_frame(),
            Core::Nds(link) => link.run_frame(),
        }
    }

    /// Re-converts the current framebuffer to RGBA without running (after a state
    /// load or a colour-mode change).
    pub fn refresh_frame(&mut self) {
        convert(framebuffer(&self.core, self.view), &mut self.rgba, self.color_lut.as_deref());
    }

    /// Converts another console's picture into the side buffer (`side_ptr`), for
    /// showing partners' screens beside the player's own.
    pub fn render_side(&mut self, player: usize) {
        if self.side.len() != self.rgba.len() {
            self.side = vec![0; self.rgba.len()];
        }
        convert(framebuffer(&self.core, player), &mut self.side, self.color_lut.as_deref());
    }

    pub fn side_ptr(&self) -> *const u8 {
        self.side.as_ptr()
    }

    pub fn side_len(&self) -> usize {
        self.side.len()
    }

    /// 0 = raw colours, 1 = the LCD look of the game's console at `strength` (0.0-1.0).
    pub fn set_color_correction(&mut self, mode: u8, strength: f32) {
        let lcd = !matches!(self.core, Core::Nds(_));
        self.color_lut = if mode == 1 && strength > 0.0 && self.is_color() && lcd {
            Some(match &self.core {
                Core::Gba(_) => pipit_gba::video::color::lcd_lut_with_strength(f64::from(strength)),
                Core::Gbc(_) => pipit_gbc::color::lcd_lut_with_strength(f64::from(strength)),
                Core::Nds(_) => unreachable!("no LCD look for the DS"),
            })
        } else {
            None
        };
        self.refresh_frame();
    }

    /// Pointer to the RGBA frame inside WASM memory (`width` × `height` × 4).
    pub fn frame_ptr(&self) -> *const u8 {
        self.rgba.as_ptr()
    }

    pub fn frame_len(&self) -> usize {
        self.rgba.len()
    }

    /// Sets the keys held on this player's console (bit layout of `Keys`).
    pub fn set_keys(&mut self, keys: u32) {
        self.set_player_keys(self.local, keys);
    }

    /// Sets the keys held on any console of the link. For a DS the value may
    /// carry the pen too, as the web app packs it for play over a network:
    /// bit 30 says it does, bit 28 that the pen is down, bits 12–19 and 20–27
    /// where it is (x, y).
    pub fn set_player_keys(&mut self, player: usize, keys: u32) {
        match &mut self.core {
            Core::Gba(link) => link.nodes_mut()[player].set_keys(Keys(keys as u16 & 0x3FF)),
            // The Game Boy's eight keys are the Advance's low byte.
            Core::Gbc(link) => link.nodes_mut()[player].set_keys(keys as u8),
            Core::Nds(link) => {
                let node = &mut link.nodes_mut()[player];
                node.set_keys(pipit_nds::Keys(keys as u16 & 0xFFF));
                if keys & PEN_INCLUDED != 0 {
                    let down = keys & PEN_DOWN != 0;
                    node.set_touch(down.then_some(((keys >> 12) as u8, (keys >> 20) as u8)));
                }
            }
        }
    }

    /// The pen on this player's DS's bottom screen (x 0–255, y 0–191); a
    /// negative coordinate lifts it.
    pub fn set_touch(&mut self, x: i32, y: i32) {
        if let Core::Nds(link) = &mut self.core {
            let touch = (x >= 0 && y >= 0).then(|| (x.min(255) as u8, y.min(191) as u8));
            link.nodes_mut()[self.local].set_touch(touch);
        }
    }

    /// What is in the GBA slot of every DS of the link: 0 nothing, 1 the
    /// Rumble Pak, 2 the GBA cartridge `rom` with its `save`.
    pub fn set_slot2(&mut self, kind: u8, rom: Option<Vec<u8>>, save: Option<Vec<u8>>) {
        let Core::Nds(link) = &mut self.core else { return };
        for node in link.nodes_mut() {
            let mut slot = match kind {
                1 => Slot2::rumble_pak(),
                2 => Slot2::cartridge(rom.clone().unwrap_or_default()),
                _ => Slot2::Empty,
            };
            if let Some(save) = &save {
                slot.load_save_data(save);
            }
            node.set_slot2(slot);
        }
    }

    /// The save memory of the GBA cartridge in this player's DS, if any.
    pub fn slot2_save_data(&self) -> Option<Vec<u8>> {
        match &self.core {
            Core::Nds(link) => link.nodes()[self.local].slot2_save_data().map(<[u8]>::to_vec),
            _ => None,
        }
    }

    pub fn take_slot2_save_dirty(&mut self) -> bool {
        match &mut self.core {
            Core::Nds(link) => link.nodes_mut()[self.local].take_slot2_save_dirty(),
            _ => false,
        }
    }

    /// Whether the Rumble Pak in this player's DS moved since the last call.
    pub fn take_rumble(&mut self) -> bool {
        match &mut self.core {
            Core::Nds(link) => link.nodes_mut()[self.local].take_rumble(),
            _ => false,
        }
    }

    /// Takes the viewed console's audio since the last call (interleaved stereo i16
    /// at 32768 Hz); the other consoles' is dropped so it does not pile up.
    pub fn drain_audio(&mut self) -> Vec<i16> {
        let view = self.view;
        let mut out = Vec::new();
        match &mut self.core {
            Core::Gba(link) => {
                for (i, node) in link.nodes_mut().iter_mut().enumerate() {
                    let samples = node.drain_audio();
                    if i == view {
                        out = samples;
                    }
                }
            }
            Core::Gbc(link) => {
                for (i, node) in link.nodes_mut().iter_mut().enumerate() {
                    let samples = node.drain_audio();
                    if i == view {
                        out = samples;
                    }
                }
            }
            Core::Nds(link) => {
                for (i, node) in link.nodes_mut().iter_mut().enumerate() {
                    let samples = node.drain_audio();
                    if i == view {
                        out = samples;
                    }
                }
            }
        }
        out
    }

    /// Shows (and plays) another console of the link.
    pub fn set_view(&mut self, player: usize) {
        self.view = player.min(self.players() - 1);
        self.refresh_frame();
    }

    /// Runs one frame without converting the picture or keeping the sound: for
    /// re-simulating after a rollback.
    pub fn run_frame_silent(&mut self) {
        self.advance();
        match &mut self.core {
            Core::Gba(link) => link.nodes_mut().iter_mut().for_each(|node| {
                node.drain_audio();
            }),
            Core::Gbc(link) => link.nodes_mut().iter_mut().for_each(|node| {
                node.drain_audio();
            }),
            Core::Nds(link) => link.nodes_mut().iter_mut().for_each(|node| {
                node.drain_audio();
            }),
        }
    }

    /// Serializes every console of the link, to roll back to later.
    pub fn save_link_state(&self) -> Vec<u8> {
        match &self.core {
            Core::Gba(link) => link.save_state(),
            Core::Gbc(link) => link.save_state(),
            Core::Nds(link) => link.save_state(),
        }
    }

    pub fn load_link_state(&mut self, data: &[u8]) -> Result<(), JsError> {
        match &mut self.core {
            Core::Gba(link) => link.load_state(data),
            Core::Gbc(link) => link.load_state(data),
            Core::Nds(link) => link.load_state(data),
        }
        .map_err(|e| JsError::new(&e.to_string()))
    }

    pub fn save_data(&self) -> Option<Vec<u8>> {
        match &self.core {
            Core::Gba(link) => link.nodes()[self.local].save_data().map(<[u8]>::to_vec),
            Core::Gbc(link) => link.nodes()[self.local].save_data(),
            Core::Nds(link) => link.nodes()[self.local].save_data().map(<[u8]>::to_vec),
        }
    }

    pub fn load_save_data(&mut self, data: &[u8]) {
        self.load_player_save_data(self.local, data);
    }

    /// Restores another console's backup memory (a partner's save, on a link).
    pub fn load_player_save_data(&mut self, player: usize, data: &[u8]) {
        match &mut self.core {
            Core::Gba(link) => link.nodes_mut()[player].load_save_data(data),
            Core::Gbc(link) => link.nodes_mut()[player].load_save_data(data),
            Core::Nds(link) => link.nodes_mut()[player].load_save_data(data),
        }
    }

    pub fn take_save_dirty(&mut self) -> bool {
        match &mut self.core {
            Core::Gba(link) => link.nodes_mut()[self.local].take_save_dirty(),
            Core::Gbc(link) => link.nodes_mut()[self.local].take_save_dirty(),
            Core::Nds(link) => link.nodes_mut()[self.local].take_save_dirty(),
        }
    }

    /// Serializes this player's console.
    pub fn save_state(&self) -> Vec<u8> {
        match &self.core {
            Core::Gba(link) => link.nodes()[self.local].save_state(),
            Core::Gbc(link) => link.nodes()[self.local].save_state(),
            Core::Nds(link) => link.nodes()[self.local].save_state(),
        }
    }

    /// Restores a state from `save_state`; throws when it does not belong to this game.
    pub fn load_state(&mut self, data: &[u8]) -> Result<(), JsError> {
        match &mut self.core {
            Core::Gba(link) => link.nodes_mut()[self.local].load_state(data),
            Core::Gbc(link) => link.nodes_mut()[self.local].load_state(data),
            Core::Nds(link) => link.nodes_mut()[self.local].load_state(data),
        }
        .map_err(|e| JsError::new(&e.to_string()))
    }

    /// How much the in-game link has done since the start: cable transfers,
    /// "data ready" wake-ups from the adapters, or wireless frames. Flat while a
    /// game's link is stuck.
    pub fn link_activity(&self) -> u32 {
        match &self.core {
            Core::Gba(link) => (link.transfers() + link.wakes()) as u32,
            Core::Gbc(link) => link.transfers() as u32,
            Core::Nds(link) => link.frames() as u32,
        }
    }

    /// A 32-bit digest of every console's state, to check that linked players agree.
    pub fn state_hash(&self) -> u32 {
        let h = match &self.core {
            Core::Gba(link) => link.state_hash(),
            Core::Gbc(link) => link.state_hash(),
            Core::Nds(link) => link.state_hash(),
        };
        (h as u32) ^ ((h >> 32) as u32)
    }

    /// Sets every console's cartridge clock from a Unix timestamp in seconds.
    pub fn set_time(&mut self, unix_seconds: f64) {
        match &mut self.core {
            Core::Gba(link) => {
                link.nodes_mut().iter_mut().for_each(|node| node.set_time(unix_seconds as i64))
            }
            Core::Gbc(link) => {
                link.nodes_mut().iter_mut().for_each(|node| node.set_time(unix_seconds as i64))
            }
            Core::Nds(link) => {
                link.nodes_mut().iter_mut().for_each(|node| node.set_time(unix_seconds as i64))
            }
        }
    }

    pub fn title(&self) -> String {
        match &self.core {
            Core::Gba(link) => link.nodes()[self.local].title(),
            Core::Gbc(link) => link.nodes()[self.local].title(),
            Core::Nds(link) => link.nodes()[self.local].title().to_string(),
        }
    }

    pub fn game_code(&self) -> String {
        match &self.core {
            Core::Gba(link) => link.nodes()[self.local].game_code(),
            Core::Gbc(link) => {
                if link.nodes()[self.local].is_color() { "GBC" } else { "GB" }.to_string()
            }
            Core::Nds(link) => link.nodes()[self.local].game_code().to_string(),
        }
    }
}
