// Runs every Nitro Engine example (small 3D demos, MIT) headless for 200
// frames and screenshots each into build/nds/ne/, reporting the ones whose
// top screen stayed black. They are eyeball tests of the 3D engine: compare
// the screenshots with the examples' own screenshots in the Nitro Engine
// repository when something looks off.
//
//   node scripts/nitro-engine.mjs [filter]
//
// Needs `cargo build --release -p pipit-cli` and `sh scripts/fetch-test-roms.sh`.
import { execFileSync } from "node:child_process";
import { mkdirSync, readdirSync, readFileSync } from "node:fs";
import { PNG } from "./lib/png.mjs";

const filter = process.argv[2];
const exe = `build/target/release/pipit${process.platform === "win32" ? ".exe" : ""}`;
const dir = "tests/roms/nds/nitro-engine";
const out = "build/nds/ne";
mkdirSync(out, { recursive: true });
let blank = 0;
for (const file of readdirSync(dir).filter((f) => f.endsWith(".nds")).sort()) {
  const name = file.replace(/\.nds$/, "");
  if (filter && !name.includes(filter)) continue;
  const shot = `${out}/${name}.png`;
  execFileSync(exe, ["run", `${dir}/${file}`, "--frames", "200", "--screenshot", shot], { stdio: "ignore" });
  const png = PNG.decode(readFileSync(shot));
  let lit = 0;
  for (let i = 0; i < 256 * 192 * 4; i += 4) if (png.data[i] | png.data[i + 1] | png.data[i + 2]) lit++;
  if (lit === 0) blank++;
  console.log(`${name}: ${lit === 0 ? "top screen black" : `${lit} lit pixels on top`}`);
}
process.exit(blank ? 1 : 0);
