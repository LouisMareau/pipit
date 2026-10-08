# Pipit

A clean, free Game Boy Advance emulator that runs in the browser, on your phone and on
the desktop — from one codebase.

- **Accurate.** A from-scratch ARM7TDMI core, cycle-counted memory bus, scanline video
  and full audio, verified against open test ROM suites. Retail games and custom
  ROMs (decomp builds, ROM hacks) are both first-class targets.
- **Portable.** The core is plain Rust with no I/O. It compiles natively and to
  WebAssembly, so the web app, the desktop app and the test runner share it.
- **Yours.** No bundled games, no bundled BIOS. ROMs you open stay on your device.
  MIT licensed.

> Pipit is not affiliated with or endorsed by Nintendo.

## Layout

```
crates/   Rust workspace — the emulator
  gba/    pipit-gba   the core: cpu/ memory/ video/ audio/ cartridge/ bios/ …
  wasm/   pipit-wasm  WebAssembly bindings for the web app
  cli/    pipit-cli   headless runner: tests, screenshots, benchmarks
web/      The app (Vite + TypeScript PWA): ui/ features/ platform/
desktop/  Tauri desktop shell around the web app (installers for Windows/macOS/Linux)
tests/    Test ROM suites and expected results
docs/     Architecture, GBA reference notes, compatibility list
scripts/  Developer helper scripts
build/    All build outputs (gitignored)
```

Each piece has one home: emulation logic only ever lives in `crates/gba`, anything a
user sees lives in `web/`, and nothing is written outside `build/` when building.

## Getting started

Requirements: [Rust](https://rustup.rs) (stable) and [Node.js](https://nodejs.org) 20+.

```bash
# Emulator core + test runner
cargo build --release
cargo test --workspace

# Fetch open-source test ROMs (needed for the test suite)
sh scripts/fetch-test-roms.sh

# Run a ROM headless for 600 frames and save a screenshot
cargo run --release -p pipit-cli -- run game.gba --frames 600 --screenshot build/shot.png

# Web app (dev server with hot reload)
cd web && npm install && npm run dev

# Desktop app (installers land in build/target/release/bundle)
cd desktop && npm install && npm run build
```

## Status

Playable. The core passes the open CPU, memory, BIOS, video and save test suites and
runs commercial games (Pokémon Emerald is the reference title) at well over full speed.
The web app loads ROMs from the device, plays with keyboard, touch or gamepad, keeps
saves in the browser and installs as an offline app. See
[docs/roadmap.md](docs/roadmap.md) for what is next.

End-to-end check of the web app in a headless browser:

```bash
cd web && npm run build && npx vite preview &
node scripts/smoke-web.mjs tests/roms/gba-tests/ppu/hello.gba
```

## License

[MIT](LICENSE). Test ROM suites under `tests/roms` keep their own licenses.
