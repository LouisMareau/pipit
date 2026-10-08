// Toolbar control for controller input.
//
// Gray: no controller, or controller input switched off (keyboard still works).
// Green: the active controller is driving the game.
// Click toggles. Holding for two seconds fills a ring inside the button's border
// and then opens a field, attached to the right, to choose among the detected
// controllers; choosing one keeps the toggle green.

import type { GamepadState } from "../platform/input";

const HOLD_MS = 2000;

export class ControllerToggle {
  readonly element: HTMLDivElement;
  private button: HTMLButtonElement;
  private picker: HTMLDivElement;
  private select: HTMLSelectElement;
  private state: GamepadState = { detected: [], active: null, enabled: false };
  private holdStart = 0;
  private holdFrame: number | null = null;
  private holdCompleted = false;
  onToggle: (enabled: boolean) => void = () => {};
  onSelect: (index: number) => void = () => {};
  onSettings: () => void = () => {};
  onNothingDetected: () => void = () => {};

  constructor() {
    this.element = document.createElement("div");
    this.element.className = "controller";
    // A plain controller silhouette; the d-pad and buttons are cut out in the
    // button's own background colour so they read as holes in any state.
    this.element.innerHTML = `
      <button class="btn btn-icon controller-toggle" aria-pressed="false" aria-label="Controller input">
        <svg viewBox="0 0 24 24" width="26" height="26" aria-hidden="true">
          <path fill="currentColor" d="M7.2 4.8C9 4.2 15 4.2 16.8 4.8c2.2.6 3.7 2.7 4.8 6.2 1 3.2 1.2 6.4-.4 7.9-1.3 1.2-3.2.7-4.2-.9-.8-1.3-1.5-2.4-2.6-2.6H9.6c-1.1.2-1.8 1.3-2.6 2.6-1 1.6-2.9 2.1-4.2.9-1.6-1.5-1.4-4.7-.4-7.9C3.5 7.5 5 5.4 7.2 4.8z"/>
          <path fill="var(--toggle-bg)" d="M7 8.7h1.2v1.2h1.2v1.2H8.2v1.2H7v-1.2H5.8V9.9H7z"/>
          <circle fill="var(--toggle-bg)" cx="16.4" cy="8.9" r=".8"/>
          <circle fill="var(--toggle-bg)" cx="18.1" cy="10.5" r=".8"/>
          <circle fill="var(--toggle-bg)" cx="16.4" cy="12.1" r=".8"/>
          <circle fill="var(--toggle-bg)" cx="14.7" cy="10.5" r=".8"/>
        </svg>
      </button>
      <div class="controller-picker hidden">
        <select class="controller-select" aria-label="Active controller"></select>
        <button class="btn btn-icon controller-gear" title="Controller settings" aria-label="Controller settings">
          <svg viewBox="0 0 24 24" width="20" height="20" aria-hidden="true">
            <path fill="currentColor" d="M12 15.5a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7zm7.4-2.5c.1-.3.1-.7.1-1s0-.7-.1-1l2.1-1.6c.2-.2.2-.4.1-.6l-2-3.5c-.1-.2-.4-.3-.6-.2l-2.5 1c-.5-.4-1.1-.7-1.7-1l-.4-2.6c0-.2-.2-.4-.5-.4h-4c-.2 0-.4.2-.5.4l-.4 2.6c-.6.3-1.2.6-1.7 1l-2.5-1c-.2-.1-.5 0-.6.2l-2 3.5c-.1.2-.1.5.1.6L4.6 11c-.1.3-.1.7-.1 1s0 .7.1 1l-2.1 1.6c-.2.2-.2.4-.1.6l2 3.5c.1.2.4.3.6.2l2.5-1c.5.4 1.1.7 1.7 1l.4 2.6c0 .2.2.4.5.4h4c.2 0 .4-.2.5-.4l.4-2.6c.6-.3 1.2-.6 1.7-1l2.5 1c.2.1.5 0 .6-.2l2-3.5c.1-.2.1-.5-.1-.6L19.4 13z"/>
          </svg>
        </button>
      </div>`;
    this.button = this.element.querySelector(".controller-toggle")!;
    this.picker = this.element.querySelector(".controller-picker")!;
    this.select = this.element.querySelector(".controller-select")!;
    this.element.querySelector(".controller-gear")!.addEventListener("click", () => {
      this.closePicker();
      this.onSettings();
    });

    this.button.addEventListener("pointerdown", (e) => {
      if (e.button !== 0) return;
      e.preventDefault();
      this.button.setPointerCapture(e.pointerId);
      this.beginHold();
    });
    const release = () => this.endHold();
    this.button.addEventListener("pointerup", release);
    this.button.addEventListener("pointercancel", release);
    this.button.addEventListener("lostpointercapture", release);
    this.button.addEventListener("contextmenu", (e) => e.preventDefault());
    this.button.addEventListener("keydown", (e) => {
      if (e.code === "Enter" || e.code === "Space") {
        e.preventDefault();
        this.toggle();
      }
    });

    this.select.addEventListener("change", () => {
      const index = Number(this.select.value);
      if (!Number.isNaN(index)) this.onSelect(index);
      this.closePicker();
    });
    document.addEventListener("pointerdown", (e) => {
      if (!this.picker.classList.contains("hidden") && !this.element.contains(e.target as Node)) {
        this.closePicker();
      }
    });
    document.addEventListener("keydown", (e) => {
      if (e.code === "Escape") this.closePicker();
    });
    this.render();
  }

  update(state: GamepadState) {
    this.state = state;
    this.render();
  }

  private toggle() {
    if (this.state.detected.length === 0) {
      this.onNothingDetected();
      return;
    }
    this.onToggle(!this.state.enabled);
  }

  private beginHold() {
    this.holdStart = performance.now();
    this.holdCompleted = false;
    this.button.classList.add("holding");
    const tick = () => {
      const progress = Math.min(1, (performance.now() - this.holdStart) / HOLD_MS);
      this.button.style.setProperty("--hold", String(progress));
      if (progress >= 1) {
        this.holdCompleted = true;
        this.holdFrame = null;
        this.button.classList.remove("holding");
        this.button.style.setProperty("--hold", "0");
        this.openPicker();
        return;
      }
      this.holdFrame = requestAnimationFrame(tick);
    };
    this.holdFrame = requestAnimationFrame(tick);
  }

  private endHold() {
    if (this.holdFrame === null && !this.button.classList.contains("holding")) return;
    if (this.holdFrame !== null) cancelAnimationFrame(this.holdFrame);
    this.holdFrame = null;
    this.button.classList.remove("holding");
    this.button.style.setProperty("--hold", "0");
    // A release before the hold completes is an ordinary click.
    if (!this.holdCompleted) this.toggle();
    this.holdCompleted = false;
  }

  private openPicker() {
    this.select.replaceChildren();
    if (this.state.detected.length === 0) {
      const option = document.createElement("option");
      option.textContent = "No controller detected";
      option.disabled = true;
      option.selected = true;
      this.select.append(option);
    } else {
      for (const pad of this.state.detected) {
        const option = document.createElement("option");
        option.value = String(pad.index);
        option.textContent = shortName(pad.id);
        option.selected = pad.index === this.state.active;
        this.select.append(option);
      }
    }
    this.picker.classList.remove("hidden");
    this.element.classList.add("open");
    this.select.focus();
  }

  private closePicker() {
    this.picker.classList.add("hidden");
    this.element.classList.remove("open");
  }

  private render() {
    const { detected, active, enabled } = this.state;
    const on = enabled && active !== null;
    this.button.classList.toggle("active", on);
    this.button.setAttribute("aria-pressed", String(on));
    const current = detected.find((p) => p.index === active);
    this.button.title =
      detected.length === 0
        ? "No controller detected — keyboard controls. Press a button on a controller to use it."
        : on
          ? `Controller input on: ${shortName(current?.id ?? "")}. Click to switch to keyboard; hold to choose a controller.`
          : "Controller input off — click to switch it on.";
  }
}

/** "Xbox Wireless Controller (STANDARD GAMEPAD Vendor: 045e Product: 0b13)" → "Xbox Wireless Controller". */
function shortName(id: string): string {
  const cleaned = id.replace(/\s*\(.*$/, "").trim();
  return cleaned || id || "Controller";
}
