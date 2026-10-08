# Contributing

Thanks for looking! A few conventions keep the project easy to navigate.

## Where things go

| You are changing… | Put it in |
|-------------------|-----------|
| How the GBA is emulated (CPU, memory, video, audio, saves, BIOS) | `crates/gba/src/<subsystem>/` |
| What JavaScript can call | `crates/wasm/` |
| Command-line tooling, test harness glue | `crates/cli/` |
| Anything a user sees or touches | `web/src/ui/` |
| App-level features (save states, fast-forward, rewind, cheats, screenshots) | `web/src/features/` |
| Browser/OS plumbing (storage, audio output, gamepads, files, PWA) | `web/src/platform/` |
| Reference material and design notes | `docs/` |

Never commit ROMs, saves or BIOS files. `.gitignore` blocks the common extensions.

## Code style

- Rust: `cargo fmt` and `cargo clippy --workspace -- -D warnings` must pass.
- TypeScript: `npm run lint` in `web/`.
- Comments explain *why*, and point at the hardware behaviour being modelled
  (cite [GBATEK](https://problemkaputt.de/gbatek.htm) sections where useful).
- Keep modules focused: one hardware block per module.

## Tests

```bash
sh scripts/fetch-test-roms.sh   # once
cargo test --workspace
```

New emulation behaviour should come with a test — either a unit test next to the code
or an expectation in `tests/expectations/` for a test ROM.

## Commits

Short imperative subject line (`Add Flash 1M write cycle`), body explaining the why
when it is not obvious.
