// Headless Chrome over the DevTools protocol, for the end-to-end scripts.
//
// `launch(url)` starts a browser with a throwaway profile, opens the page and
// returns helpers bound to it; `newWindow(url)` opens a second page as its own
// window (a background tab would get no animation frames, which pace the
// emulator). Node 22+ (global WebSocket).

import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const CANDIDATES = [
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

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export async function launch(url, { width = 900, height = 700 } = {}) {
  const browser = CANDIDATES.find((p) => existsSync(p));
  if (!browser) {
    console.error("no Chrome or Edge found; set PIPIT_BROWSER to a browser executable");
    process.exit(2);
  }
  const port = 9333 + Math.floor(Math.random() * 1000);
  const profile = mkdtempSync(join(tmpdir(), "pipit-e2e-"));
  const child = spawn(
    browser,
    [
      "--headless=new",
      `--remote-debugging-port=${port}`,
      `--user-data-dir=${profile}`,
      "--no-first-run",
      "--no-default-browser-check",
      "--autoplay-policy=no-user-gesture-required",
      `--window-size=${width},${height}`,
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
  process.on("exit", () => {
    child.kill();
    setTimeout(() => rmSync(profile, { recursive: true, force: true }), 500);
  });

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
  /** Console output and errors from every page, as `level: text` lines. */
  const logs = [];
  ws.onmessage = (e) => {
    const msg = JSON.parse(e.data);
    if (msg.id) {
      const waiter = pending.get(msg.id);
      if (!waiter) return;
      pending.delete(msg.id);
      msg.error ? waiter.reject(new Error(msg.error.message)) : waiter.resolve(msg.result);
    } else if (msg.method === "Runtime.consoleAPICalled") {
      logs.push(`${msg.params.type}: ${msg.params.args.map((a) => a.value ?? a.description ?? "").join(" ")}`);
    } else if (msg.method === "Runtime.exceptionThrown") {
      logs.push(`exception: ${msg.params.exceptionDetails.text} ${msg.params.exceptionDetails.exception?.description ?? ""}`);
    } else if (msg.method === "Log.entryAdded" && msg.params.entry.level === "error") {
      logs.push(`error: ${msg.params.entry.text} ${msg.params.entry.url ?? ""}`);
    }
  };
  // `sessionId` addresses another page attached with Target.attachToTarget (flatten mode).
  const send = (method, params = {}, sessionId = undefined) =>
    new Promise((resolve, reject) => {
      const id = nextId++;
      pending.set(id, { resolve, reject });
      ws.send(JSON.stringify({ id, method, params, sessionId }));
    });
  const helpers = (sessionId) => ({
    send: (method, params = {}) => send(method, params, sessionId),
    evaluate: async (expression) => {
      const r = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true }, sessionId);
      if (r.exceptionDetails) throw new Error(r.exceptionDetails.text);
      return r.result.value;
    },
    pressKey: async (code, key, keyCode, modifiers = 0) => {
      await send("Input.dispatchKeyEvent", { type: "keyDown", code, key, windowsVirtualKeyCode: keyCode, modifiers }, sessionId);
      await send("Input.dispatchKeyEvent", { type: "keyUp", code, key, windowsVirtualKeyCode: keyCode, modifiers }, sessionId);
    },
    screenshot: async () => Buffer.from((await send("Page.captureScreenshot", { format: "png" }, sessionId)).data, "base64"),
  });
  const enable = async (sessionId) => {
    await send("Page.enable", {}, sessionId);
    await send("Runtime.enable", {}, sessionId);
    await send("Log.enable", {}, sessionId);
  };

  await enable();
  await send("Page.navigate", { url });
  return {
    ...helpers(undefined),
    logs,
    send,
    close: () => ws.close(),
    /** Opens `url` in a new window and returns helpers bound to it. */
    newWindow: async (windowUrl) => {
      const { targetId } = await send("Target.createTarget", { url: "about:blank", newWindow: true });
      const { sessionId } = await send("Target.attachToTarget", { targetId, flatten: true });
      await enable(sessionId);
      await send("Page.navigate", { url: windowUrl }, sessionId);
      return { ...helpers(sessionId), sessionId, closeWindow: () => send("Target.closeTarget", { targetId }) };
    },
  };
}
