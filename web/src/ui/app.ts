// Top-level UI: library screen and player screen, wired to the emulator worker.

import { exportSave, pickSaveFile } from "../features/saves";
import { takeScreenshot } from "../features/screenshots";
import { AudioOutput } from "../platform/audio";
import { EmulatorClient } from "../platform/emulator-client";
import { Input } from "../platform/input";
import * as storage from "../platform/storage";
import type { RomEntry, Settings } from "../types";
import { STATE_SLOTS } from "../types";
import { Library } from "./library";
import { Screen } from "./screen";
import { TouchControls } from "./touch-controls";

export class App {
  private root: HTMLElement;
  private library = new Library();
  private player: HTMLDivElement;
  private screen: Screen;
  private touch = new TouchControls();
  private emulator: EmulatorClient;
  private audio = new AudioOutput();
  private input = new Input();
  private settings: Settings;
  private current: RomEntry | null = null;
  private paused = false;
  private saveTimer: number | null = null;
  private latestSave: ArrayBuffer | null = null;

  constructor(root: HTMLElement, settings: Settings) {
    this.root = root;
    this.settings = settings;
    this.emulator = new EmulatorClient(settings.rewindSeconds);
    this.player = document.createElement("div");
    this.player.className = "player hidden";
    const slotButtons = (action: string) =>
      Array.from({ length: STATE_SLOTS }, (_, i) => `<button class="btn" data-action="${action}" data-slot="${i + 1}">${i + 1}</button>`).join("");
    this.player.innerHTML = `
      <div class="toolbar">
        <button class="btn btn-icon" data-action="back" title="Library">‹</button>
        <span class="toolbar-title"></span>
        <span class="toolbar-fps muted small"></span>
        <span class="toolbar-spacer"></span>
        <button class="btn btn-icon" data-action="pause" title="Pause (P)">❚❚</button>
        <button class="btn btn-icon" data-action="fast" title="Fast-forward (hold Space)">»</button>
        <button class="btn btn-icon" data-action="shot" title="Screenshot">📷</button>
        <button class="btn btn-icon" data-action="menu" title="More">⋯</button>
      </div>
      <div class="screen-box"></div>
      <div class="menu hidden">
        <h4>Save states</h4>
        <div class="slots"><span class="small">Save</span>${slotButtons("save-state")}</div>
        <div class="slots"><span class="small">Load</span>${slotButtons("load-state")}</div>
        <p class="muted small">Shift+F1–F3 saves, F1–F3 loads. Hold R to rewind.</p>
        <h4>Battery save</h4>
        <button class="btn" data-action="export">Export save (.sav)</button>
        <button class="btn" data-action="import">Import save (.sav)</button>
        <h4>Settings</h4>
        <label class="row"><span>Volume</span><input type="range" min="0" max="1" step="0.05" data-setting="volume" /></label>
        <label class="row"><span>Rewind</span><input type="range" min="0" max="10" step="1" data-setting="rewindSeconds" /><span class="small rewind-label"></span></label>
        <label class="row"><input type="checkbox" data-setting="integerScale" /><span>Integer pixel scaling</span></label>
        <label class="row"><input type="checkbox" data-setting="alwaysShowTouch" /><span>Always show touch controls</span></label>
        <button class="btn" data-action="fullscreen">Fullscreen</button>
      </div>`;
    this.screen = new Screen();
    this.player.querySelector(".screen-box")!.append(this.screen.element);
    this.player.append(this.touch.element);
    this.root.append(this.library.element, this.player);

    this.library.onPlay = (entry) => this.play(entry);
    this.wireToolbar();
    this.wireSettings();
    this.wireEmulator();

    this.input.attach(window);
    this.input.onChange = (keys) => this.emulator.setKeys(keys);
    this.input.onFastForward = (held) => this.setFastForward(held);
    this.input.onRewind = (held) => this.emulator.setRewind(held);
    this.touch.onChange = (keys) => this.input.setTouch(keys);
    this.touch.onRewind = (held) => this.emulator.setRewind(held);

    // Audio can only start from a user gesture.
    const unlock = () => this.audio.resume();
    window.addEventListener("pointerdown", unlock, { passive: true });
    window.addEventListener("keydown", unlock);

    document.addEventListener("visibilitychange", () => {
      if (document.hidden && this.current && !this.paused) this.emulator.requestSave();
    });
    window.addEventListener("beforeunload", () => this.flushSave());
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
    this.screen.fit();
    this.audio.clear();
    this.emulator.load(rom, save, null);
    await storage.touchRom(entry.id);
  }

  private backToLibrary() {
    this.emulator.requestSave();
    this.emulator.pause();
    this.flushSave();
    this.current = null;
    this.player.classList.add("hidden");
    this.library.element.classList.remove("hidden");
    void this.library.refresh();
  }

  private wireEmulator() {
    this.emulator.on("loaded", () => this.emulator.run());
    this.emulator.on("frame", (pixels, audio, fps) => {
      this.screen.draw(pixels);
      this.emulator.returnFrame(pixels);
      this.audio.push(new Int16Array(audio));
      this.player.querySelector(".toolbar-fps")!.textContent = `${fps} fps`;
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
    });
    this.emulator.on("error", (message) => this.toast(message));
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
    if (this.paused) this.emulator.pause();
    else this.emulator.run();
    this.player.querySelector('[data-action="pause"]')!.classList.toggle("active", this.paused);
  }

  private wireToolbar() {
    const menu = this.player.querySelector<HTMLDivElement>(".menu")!;
    this.player.addEventListener("click", async (e) => {
      const button = (e.target as HTMLElement).closest<HTMLElement>("[data-action]");
      if (!button) return;
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
      if (e.code === "KeyP") this.togglePause();
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
    for (const el of this.player.querySelectorAll<HTMLInputElement>("[data-setting]")) {
      const key = el.dataset["setting"] as keyof Settings;
      el.addEventListener("input", () => {
        const value = el.type === "checkbox" ? el.checked : Number(el.value);
        this.settings = { ...this.settings, [key]: value };
        this.applySettings();
        void storage.saveSettings(this.settings);
      });
    }
  }

  private applySettings() {
    for (const el of this.player.querySelectorAll<HTMLInputElement>("[data-setting]")) {
      const key = el.dataset["setting"] as keyof Settings;
      const value = this.settings[key];
      if (el.type === "checkbox") el.checked = Boolean(value);
      else el.value = String(value);
    }
    this.player.querySelector(".rewind-label")!.textContent =
      this.settings.rewindSeconds === 0 ? "off" : `${this.settings.rewindSeconds} s`;
    this.emulator.setRewindSeconds(this.settings.rewindSeconds);
    this.audio.setVolume(this.settings.volume);
    this.screen.setIntegerScale(this.settings.integerScale);
    this.player.classList.toggle("force-touch", this.settings.alwaysShowTouch);
  }
}
