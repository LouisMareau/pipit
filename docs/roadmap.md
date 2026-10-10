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
- [x] Link cable: multi-play mode, up to four consoles in lockstep in one process (`pipit link`; Emerald trades with itself headless, see [link-testing.md](link-testing.md))

## Web app

- [x] Load a ROM from the device, play with keyboard
- [x] Touch controls (built; needs a round of testing on real phones)
- [x] Controller support: auto-detect, toolbar toggle (click = on/off, hold = pick among several)
- [x] Controller button remapping (gear next to the picker; saved per controller model)
- [x] Touch layouts: GBA (beside the screen), GBA SP (below it), Auto by orientation
- [x] Touch layout editor: drag and resize the screen and every control, saved per layout
- [x] Audio output (AudioWorklet)
- [x] Save data persistence (IndexedDB), .sav export/import
- [x] Fast-forward, screenshots
- [x] Installable PWA, offline
- [x] Save state slots (3 slots, Shift+F1–F3 / F1–F3)
- [x] Remappable keyboard controls (⋯ menu → Change key bindings; includes fast-forward and pause)
- [x] Colour correction: GBA LCD look (gba-color transform, precomputed 15-bit table)
- [x] Deployed online (GitHub Pages, on every push to `main`)
- [x] Play together online: host a game, join with a six-letter code; both consoles run on both machines in lockstep over a WebRTC data channel (`platform/netplay.ts`; an Emerald trade between two browser windows is `scripts/link-trade-web.mjs`)

## Desktop

- [x] Tauri shell (`desktop/`, installers via `npm run build`)
- [ ] Native file associations (.gba double-click)
- [ ] Code-signed releases

## Later

- [ ] GBC core — the library already has a hidden GBC tab waiting for it
- [ ] Playing together: input delay measured from the ping instead of fixed, a self-hosted introduction server and TURN relay for strict NATs, three and four players
