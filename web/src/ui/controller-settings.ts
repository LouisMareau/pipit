// Controller settings: a modal over the game for remapping buttons.
//
// Each GBA key shows the controller button driving it; "Change" waits for the
// next press on the controller. Changes apply immediately and are saved per
// controller model.

import type { Capture } from "../platform/input";
import type { ControllerMapping, KeyName } from "../types";
import { DEFAULT_MAPPING, GAMEPAD_BUTTON_NAMES, KEY_NAMES } from "../types";
import { icon } from "./icons";

const LABELS: Record<KeyName, string> = {
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
};

export class ControllerSettings {
  readonly element: HTMLDivElement;
  private mapping: ControllerMapping = DEFAULT_MAPPING;
  private capture: Capture<number> | null = null;
  private capturing: KeyName | null = null;
  onChange: (mapping: ControllerMapping) => void = () => {};
  onClose: () => void = () => {};
  /** Supplied by the app: starts listening for the next controller button. */
  captureButton: () => Capture<number> = () => ({ promise: Promise.resolve(null), cancel() {} });

  constructor() {
    this.element = document.createElement("div");
    this.element.className = "modal-backdrop hidden";
    this.element.innerHTML = `
      <div class="modal" role="dialog" aria-modal="true" aria-labelledby="controller-settings-title">
        <header class="modal-header">
          <h2 id="controller-settings-title">Controller settings</h2>
          <button class="btn btn-icon" data-action="close" aria-label="Close">${icon("close")}</button>
        </header>
        <p class="muted small controller-name"></p>
        <div class="mapping"></div>
        <label class="row"><input type="checkbox" data-option="stickDpad" /><span>Left stick also works as the D-pad</span></label>
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
          this.mapping = structuredClone(DEFAULT_MAPPING);
          this.onChange(this.mapping);
          this.render();
          break;
        case "change":
          void this.changeKey(button.dataset["key"] as KeyName);
          break;
        case "cancel":
          this.cancelCapture();
          this.render();
          break;
      }
    });
    this.element.querySelector<HTMLInputElement>("[data-option=stickDpad]")!.addEventListener("change", (e) => {
      this.mapping = { ...this.mapping, stickDpad: (e.target as HTMLInputElement).checked };
      this.onChange(this.mapping);
    });
    document.addEventListener("keydown", (e) => {
      if (e.code === "Escape" && !this.element.classList.contains("hidden")) {
        if (this.capturing) {
          this.cancelCapture();
          this.render();
        } else {
          this.close();
        }
      }
    });
  }

  open(controllerName: string, mapping: ControllerMapping) {
    this.mapping = structuredClone(mapping);
    this.element.querySelector(".controller-name")!.textContent = controllerName;
    this.render();
    this.element.classList.remove("hidden");
  }

  close() {
    this.cancelCapture();
    this.element.classList.add("hidden");
    this.onClose();
  }

  private async changeKey(key: KeyName) {
    this.cancelCapture();
    this.capturing = key;
    this.render();
    this.capture = this.captureButton();
    const button = await this.capture.promise;
    if (this.capturing !== key) return; // cancelled or superseded
    this.capturing = null;
    this.capture = null;
    if (button !== null) {
      const buttons = { ...this.mapping.buttons };
      // A controller button drives one GBA key: unassign it elsewhere.
      for (const other of KEY_NAMES) if (buttons[other] === button) delete buttons[other];
      buttons[key] = button;
      this.mapping = { ...this.mapping, buttons };
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
    for (const key of KEY_NAMES) {
      const row = document.createElement("div");
      row.className = "mapping-row";
      const index = this.mapping.buttons[key];
      const value =
        index === undefined ? "—" : (GAMEPAD_BUTTON_NAMES[index] ?? `Button ${index}`);
      if (this.capturing === key) {
        row.classList.add("capturing");
        row.innerHTML = `
          <span class="mapping-key"></span>
          <span class="mapping-value">Press a button on the controller…</span>
          <button class="btn" data-action="cancel">Cancel</button>`;
      } else {
        row.innerHTML = `
          <span class="mapping-key"></span>
          <span class="mapping-value"></span>
          <button class="btn" data-action="change">Change</button>`;
        row.querySelector(".mapping-value")!.textContent = value;
        row.querySelector<HTMLElement>("[data-action=change]")!.dataset["key"] = key;
      }
      row.querySelector(".mapping-key")!.textContent = LABELS[key];
      list.append(row);
    }
    this.element.querySelector<HTMLInputElement>("[data-option=stickDpad]")!.checked = this.mapping.stickDpad;
  }
}
