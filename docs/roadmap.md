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
- [x] Play together online: host a game, join with a six-letter code, two to four players; every console runs on every machine in lockstep over WebRTC data channels, the host relays keys and picks the input delay from the measured ping (`platform/netplay.ts`; an Emerald trade between two browser windows is `scripts/link-trade-web.mjs`)
- [x] Self-hosted introduction server and relay: connection settings in the dialog, `server/` and [hosting-online-play.md](hosting-online-play.md)

## Desktop

- [x] Tauri shell (`desktop/`, installers via `npm run build`)
- [ ] Native file associations (.gba double-click)
- [ ] Code-signed releases

## Later

- [ ] GBC core — the library already has a hidden GBC tab waiting for it
- [x] Playing together: the host adjusts the delay while playing from the live ping; late keys are guessed from the last seen ones and a wrong guess rolls back and re-runs up to eight frames; any console of the link can be watched (Watch in the player menu)
