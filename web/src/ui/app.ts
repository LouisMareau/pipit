// Top-level UI: library screen and player screen, wired to the emulator worker.

import { exportSave, pickSaveFile } from "../features/saves";
import { takeScreenshot } from "../features/screenshots";
import { AudioOutput } from "../platform/audio";
import { EmulatorClient } from "../platform/emulator-client";
import { Input, keyLabel } from "../platform/input";
import { FramePacer } from "../platform/pacer";
import * as storage from "../platform/storage";
import type { ControllerMapping, RomEntry, Settings, TouchLayout } from "../types";
import { DEFAULT_KEYBOARD, DEFAULT_MAPPING, STATE_SLOTS } from "../types";
import { ControllerSettings } from "./controller-settings";
import { ControllerToggle } from "./controller-toggle";
import { icon } from "./icons";
import { KeyboardSettings } from "./keyboard-settings";
import { Library } from "./library";
import { Screen } from "./screen";
import { TouchControls } from "./touch-controls";

export class App {
  private root: HTMLElement;
  private library = new Library();
  private player: HTMLDivElement;
  private screen: Screen;
  private touch = new TouchControls();
  private controller = new ControllerToggle();
  private controllerSettings = new ControllerSettings();
  private keyboardSettings = new KeyboardSettings();
  private emulator: EmulatorClient;
  private audio = new AudioOutput();
  private input = new Input();
  private pacer = new FramePacer();
  private settings: Settings;
  private current: RomEntry | null = null;
  private paused = false;
  private saveTimer: number | null = null;
  private latestSave: ArrayBuffer | null = null;

  constructor(root: HTMLElement, settings: Settings) {
    this.root = root;
    // Older saved settings may predate some actions: fill the gaps with defaults.
    this.settings = { ...settings, keyboardMapping: { ...DEFAULT_KEYBOARD, ...settings.keyboardMapping } };
    this.emulator = new EmulatorClient();
    this.player = document.createElement("div");
    this.player.className = "player hidden";
    const slotButtons = (action: string) =>
      Array.from({ length: STATE_SLOTS }, (_, i) => `<button class="btn" data-action="${action}" data-slot="${i + 1}">${i + 1}</button>`).join("");
    this.player.innerHTML = `
      <div class="toolbar">
        <button class="btn btn-icon" data-action="back" title="Library" aria-label="Library">${icon("back")}</button>
        <span class="toolbar-title"></span>
        <span class="toolbar-fps muted small"></span>
        <span class="toolbar-spacer"></span>
        <span class="toolbar-controller"></span>
        <button class="btn btn-icon" data-action="pause" title="Pause" aria-label="Pause">${icon("pause")}</button>
        <button class="btn btn-icon" data-action="fast" title="Fast-forward" aria-label="Fast-forward">${icon("fastForward")}</button>
        <button class="btn btn-icon" data-action="shot" title="Screenshot" aria-label="Screenshot">${icon("camera")}</button>
        <button class="btn btn-icon" data-action="menu" title="More" aria-label="More">${icon("more")}</button>
      </div>
      <div class="screen-box"></div>
      <div class="menu hidden">
        <h4>Save states</h4>
        <div class="slots"><span class="small">Save</span>${slotButtons("save-state")}</div>
        <div class="slots"><span class="small">Load</span>${slotButtons("load-state")}</div>
        <p class="muted small">Shift+F1–F3 saves, F1–F3 loads.</p>
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
        <label class="row"><input type="checkbox" data-setting="integerScale" /><span>Integer pixel scaling</span></label>
        <label class="row"><input type="checkbox" data-setting="alwaysShowTouch" /><span>Always show touch controls</span></label>
        <button class="btn" data-action="fullscreen">Fullscreen</button>
      </div>`;
    this.screen = new Screen();
    this.player.querySelector(".screen-box")!.append(this.screen.element);
    this.player.querySelector(".toolbar-controller")!.append(this.controller.element);
    this.player.append(this.touch.element, this.controllerSettings.element, this.keyboardSettings.element);
    this.root.append(this.library.element, this.player);

    this.library.onPlay = (entry) => this.play(entry);
    this.wireToolbar();
    this.wireSettings();
    this.wireEmulator();
    this.wireController();
    this.wireKeyboard();

    this.input.attach(window);
    this.input.onChange = (keys) => this.emulator.setKeys(keys);
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
    this.player.querySelector(".toolbar-title")!.textContent = entry.name;
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
    this.emulator.requestSave();
    this.setRunning(false);
    this.flushSave();
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
    this.pacer.onFrame = () => this.emulator.requestFrame();
    this.emulator.on("loaded", () => this.setRunning(true));
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
  private wireController() {
    this.input.onGamepads = (state) => {
      this.controller.update(state);
      this.input.setMapping(this.mappingFor(this.input.activeGamepadId()));
      if (state.enabled && state.active !== null) this.toast("Controller connected");
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
    this.controller.update(this.input.gamepadState());
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
    this.library.setHelp(
      `Keyboard: ${k(m.Up)}${k(m.Down)}${k(m.Left)}${k(m.Right)} = D-pad · ${k(m.A)} = A · ${k(m.B)} = B · ` +
        `${k(m.Start)} = Start · ${k(m.Select)} = Select · ${k(m.L)}/${k(m.R)} = L/R · ` +
        `hold ${k(m.FastForward)} to fast-forward · ${k(m.Pause)} pauses. ` +
        `Change them in the player's ⋯ menu. Controllers work too: press a button on one.`,
    );
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

  private toast(message: string) {
    const el = document.createElement("div");
    el.className = "toast";
    el.textContent = message;
    document.body.append(el);
    el.addEventListener("animationend", () => el.remove());
  }

  private setFastForward(on: boolean) {
    this.emulator.setFastForward(on);
    this.player.querySelector('[data-action="fast"]')!.classList.toggle("active", on);
  }

  private togglePause() {
    this.paused = !this.paused;
    this.setRunning(!this.paused);
    this.player.querySelector('[data-action="pause"]')!.classList.toggle("active", this.paused);
  }

  private wireToolbar() {
    const menu = this.player.querySelector<HTMLDivElement>(".menu")!;
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
          menu.classList.toggle("hidden");
          break;
        case "keyboard":
          menu.classList.add("hidden");
          this.pauseForDialog(true);
          this.keyboardSettings.open(this.settings.keyboardMapping);
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
    this.player.dataset["layout"] = layout;
    this.player.classList.toggle("touch-on", coarse || this.settings.alwaysShowTouch);
    this.screen.fit();
    // Measure after the layout has applied.
    requestAnimationFrame(() => this.touch.fit(layout, this.player));
  }
}
