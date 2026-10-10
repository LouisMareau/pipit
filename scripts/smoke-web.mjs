#!/usr/bin/env node
// End-to-end smoke test for the web app, with no dependencies: drives a headless
// Chrome/Edge over the DevTools protocol, adds a ROM through the real file input,
// starts it, and checks that frames are being drawn and no errors were logged.
//
//   node scripts/smoke-web.mjs <rom.gba> [url] [screenshot.png]
//
// Defaults: url = http://localhost:4173 (vite preview), screenshot = build/smoke.png.

import { mkdirSync, writeFileSync } from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { launch, sleep } from "./lib/cdp.mjs";

const [romArg, url = "http://localhost:4173", shotArg] = process.argv.slice(2);
if (!romArg) {
  console.error("usage: node scripts/smoke-web.mjs <rom.gba> [url] [screenshot.png]");
  process.exit(2);
}
const rom = resolve(romArg);
const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const screenshot = resolve(shotArg ?? join(root, "build", "smoke.png"));
// The library names a game after its file, minus the extension.
const romTitle = basename(rom).replace(/\.gba$/i, "");

const { send, evaluate, pressKey, logs, close, newWindow } = await launch(url);
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

// Landing page: a GBA tab (GBC hidden for now), an "Add a .gba ROM" button with an
// (i) popover instead of a caption, and the keyboard summary as an info box that
// fits the width.
const landingCheck = `(() => {
  const help = document.querySelector('.keys-help');
  const pop = document.querySelector('.info-pop');
  const hidden = (el) => !el || getComputedStyle(el).display === 'none';
  const before = hidden(pop);
  document.querySelector('.info-btn').click();
  const open = !hidden(pop);
  document.body.click();
  return {
    activeTab: document.querySelector('.tabs .tab.active')?.textContent,
    gbcHidden: hidden(document.querySelector('.tab[data-tab=gbc]')),
    addLabel: document.querySelector('.add-rom .btn')?.textContent.replace(/\s+/g, ' ').trim(),
    popHiddenBefore: before,
    popOpensOnTap: open,
    popClosesOutside: hidden(pop),
    kbd: document.querySelectorAll('.keys-help kbd').length,
    helpOverflow: help.scrollWidth - help.clientWidth,
    pageOverflow: document.documentElement.scrollWidth - window.innerWidth,
  };
})()`;
const landing = await evaluate(landingCheck);
console.log(`landing: ${JSON.stringify(landing)}`);
if (process.env.PIPIT_SMOKE_LIBRARY_SHOTS) {
  const s = await send("Page.captureScreenshot", { format: "png" });
  writeFileSync(join(process.env.PIPIT_SMOKE_LIBRARY_SHOTS, "library-desktop.png"), Buffer.from(s.data, "base64"));
}
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
// Notices are cards that animate in and slide off; the element goes with the slide.
const toastAnimation = await evaluate("getComputedStyle(document.querySelector('.toast')).animationName");
await sleep(3000);
const toastGone = await evaluate("document.querySelectorAll('.toast').length === 0");
console.log(`save state: "${savedToast}" / "${loadedToast}"; toast animation "${toastAnimation}", gone after 3 s: ${toastGone}`);

// Play together: a second tab joins the first with its code. Both then run the two
// consoles in lockstep and compare state digests every second, so staying linked
// for a few seconds (with keys pressed on one side) proves the inputs and the
// emulation agree. When the guest leaves, the host is told and returns to the library.
const guest = await newWindow(url);
const guest2 = await newWindow(url);
await sleep(1500);
for (const page of [guest, guest2]) await page.evaluate("document.querySelector('.rom-main').click(); true");
await sleep(1500);
const openLinkDialog = (page) =>
  page.evaluate("document.querySelector('[data-action=menu]').click(); document.querySelector('.menu [data-action=link]').click(); true");
await openLinkDialog({ evaluate });
await evaluate("document.querySelector('[data-action=host]').click(); true");
await sleep(400);
const code = await evaluate("document.querySelector('.link-code').textContent");
for (const page of [guest, guest2]) {
  await openLinkDialog(page);
  await page.evaluate(
    `document.querySelector('.link-input').value = ${JSON.stringify(code)}; document.querySelector('[data-action=join-form]').requestSubmit(); true`,
  );
}
await sleep(1500);
const lobby = await evaluate(
  "({ players: document.querySelector('.link-players').textContent, canStart: !document.querySelector('[data-action=start]').disabled })",
);
console.log(`lobby: ${JSON.stringify(lobby)}`);
await evaluate("document.querySelector('[data-action=start]').click(); true");
await sleep(4500);
const linkState = (page = { evaluate }) =>
  page.evaluate(
    `(() => { const p = document.querySelector('.player'); const badge = document.querySelector('.link-badge'); return { linked: p.classList.contains('linked'), waiting: p.classList.contains('waiting'),
      badge: getComputedStyle(badge).display !== 'none', badgeText: badge.textContent, fps: document.querySelector('.toolbar-fps').textContent,
      frame: Number(p.dataset.linkFrame), delay: Number(p.dataset.linkDelay), rollbacks: Number(p.dataset.linkRollbacks),
      toast: document.querySelector('.toast')?.textContent ?? '' }; })()`,
  );
const linkHost = await linkState();
const linkGuest = await linkState(guest);
const linkGuest2 = await linkState(guest2);
await sleep(1000);
const later = await Promise.all([linkState(), linkState(guest), linkState(guest2)]);
console.log(`link (code ${code}): host ${JSON.stringify(linkHost)} guest ${JSON.stringify(linkGuest)} guest2 ${JSON.stringify(linkGuest2)}`);
console.log(`link a second later: frames ${later.map((s) => `${s.frame} (${s.waiting ? "waiting" : "running"})`).join(", ")}`);
await guest2.send("Input.dispatchKeyEvent", { type: "keyDown", code: "ArrowRight", key: "ArrowRight", windowsVirtualKeyCode: 39 });
await sleep(600);
await guest2.send("Input.dispatchKeyEvent", { type: "keyUp", code: "ArrowRight", key: "ArrowRight", windowsVirtualKeyCode: 39 });
await sleep(2500);
const linkAfterKeys = await linkState();
// Watching: the host switches to player 2's console and back; the session must not notice.
const watch = (player) => evaluate(`(() => { const s = document.querySelector('[data-view]'); s.value = '${player}'; s.dispatchEvent(new Event('change')); return s.value; })()`);
await watch(1);
await sleep(1000);
const whileWatching = await linkState();
await watch(0);
const laggy = /[?&]lag=/.test(url);
console.log(`watching player 2: frame ${whileWatching.frame} (${whileWatching.linked ? "linked" : "not linked"}); ${laggy ? `lagged run: delay ${linkHost.delay} → ${linkAfterKeys.delay}, rollbacks ${linkAfterKeys.rollbacks}` : "no lag"}`);
// One guest leaves: the session is over for everyone.
await guest.evaluate("document.querySelector('[data-action=menu]').click(); document.querySelector('.menu [data-action=leave-link]').click(); true");
await sleep(800);
const afterLeave = "({ library: !document.querySelector('.library').classList.contains('hidden'), toast: document.querySelector('.toast')?.textContent ?? '' })";
const hostAfterLeave = await evaluate(afterLeave);
const guest2AfterLeave = await guest2.evaluate(afterLeave);
console.log(`link after keys: ${JSON.stringify(linkAfterKeys)}; after a guest left: host ${JSON.stringify(hostAfterLeave)}, other guest ${JSON.stringify(guest2AfterLeave)}`);
await guest.closeWindow();
await guest2.closeWindow();
await evaluate("document.querySelector('.rom-main').click(); true");
await sleep(2500);

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
    const pills = Array.from(document.querySelectorAll('.tbtn-pill')).map((e) => {
      const b = e.getBoundingClientRect();
      return { key: e.textContent, top: Math.round(b.top), left: Math.round(b.left), right: Math.round(b.right) };
    });
    const start = pills.find((p) => p.key === 'start');
    const select = pills.find((p) => p.key === 'select');
    const screen = document.querySelector('.screen-box').getBoundingClientRect();
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
      selectLeftOfStart: select.left < start.left,
      pillsSameRow: Math.abs(start.top - select.top) <= 1,
      // Landscape: Select ends where the screen column begins, Start begins where it ends.
      pillsHugScreen: select.right <= Math.round(screen.left) + 1 && start.left >= Math.round(screen.right) - 1,
      drawer: player.classList.contains('drawer-mode'),
      toolbarShown: getComputedStyle(document.querySelector('.toolbar')).display !== 'none',
      fabShown: getComputedStyle(document.querySelector('.menu-fab')).display !== 'none',
      toolbarIcons: document.querySelectorAll('.toolbar svg').length,
    };
  })()`);
  if (name === "portrait") {
    // The editor is reachable from the toolbar menu in portrait too; Cancel leaves no trace.
    await evaluate("document.querySelector('.toolbar [data-action=menu]').click(); true");
    await sleep(200);
    await evaluate("document.querySelector('[data-action=edit-layout]').click(); true");
    await sleep(300);
    const editing = await evaluate("document.querySelector('.stage').classList.contains('editing')");
    const handles = await evaluate("document.querySelectorAll('.edit-box').length");
    if (process.env.PIPIT_SMOKE_PHONE_SHOTS) {
      const s = await send("Page.captureScreenshot", { format: "png" });
      writeFileSync(join(process.env.PIPIT_SMOKE_PHONE_SHOTS, "phone-portrait-editor.png"), Buffer.from(s.data, "base64"));
    }
    await evaluate("document.querySelector('[data-edit=cancel]').click(); true");
    await sleep(300);
    const cancelled = await evaluate(
      "(() => { const s = document.querySelector('.stage'); return !s.classList.contains('custom') && !s.classList.contains('editing'); })()",
    );
    phone[name].editor = { editing, handles, cancelled };
  }
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

    // Layout editor: open it from the menu, drag the D-pad 40 px to the right,
    // save, check the move stuck, then reset to the stock layout.
    const openEditor = async () => {
      await evaluate("document.querySelector('.menu-fab').click(); true");
      await sleep(200);
      await evaluate("document.querySelector('[data-action=edit-layout]').click(); true");
      await sleep(300);
    };
    const dpadLeft = () => evaluate("document.querySelector('.dpad').getBoundingClientRect().left");
    await openEditor();
    const editing = await evaluate("document.querySelector('.stage').classList.contains('editing')");
    const handles = await evaluate("document.querySelectorAll('.edit-box').length");
    const before = await dpadLeft();
    const handle = await evaluate(
      "(() => { const r = document.querySelector('.edit-box[data-id=dpad]').getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()",
    );
    const drag = (type, dx, buttons) =>
      send("Input.dispatchMouseEvent", { type, x: handle.x + dx, y: handle.y, button: "left", buttons, clickCount: 1 });
    await drag("mousePressed", 0, 1);
    await drag("mouseMoved", 20, 1);
    await drag("mouseMoved", 40, 1);
    await drag("mouseReleased", 40, 0);
    await sleep(150);
    const moved = Math.round((await dpadLeft()) - before);
    if (process.env.PIPIT_SMOKE_PHONE_SHOTS) {
      const s = await send("Page.captureScreenshot", { format: "png" });
      writeFileSync(join(process.env.PIPIT_SMOKE_PHONE_SHOTS, "phone-landscape-editor.png"), Buffer.from(s.data, "base64"));
    }
    await evaluate("document.querySelector('[data-edit=done]').click(); true");
    await sleep(300);
    const saved = await evaluate(
      "(() => { const s = document.querySelector('.stage'); return { custom: s.classList.contains('custom'), editing: s.classList.contains('editing') }; })()",
    );
    const savedMoved = Math.round((await dpadLeft()) - before);
    await openEditor();
    await evaluate("document.querySelector('[data-edit=reset]').click(); true");
    await sleep(300);
    const resetCustom = await evaluate("document.querySelector('.stage').classList.contains('custom')");
    phone[name].editor = { editing, handles, moved, savedCustom: saved.custom, savedEditing: saved.editing, savedMoved, resetCustom };
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

// Back to the library on a phone: the landing page has to fit that width too.
await send("Emulation.setDeviceMetricsOverride", { width: 390, height: 844, deviceScaleFactor: 2, mobile: true });
await evaluate("document.querySelector('[data-action=back]').click(); true");
await sleep(500);
const landingPhone = await evaluate(landingCheck);
console.log(`landing (phone): ${JSON.stringify(landingPhone)}`);
if (process.env.PIPIT_SMOKE_LIBRARY_SHOTS) {
  const s = await send("Page.captureScreenshot", { format: "png" });
  writeFileSync(join(process.env.PIPIT_SMOKE_LIBRARY_SHOTS, "library-phone.png"), Buffer.from(s.data, "base64"));
}
await send("Emulation.clearDeviceMetricsOverride");

const problems = logs.filter((l) => /^(error|exception)/.test(l));
const landingOk = (l) =>
  l.activeTab === "GBA" &&
  l.gbcHidden &&
  l.addLabel === "Add a .gba ROM" &&
  l.popHiddenBefore &&
  l.popOpensOnTap &&
  l.popClosesOutside &&
  l.kbd === 12 &&
  l.helpOverflow <= 0 &&
  l.pageOverflow <= 0;
dumpLogs();
close();
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
  !phone.portrait.selectLeftOfStart ||
  !phone.portrait.pillsSameRow ||
  !phone.portrait.editor.editing ||
  phone.portrait.editor.handles !== 7 ||
  !phone.portrait.editor.cancelled ||
  !phone.landscape.drawer ||
  phone.landscape.toolbarShown ||
  !phone.landscape.fabShown ||
  !phone.landscape.selectLeftOfStart ||
  !phone.landscape.pillsSameRow ||
  !phone.landscape.pillsHugScreen ||
  phone.landscape.shoulderWidthRatio < 0.9 ||
  !phone.landscape.editor.editing ||
  phone.landscape.editor.handles !== 7 ||
  Math.abs(phone.landscape.editor.moved - 40) > 2 ||
  !phone.landscape.editor.savedCustom ||
  phone.landscape.editor.savedEditing ||
  Math.abs(phone.landscape.editor.savedMoved - 40) > 2 ||
  phone.landscape.editor.resetCustom ||
  !phone.landscape.drawerOpen ||
  phone.landscape.drawerTitle !== romTitle ||
  phone.landscape.drawerActions < 5 ||
  !phone.landscape.drawerClosed ||
  !/toast-in/.test(toastAnimation) ||
  !toastGone ||
  !landingOk(landing) ||
  !landingOk(landingPhone) ||
  !lobby.canStart ||
  !linkHost.linked ||
  !linkGuest.linked ||
  !linkGuest2.linked ||
  linkHost.waiting ||
  linkGuest.waiting ||
  linkGuest2.waiting ||
  !linkHost.badge ||
  !/3 players/.test(linkHost.badgeText) ||
  /Session over/.test(linkHost.toast) ||
  !linkAfterKeys.linked ||
  !whileWatching.linked ||
  whileWatching.frame <= linkAfterKeys.frame ||
  (laggy && (linkAfterKeys.delay <= linkHost.delay || linkAfterKeys.rollbacks < 1)) ||
  !hostAfterLeave.library ||
  !/player left/.test(hostAfterLeave.toast) ||
  !guest2AfterLeave.library
) {
  console.error("SMOKE TEST FAILED");
  process.exit(1);
}
console.log("SMOKE TEST PASSED");
process.exit(0);
