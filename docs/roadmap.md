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
- [x] Save states (exact: restored runs continue bit-identically)
- [x] ROM prefetch buffer timing
- [ ] Cheat codes (GameShark / CodeBreaker)
- [ ] Link cable (serial) — not planned for now

## Web app

- [x] Load a ROM from the device, play with keyboard
- [x] Touch controls (built; needs a round of testing on real phones)
- [x] Controller support: auto-detect, toolbar toggle (click = on/off, hold = pick among several)
- [x] Audio output (AudioWorklet)
- [x] Save data persistence (IndexedDB), .sav export/import
- [x] Fast-forward, screenshots
- [x] Installable PWA, offline
- [x] Save state slots (3 slots, Shift+F1–F3 / F1–F3)
- [x] Rewind (hold R or the ⟲ touch button; length configurable)
- [ ] Remappable controls
- [ ] Colour correction / LCD look
- [ ] Deployed online

## Desktop

- [x] Tauri shell (`desktop/`, installers via `npm run build`)
- [ ] Native file associations (.gba double-click)
- [ ] Code-signed releases
