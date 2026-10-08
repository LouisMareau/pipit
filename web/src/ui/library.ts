// The ROM library: the list of games stored on this device.

import * as storage from "../platform/storage";
import type { RomEntry } from "../types";

export class Library {
  readonly element: HTMLDivElement;
  onPlay: (entry: RomEntry) => void = () => {};

  constructor() {
    this.element = document.createElement("div");
    this.element.className = "library";
    this.element.innerHTML = `
      <header class="library-header">
        <h1><img src="${import.meta.env.BASE_URL}icons/icon.svg" alt="" width="36" height="36" /> Pipit</h1>
        <p class="muted">Your games stay on this device. Nothing is uploaded.</p>
      </header>
      <label class="add-rom">
        <input type="file" accept=".gba,application/octet-stream" multiple hidden />
        <span class="btn btn-primary">Add a ROM</span>
        <span class="muted small">.gba files you own, including your own builds and hacks</span>
      </label>
      <ul class="rom-list"></ul>
      <p class="muted small keys-help">
        Keyboard: arrows · Z = A · X = B · Enter = Start · Backspace = Select · A/S = L/R · hold Space to fast-forward.
        Controllers work too: press a button on one and the 🎮 toggle in the player turns green.
      </p>`;
    const input = this.element.querySelector<HTMLInputElement>("input[type=file]")!;
    input.addEventListener("change", async () => {
      for (const file of input.files ?? []) await storage.addRom(file);
      input.value = "";
      await this.refresh();
    });
  }

  async refresh() {
    const list = this.element.querySelector<HTMLUListElement>(".rom-list")!;
    const entries = await storage.listRoms();
    list.replaceChildren();
    if (entries.length === 0) {
      const empty = document.createElement("li");
      empty.className = "muted empty";
      empty.textContent = "No games yet. Add a .gba file to get started.";
      list.append(empty);
      return;
    }
    for (const entry of entries) {
      const item = document.createElement("li");
      item.className = "rom";
      item.innerHTML = `
        <button class="rom-main">
          <span class="rom-name"></span>
          <span class="rom-meta muted small"></span>
        </button>
        <button class="btn btn-icon rom-delete" title="Remove from library" aria-label="Remove">✕</button>`;
      item.querySelector(".rom-name")!.textContent = entry.name;
      item.querySelector(".rom-meta")!.textContent =
        `${entry.title || "Untitled"} · ${entry.gameCode || "????"} · ${(entry.size / 1048576).toFixed(1)} MB`;
      item.querySelector(".rom-main")!.addEventListener("click", () => this.onPlay(entry));
      item.querySelector(".rom-delete")!.addEventListener("click", async () => {
        if (confirm(`Remove "${entry.name}" and its save data from this device?`)) {
          await storage.deleteRom(entry.id);
          await this.refresh();
        }
      });
      list.append(item);
    }
  }
}
