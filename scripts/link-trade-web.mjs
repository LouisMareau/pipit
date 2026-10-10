// End to end, through the web app: two windows play together and trade a
// Pokémon in Emerald. Needs a save standing at the Cable Club counter and the
// key scripts for both players (see docs/link-testing.md); the ROM is never
// part of the repository.
//
//   node scripts/link-trade-web.mjs <rom.gba> <counter.sav> <keys-a.txt> <keys-b.txt> [url]
//
// Both players start from the same save; player 1 trades its first Pokémon for
// player 2's second, so afterwards the saves (read back through the app's test
// hook) show the swapped parties. Runs at the display rate: about three minutes.

import { readFileSync } from "node:fs";
import { basename, resolve } from "node:path";
import { launch, sleep } from "./lib/cdp.mjs";

const [romArg, saveArg, keysAArg, keysBArg, urlArg] = process.argv.slice(2);
if (!romArg || !saveArg || !keysAArg || !keysBArg) {
  console.error("usage: node scripts/link-trade-web.mjs <rom.gba> <counter.sav> <keys-a.txt> <keys-b.txt> [url]");
  process.exit(2);
}
const url = urlArg ?? "http://localhost:4173";
const keysA = readFileSync(keysAArg, "utf8").trim();
const keysB = readFileSync(keysBArg, "utf8").trim();
const FRAMES = Number(process.env.PIPIT_TRADE_FRAMES ?? 9100);
/** Rejects when `promise` takes longer than `ms`. */
const within = (ms, promise, what) =>
  Promise.race([promise, new Promise((_, reject) => setTimeout(() => reject(new Error(`timed out: ${what}`)), ms))]);

const t0 = Date.now();
const step = (what) => console.log(`[${((Date.now() - t0) / 1000).toFixed(1)} s] ${what}`);
const host = await launch(url);
step("browser up");
await sleep(1500);

// The ROM goes in through the library's file input; the save straight into the
// app's database, as if it had been played.
const { root } = await host.send("DOM.getDocument", { depth: 1 });
const { nodeId } = await host.send("DOM.querySelector", { nodeId: root.nodeId, selector: "input[type=file]" });
// The browser reads the file itself, so it needs an absolute path.
await host.send("DOM.setFileInputFiles", { nodeId, files: [resolve(romArg)] });
step("rom added");
await sleep(1500);
const saveBase64 = readFileSync(saveArg).toString("base64");
const romId = await host.evaluate(`(async () => {
  const db = await new Promise((ok, err) => { const r = indexedDB.open("pipit"); r.onsuccess = () => ok(r.result); r.onerror = () => err(r.error); });
  const roms = await new Promise((ok, err) => { const r = db.transaction("roms").objectStore("roms").getAll(); r.onsuccess = () => ok(r.result); r.onerror = () => err(r.error); });
  const id = roms[0].id;
  const bytes = Uint8Array.from(atob(${JSON.stringify(saveBase64)}), (c) => c.charCodeAt(0));
  await new Promise((ok, err) => { const r = db.transaction("saves", "readwrite").objectStore("saves").put({ id, data: bytes.buffer, updatedAt: Date.now() }); r.onsuccess = ok; r.onerror = () => err(r.error); });
  return id;
})()`);
console.log(`rom ${basename(romArg)} as entry ${romId}, save seeded`);

const play = async (page, keyScript) => {
  await page.evaluate(`window.pipit.keyScript = ${JSON.stringify(keyScript)}; true`);
  await page.evaluate("document.querySelector('.rom-main').click(); true");
  await sleep(1500);
  await page.evaluate("document.querySelector('[data-action=menu]').click(); document.querySelector('.menu [data-action=link]').click(); true");
  await sleep(300);
};
await play(host, keysA);
step("host playing");
await host.evaluate("document.querySelector('[data-action=host]').click(); true");
await sleep(400);
const code = await host.evaluate("document.querySelector('.link-code').textContent");
step(`hosting as ${code}`);

const guest = await host.newWindow(url);
step("guest window up");
await sleep(1500);
await play(guest, keysB);
step("guest playing");
await guest.evaluate(`document.querySelector('.link-input').value = ${JSON.stringify(code)}; document.querySelector('[data-action=join-form]').requestSubmit(); true`);
console.log(`joined with code ${code}; playing ${FRAMES} frames…`);

const frameOf = (page) => page.evaluate("Number(document.querySelector('.player').dataset.linkFrame ?? 0)");
const linked = (page) => page.evaluate("document.querySelector('.player').classList.contains('linked')");
const started = Date.now();
for (;;) {
  await sleep(5000);
  const [a, b] = await Promise.all([frameOf(host), frameOf(guest)]);
  console.log(`  frames: host ${a}, guest ${b}`);
  if (a >= FRAMES && b >= FRAMES) break;
  if (!(await linked(host)) || !(await linked(guest))) {
    console.error("the session ended early");
    console.error((await host.evaluate("document.querySelector('.toast')?.textContent ?? ''")) || "(no notice)");
    process.exit(1);
  }
  if (Date.now() - started > 6 * 60_000) {
    console.error("timed out");
    process.exit(1);
  }
}

// Party nicknames from a save: the newest slot's section 1 holds the party.
const party = (base64) => {
  const d = Buffer.from(base64, "base64");
  let best = null;
  for (let slot = 0; slot < 2; slot++) {
    const sections = new Map();
    let index = 0;
    for (let i = 0; i < 14; i++) {
      const o = slot * 0xe000 + i * 0x1000;
      sections.set(d.readUInt16LE(o + 0xff4), o);
      index = d.readUInt32LE(o + 0xffc);
    }
    if (!best || index > best.index) best = { index, sections };
  }
  const o = best.sections.get(1);
  const names = [];
  for (let i = 0; i < d[o + 0x234]; i++) {
    let name = "";
    for (let j = 0; j < 10; j++) {
      const c = d[o + 0x238 + i * 100 + 8 + j];
      if (c === 0xff) break;
      name += c >= 0xbb && c <= 0xd4 ? String.fromCharCode(65 + c - 0xbb) : "?";
    }
    names.push(name);
  }
  return names;
};
const saveOf = (page, who) =>
  within(
    15_000,
    page.evaluate(`window.pipit.save().then((b) => {
      const bytes = new Uint8Array(b);
      let s = "";
      for (let i = 0; i < bytes.length; i += 8192) s += String.fromCharCode(...bytes.subarray(i, i + 8192));
      return btoa(s);
    })`),
    `${who}'s save`,
  );
process.on("uncaughtException", (error) => {
  console.error(error.message);
  for (const line of host.logs.slice(-20)) console.error(`  console ${line}`);
  process.exit(1);
});
const hostParty = party(await saveOf(host, "host"));
const guestParty = party(await saveOf(guest, "guest"));
console.log(`host party: ${hostParty.join(", ")}; guest party: ${guestParty.join(", ")}`);
host.close();
const ok = hostParty.join() === "DUPE,DUPE" && guestParty.join() === "MUDKIP,MUDKIP";
console.log(ok ? "TRADE OVER THE WEB LINK PASSED" : "TRADE OVER THE WEB LINK FAILED");
process.exit(ok ? 0 : 1);
