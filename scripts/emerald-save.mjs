// Two chores on Pokémon Emerald saves for the link tests (see docs/link-testing.md):
//
//   node scripts/emerald-save.mjs party <file.sav>
//     prints the party as NICKNAME (ORIGINAL TRAINER), from the newest save slot
//   node scripts/emerald-save.mjs as-trainer <in.sav> <out.sav> <NAME>
//     copies the save as another trainer (the player's name and the party's
//     original-trainer names; up to seven letters A–Z), fixing the section
//     checksums. Trainer ids are left alone: they key the Pokémon data.
import fs from "node:fs";

const [command, inPath, outPath, name] = process.argv.slice(2);
const SECTION_SIZES = [3884, 3968, 3968, 3968, 3848, 3968, 3968, 3968, 3968, 3968, 3968, 3968, 3968, 2000];
const SLOT = 0xe000;
const SECTION = 0x1000;
const encode = (s) => Array.from(s.toUpperCase(), (c) => 0xbb + c.charCodeAt(0) - 65);
const decode = (d, o, len) => {
  let s = "";
  for (let j = 0; j < len && d[o + j] !== 0xff; j++) s += d[o + j] >= 0xbb && d[o + j] <= 0xd4 ? String.fromCharCode(65 + d[o + j] - 0xbb) : "?";
  return s;
};

/** The sections of the newest slot, by id. */
function newest(d) {
  let best = null;
  for (let slot = 0; slot < 2; slot++) {
    const sections = new Map();
    let index = 0;
    for (let i = 0; i < 14; i++) {
      const o = slot * SLOT + i * SECTION;
      sections.set(d.readUInt16LE(o + 0xff4), o);
      index = d.readUInt32LE(o + 0xffc);
    }
    if (!best || index > best.index) best = { index, sections };
  }
  return best.sections;
}

if (command === "party" && inPath) {
  const d = fs.readFileSync(inPath);
  const o = newest(d).get(1);
  const mons = [];
  for (let i = 0; i < d[o + 0x234]; i++) {
    const m = o + 0x238 + i * 100;
    mons.push(`${decode(d, m + 8, 10)} (${decode(d, m + 20, 7)})`);
  }
  console.log(mons.join(", "));
} else if (command === "as-trainer" && inPath && outPath && /^[A-Za-z]{1,7}$/.test(name ?? "")) {
  const d = fs.readFileSync(inPath);
  const bytes = encode(name);
  for (let slot = 0; slot < 2; slot++) {
    for (let i = 0; i < 14; i++) {
      const o = slot * SLOT + i * SECTION;
      const id = d.readUInt16LE(o + 0xff4);
      if (id === 0) {
        d.fill(0xff, o, o + 8);
        d.set(bytes, o);
      }
      if (id === 1) {
        for (let m = 0; m < d[o + 0x234]; m++) {
          const mon = o + 0x238 + m * 100;
          d.fill(0xff, mon + 20, mon + 27);
          d.set(bytes, mon + 20);
        }
      }
      let sum = 0;
      for (let j = 0; j < SECTION_SIZES[id]; j += 4) sum = (sum + d.readUInt32LE(o + j)) >>> 0;
      d.writeUInt16LE(((sum >>> 16) + sum) & 0xffff, o + 0xff6);
    }
  }
  fs.writeFileSync(outPath, d);
  console.log(`wrote ${outPath} as ${name.toUpperCase()}`);
} else {
  console.error("usage: node scripts/emerald-save.mjs party <file.sav> | as-trainer <in.sav> <out.sav> <NAME>");
  process.exit(2);
}
