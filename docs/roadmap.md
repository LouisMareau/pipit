# Roadmap

Checked items are done and covered by tests.

## Core

- [x] ARM7TDMI: ARM instruction set (`gba-tests/arm`, FuzzARM)
- [x] ARM7TDMI: Thumb instruction set (`gba-tests/thumb`, FuzzARM)
- [x] Memory map, mirrors, wait states (`gba-tests/memory`)
- [x] Interrupts, halt, BIOS HLE (`gba-tests/bios`)
- [x] Video modes 0–5, sprites, windows, blending (`gba-tests/ppu`)
- [x] DMA, timers
- [x] Audio: PSG + Direct Sound
- [x] Saves: SRAM, Flash 64K/128K, EEPROM 4K/64K (`gba-tests/save`)
- [x] RTC
- [x] Open-bus behaviour (`gba-tests/unsafe`)
- [ ] ROM prefetch buffer timing
- [ ] Save states
- [ ] Cheat codes (GameShark / CodeBreaker)

## Web app

- [x] Load a ROM from the device, play with keyboard
- [x] Touch controls (built; needs a round of testing on real phones)
- [x] Gamepad support
- [x] Audio output (AudioWorklet)
- [x] Save data persistence (IndexedDB), .sav export/import
- [x] Fast-forward, screenshots
- [x] Installable PWA, offline
- [ ] Save state slots (needs core save states)
- [ ] Rewind
- [ ] Remappable controls
- [ ] Colour correction / LCD look
- [ ] Deployed online

## Desktop

- [ ] Tauri shell
