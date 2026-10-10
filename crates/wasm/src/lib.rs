//! WebAssembly bindings. The web app talks to `Emulator` from a Web Worker.
//!
//! An `Emulator` is one console, or several on a link cable when made with
//! `linked`: every console is emulated here, and `local` is the one this player
//! sees and controls. Frames are exposed as a pointer into WASM memory (RGBA
//! bytes, ready for an `ImageData`) so the worker can copy them out without an
//! intermediate `Vec`.

use pipit_gba::video::color;
use pipit_gba::{Gba, Keys, Link, SCREEN_HEIGHT, SCREEN_WIDTH};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct Emulator {
    link: Link,
    local: usize,
    /// The console whose picture and sound are shown: the local one, or a partner's.
    view: usize,
    rgba: Vec<u8>,
    /// Colour correction table, when the LCD look is on.
    color_lut: Option<Vec<[u8; 4]>>,
}

#[wasm_bindgen]
impl Emulator {
    /// Creates an emulator for the given ROM bytes and optional BIOS image.
    #[wasm_bindgen(constructor)]
    pub fn new(rom: &[u8], bios: Option<Vec<u8>>) -> Emulator {
        Self::linked(rom, 1, 0, bios)
    }

    /// `players` consoles on a link cable, all running this ROM; `local` is the
    /// one this player sees and controls.
    pub fn linked(rom: &[u8], players: usize, local: usize, bios: Option<Vec<u8>>) -> Emulator {
        let nodes = (0..players).map(|_| Gba::new(rom.to_vec(), bios.clone())).collect();
        Emulator {
            link: Link::new(nodes),
            local,
            view: local,
            rgba: vec![0; SCREEN_WIDTH * SCREEN_HEIGHT * 4],
            color_lut: None,
        }
    }

    fn gba(&self) -> &Gba {
        &self.link.nodes()[self.local]
    }

    fn gba_mut(&mut self) -> &mut Gba {
        &mut self.link.nodes_mut()[self.local]
    }

    pub fn players(&self) -> usize {
        self.link.nodes().len()
    }

    /// Emulates until the next frame is complete and converts it to RGBA.
    pub fn run_frame(&mut self) {
        if self.players() == 1 {
            self.gba_mut().run_frame();
        } else {
            self.link.run_frame();
        }
        self.refresh_frame();
    }

    /// Re-converts the current framebuffer to RGBA without running (after a state
    /// load or a colour-mode change).
    pub fn refresh_frame(&mut self) {
        let framebuffer = self.link.nodes()[self.view].framebuffer();
        let (pixels, _) = self.rgba.as_chunks_mut::<4>();
        if let Some(lut) = &self.color_lut {
            for (px, out) in framebuffer.iter().zip(pixels) {
                *out = lut[color::lut_index(*px)];
            }
        } else {
            for (px, out) in framebuffer.iter().zip(pixels) {
                *out = [(px >> 16) as u8, (px >> 8) as u8, *px as u8, 0xFF];
            }
        }
    }

    /// 0 = raw colours, 1 = the GBA LCD look at `strength` (0.0-1.0).
    pub fn set_color_correction(&mut self, mode: u8, strength: f32) {
        self.color_lut = if mode == 1 && strength > 0.0 {
            Some(color::lcd_lut_with_strength(f64::from(strength)))
        } else {
            None
        };
        self.refresh_frame();
    }

    /// Pointer to the 240×160×4 RGBA frame inside WASM memory.
    pub fn frame_ptr(&self) -> *const u8 {
        self.rgba.as_ptr()
    }

    pub fn frame_len(&self) -> usize {
        self.rgba.len()
    }

    /// Sets the keys held on this player's console (bit layout of `Keys`).
    pub fn set_keys(&mut self, keys: u16) {
        self.gba_mut().set_keys(Keys(keys));
    }

    /// Sets the keys held on any console of the link.
    pub fn set_player_keys(&mut self, player: usize, keys: u16) {
        self.link.nodes_mut()[player].set_keys(Keys(keys));
    }

    /// Takes the viewed console's audio since the last call (interleaved stereo i16
    /// at 32768 Hz); the other consoles' is dropped so it does not pile up.
    pub fn drain_audio(&mut self) -> Vec<i16> {
        let view = self.view;
        let mut out = Vec::new();
        for (i, node) in self.link.nodes_mut().iter_mut().enumerate() {
            let samples = node.drain_audio();
            if i == view {
                out = samples;
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
        if self.players() == 1 {
            self.gba_mut().run_frame();
        } else {
            self.link.run_frame();
        }
        for node in self.link.nodes_mut() {
            node.drain_audio();
        }
    }

    /// Serializes every console of the link, to roll back to later.
    pub fn save_link_state(&self) -> Vec<u8> {
        self.link.save_state()
    }

    pub fn load_link_state(&mut self, data: &[u8]) -> Result<(), JsError> {
        self.link.load_state(data).map_err(|e| JsError::new(&e.to_string()))
    }

    pub fn save_data(&self) -> Option<Vec<u8>> {
        self.gba().save_data().map(<[u8]>::to_vec)
    }

    pub fn load_save_data(&mut self, data: &[u8]) {
        self.gba_mut().load_save_data(data);
    }

    /// Restores another console's backup memory (a partner's save, on a link).
    pub fn load_player_save_data(&mut self, player: usize, data: &[u8]) {
        self.link.nodes_mut()[player].load_save_data(data);
    }

    pub fn take_save_dirty(&mut self) -> bool {
        self.gba_mut().take_save_dirty()
    }

    /// Serializes this player's console.
    pub fn save_state(&self) -> Vec<u8> {
        self.gba().save_state()
    }

    /// Restores a state from `save_state`; throws when it does not belong to this game.
    pub fn load_state(&mut self, data: &[u8]) -> Result<(), JsError> {
        self.gba_mut().load_state(data).map_err(|e| JsError::new(&e.to_string()))
    }

    /// A 32-bit digest of every console's state, to check that linked players agree.
    pub fn state_hash(&self) -> u32 {
        let h = self.link.state_hash();
        (h as u32) ^ ((h >> 32) as u32)
    }

    /// Sets every console's cartridge clock from a Unix timestamp in seconds.
    pub fn set_time(&mut self, unix_seconds: f64) {
        for node in self.link.nodes_mut() {
            node.set_time(unix_seconds as i64);
        }
    }

    pub fn title(&self) -> String {
        self.gba().title()
    }

    pub fn game_code(&self) -> String {
        self.gba().game_code()
    }
}
