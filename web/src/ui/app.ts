// Top-level UI: library screen and player screen, wired to the emulator worker.

import { exportSave, pickSaveFile } from "../features/saves";
import { takeScreenshot } from "../features/screenshots";
import { AudioOutput } from "../platform/audio";
import { EmulatorClient } from "../platform/emulator-client";
import { Input } from "../platform/input";
import * as storage from "../platform/storage";
import type { RomEntry, Settings } from "../types";
import { Library } from "./library";
import { Screen } from "./screen";
import { TouchControls } from "./touch-controls";

export class App {
  private root: HTMLElement;
  private library = new Library();
  private player: HTMLDivElement;
  private screen: Screen;
  private touch = new TouchControls();
  private emulator = new EmulatorClient();
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
    this.player = document.createElement("div");
    this.player.className = "player hidden";
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
        <button class="btn" data-action="export">Export save (.sav)</button>
        <button class="btn" data-action="import">Import save (.sav)</button>
        <label class="row"><span>Volume</span><input type="range" min="0" max="1" step="0.05" data-setting="volume" /></label>
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
    this.touch.onChange = (keys) => this.input.setTouch(keys);

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
    this.emulator.on("error", (message) => alert(message));
  }

  private flushSave() {
    if (this.saveTimer !== null) clearTimeout(this.saveTimer);
    this.saveTimer = null;
    if (this.current && this.latestSave) void storage.putSave(this.current.id, this.latestSave);
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
      if (e.code === "KeyP" && this.current) this.togglePause();
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
    this.audio.setVolume(this.settings.volume);
    this.screen.setIntegerScale(this.settings.integerScale);
    this.player.classList.toggle("force-touch", this.settings.alwaysShowTouch);
  }
}
