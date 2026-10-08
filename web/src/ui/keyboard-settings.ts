// Keyboard settings: a modal for rebinding keys, opened from the player menu.
//
// Lists the GBA keys and the emulator actions (fast-forward, rewind, pause);
// "Change" waits for the next key press. A key can drive only one action.

import type { Capture } from "../platform/input";
import { keyLabel } from "../platform/input";
import type { KeyboardAction, KeyboardMapping } from "../types";
import { DEFAULT_KEYBOARD, KEYBOARD_ACTIONS } from "../types";
import { icon } from "./icons";

const LABELS: Record<KeyboardAction, string> = {
  A: "A",
  B: "B",
  L: "L",
  R: "R",
  Start: "Start",
  Select: "Select",
  Up: "D-pad up",
  Down: "D-pad down",
  Left: "D-pad left",
  Right: "D-pad right",
  FastForward: "Fast-forward (hold)",
  Rewind: "Rewind (hold)",
  Pause: "Pause",
};

export class KeyboardSettings {
  readonly element: HTMLDivElement;
  private mapping: KeyboardMapping = DEFAULT_KEYBOARD;
  private capture: Capture<string> | null = null;
  private capturing: KeyboardAction | null = null;
  onChange: (mapping: KeyboardMapping) => void = () => {};
  onClose: () => void = () => {};
  /** Supplied by the app: starts listening for the next key press. */
  captureKey: () => Capture<string> = () => ({ promise: Promise.resolve(null), cancel() {} });

  constructor() {
    this.element = document.createElement("div");
    this.element.className = "modal-backdrop hidden";
    this.element.innerHTML = `
      <div class="modal" role="dialog" aria-modal="true" aria-labelledby="keyboard-settings-title">
        <header class="modal-header">
          <h2 id="keyboard-settings-title">Keyboard settings</h2>
          <button class="btn btn-icon" data-action="close" aria-label="Close">${icon("close")}</button>
        </header>
        <p class="muted small">Click Change, then press the key you want. Esc cancels.</p>
        <div class="mapping"></div>
        <footer class="modal-footer">
          <button class="btn" data-action="reset">Reset to defaults</button>
          <button class="btn btn-primary" data-action="close">Done</button>
        </footer>
      </div>`;
    this.element.addEventListener("click", (e) => {
      const target = e.target as HTMLElement;
      if (target === this.element) {
        this.close();
        return;
      }
      const button = target.closest<HTMLElement>("[data-action]");
      if (!button) return;
      switch (button.dataset["action"]) {
        case "close":
          this.close();
          break;
        case "reset":
          this.cancelCapture();
          this.mapping = { ...DEFAULT_KEYBOARD };
          this.onChange(this.mapping);
          this.render();
          break;
        case "change":
          void this.changeAction(button.dataset["key"] as KeyboardAction);
          break;
        case "cancel":
          this.cancelCapture();
          this.render();
          break;
      }
    });
    document.addEventListener("keydown", (e) => {
      // While capturing, Escape is consumed by the capture itself.
      if (e.code === "Escape" && !this.capturing && !this.element.classList.contains("hidden")) {
        this.close();
      }
    });
  }

  open(mapping: KeyboardMapping) {
    this.mapping = { ...mapping };
    this.render();
    this.element.classList.remove("hidden");
  }

  close() {
    this.cancelCapture();
    this.element.classList.add("hidden");
    this.onClose();
  }

  private async changeAction(action: KeyboardAction) {
    this.cancelCapture();
    this.capturing = action;
    this.render();
    this.capture = this.captureKey();
    const code = await this.capture.promise;
    if (this.capturing !== action) return; // cancelled or superseded
    this.capturing = null;
    this.capture = null;
    if (code !== null) {
      const mapping = { ...this.mapping };
      // One key per action: take it away from whatever used it.
      for (const other of KEYBOARD_ACTIONS) if (mapping[other] === code) mapping[other] = "";
      mapping[action] = code;
      this.mapping = mapping;
      this.onChange(this.mapping);
    }
    this.render();
  }

  private cancelCapture() {
    this.capture?.cancel();
    this.capture = null;
    this.capturing = null;
  }

  private render() {
    const list = this.element.querySelector<HTMLDivElement>(".mapping")!;
    list.replaceChildren();
    for (const action of KEYBOARD_ACTIONS) {
      const row = document.createElement("div");
      row.className = "mapping-row";
      if (this.capturing === action) {
        row.classList.add("capturing");
        row.innerHTML = `
          <span class="mapping-key"></span>
          <span class="mapping-value">Press a key…</span>
          <button class="btn" data-action="cancel">Cancel</button>`;
      } else {
        row.innerHTML = `
          <span class="mapping-key"></span>
          <span class="mapping-value"></span>
          <button class="btn" data-action="change">Change</button>`;
        row.querySelector(".mapping-value")!.textContent = keyLabel(this.mapping[action]);
        row.querySelector<HTMLElement>("[data-action=change]")!.dataset["key"] = action;
      }
      row.querySelector(".mapping-key")!.textContent = LABELS[action];
      list.append(row);
    }
  }
}
