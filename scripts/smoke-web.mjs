#!/usr/bin/env node
// End-to-end smoke test for the web app, with no dependencies: drives a headless
// Chrome/Edge over the DevTools protocol, adds a ROM through the real file input,
// starts it, and checks that frames are being drawn and no errors were logged.
//
//   node scripts/smoke-web.mjs <rom.gba> [url] [screenshot.png]
//
// Defaults: url = http://localhost:4173 (vite preview), screenshot = build/smoke.png.

import { spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const [romArg, url = "http://localhost:4173", shotArg] = process.argv.slice(2);
if (!romArg) {
  console.error("usage: node scripts/smoke-web.mjs <rom.gba> [url] [screenshot.png]");
  process.exit(2);
}
const rom = resolve(romArg);
const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const screenshot = resolve(shotArg ?? join(root, "build", "smoke.png"));

const candidates = [
  process.env.PIPIT_BROWSER,
  "C:/Program Files/Google/Chrome/Application/chrome.exe",
  "C:/Program Files (x86)/Google/Chrome/Application/chrome.exe",
  `${process.env.LOCALAPPDATA}/Google/Chrome/Application/chrome.exe`,
  "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
  "C:/Program Files/Microsoft/Edge/Application/msedge.exe",
  "/usr/bin/google-chrome",
  "/usr/bin/chromium",
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
].filter(Boolean);
const browser = candidates.find((p) => existsSync(p));
if (!browser) {
  console.error("no Chrome or Edge found; set PIPIT_BROWSER to a browser executable");
  process.exit(2);
}

const port = 9333 + Math.floor(Math.random() * 1000);
const profile = mkdtempSync(join(tmpdir(), "pipit-smoke-"));
const child = spawn(
  browser,
  [
    "--headless=new",
    `--remote-debugging-port=${port}`,
    `--user-data-dir=${profile}`,
    "--no-first-run",
    "--no-default-browser-check",
    "--autoplay-policy=no-user-gesture-required",
    "--window-size=900,700",
    // CI containers have no usable sandbox or /dev/shm; harmless elsewhere.
    "--no-sandbox",
    "--disable-dev-shm-usage",
    "--disable-gpu",
    "about:blank",
  ],
  { stdio: ["ignore", "ignore", "pipe"] },
);
let browserStderr = "";
child.stderr.on("data", (chunk) => {
  browserStderr += chunk;
});
const cleanup = () => {
  child.kill();
  setTimeout(() => rmSync(profile, { recursive: true, force: true }), 500);
};
process.on("exit", cleanup);

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

let targets = [];
for (let i = 0; i < 150 && !targets.some((t) => t.type === "page"); i++) {
  await sleep(200);
  targets = await fetch(`http://127.0.0.1:${port}/json/list`)
    .then((r) => r.json())
    .catch(() => []);
}
const page = targets.find((t) => t.type === "page");
if (!page) {
  console.error(`browser: ${browser}\n${browserStderr.slice(-2000)}`);
  throw new Error("browser did not expose a page target");
}

const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  ws.onopen = resolve;
  ws.onerror = reject;
});
let nextId = 1;
const pending = new Map();
const events = [];
const logs = [];
ws.onmessage = (e) => {
  const msg = JSON.parse(e.data);
  if (msg.id) {
    const { resolve, reject } = pending.get(msg.id);
    pending.delete(msg.id);
    msg.error ? reject(new Error(msg.error.message)) : resolve(msg.result);
  } else {
    events.push(msg);
    if (msg.method === "Runtime.consoleAPICalled") {
      logs.push(`${msg.params.type}: ${msg.params.args.map((a) => a.value ?? a.description ?? "").join(" ")}`);
    } else if (msg.method === "Runtime.exceptionThrown") {
      logs.push(`exception: ${msg.params.exceptionDetails.text} ${msg.params.exceptionDetails.exception?.description ?? ""}`);
    } else if (msg.method === "Log.entryAdded" && msg.params.entry.level === "error") {
      logs.push(`error: ${msg.params.entry.text} ${msg.params.entry.url ?? ""}`);
    }
  }
};
const send = (method, params = {}) =>
  new Promise((resolve, reject) => {
    const id = nextId++;
    pending.set(id, { resolve, reject });
    ws.send(JSON.stringify({ id, method, params }));
  });
const evaluate = async (expression) => {
  const r = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(r.exceptionDetails.text);
  return r.result.value;
};
const pressKey = async (code, key, keyCode, modifiers = 0) => {
  await send("Input.dispatchKeyEvent", { type: "keyDown", code, key, windowsVirtualKeyCode: keyCode, modifiers });
  await send("Input.dispatchKeyEvent", { type: "keyUp", code, key, windowsVirtualKeyCode: keyCode, modifiers });
};

await send("Page.enable");
await send("Runtime.enable");
await send("Log.enable");
await send("Page.navigate", { url });
await sleep(1500);

const dumpLogs = () => {
  for (const l of logs) console.log(`  console ${l}`);
};
process.on("uncaughtException", async (error) => {
  console.error(error.message);
  dumpLogs();
  try {
    console.error("page html:", (await evaluate("document.body.innerHTML.slice(0, 600)")) ?? "");
  } catch {}
  process.exit(1);
});

// Add the ROM through the library's file input, exactly as a user would.
const { root: doc } = await send("DOM.getDocument", { depth: 1 });
const { nodeId } = await send("DOM.querySelector", { nodeId: doc.nodeId, selector: "input[type=file]" });
if (!nodeId) throw new Error("file input not found: did the library render?");
await send("DOM.setFileInputFiles", { nodeId, files: [rom] });
await sleep(1500);

const romCount = await evaluate("document.querySelectorAll('.rom-main').length");
console.log(`library entries: ${romCount}`);
await evaluate("document.querySelector('.rom-main').click(); true");
await sleep(4000);

// Let it run a while: the worker reports the longest frame-to-frame gap of each
// second, which exposes periodic stalls (garbage collection, pacing bugs).
const gaps = [];
for (let i = 0; i < 8; i++) {
  await sleep(1000);
  gaps.push(Number(await evaluate("document.querySelector('.toolbar-fps').dataset.maxGap ?? 0")));
}
console.log(`worst frame gap per second (ms): ${gaps.join(" ")}`);
const fps = await evaluate("document.querySelector('.toolbar-fps').textContent");

// Colour correction: the brightest pixel (the test ROMs all draw pure white) reads
// 248 with the LCD look (default) and 255 with raw colours.
const samplePixel = () =>
  evaluate(`(() => {
    const c = document.querySelector('canvas');
    const d = c.getContext('2d').getImageData(0, 0, c.width, c.height).data;
    let max = 0;
    for (let i = 0; i < d.length; i += 4) if (d[i] > max) max = d[i];
    return max;
  })()`);
const lcdWhite = await samplePixel();
await evaluate(`(() => { const s = document.querySelector('[data-setting=colorCorrection]'); s.value = 'off'; s.dispatchEvent(new Event('change')); })()`);
await sleep(300);
const rawWhite = await samplePixel();
await evaluate(`(() => { const s = document.querySelector('[data-setting=colorCorrection]'); s.value = 'gba'; s.dispatchEvent(new Event('change')); })()`);
await sleep(300);
await evaluate(`(() => { const s = document.querySelector('[data-setting=colorStrength]'); s.value = '50'; s.dispatchEvent(new Event('input')); })()`);
await sleep(300);
const halfWhite = await samplePixel();
await evaluate(`(() => { const s = document.querySelector('[data-setting=colorStrength]'); s.value = '100'; s.dispatchEvent(new Event('input')); })()`);
await sleep(300);
console.log(`colour correction: white = ${lcdWhite} (LCD) / ${halfWhite} (50 %) / ${rawWhite} (raw)`);
// Usually no controller is plugged in: the toggle renders gray but clickable.
// With one connected it must be green instead, and the picker must list it.
const pads = await evaluate("Array.from(navigator.getGamepads()).filter(Boolean).map((p) => p.id)");
const controller = await evaluate(`(() => {
  const b = document.querySelector('.controller-toggle');
  return b ? { present: true, active: b.classList.contains('active'), disabled: b.disabled } : { present: false };
})()`);
console.log(`controllers: ${JSON.stringify(pads)}; toggle: ${JSON.stringify(controller)}`);

// Holding the toggle for two seconds opens the controller picker without toggling.
const toggleBox = await evaluate(`(() => {
  const r = document.querySelector('.controller-toggle').getBoundingClientRect();
  return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
})()`);
const mouse = (type) =>
  send("Input.dispatchMouseEvent", { type, x: toggleBox.x, y: toggleBox.y, button: "left", clickCount: 1 });
await mouse("mouseMoved");
await mouse("mousePressed");
await sleep(1200);
if (process.env.PIPIT_SMOKE_HOLD_SHOT) {
  // Debug aid: capture the hold ring halfway through filling.
  const mid = await send("Page.captureScreenshot", {
    format: "png",
    clip: { x: toggleBox.x - 120, y: toggleBox.y - 24, width: 400, height: 48, scale: 3 },
  });
  writeFileSync(process.env.PIPIT_SMOKE_HOLD_SHOT, Buffer.from(mid.data, "base64"));
}
if (process.env.PIPIT_SMOKE_ACTIVE_SHOT) {
  // Debug aid: preview the active (green) look without a real controller.
  await evaluate("document.querySelector('.controller-toggle').classList.add('active'); true");
  const shot = await send("Page.captureScreenshot", {
    format: "png",
    clip: { x: toggleBox.x - 120, y: toggleBox.y - 24, width: 400, height: 48, scale: 3 },
  });
  writeFileSync(process.env.PIPIT_SMOKE_ACTIVE_SHOT, Buffer.from(shot.data, "base64"));
  await evaluate("document.querySelector('.controller-toggle').classList.remove('active'); true");
}
await sleep(1100);
await mouse("mouseReleased");
await sleep(200);
const picker = await evaluate(`(() => {
  const p = document.querySelector('.controller-picker');
  const options = Array.from(document.querySelectorAll('.controller-select option')).map((o) => o.textContent);
  return { open: !p.classList.contains('hidden'), options };
})()`);
console.log(`controller picker after hold: ${JSON.stringify(picker)}`);

// The gear next to the picker opens the remapping modal; Escape closes it.
await evaluate("document.querySelector('.controller-gear').click(); true");
await sleep(200);
const modal = await evaluate(`(() => {
  const m = document.querySelector('.modal-backdrop');
  return { open: !m.classList.contains('hidden'), rows: document.querySelectorAll('.mapping-row').length };
})()`);
await pressKey("Escape", "Escape", 27);
await sleep(200);
const modalClosed = await evaluate("document.querySelector('.modal-backdrop').classList.contains('hidden')");
console.log(`controller settings modal: ${JSON.stringify(modal)}, closed after Escape: ${modalClosed}`);

// Keyboard remapping: open from the menu, rebind fast-forward to F, check it works.
await evaluate("document.querySelector('[data-action=menu]').click(); document.querySelector('[data-action=keyboard]').click(); true");
await sleep(200);
const keyboardRows = await evaluate("document.querySelectorAll('.modal-backdrop:not(.hidden) .mapping-row').length");
await evaluate("document.querySelector('.modal-backdrop:not(.hidden) [data-action=change][data-key=FastForward]').click(); true");
await sleep(100);
await pressKey("KeyF", "f", 70);
await sleep(200);
const fastForwardLabel = await evaluate(
  "document.querySelector('.modal-backdrop:not(.hidden) [data-key=FastForward]').parentElement.querySelector('.mapping-value').textContent",
);
await pressKey("Escape", "Escape", 27);
await sleep(200);
await send("Input.dispatchKeyEvent", { type: "keyDown", code: "KeyF", key: "f", windowsVirtualKeyCode: 70 });
await sleep(100);
const fastActive = await evaluate("document.querySelector('[data-action=fast]').classList.contains('active')");
await send("Input.dispatchKeyEvent", { type: "keyUp", code: "KeyF", key: "f", windowsVirtualKeyCode: 70 });
await sleep(100);
const fastReleased = await evaluate("!document.querySelector('[data-action=fast]').classList.contains('active')");
const keyboardModalClosed = await evaluate("Array.from(document.querySelectorAll('.modal-backdrop')).every((m) => m.classList.contains('hidden'))");
console.log(
  `keyboard remap: rows=${keyboardRows}, fast-forward now "${fastForwardLabel}", ` +
    `dialog closed=${keyboardModalClosed}, F engages=${fastActive}, release clears=${fastReleased}`,
);
const litPixels = await evaluate(`(() => {
  const c = document.querySelector('canvas');
  const d = c.getContext('2d').getImageData(0, 0, c.width, c.height).data;
  let n = 0;
  for (let i = 0; i < d.length; i += 4) if (d[i] | d[i + 1] | d[i + 2]) n++;
  return n;
})()`);
console.log(`fps counter: "${fps}"; non-black pixels: ${litPixels}`);

// Save states: Shift+F1 saves slot 1, F1 loads it; each shows a toast.
const lastToast = () => evaluate("Array.from(document.querySelectorAll('.toast')).at(-1)?.textContent ?? ''");
const slotBefore = await evaluate("document.querySelector('[data-action=save-state][data-slot=\"1\"]').classList.contains('has-state')");
await pressKey("F1", "F1", 112, 8);
await sleep(800);
const savedToast = await lastToast();
const slotAfter = await evaluate("document.querySelector('[data-action=save-state][data-slot=\"1\"]').classList.contains('has-state')");
console.log(`save slot 1 highlighted: before=${slotBefore} after=${slotAfter}`);
await pressKey("F1", "F1", 112);
await sleep(800);
const loadedToast = await lastToast();
console.log(`save state: "${savedToast}" / "${loadedToast}"`);

// Phone check: emulate a touch device in both orientations; nothing may overflow
// horizontally, and the touch layout must follow the orientation (Auto setting).
const phone = {};
for (const [name, width, height] of [
  ["portrait", 390, 844],
  ["landscape", 844, 390],
]) {
  await send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 2, mobile: true });
  // Touch emulation is what flips `(pointer: coarse)` for the page.
  await send("Emulation.setTouchEmulationEnabled", { enabled: true, maxTouchPoints: 5 });
  await send("Emulation.setEmulatedMedia", { features: [{ name: "pointer", value: "coarse" }, { name: "hover", value: "none" }] });
  await evaluate("window.dispatchEvent(new Event('resize')); true");
  await sleep(400);
  phone[name] = await evaluate(`(() => {
    const player = document.querySelector('.player');
    const touch = document.querySelector('.touch');
    const widest = Math.max(...Array.from(document.querySelectorAll('.player *')).map((e) => e.getBoundingClientRect().right));
    const [l, r] = Array.from(document.querySelectorAll('.tbtn-shoulder')).map((e) => Math.round(e.getBoundingClientRect().top));
    const centre = (sel) => { const b = document.querySelector(sel).getBoundingClientRect(); return (b.top + b.bottom) / 2; };
    const dpad = document.querySelector('.dpad').getBoundingClientRect();
    const touchBox = touch.getBoundingClientRect();
    const pills = Array.from(document.querySelectorAll('.tbtn-pill')).map((e) => ({ key: e.textContent, top: Math.round(e.getBoundingClientRect().top) }));
    const start = pills.find((p) => p.key === 'start');
    const select = pills.find((p) => p.key === 'select');
    return {
      layout: player.dataset.layout,
      touchShown: getComputedStyle(touch).display !== 'none',
      overflow: Math.round(Math.max(document.documentElement.scrollWidth, widest) - window.innerWidth),
      shoulderMisalignment: Math.abs(l - r),
      groupMisalignment: Math.round(Math.abs(centre('.dpad') - centre('.abpad'))),
      padSize: Math.round(dpad.width),
      // How much of the control area's height the shoulder + disc column uses (GBA SP).
      fill: touchBox.height ? Math.round(((dpad.bottom - l) / touchBox.height) * 100) : null,
      shoulderWidthRatio: Math.round((document.querySelector('.tbtn-shoulder').getBoundingClientRect().width / dpad.width) * 100) / 100,
      startAboveSelect: start.top < select.top,
      drawer: player.classList.contains('drawer-mode'),
      toolbarShown: getComputedStyle(document.querySelector('.toolbar')).display !== 'none',
      fabShown: getComputedStyle(document.querySelector('.menu-fab')).display !== 'none',
      toolbarIcons: document.querySelectorAll('.toolbar svg').length,
    };
  })()`);
  if (name === "landscape") {
    // The menu disc opens the drawer with the game title; the backdrop closes it.
    await evaluate("document.querySelector('.menu-fab').click(); true");
    await sleep(250);
    phone[name].drawerOpen = await evaluate("!document.querySelector('.menu').classList.contains('hidden')");
    phone[name].drawerTitle = await evaluate("document.querySelector('.menu-title').textContent");
    phone[name].drawerActions = await evaluate("document.querySelectorAll('.menu .actions [data-action]').length");
    if (process.env.PIPIT_SMOKE_PHONE_SHOTS) {
      const s = await send("Page.captureScreenshot", { format: "png" });
      writeFileSync(join(process.env.PIPIT_SMOKE_PHONE_SHOTS, "phone-landscape-drawer.png"), Buffer.from(s.data, "base64"));
    }
    await evaluate("document.querySelector('.menu-backdrop').click(); true");
    await sleep(150);
    phone[name].drawerClosed = await evaluate("document.querySelector('.menu').classList.contains('hidden')");
  }
  if (process.env.PIPIT_SMOKE_PHONE_SHOTS) {
    const s = await send("Page.captureScreenshot", { format: "png" });
    writeFileSync(join(process.env.PIPIT_SMOKE_PHONE_SHOTS, `phone-${name}.png`), Buffer.from(s.data, "base64"));
  }
}
await send("Emulation.clearDeviceMetricsOverride");
await send("Emulation.setTouchEmulationEnabled", { enabled: false });
await send("Emulation.setEmulatedMedia", { features: [] });
await evaluate("window.dispatchEvent(new Event('resize')); true");
await sleep(300);
console.log(`phone: ${JSON.stringify(phone)}`);

const shot = await send("Page.captureScreenshot", { format: "png" });
mkdirSync(dirname(screenshot), { recursive: true });
writeFileSync(screenshot, Buffer.from(shot.data, "base64"));
console.log(`screenshot: ${screenshot}`);

const problems = logs.filter((l) => /^(error|exception)/.test(l));
dumpLogs();
ws.close();
if (
  problems.length ||
  litPixels === 0 ||
  !/\d+ fps/.test(fps) ||
  savedToast !== "State 1 saved" ||
  loadedToast !== "State 1 loaded" ||
  slotBefore ||
  !slotAfter ||
  lcdWhite !== 248 ||
  halfWhite !== 251 ||
  rawWhite !== 255 ||
  !controller.present ||
  controller.active !== pads.length > 0 ||
  controller.disabled ||
  !picker.open ||
  (pads.length === 0 ? picker.options[0] !== "No controller detected" : picker.options.length !== pads.length) ||
  !modal.open ||
  modal.rows !== 12 ||
  !modalClosed ||
  keyboardRows !== 12 ||
  fastForwardLabel !== "F" ||
  !fastActive ||
  !fastReleased ||
  phone.portrait.layout !== "gbasp" ||
  phone.landscape.layout !== "gba" ||
  !phone.portrait.touchShown ||
  phone.portrait.overflow > 0 ||
  phone.landscape.overflow > 0 ||
  phone.portrait.shoulderMisalignment > 1 ||
  phone.landscape.shoulderMisalignment > 1 ||
  phone.portrait.groupMisalignment > 1 ||
  phone.landscape.groupMisalignment > 1 ||
  phone.portrait.toolbarIcons < 6 ||
  phone.portrait.drawer ||
  !phone.portrait.toolbarShown ||
  phone.portrait.fabShown ||
  phone.portrait.startAboveSelect ||
  !phone.landscape.drawer ||
  phone.landscape.toolbarShown ||
  !phone.landscape.fabShown ||
  !phone.landscape.startAboveSelect ||
  phone.landscape.shoulderWidthRatio < 0.9 ||
  !phone.landscape.drawerOpen ||
  phone.landscape.drawerTitle !== "nes" ||
  phone.landscape.drawerActions < 5 ||
  !phone.landscape.drawerClosed
) {
  console.error("SMOKE TEST FAILED");
  process.exit(1);
}
console.log("SMOKE TEST PASSED");
process.exit(0);
