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

Run everything with:

```bash
cargo test --workspace
```
