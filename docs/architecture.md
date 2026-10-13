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
            │  crates/gba   crates/gbc │   the GBA; the Game Boy (Color);
            │  crates/nds              │   the DS (in progress)
            └────────────┬─────────────┘
            ┌────────────▼─────────────┐
            │  crates/arm              │   the ARM interpreter the GBA and DS share
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
| `sio.rs` | Serial port | Multi-play (link cable) mode and 32-bit normal mode (the Wireless Adapter's), with timing and IRQ; UART, JOY bus and general purpose are register stubs |
| `rfu.rs` | Wireless Adapter | One adapter as its console sees it: the "NINTENDO" login, command and acknowledgement words, the clock hand-over for waits |
| `link.rs` | Link | Up to four consoles run in lockstep in one process, joined by a cable (multi-play words) or by adapters |
| `wireless.rs` | Radio | What goes between adapters: who hosts, who joins which slot, the rounds that carry data frames, disconnects |
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

## The Game Boy core (`crates/gbc`)

`Gbc` has the same shape of API as `Gba` and the same rules: no I/O, serde save
states (through `crates/common`, which holds only the state file format the two
cores share), deterministic. The machine is stepped per CPU M-cycle: every memory
access ticks the bus once, and the tick advances the timer, the picture, the sound
and the serial port, so instruction timing falls out of the access pattern.

| Module | Hardware | Notes |
|--------|----------|-------|
| `cpu.rs` | SM83 | Every instruction, interrupts with the EI delay and the HALT bug, STOP for the speed switch |
| `memory.rs` | Bus | Memory map, WRAM banks, I/O dispatch, OAM DMA, general and HBlank DMA, double speed |
| `cartridge.rs` | Game Pak | MBC1/2/3/5 (MBC30 sizes included), battery RAM, the MBC3 clock with the usual save trailer |
| `video.rs` | PPU | Modes and STAT dot by dot, lines drawn at the start of their mode 3; colour palettes, attributes, both VRAM banks |
| `audio.rs` | APU | Two pulse channels, wave, noise, the frame sequencer, stereo at 32768 Hz |
| `timer.rs`, `joypad.rs`, `serial.rs` | — | DIV/TIMA with the edge quirks; the joypad; a serial port that keeps what test ROMs print |
| `link.rs` | Link cable | Two consoles in lockstep, the one behind in time always stepping next, so either may clock a transfer |

A colour game runs on a Game Boy Color; a classic game runs on a classic Game Boy
with its four shades. No boot ROM is used. Tests: Blargg's `cpu_instrs`,
`instr_timing` and `mem_timing`, both acid2 pictures, and the mooneye acceptance
tests, of which the ones about where within an instruction an access lands are
listed as known failures.

## The DS core (`crates/nds`)

Two CPUs share one machine. `memory/mod.rs` holds everything both see (main RAM,
the shared WRAM and its split, the TCMs, VRAM, the card, IPC, one interrupt
controller, timer set and DMA controller per CPU); `memory/arm9.rs` and
`memory/arm7.rs` are the two memory maps, each a `pipit_arm::Bus` over that
state. The ARM9 is an `Arch::V5TE` core, the ARM7 an `Arch::V4T` one, both from
`crates/arm`.

| Module | Hardware | Notes |
|--------|----------|-------|
| `memory/` | Buses | The shared state, the two maps, the I/O registers as each CPU sees them, the DMA runner |
| `video/` | LCD, the two 2D engines | Scanline timing and the engines' registers (`mod.rs`), the VRAM banks and their page tables (`vram.rs`), backgrounds (`bg.rs`), objects (`obj.rs`), composition through windows and colour effects, display modes, capture and master brightness (`render.rs`); pixels are kept at 18 bits until the screen |
| `cart.rs` | DS card | The header (what direct boot loads), ROMCTRL and the data port: plain `B7` reads and the chip id |
| `ipc.rs` | IPC | IPCSYNC and the two 16-word FIFOs with their interrupts |
| `sound.rs` | Sound | Sixteen channels (PCM8/16, ADPCM, square, noise) read through the ARM7's view of memory, the mixer, the two captures; one stereo sample per 1024 bus cycles |
| `spi.rs`, `firmware.rs` | SPI bus | The power management chip, the firmware flash (a default image when the user has none) and the touch screen controller; transfers complete at once, the hold bit keeps a chip selected across bytes |
| `rtc.rs` | Real-time clock | The bit-banged S-35180 protocol, status and alarm registers, a once-a-second event for its interrupt; time runs on emulated cycles from a Unix time the front-end sets |
| `backup.rs` | Card saves | The save chip behind AUXSPI: EEPROM (4 Kbit to 512 Kbit) or Flash, told apart by the first write's address bytes or a save file's size; infrared cards' pass-through |
| `gpu3d/` | 3D engine | `geometry.rs`: the command set, matrices, lighting, clipping, polygon assembly; `render.rs`: the scanline rasterizer with depth, texturing (`texture.rs`), shading modes, blending, shadows, edge marking and fog; the frame goes to engine A's BG0 at the swap |
| `math.rs` | Divider, square root | The ARM9's 280h–2BFh block; results are immediate |
| `dma.rs`, `timers.rs`, `irq.rs` | Per CPU | The ARM9's eight DMA timings and 21-bit counts, the ARM7's four and 16-bit; 32-bit IE/IF |
| `bios/` | BIOS | Replacement images with only the exception vectors and interrupt dispatchers; every SWI in `bios/swi.rs`; optional real BIOS images |
| `scheduler.rs` | — | The shared clock, in ARM9 cycles (two per bus cycle) |

The web app shows both screens stacked in one 256 × 384 picture; a pointer on the
bottom half is the pen (`Emulator::set_touch`), and the key bits add X and Y above
the Advance's ten.

### Timing model

Each CPU has its own clock in ARM9 cycles (the ARM7 adds two per bus cycle). They
run alternately in slices of 64 cycles or up to the next event, whichever is
sooner; a halted CPU jumps to the end of the slice. Between slices the scheduler
advances and runs the events that came due (scanlines, timers, card words), so
nothing one CPU does is seen by the other more than a slice late. The ARM9's
caches are not modelled: accesses outside the TCMs cost one bus cycle per
16-bit transfer, plus one for a non-sequential one to main memory.

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
  in-process link, and only button states cross the wire. The host chooses
  whether that link is a cable or Wireless Adapters; the adapters are emulated
  in the core too (`rfu.rs`, `wireless.rs`), so the wire carries the same
  button states either way. Emulation is
  deterministic, so both machines compute the same two games and each shows its
  own console; keys are applied a few frames late to hide latency, and everyone
  compares state digests every second to catch drift. Keys that are late are
  guessed (each player is assumed to hold what they last sent) for up to eight
  frames rather than waited for; the worker keeps the state before any such
  frame, and a wrong guess rolls back to it and re-runs silently with the keys
  now known (`Link::save_state`/`load_state` capture every console and the
  cable). The host raises the delay as soon as the pings call for it and lowers
  it after a calm while. The host is the hub for up
  to four players: guests connect to it, it relays their keys to one another,
  pings everyone and picks the delay from the slowest path when the host starts
  the game. Joining sends the guest's save and ROM digest; the start sends every
  save, the delay and the cartridge clock. Connections are WebRTC data channels
  introduced by a PeerJS server (the host's code is its id), the public one or
  your own (see hosting-online-play.md); `?link=local` joins tabs of one browser
  instead, which is what the smoke test uses.
- `desktop/` wraps the same web app with Tauri (installers via `npm run build`).
