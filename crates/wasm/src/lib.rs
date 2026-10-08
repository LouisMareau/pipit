//! WebAssembly bindings. The web app talks to `Emulator` from a Web Worker.
//!
//! Frames are exposed as a pointer into WASM memory (RGBA bytes, ready for an
//! `ImageData`) so the worker can copy them out without an intermediate `Vec`.

use pipit_gba::{Gba, Keys, SCREEN_HEIGHT, SCREEN_WIDTH};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct Emulator {
    gba: Gba,
    rgba: Vec<u8>,
    /// Reused for `snapshot`, so rewind captures allocate nothing.
    state: Vec<u8>,
}

#[wasm_bindgen]
impl Emulator {
    /// Creates an emulator for the given ROM bytes and optional BIOS image.
    #[wasm_bindgen(constructor)]
    pub fn new(rom: &[u8], bios: Option<Vec<u8>>) -> Emulator {
        Emulator {
            gba: Gba::new(rom.to_vec(), bios),
            rgba: vec![0; SCREEN_WIDTH * SCREEN_HEIGHT * 4],
            state: Vec::new(),
        }
    }

    /// Emulates until the next frame is complete and converts it to RGBA.
    pub fn run_frame(&mut self) {
        self.gba.run_frame();
        self.refresh_frame();
    }

    /// Re-converts the current framebuffer to RGBA without running (after a state load).
    pub fn refresh_frame(&mut self) {
        let (pixels, _) = self.rgba.as_chunks_mut::<4>();
        for (px, out) in self.gba.framebuffer().iter().zip(pixels) {
            out[0] = (px >> 16) as u8;
            out[1] = (px >> 8) as u8;
            out[2] = *px as u8;
            out[3] = 0xFF;
        }
    }

    /// Pointer to the 240×160×4 RGBA frame inside WASM memory.
    pub fn frame_ptr(&self) -> *const u8 {
        self.rgba.as_ptr()
    }

    pub fn frame_len(&self) -> usize {
        self.rgba.len()
    }

    /// Sets the held keys (bit layout of `Keys`).
    pub fn set_keys(&mut self, keys: u16) {
        self.gba.set_keys(Keys(keys));
    }

    /// Takes the audio produced since the last call: interleaved stereo i16 at 32768 Hz.
    pub fn drain_audio(&mut self) -> Vec<i16> {
        self.gba.drain_audio()
    }

    pub fn save_data(&self) -> Option<Vec<u8>> {
        self.gba.save_data().map(<[u8]>::to_vec)
    }

    pub fn load_save_data(&mut self, data: &[u8]) {
        self.gba.load_save_data(data);
    }

    pub fn take_save_dirty(&mut self) -> bool {
        self.gba.take_save_dirty()
    }

    /// Serializes the whole machine state.
    pub fn save_state(&self) -> Vec<u8> {
        self.gba.save_state()
    }

    /// Serializes the state into an internal buffer and returns its length; read
    /// it through `state_ptr` before the next call. No allocation after the first.
    pub fn snapshot(&mut self) -> usize {
        self.gba.save_state_into(&mut self.state);
        self.state.len()
    }

    pub fn state_ptr(&self) -> *const u8 {
        self.state.as_ptr()
    }

    /// Restores a state from `save_state`; throws when it does not belong to this game.
    pub fn load_state(&mut self, data: &[u8]) -> Result<(), JsError> {
        self.gba.load_state(data).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Sets the cartridge clock from a Unix timestamp in seconds.
    pub fn set_time(&mut self, unix_seconds: f64) {
        self.gba.set_time(unix_seconds as i64);
    }

    pub fn title(&self) -> String {
        self.gba.title()
    }

    pub fn game_code(&self) -> String {
        self.gba.game_code()
    }
}
