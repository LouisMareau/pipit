// Runs every rockwrestler sub-test (RockPolish's DS test ROM) headless and
// screenshots each result into build/nds/. The ROM's menu is driven with key
// scripts; a passing test shows only "OK", so every passing screenshot is the
// same image: each one is compared with the first test's.
//
//   node scripts/rockwrestler.mjs [filter]
//
// Needs `cargo build --release -p pipit-cli` and `sh scripts/fetch-test-roms.sh`.
import { execFileSync } from "node:child_process";
import { readFileSync, mkdirSync } from "node:fs";

const filter = process.argv[2];
const exe = `build/target/release/pipit${process.platform === "win32" ? ".exe" : ""}`;
const rom = "tests/roms/nds/rockwrestler.nds";
const out = "build/nds";
mkdirSync(out, { recursive: true });

// The menu, in order: category → sub-tests.
const menu = {
  armv4: ["cond"],
  armv5: ["clz", "qadd", "qdadd", "smulxy", "smulwy", "smlaxy", "smlawy", "smlalxy", "blx", "ldr-pc", "ldm-stm"],
  ipc: ["sync", "fifo", "fifo-irq"],
  math: ["sqrt32", "sqrt64", "div32", "div64-32", "div64"],
  memory: ["wramcnt", "vramcnt", "tcm"],
  // The initial-state pages only show values (no verdict); compare by eye.
  initial: ["ipc-irq-cpsr", "cp15"],
};

let reference = null;
let category = 0;
let failures = 0;
for (const [cat, subs] of Object.entries(menu)) {
  for (let j = 0; j < subs.length; j++) {
    const name = `${cat}-${subs[j]}`;
    if (filter && !name.includes(filter)) continue;
    const keys = [];
    let f = 20;
    const press = (k) => { keys.push(`${f}=${k}`, `${f + 4}=`); f += 10; };
    for (let n = 0; n < category; n++) press("DOWN");
    press("A");
    f += 10;
    for (let n = 0; n < j; n++) press("DOWN");
    press("A");
    const shot = `${out}/${name}.png`;
    execFileSync(exe, ["run", rom, "--frames", String(f + 300), "--keys", keys.join(","), "--screenshot", shot], { stdio: "ignore" });
    const png = readFileSync(shot);
    if (cat === "initial") {
      console.log(`${name}: see ${shot}`);
      continue;
    }
    reference ??= png;
    const ok = png.equals(reference);
    if (!ok) failures++;
    console.log(`${name}: ${ok ? "OK" : `differs, see ${shot}`}`);
  }
  category++;
}
process.exit(failures ? 1 : 0);
