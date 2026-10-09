// The ROM library: the list of games stored on this device, one tab per system.
// Only the GBA tab shows for now; the GBC tab is in the DOM but hidden until a
// core for it exists.

import * as storage from "../platform/storage";
import type { RomEntry } from "../types";
import { icon } from "./icons";

/** One line of the keyboard summary: an action and the key(s) bound to it. */
export interface KeyHelpRow {
  action: string;
  keys: string[];
  /** Shown before the keys, e.g. "hold". */
  note?: string;
}

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
      <nav class="tabs" role="tablist" aria-label="Systems">
        <button class="tab active" role="tab" aria-selected="true" data-tab="gba">GBA</button>
        <button class="tab hidden" role="tab" aria-selected="false" data-tab="gbc">GBC</button>
      </nav>
      <section class="tab-panel" role="tabpanel" data-panel="gba">
        <div class="add-row">
          <label class="add-rom">
            <input type="file" accept=".gba,application/octet-stream" multiple hidden />
            <span class="btn btn-primary">Add a <code>.gba</code> ROM</span>
          </label>
          <span class="info">
            <button type="button" class="info-btn" aria-label="Which files can I add?" aria-expanded="false">
              ${icon("info", 18)}
            </button>
            <span class="info-pop" role="tooltip"><code>.gba</code> files you own, including your own builds and hacks.</span>
          </span>
        </div>
        <ul class="rom-list"></ul>
      </section>
      <section class="tab-panel hidden" role="tabpanel" data-panel="gbc">
        <p class="muted empty">GBC support is coming later.</p>
      </section>
      <aside class="alert keys-help">
        <span class="alert-icon">${icon("info", 20)}</span>
        <div class="alert-body">
          <strong>Keyboard controls</strong>
          <div class="keys"></div>
          <p class="muted small">Change them in the player&#8217;s ⋯ menu. Controllers work too: press a button on one.</p>
        </div>
      </aside>`;

    const input = this.element.querySelector<HTMLInputElement>("input[type=file]")!;
    input.addEventListener("change", async () => {
      for (const file of input.files ?? []) await storage.addRom(file);
      input.value = "";
      await this.refresh();
    });

    for (const tab of this.element.querySelectorAll<HTMLButtonElement>(".tab")) {
      tab.addEventListener("click", () => this.showTab(tab.dataset["tab"]!));
    }

    // The (i) popover: a tap toggles it; a tap anywhere else or Escape closes it.
    // (With a mouse, hovering shows it too; see the CSS.)
    const info = this.element.querySelector<HTMLElement>(".info")!;
    const infoButton = info.querySelector<HTMLButtonElement>(".info-btn")!;
    const setInfo = (open: boolean) => {
      info.classList.toggle("open", open);
      infoButton.setAttribute("aria-expanded", String(open));
    };
    infoButton.addEventListener("click", (e) => {
      e.stopPropagation();
      setInfo(!info.classList.contains("open"));
    });
    document.addEventListener("click", () => setInfo(false));
    document.addEventListener("keydown", (e) => {
      if (e.key === "Escape") setInfo(false);
    });
  }

  showTab(id: string) {
    for (const tab of this.element.querySelectorAll<HTMLButtonElement>(".tab")) {
      const on = tab.dataset["tab"] === id;
      tab.classList.toggle("active", on);
      tab.setAttribute("aria-selected", String(on));
    }
    for (const panel of this.element.querySelectorAll<HTMLElement>(".tab-panel")) {
      panel.classList.toggle("hidden", panel.dataset["panel"] !== id);
    }
  }

  /** Fills the keyboard summary from the current bindings. */
  setKeys(rows: KeyHelpRow[]) {
    const keys = this.element.querySelector<HTMLElement>(".keys")!;
    keys.replaceChildren();
    for (const row of rows) {
      const item = document.createElement("div");
      item.className = "key-row";
      const action = document.createElement("span");
      action.className = "key-action";
      action.textContent = row.action;
      const bound = document.createElement("span");
      bound.className = "key-keys";
      if (row.note) {
        const note = document.createElement("span");
        note.className = "muted small";
        note.textContent = row.note;
        bound.append(note);
      }
      for (const key of row.keys) {
        const kbd = document.createElement("kbd");
        kbd.textContent = key;
        bound.append(kbd);
      }
      item.append(action, bound);
      keys.append(item);
    }
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
        <button class="btn btn-icon rom-delete" title="Remove from library" aria-label="Remove">&#10005;</button>`;
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
