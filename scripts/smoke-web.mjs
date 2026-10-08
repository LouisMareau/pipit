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

const fps = await evaluate("document.querySelector('.toolbar-fps').textContent");
// No controller in a headless browser: the toggle renders gray but clickable.
const controller = await evaluate(`(() => {
  const b = document.querySelector('.controller-toggle');
  return b ? { present: true, active: b.classList.contains('active'), disabled: b.disabled } : { present: false };
})()`);
console.log(`controller toggle: ${JSON.stringify(controller)}`);

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
const litPixels = await evaluate(`(() => {
  const c = document.querySelector('canvas');
  const d = c.getContext('2d').getImageData(0, 0, c.width, c.height).data;
  let n = 0;
  for (let i = 0; i < d.length; i += 4) if (d[i] | d[i + 1] | d[i + 2]) n++;
  return n;
})()`);
console.log(`fps counter: "${fps}"; non-black pixels: ${litPixels}`);

// Save states: Shift+F1 saves slot 1, F1 loads it; each shows a toast.
const pressKey = async (code, key, keyCode, modifiers = 0) => {
  await send("Input.dispatchKeyEvent", { type: "keyDown", code, key, windowsVirtualKeyCode: keyCode, modifiers });
  await send("Input.dispatchKeyEvent", { type: "keyUp", code, key, windowsVirtualKeyCode: keyCode, modifiers });
};
const lastToast = () => evaluate("Array.from(document.querySelectorAll('.toast')).at(-1)?.textContent ?? ''");
await pressKey("F1", "F1", 112, 8);
await sleep(800);
const savedToast = await lastToast();
await pressKey("F1", "F1", 112);
await sleep(800);
const loadedToast = await lastToast();
console.log(`save state: "${savedToast}" / "${loadedToast}"`);

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
  !controller.present ||
  controller.active ||
  controller.disabled ||
  !picker.open ||
  picker.options[0] !== "No controller detected"
) {
  console.error("SMOKE TEST FAILED");
  process.exit(1);
}
console.log("SMOKE TEST PASSED");
process.exit(0);
