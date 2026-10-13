# Tests

| Path | What it holds |
|------|---------------|
| `roms/` | Open-source test ROM suites, cloned by `scripts/fetch-test-roms.sh`. Gitignored. |
| `expectations/` | Expected results per test ROM (register values or screen hashes). |
| `../crates/gba/tests/` | Rust integration tests that run the ROMs headless through `pipit-gba`. |

Game ROMs are never part of the repository. To check a real game, run it with
`pipit-cli` and compare against a reference emulator by hand.

The DS core is checked with RockPolish's rockwrestler (`roms/nds/`): `node
scripts/rockwrestler.mjs` drives its menu headless and reports every sub-test.
Two homebrew titles serve as eyeball tests of the 2D engines: `pipit run
tests/roms/nds/GodMode9i.nds --frames 300 --screenshot build/nds/gm9i.png` shows
its console on both screens, and the same with `WordleDS.nds` its how-to-play
screens (a BlocksDS build, so it also exercises the cothread scheduler). The 3D
engine is eyeballed with the 44 Nitro Engine examples (`roms/nds/nitro-engine/`):
`node scripts/nitro-engine.mjs` runs them all and screenshots each into `build/nds/ne/`.
Two consoles within wireless range are checked by `crates/nds/tests/wireless.rs` (a
data frame, then a host command with its reply and acknowledgement), and `pipit link
a.nds b.nds --frames 120 --check-rollback 30` runs a pair in lockstep and proves their
states restore exactly. The GBA slot is covered by `crates/nds/tests/slot2.rs`: a
cartridge's header and Flash chip, which CPU owns the slot, the Rumble Pak, an
empty slot, and a save state keeping the cartridge. The save chips' protocols,
the games table and the firmware's user settings have unit tests in
`backup.rs`, `savedb.rs` and `firmware.rs`.

Run everything with:

```bash
cargo test --workspace
```
