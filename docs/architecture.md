# Architecture

Pipit is split into a pure emulation core and thin front-ends around it.

```
            ┌──────────────────────────┐
            │  web/ (PWA)   desktop/   │   UI, input, audio output, storage
            └────────────┬─────────────┘
                         │ frames, samples, key state, save data
            ┌────────────▼─────────────┐
            │  crates/wasm  crates/cli │   bindings / tooling
            └────────────┬─────────────┘
            ┌────────────▼─────────────┐
            │  crates/gba (pipit-gba)  │   the GBA
            └──────────────────────────┘
```

## The core (`crates/gba`)

`Gba` owns every hardware block and exposes a small API: load a cartridge, run one
frame, read the framebuffer and audio samples, set the key state, get or set save
data, serialize to a save state.

| Module | Hardware | Notes |
|--------|----------|-------|
| `cpu/` | ARM7TDMI | ARM + Thumb interpreters, 3-stage pipeline, all 7 modes, exceptions |
| `memory/` | Bus & wait states | Region decode, 8/16/32-bit access, mirrors, open bus, prefetch buffer |
| `video/` | LCD (PPU) | Modes 0–5, 4 BGs, affine, 128 OBJs, windows, blending, mosaic; renders per scanline |
| `audio/` | Sound | 4 PSG channels + 2 Direct Sound FIFOs, mixer, resampler |
| `cartridge/` | Game Pak | ROM, SRAM / Flash 64K & 128K / EEPROM 4K & 64K, RTC, save-type detection |
| `bios/` | BIOS | High-level emulation of all SWI calls, replacement boot/IRQ stub; optional real BIOS |
| `dma.rs` | 4 DMA channels | Immediate, VBlank, HBlank, FIFO, video capture timing |
| `timers.rs` | 4 timers | Prescalers, cascade, IRQs, FIFO feeding |
| `irq.rs` | Interrupt controller | IE / IF / IME, halt, IRQ delay |
| `keypad.rs` | Keys | KEYINPUT / KEYCNT with IRQ conditions |
| `sio.rs` | Serial port | Multi-play (link cable) mode with timing and IRQ; the other modes are register stubs |
| `link.rs` | Link cable | Up to four consoles run in lockstep in one process, exchanging multi-play words |
| `scheduler.rs` | — | Event queue driving everything above on a shared cycle counter |

### Timing model

The CPU is the only component that "runs". Everything else registers events on the
scheduler (end of HBlank, timer overflow, DMA start, FIFO drain …) at absolute cycle
times. After every instruction the CPU checks whether the next event is due. This keeps
the hot loop small, which matters for WebAssembly on phones.

Memory accesses return their cost (N/S cycles, wait states, prefetch hits) so the
CPU's cycle count is accurate without ticking components each cycle.

### Compatibility targets

1. Open test suites: jsmolka `gba-tests`, FuzzARM, armwrestler, and later AGS-style
   timing tests.
2. Retail games, with Pokémon Ruby/Sapphire/Emerald/FireRed/LeafGreen as the first
   milestone (Flash 128K, RTC, DMA-driven audio engine, EEPROM for other titles).
3. Decomp-built and hacked ROMs. These are tested against mGBA by their authors, so
   where documentation is ambiguous Pipit follows mGBA's behaviour.

## Front-ends

- `web/` runs the core inside a Web Worker. Emulation is locked to the display:
  the UI thread's `requestAnimationFrame` loop (`platform/pacer.ts`) asks the
  worker for one frame per refresh on 60 Hz screens (every second refresh on
  120 Hz; time-based on other rates), which avoids the periodic duplicated frame
  that wall-clock pacing of 59.73 Hz content on a 60 Hz screen produces. The
  0.46 % speed difference is absorbed by the `AudioWorklet`, which nudges its
  resampling rate by up to ±1 % to keep its buffer near 125 ms. Frames are posted
  as transferable buffers and recycled. ROMs and saves live in IndexedDB on the
  device and are never uploaded anywhere.
- Playing together (`platform/netplay.ts`) does not send cable traffic over the
  network: both players' consoles run on both machines, joined by the core's
  in-process link, and only button states cross the wire. Emulation is
  deterministic, so both machines compute the same two games and each shows its
  own console; keys are applied a few frames late to hide latency, and everyone
  compares state digests every second to catch drift. The host is the hub for up
  to four players: guests connect to it, it relays their keys to one another,
  pings everyone and picks the delay from the slowest path when the host starts
  the game. Joining sends the guest's save and ROM digest; the start sends every
  save, the delay and the cartridge clock. Connections are WebRTC data channels
  introduced by a PeerJS server (the host's code is its id), the public one or
  your own (see hosting-online-play.md); `?link=local` joins tabs of one browser
  instead, which is what the smoke test uses.
- `desktop/` wraps the same web app with Tauri (installers via `npm run build`).
