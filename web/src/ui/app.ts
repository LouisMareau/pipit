// Top-level UI: library screen and player screen, wired to the emulator worker.

import { exportSave, pickSaveFile } from "../features/saves";
import { takeScreenshot } from "../features/screenshots";
import { AudioOutput } from "../platform/audio";
import { EmulatorClient } from "../platform/emulator-client";
import { Input, keyLabel } from "../platform/input";
import type { Pending, Role, SessionStart } from "../platform/netplay";
import { connect, generateCode, hashRom, LinkSession, listen } from "../platform/netplay";
import { FramePacer } from "../platform/pacer";
import * as storage from "../platform/storage";
import type { ControllerMapping, ResolvedTouchLayout, RomEntry, Settings, TouchLayout } from "../types";
import { DEFAULT_CONNECTION, DEFAULT_KEYBOARD, DEFAULT_MAPPING, Key, STATE_SLOTS } from "../types";
import { ControllerSettings } from "./controller-settings";
import { ControllerToggle } from "./controller-toggle";
import { icon } from "./icons";
import { KeyboardSettings } from "./keyboard-settings";
import { LayoutEditor } from "./layout-editor";
import { Library } from "./library";
import { LinkDialog } from "./link-dialog";
import { Screen } from "./screen";
import { TouchControls } from "./touch-controls";

/** `window.pipit`, for the end-to-end scripts in `scripts/`. */
interface TestHooks {
  /** Keys per game frame on a link, as `frame=KEYS,…` (see `parseKeyScript`). */
  keyScript: string | null;
  save: () => Promise<ArrayBuffer>;
}

/** `300=START,330=,600=A+RIGHT`: the keys held from each frame on (names as in `Key`). */
class ScriptedKeys {
  private readonly entries: [number, number][] = [];

  constructor(script: string) {
    for (const entry of script.split(",")) {
      const [frame, names = ""] = entry.split("=");
      if (!frame?.trim()) continue;
      let keys = 0;
      for (const name of names.split("+")) {
        const key = (Object.keys(Key) as (keyof typeof Key)[]).find((k) => k.toLowerCase() === name.trim().toLowerCase());
        if (key) keys |= Key[key];
      }
      this.entries.push([Number(frame), keys]);
    }
    this.entries.sort((a, b) => a[0] - b[0]);
  }

  /** The keys held at `frame`: the latest entry at or before it. */
  at(frame: number): number {
    let keys = 0;
    for (const [from, value] of this.entries) {
      if (from > frame) break;
      keys = value;
    }
    return keys;
  }
}

export class App {
  private root: HTMLElement;
  private library = new Library();
  private player: HTMLDivElement;
  private screen: Screen;
  private touch = new TouchControls();
  private controller = new ControllerToggle();
  private controllerSettings = new ControllerSettings();
  private keyboardSettings = new KeyboardSettings();
  private layoutEditor: LayoutEditor;
  /** The touch layout in effect after "auto" is resolved from the orientation. */
  private resolvedLayout: ResolvedTouchLayout = "gbasp";
  private emulator: EmulatorClient;
  private audio = new AudioOutput();
  private input = new Input();
  private pacer = new FramePacer();
  private settings: Settings;
  private current: RomEntry | null = null;
  private paused = false;
  private saveTimer: number | null = null;
  private latestSave: ArrayBuffer | null = null;
  private linkDialog = new LinkDialog();
  /** Playing together (see platform/netplay.ts); null when playing alone. */
  private session: LinkSession | null = null;
  /** A connection being made for a session (hosting or joining). */
  private pendingLink: { cancel(): void } | null = null;
  /** Keys the player holds now; on a link they reach the game a few frames later. */
  private heldKeys = 0;
  private stalledTicks = 0;
  /** Test hooks (see scripts/): scripted keys on a link, and a copy of the save. */
  private hooks: TestHooks = { keyScript: null, save: () => this.copyOfSave() };
  private keyScript: ScriptedKeys | null = null;
  private saveWaiters: ((data: ArrayBuffer) => void)[] = [];

  constructor(root: HTMLElement, settings: Settings) {
    this.root = root;
    // Older saved settings may predate some actions: fill the gaps with defaults.
    this.settings = {
      ...settings,
      keyboardMapping: { ...DEFAULT_KEYBOARD, ...settings.keyboardMapping },
      touchLayouts: settings.touchLayouts ?? {},
      connection: { ...DEFAULT_CONNECTION, ...settings.connection },
    };
    this.emulator = new EmulatorClient();
    this.player = document.createElement("div");
    this.player.className = "player hidden";
    const slotButtons = (action: string) =>
      Array.from({ length: STATE_SLOTS }, (_, i) => `<button class="btn" data-action="${action}" data-slot="${i + 1}">${i + 1}</button>`).join("");
    // `.actions` lives in the toolbar, or inside the menu drawer in the landscape
    // touch layout (see applyLayout).
    this.player.innerHTML = `
      <div class="toolbar">
        <div class="actions">
          <button class="btn btn-icon" data-action="back" title="Library" aria-label="Library">${icon("back")}</button>
          <span class="toolbar-spacer"></span>
          <span class="toolbar-controller"></span>
          <button class="btn btn-icon" data-action="pause" title="Pause" aria-label="Pause">${icon("pause")}</button>
          <button class="btn btn-icon" data-action="fast" title="Fast-forward" aria-label="Fast-forward">${icon("fastForward")}</button>
          <button class="btn btn-icon" data-action="shot" title="Screenshot" aria-label="Screenshot">${icon("camera")}</button>
          <button class="btn btn-icon" data-action="menu" title="More" aria-label="More">${icon("more")}</button>
        </div>
      </div>
      <div class="stage">
        <div class="screen-box"></div>
        <button class="menu-fab" data-action="menu" title="Menu" aria-label="Menu">${icon("menu", 22)}</button>
        <div class="link-wait">Waiting for your partner…</div>
      </div>
      <div class="menu-backdrop hidden" data-action="menu"></div>
      <div class="menu hidden">
        <header class="menu-header">
          <h3 class="menu-title"></h3>
          <span class="link-badge">Linked</span>
          <span class="toolbar-fps muted small"></span>
        </header>
        <div class="menu-actions"></div>
        <div class="states">
          <h4>Save states</h4>
          <div class="slots"><span class="small">Save</span>${slotButtons("save-state")}</div>
          <div class="slots"><span class="small">Load</span>${slotButtons("load-state")}</div>
          <p class="muted small">Shift+F1–F3 saves, F1–F3 loads.</p>
        </div>
        <h4>Link cable</h4>
        <button class="btn" data-action="link">Play together…</button>
        <button class="btn hidden" data-action="leave-link">Leave the session</button>
        <label class="row link-view hidden"><span>Watch</span><select data-view></select></label>
        <h4>Keyboard</h4>
        <button class="btn" data-action="keyboard">Change key bindings…</button>
        <h4>Battery save</h4>
        <button class="btn" data-action="export">Export save (.sav)</button>
        <button class="btn" data-action="import">Import save (.sav)</button>
        <h4>Settings</h4>
        <label class="row"><span>Volume</span><input type="range" min="0" max="1" step="0.05" data-setting="volume" /></label>
        <label class="row"><span>Colours</span>
          <select data-setting="colorCorrection">
            <option value="gba">GBA LCD — muted, as on the original screen</option>
            <option value="off">Raw — the palette as stored in the game</option>
          </select>
        </label>
        <label class="row color-strength"><span>Strength</span><input type="range" min="0" max="100" step="5" data-setting="colorStrength" /><span class="small strength-label"></span></label>
        <label class="row"><span>Touch layout</span>
          <select data-setting="touchLayout">
            <option value="auto">Auto (GBA in landscape, GBA SP in portrait)</option>
            <option value="gba">GBA — controls beside the screen</option>
            <option value="gbasp">GBA SP — controls below the screen</option>
          </select>
        </label>
        <button class="btn" data-action="edit-layout">Edit touch layout…</button>
        <label class="row"><input type="checkbox" data-setting="integerScale" /><span>Integer pixel scaling</span></label>
        <label class="row"><input type="checkbox" data-setting="alwaysShowTouch" /><span>Always show touch controls</span></label>
        <button class="btn" data-action="fullscreen">Fullscreen</button>
      </div>`;
    this.screen = new Screen();
    this.player.querySelector(".screen-box")!.append(this.screen.element);
    this.player.querySelector(".toolbar-controller")!.append(this.controller.element);
    this.stage.append(this.touch.element);
    this.layoutEditor = new LayoutEditor(this.stage);
    this.player.append(this.controllerSettings.element, this.keyboardSettings.element, this.linkDialog.element);
    this.root.append(this.library.element, this.player);

    this.library.onPlay = (entry) => this.play(entry);
    this.wireToolbar();
    this.wireSettings();
    this.wireEmulator();
    this.wireController();
    this.wireKeyboard();
    this.wireLayoutEditor();
    this.wireLink();

    this.input.attach(window);
    this.input.onChange = (keys) => {
      this.heldKeys = keys;
      this.emulator.setKeys(keys);
    };
    this.input.onFastForward = (held) => this.setFastForward(held);
    this.input.onPause = () => {
      if (this.current) this.togglePause();
    };
    this.touch.onChange = (keys) => this.input.setTouch(keys);

    // Audio can only start from a user gesture.
    const unlock = () => this.audio.resume();
    window.addEventListener("pointerdown", unlock, { passive: true });
    window.addEventListener("keydown", unlock);

    document.addEventListener("visibilitychange", () => {
      if (document.hidden && this.current && !this.paused) this.emulator.requestSave();
    });
    window.addEventListener("beforeunload", () => this.flushSave());
    window.addEventListener("resize", () => this.applyLayout());
    this.applySettings();
    (window as unknown as { pipit: TestHooks }).pipit = this.hooks;
  }

  async start() {
    await this.library.refresh();
  }

  private async play(entry: RomEntry) {
    const rom = await storage.getRomData(entry.id);
    if (!rom) return;
    const save = await storage.getSave(entry.id);
    this.current = entry;
    this.paused = false;
    this.player.querySelector(".menu-title")!.textContent = entry.name;
    this.setMenuOpen(false);
    this.library.element.classList.add("hidden");
    this.player.classList.remove("hidden");
    this.applyLayout();
    this.screen.fit();
    this.audio.clear();
    this.emulator.load(rom, save, null);
    await storage.touchRom(entry.id);
    await this.refreshStateSlots();
  }

  /** Marks the Save buttons of slots that already hold a state. */
  private async refreshStateSlots() {
    const saved = this.current ? await storage.listStates(this.current.id) : [];
    for (const button of this.player.querySelectorAll<HTMLElement>('[data-action="save-state"]')) {
      const entry = saved.find((s) => s.slot === Number(button.dataset["slot"]));
      button.classList.toggle("has-state", entry !== undefined);
      button.title = entry ? `Saved ${new Date(entry.savedAt).toLocaleString()} — click to overwrite` : "Empty slot";
    }
  }

  private backToLibrary() {
    if (this.session) {
      const session = this.session;
      this.session = null;
      session.leave();
    }
    this.clearLinkUi();
    this.emulator.requestSave();
    this.setRunning(false);
    this.flushSave();
    this.setMenuOpen(false);
    this.current = null;
    this.player.classList.add("hidden");
    this.library.element.classList.remove("hidden");
    void this.library.refresh();
  }

  /** Starts or stops both the worker and the display loop that feeds it. */
  private setRunning(on: boolean) {
    if (on) {
      this.emulator.run();
      this.pacer.start();
    } else {
      this.pacer.stop();
      this.emulator.pause();
    }
  }

  private wireEmulator() {
    this.pacer.onFrame = () => this.onDisplayFrame();
    this.emulator.on("loaded", () => this.setRunning(true));
    this.emulator.on("hash", (frame, hash) => this.session?.reportHash(frame, hash));
    const fpsLabel = this.player.querySelector<HTMLElement>(".toolbar-fps")!;
    let lastFpsText = "";
    this.emulator.on("frame", (pixels, audio, fps, maxGapMs) => {
      this.screen.draw(pixels);
      this.emulator.returnFrame(pixels);
      this.audio.push(new Int16Array(audio));
      const text = `${fps} fps`;
      if (text !== lastFpsText) {
        fpsLabel.textContent = text;
        lastFpsText = text;
      }
      // Worst frame-to-frame gap of the last second, for spotting stutter.
      fpsLabel.dataset["maxGap"] = String(maxGapMs);
      fpsLabel.title = `longest pause between frames in the last second: ${maxGapMs} ms`;
    });
    this.emulator.on("save", (data) => {
      this.latestSave = data;
      for (const resolve of this.saveWaiters.splice(0)) resolve(data.slice(0));
      if (this.saveTimer !== null) clearTimeout(this.saveTimer);
      this.saveTimer = window.setTimeout(() => this.flushSave(), 500);
    });
    this.emulator.on("state", async (slot, data) => {
      if (!this.current) return;
      await storage.putState(this.current.id, slot, data);
      this.toast(`State ${slot} saved`);
      await this.refreshStateSlots();
    });
    this.emulator.on("error", (message) => this.toast(message));
  }

  // Controller toggle: detection drives the button; the button drives the input.
  /** Whether a controller was present at the last detection, for the connect/disconnect notices. */
  private padConnected = false;
  private wireController() {
    this.input.onGamepads = (state) => {
      this.controller.update(state);
      this.input.setMapping(this.mappingFor(this.input.activeGamepadId()));
      // Only a change is worth a notice; switching input on or off is not one.
      const connected = state.active !== null;
      if (connected !== this.padConnected) {
        this.toast(connected ? "Controller connected" : "Controller disconnected", { brief: true });
      }
      this.padConnected = connected;
    };
    this.controller.onToggle = (enabled) => this.input.setGamepadEnabled(enabled);
    this.controller.onSelect = (index) => this.input.setActiveGamepad(index);
    this.controller.onNothingDetected = () =>
      this.toast("No controller detected — press a button on it to wake it up");
    this.controller.onSettings = () => {
      const id = this.input.activeGamepadId();
      const name = id ? id.replace(/\s*\(.*$/, "") : "No controller connected";
      this.pauseForDialog(true);
      this.controllerSettings.open(name, this.mappingFor(id));
    };
    this.controllerSettings.captureButton = () => this.input.captureButton();
    this.controllerSettings.onChange = (mapping) => {
      const id = this.input.activeGamepadId();
      if (id) {
        this.settings = {
          ...this.settings,
          controllerMappings: { ...this.settings.controllerMappings, [id]: mapping },
        };
        void storage.saveSettings(this.settings);
      }
      this.input.setMapping(mapping);
    };
    this.controllerSettings.onClose = () => this.pauseForDialog(false);
    const initial = this.input.gamepadState();
    this.padConnected = initial.active !== null;
    this.controller.update(initial);
  }

  private mappingFor(id: string | null): ControllerMapping {
    return (id && this.settings.controllerMappings[id]) || DEFAULT_MAPPING;
  }

  private wireKeyboard() {
    this.keyboardSettings.captureKey = () => this.input.captureKey();
    this.keyboardSettings.onChange = (mapping) => {
      this.settings = { ...this.settings, keyboardMapping: mapping };
      void storage.saveSettings(this.settings);
      this.applyKeyboard();
    };
    this.keyboardSettings.onClose = () => this.pauseForDialog(false);
  }

  /** Pushes the key bindings to the input layer and refreshes the texts that cite them. */
  private applyKeyboard() {
    const m = this.settings.keyboardMapping;
    this.input.setKeyboardMapping(m);
    const k = (code: string) => keyLabel(code);
    this.library.setKeys([
      { action: "D-pad", keys: [k(m.Up), k(m.Down), k(m.Left), k(m.Right)] },
      { action: "A", keys: [k(m.A)] },
      { action: "B", keys: [k(m.B)] },
      { action: "Start", keys: [k(m.Start)] },
      { action: "Select", keys: [k(m.Select)] },
      { action: "L / R", keys: [k(m.L), k(m.R)] },
      { action: "Fast-forward", keys: [k(m.FastForward)], note: "hold" },
      { action: "Pause", keys: [k(m.Pause)] },
    ]);
  }

  /** Dialogs pause the game, unless the player had paused it already. */
  private dialogPaused = false;
  private pauseForDialog(open: boolean) {
    if (!this.current) return;
    if (open && !this.paused) {
      this.setRunning(false);
      this.dialogPaused = true;
    } else if (!open && this.dialogPaused) {
      this.setRunning(true);
      this.dialogPaused = false;
    }
  }

  private flushSave() {
    if (this.saveTimer !== null) clearTimeout(this.saveTimer);
    this.saveTimer = null;
    if (this.current && this.latestSave) void storage.putSave(this.current.id, this.latestSave);
  }

  private async loadState(slot: number) {
    if (!this.current) return;
    const entry = await storage.getState(this.current.id, slot);
    if (!entry) {
      this.toast(`State ${slot} is empty`);
      return;
    }
    // The buffer is transferred to the worker, so hand over a copy.
    this.emulator.loadState(entry.data.slice(0));
    this.toast(`State ${slot} loaded`);
  }

  // ---------------------------------------------------------------------------
  // Play together (see platform/netplay.ts)
  // ---------------------------------------------------------------------------

  private onDisplayFrame() {
    if (!this.session) {
      this.emulator.requestFrame();
      return;
    }
    // Scripted keys (a test hook) are given per game frame; otherwise whatever
    // is held now goes out for every frame published.
    const script = this.keyScript;
    const tick = this.session.tick(script ? (frame) => script.at(frame) : () => this.heldKeys);
    if (tick) {
      this.stalledTicks = 0;
      this.emulator.requestFrame(tick.keys, tick.frame, tick.guessed);
      // For diagnostics (the smoke test reads them): how far the session has run, and how.
      this.player.dataset["linkFrame"] = String(this.session.frame);
      this.player.dataset["linkDelay"] = String(this.session.delayFrames);
      this.player.dataset["linkRollbacks"] = String(this.session.rollbacks);
    } else {
      this.stalledTicks++;
    }
    // A hiccup of a few frames is invisible; a longer wait gets a notice.
    this.player.classList.toggle("waiting", this.stalledTicks > 15);
  }

  private wireLink() {
    this.player.querySelector<HTMLSelectElement>("[data-view]")!.addEventListener("change", (e) => {
      const player = Number((e.target as HTMLSelectElement).value);
      this.emulator.setView(player);
      this.toast(player === (this.session?.local ?? 0) ? "Back to your screen" : `Watching player ${player + 1}`, { brief: true });
    });
    this.linkDialog.onHost = () => void this.startSession("host", generateCode());
    this.linkDialog.onJoin = (code) => void this.startSession("guest", code);
    this.linkDialog.onStart = () => this.session?.start();
    this.linkDialog.onSettings = (connection) => {
      this.settings = { ...this.settings, connection };
      void storage.saveSettings(this.settings);
    };
    this.linkDialog.onCancel = () => {
      // Cancelling before the game started drops the session.
      this.pendingLink?.cancel();
      this.pendingLink = null;
      if (this.session && !this.player.classList.contains("linked")) {
        const session = this.session;
        this.session = null;
        session.leave();
      }
      this.pauseForDialog(false);
    };
  }

  private openLinkDialog() {
    if (!this.current || this.session) return;
    this.pauseForDialog(true);
    this.linkDialog.open(this.settings.connection);
  }

  private async startSession(role: Role, code: string) {
    if (!this.current || this.session || this.pendingLink) return;
    const entry = this.current;
    this.linkDialog.setBusy(true);
    this.linkDialog.setStatus(role === "host" ? "Setting up…" : "Joining…");
    this.linkDialog.showCode(role === "host" ? code : null);
    this.flushSave();
    const [rom, save] = await Promise.all([storage.getRomData(entry.id), storage.getSave(entry.id)]);
    if (!rom) return;
    const romHash = await hashRom(rom);
    const local = new URLSearchParams(location.search).get("link") === "local";
    let session: LinkSession;
    if (role === "host") {
      const listener = await this.settle(listen(code, local, this.settings.connection), entry);
      if (!listener) return;
      session = LinkSession.host(listener, romHash, save);
    } else {
      const transport = await this.settle(connect(code, local, this.settings.connection), entry);
      if (!transport) return;
      session = LinkSession.join(transport, romHash, save);
    }
    this.session = session;
    session.onStatus = (text) => this.linkDialog.setStatus(text);
    session.onLobby = (players) => this.linkDialog.setPlayers(players);
    session.onPing = (ms) => this.showPing(ms);
    session.onStart = (start) => this.startLinkedGame(rom, start);
    session.onEnd = (reason) => this.endSession(session, reason);
    session.onRollback = (toFrame, inputs, snapshots) => this.emulator.rollback(toFrame, inputs, snapshots);
    session.onConfirm = (frame) => this.emulator.confirm(frame);
    if (role === "host") this.linkDialog.setPlayers(1);
  }

  /** Waits for a connection step; null when it failed (the dialog says why) or was overtaken. */
  private async settle<T extends { close(): void }>(pending: Pending<T>, entry: RomEntry): Promise<T | null> {
    this.pendingLink = pending;
    let ready: T;
    try {
      ready = await pending.ready;
    } catch (error) {
      if (this.pendingLink === pending) this.linkFailed(error instanceof Error ? error.message : String(error));
      return null;
    }
    if (this.pendingLink !== pending || this.current !== entry || this.session) {
      ready.close();
      return null;
    }
    this.pendingLink = null;
    return ready;
  }

  private sessionPlayers = 0;

  private showPing(ms: number) {
    if (!this.session) return;
    const badge = this.player.querySelector(".link-badge")!;
    badge.textContent = this.sessionPlayers
      ? `Linked · ${this.sessionPlayers} players · ${Math.round(ms)} ms · delay ${this.session.delayFrames}`
      : "Linked";
    if (!this.player.classList.contains("linked")) this.linkDialog.setStatus(`Connected, ${Math.round(ms)} ms round trip`);
  }

  /** Both saves are in hand: restart the game as two linked consoles. */
  private startLinkedGame(rom: ArrayBuffer, start: SessionStart) {
    this.keyScript = this.hooks.keyScript ? new ScriptedKeys(this.hooks.keyScript) : null;
    this.linkDialog.close();
    this.dialogPaused = false;
    this.paused = false;
    this.player.querySelector('[data-action="pause"]')!.classList.remove("active");
    this.player.classList.add("linked");
    this.player.querySelector('[data-action="link"]')!.classList.add("hidden");
    this.player.querySelector('[data-action="leave-link"]')!.classList.remove("hidden");
    this.audio.clear();
    this.stalledTicks = 0;
    // The worker reports `loaded`, which starts the display loop.
    this.emulator.load(rom, null, null, start.link, start.epoch);
    this.sessionPlayers = start.link.players;
    this.player.querySelector(".link-badge")!.textContent = `Linked · ${start.link.players} players`;
    // Watching: any console of the link can be shown; keys still go to your own.
    const view = this.player.querySelector<HTMLSelectElement>("[data-view]")!;
    view.replaceChildren(
      ...Array.from({ length: start.link.players }, (_, p) => {
        const option = document.createElement("option");
        option.value = String(p);
        option.textContent = p === start.link.local ? "Your screen" : `Player ${p + 1}`;
        return option;
      }),
    );
    view.value = String(start.link.local);
    view.closest(".link-view")!.classList.remove("hidden");
    this.toast(`Linked: ${start.link.players} players, you are player ${start.link.local + 1}; keys land ${start.delay} frames later`);
  }

  private endSession(session: LinkSession, reason: string) {
    if (this.session !== session) return;
    this.session = null;
    const linked = this.player.classList.contains("linked");
    if (linked) {
      // The two games cannot go on alone from here; the save is kept.
      this.clearLinkUi();
      this.backToLibrary();
      this.toast(`Session over: ${reason}`);
    } else {
      this.linkDialog.setBusy(false);
      this.linkDialog.showCode(null);
      this.linkDialog.setPlayers(0);
      this.linkDialog.setStatus(reason);
    }
  }

  /** The save as the game last wrote it (asks the worker for a fresh copy). */
  private copyOfSave(): Promise<ArrayBuffer> {
    return new Promise((resolve) => {
      this.saveWaiters.push(resolve);
      this.emulator.requestSave();
    });
  }

  private linkFailed(message: string) {
    this.pendingLink = null;
    this.linkDialog.setBusy(false);
    this.linkDialog.showCode(null);
    this.linkDialog.setStatus(message);
  }

  private clearLinkUi() {
    this.pendingLink?.cancel();
    this.pendingLink = null;
    this.sessionPlayers = 0;
    this.linkDialog.close();
    this.player.classList.remove("linked", "waiting");
    this.player.querySelector('[data-action="link"]')!.classList.remove("hidden");
    this.player.querySelector('[data-action="leave-link"]')!.classList.add("hidden");
    this.player.querySelector(".link-view")!.classList.add("hidden");
  }

  /** Shows a notice card; a `brief` one (controller events) stays about a second. */
  private toast(message: string, options: { brief?: boolean } = {}) {
    // One notice at a time: a new one replaces whatever is still showing.
    for (const old of document.querySelectorAll(".toast")) old.remove();
    const el = document.createElement("div");
    el.className = options.brief ? "toast brief" : "toast";
    el.textContent = message;
    document.body.append(el);
    // The card fades in, holds, then slides off the screen; it goes once the slide ends.
    el.addEventListener("animationend", (e) => {
      if (e.animationName === "toast-out") el.remove();
    });
  }

  private setFastForward(on: boolean) {
    // Fast-forwarding alone would leave a link partner behind.
    if (this.session) return;
    this.emulator.setFastForward(on);
    this.player.querySelector('[data-action="fast"]')!.classList.toggle("active", on);
  }

  private togglePause() {
    this.paused = !this.paused;
    this.setRunning(!this.paused);
    this.player.querySelector('[data-action="pause"]')!.classList.toggle("active", this.paused);
  }

  /** The player minus the toolbar: the screen and the touch controls. */
  private get stage(): HTMLElement {
    return this.player.querySelector<HTMLElement>(".stage")!;
  }

  private wireLayoutEditor() {
    this.layoutEditor.onApplied = () => this.screen.fit();
    this.layoutEditor.onDone = (layout) => {
      this.settings = {
        ...this.settings,
        touchLayouts: { ...this.settings.touchLayouts, [this.resolvedLayout]: layout },
      };
      void storage.saveSettings(this.settings);
      this.endLayoutEdit();
      this.toast("Touch layout saved");
    };
    this.layoutEditor.onCancel = () => this.endLayoutEdit();
    this.layoutEditor.onReset = () => {
      const touchLayouts = { ...this.settings.touchLayouts };
      delete touchLayouts[this.resolvedLayout];
      this.settings = { ...this.settings, touchLayouts };
      void storage.saveSettings(this.settings);
      this.endLayoutEdit();
      this.toast("Default layout restored");
    };
  }

  private openLayoutEditor() {
    if (!this.player.classList.contains("touch-on")) {
      this.toast("Touch controls are hidden — turn on “Always show touch controls” first");
      return;
    }
    this.setMenuOpen(false);
    this.pauseForDialog(true);
    // Start from the saved layout, or from wherever the stock layout put things.
    const initial = this.settings.touchLayouts[this.resolvedLayout] ?? this.layoutEditor.measure();
    this.stage.classList.add("custom", "editing");
    this.layoutEditor.open(initial);
  }

  private endLayoutEdit() {
    this.stage.classList.remove("editing");
    this.pauseForDialog(false);
    this.applyLayout();
  }

  private setMenuOpen(open: boolean) {
    this.player.querySelector(".menu")!.classList.toggle("hidden", !open);
    this.player.querySelector(".menu-backdrop")!.classList.toggle("hidden", !open);
  }

  private isMenuOpen(): boolean {
    return !this.player.querySelector(".menu")!.classList.contains("hidden");
  }

  private wireToolbar() {
    this.player.addEventListener("click", async (e) => {
      const button = (e.target as HTMLElement).closest<HTMLElement>("[data-action]");
      if (!button || button.closest(".modal-backdrop")) return;
      const slot = Number(button.dataset["slot"] ?? 0);
      switch (button.dataset["action"]) {
        case "back":
          this.backToLibrary();
          break;
        case "pause":
          this.togglePause();
          break;
        case "fast":
          this.setFastForward(!button.classList.contains("active"));
          break;
        case "shot":
          await takeScreenshot(this.screen, this.current?.name ?? "pipit");
          break;
        case "menu":
          this.setMenuOpen(!this.isMenuOpen());
          break;
        case "keyboard":
          this.setMenuOpen(false);
          this.pauseForDialog(true);
          this.keyboardSettings.open(this.settings.keyboardMapping);
          break;
        case "link":
          this.setMenuOpen(false);
          this.openLinkDialog();
          break;
        case "leave-link":
          this.setMenuOpen(false);
          this.session?.leave();
          break;
        case "edit-layout":
          this.openLayoutEditor();
          break;
        case "save-state":
          this.emulator.saveState(slot);
          break;
        case "load-state":
          await this.loadState(slot);
          break;
        case "export":
          this.emulator.requestSave();
          setTimeout(() => {
            if (this.latestSave && this.current) exportSave(this.latestSave, this.current.name);
          }, 100);
          break;
        case "import": {
          const data = await pickSaveFile();
          if (data && this.current) {
            await storage.putSave(this.current.id, data);
            await this.play(this.current);
          }
          break;
        }
        case "fullscreen":
          if (document.fullscreenElement) await document.exitFullscreen();
          else await this.player.requestFullscreen().catch(() => {});
          break;
      }
    });
    window.addEventListener("keydown", (e) => {
      if (!this.current) return;
      if (e.code === "Escape") this.setMenuOpen(false);
      const fkey = /^F([1-9])$/.exec(e.code);
      if (fkey) {
        const slot = Number(fkey[1]);
        if (slot <= STATE_SLOTS) {
          e.preventDefault();
          if (e.shiftKey) this.emulator.saveState(slot);
          else void this.loadState(slot);
        }
      }
    });
  }

  private wireSettings() {
    for (const el of this.player.querySelectorAll<HTMLInputElement | HTMLSelectElement>("[data-setting]")) {
      const key = el.dataset["setting"] as keyof Settings;
      const handler = () => {
        const value =
          el instanceof HTMLSelectElement ? el.value
          : el.type === "checkbox" ? el.checked
          : Number(el.value);
        this.settings = { ...this.settings, [key]: value };
        this.applySettings();
        void storage.saveSettings(this.settings);
      };
      el.addEventListener(el instanceof HTMLSelectElement ? "change" : "input", handler);
    }
  }

  private applySettings() {
    for (const el of this.player.querySelectorAll<HTMLInputElement | HTMLSelectElement>("[data-setting]")) {
      const key = el.dataset["setting"] as keyof Settings;
      const value = this.settings[key];
      if (el instanceof HTMLSelectElement) el.value = String(value);
      else if (el.type === "checkbox") el.checked = Boolean(value);
      else el.value = String(value);
    }
    const lcd = this.settings.colorCorrection === "gba";
    this.emulator.setColorCorrection(lcd ? 1 : 0, this.settings.colorStrength / 100);
    this.player.querySelector(".strength-label")!.textContent = `${this.settings.colorStrength}%`;
    this.player.querySelector(".color-strength")!.classList.toggle("hidden", !lcd);
    this.audio.setVolume(this.settings.volume);
    this.screen.setIntegerScale(this.settings.integerScale);
    this.applyKeyboard();
    this.applyLayout();
  }

  /** Resolves the touch layout for the current orientation and shows the controls. */
  private applyLayout() {
    const landscape = matchMedia("(orientation: landscape)").matches;
    const coarse = matchMedia("(pointer: coarse)").matches;
    const chosen: TouchLayout = this.settings.touchLayout;
    const layout = chosen === "auto" ? (landscape ? "gba" : "gbasp") : chosen;
    const touchOn = coarse || this.settings.alwaysShowTouch;
    this.player.dataset["layout"] = layout;
    this.player.classList.toggle("touch-on", touchOn);

    // Rotating the device while editing ends the edit: the layouts are per orientation.
    if (this.layoutEditor.isOpen && layout !== this.resolvedLayout) {
      this.layoutEditor.close();
      this.stage.classList.remove("editing");
      this.pauseForDialog(false);
    }
    this.resolvedLayout = layout;

    // Landscape touch layout: no toolbar; the actions live in a drawer opened
    // from the menu disc at the top-right. Everywhere else they stay in the toolbar.
    const drawer = touchOn && layout === "gba";
    this.player.classList.toggle("drawer-mode", drawer);
    document.body.dataset["chrome"] = drawer ? "drawer" : "toolbar";
    const actions = this.player.querySelector(".actions")!;
    const slot = this.player.querySelector(drawer ? ".menu-actions" : ".toolbar")!;
    if (actions.parentElement !== slot) {
      slot.append(actions);
      this.setMenuOpen(false);
    }

    if (this.layoutEditor.isOpen) {
      // Mid-edit (e.g. a resize): keep the working layout on screen.
      this.layoutEditor.reapply();
      return;
    }
    const custom = touchOn ? this.settings.touchLayouts[layout] : undefined;
    const stage = this.stage;
    if (custom) {
      stage.classList.add("custom");
      this.layoutEditor.apply(custom);
    } else {
      stage.classList.remove("custom");
      this.layoutEditor.clear();
      this.screen.fit();
      // Measure after the layout has applied.
      requestAnimationFrame(() => {
        this.touch.fit(layout, stage);
        this.screen.fit();
      });
    }
  }
}
