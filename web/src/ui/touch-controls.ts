// On-screen controls for phones and tablets, in the style of a round D-pad disc
// and a round A/B disc. Pointer events handle multi-touch; the D-pad disc resolves
// eight directions from the touch position so diagonals work anywhere on it. A
// corner gives one direction, the one that differs from the last single
// direction held, so a thumb sliding from Up towards Right turns Right as soon
// as it reaches the corner (see `resolve`).
//
// The DOM is grouped as a left side (L, the D-pad disc, Select) and a right side
// (R, the A/B disc, Start). CSS arranges them either beside the screen with
// Select and Start hugging the inner bottom corners (GBA layout), or below the
// screen with Select/Start side by side at the bottom (GBA SP layout); `fit`
// sizes the discs to fill the space the layout gives them. The layout editor can
// override all of this with absolute positions (see `layout-editor.ts`).

import { Key } from "../types";
import { chevron } from "./icons";

export class TouchControls {
  readonly element: HTMLDivElement;
  private held = 0;
  private dpadKeys = 0;
  /** The last single direction the D-pad gave (kept across touches). */
  private lastCardinal = 0;
  /** The corner the thumb is in, and what it was resolved to. */
  private corner = 0;
  private cornerKeys = 0;
  private buttons = new Map<number, number>(); // pointerId → key bits
  onChange: (keys: number) => void = () => {};

  constructor() {
    this.element = document.createElement("div");
    this.element.className = "touch";
    this.element.innerHTML = `
      <div class="touch-left">
        <button class="tbtn tbtn-shoulder" data-key="${Key.L}">L</button>
        <div class="pad dpad" data-dpad>
          <span class="dpad-btn dpad-up">${chevron("up")}</span>
          <span class="dpad-btn dpad-down">${chevron("down")}</span>
          <span class="dpad-btn dpad-left">${chevron("left")}</span>
          <span class="dpad-btn dpad-right">${chevron("right")}</span>
          <span class="pad-dot pad-dot-nw"></span>
          <span class="pad-dot pad-dot-ne"></span>
          <span class="pad-dot pad-dot-sw"></span>
          <span class="pad-dot pad-dot-se"></span>
        </div>
        <button class="tbtn tbtn-pill tbtn-select" data-key="${Key.Select}">select</button>
      </div>
      <div class="touch-right">
        <button class="tbtn tbtn-shoulder" data-key="${Key.R}">R</button>
        <div class="pad abpad">
          <button class="tbtn tbtn-b" data-key="${Key.B}">B</button>
          <button class="tbtn tbtn-a" data-key="${Key.A}">A</button>
          <button class="tbtn tbtn-x" data-key="${Key.X}">X</button>
          <button class="tbtn tbtn-y" data-key="${Key.Y}">Y</button>
          <span class="pad-dot pad-dot-nw"></span>
          <span class="pad-dot pad-dot-se"></span>
        </div>
        <button class="tbtn tbtn-pill tbtn-start" data-key="${Key.Start}">start</button>
      </div>`;

    for (const button of this.element.querySelectorAll<HTMLButtonElement>("[data-key]")) {
      const bits = Number(button.dataset["key"]);
      const press = (e: PointerEvent) => {
        e.preventDefault();
        button.setPointerCapture(e.pointerId);
        this.buttons.set(e.pointerId, bits);
        button.classList.add("active");
        this.emit();
      };
      const release = (e: PointerEvent) => {
        if (!this.buttons.has(e.pointerId)) return;
        this.buttons.delete(e.pointerId);
        button.classList.remove("active");
        this.emit();
      };
      button.addEventListener("pointerdown", press);
      button.addEventListener("pointerup", release);
      button.addEventListener("pointercancel", release);
      button.addEventListener("lostpointercapture", release);
      button.addEventListener("contextmenu", (e) => e.preventDefault());
    }

    const dpad = this.element.querySelector<HTMLDivElement>("[data-dpad]")!;
    let dpadPointer: number | null = null;
    const update = (e: PointerEvent) => {
      const rect = dpad.getBoundingClientRect();
      const x = (e.clientX - rect.left) / rect.width - 0.5;
      const y = (e.clientY - rect.top) / rect.height - 0.5;
      let keys = 0;
      const dead = 0.1;
      if (Math.hypot(x, y) > dead) {
        const angle = Math.atan2(y, x); // -π..π, 0 = right
        const sector = Math.round(angle / (Math.PI / 4)); // -4..4, 8 directions
        const table: Record<number, number> = {
          0: Key.Right,
          1: Key.Right | Key.Down,
          2: Key.Down,
          3: Key.Down | Key.Left,
          4: Key.Left,
          [-4]: Key.Left,
          [-3]: Key.Left | Key.Up,
          [-2]: Key.Up,
          [-1]: Key.Up | Key.Right,
        };
        keys = this.resolve(table[sector] ?? 0);
      } else {
        this.resolve(0);
      }
      if (keys !== this.dpadKeys) {
        this.dpadKeys = keys;
        dpad.dataset["dir"] = String(keys);
        this.emit();
      }
    };
    dpad.addEventListener("pointerdown", (e) => {
      e.preventDefault();
      dpadPointer = e.pointerId;
      try {
        dpad.setPointerCapture(e.pointerId);
      } catch {
        // A pointer the browser does not know (a synthetic event in tests): no capture.
      }
      update(e);
    });
    dpad.addEventListener("pointermove", (e) => {
      if (e.pointerId === dpadPointer) update(e);
    });
    const end = (e: PointerEvent) => {
      if (e.pointerId !== dpadPointer) return;
      dpadPointer = null;
      this.resolve(0);
      this.dpadKeys = 0;
      dpad.dataset["dir"] = "0";
      this.emit();
    };
    dpad.addEventListener("pointerup", end);
    dpad.addEventListener("pointercancel", end);
    dpad.addEventListener("contextmenu", (e) => e.preventDefault());
  }

  /**
   * Sizes the discs to the space the stock layout gives them: in the GBA SP layout
   * the bottom half of the stage, in the GBA layout the columns beside the screen.
   */
  fit(layout: "gba" | "gbasp", stage: HTMLElement) {
    const styles = getComputedStyle(this.element);
    const shoulderRow = parseFloat(styles.getPropertyValue("--shoulder-height")) || 36;
    const pillRow = parseFloat(styles.getPropertyValue("--system-height")) || 28;
    const gap = 12;
    let size: number;
    if (layout === "gbasp") {
      // Rows: shoulders, discs, Select/Start; columns: two halves with 8px sides.
      const { clientWidth: w, clientHeight: h } = this.element;
      const systemRow = pillRow + 6;
      const padding = 8 + 14;
      size = Math.min(h - shoulderRow - gap - systemRow - padding - 2 * gap, (w - 16) / 2 - 6);
    } else {
      // Rows in each side column: shoulder at the top, then the disc and pill
      // anchored at the bottom. The discs stay small so a thumb resting on the
      // bottom corner of the phone reaches everything without stretching inward.
      const left = this.element.querySelector<HTMLElement>(".touch-left")!;
      size = Math.min(
        left.clientHeight - shoulderRow - pillRow - 2 * gap - 16,
        stage.clientWidth * 0.19,
        stage.clientHeight * 0.45,
      );
    }
    this.element.style.setProperty("--pad-size", `${Math.max(120, Math.floor(size))}px`);
  }

  /**
   * What the thumb's zone means. A single direction is itself, and is
   * remembered. A corner gives the one direction that differs from the
   * remembered one, so sliding from Up towards Right turns Right the moment
   * the thumb reaches the corner, and sliding back from Right gives Up; the
   * choice holds while the thumb stays in that corner. A corner with no such
   * history (a fresh press, or coming from the other axis) keeps both
   * directions, as a real D-pad would.
   */
  private resolve(zone: number): number {
    const singles = [Key.Up, Key.Down, Key.Left, Key.Right].filter((k) => zone & k);
    if (singles.length === 1) {
      this.lastCardinal = zone;
      this.corner = 0;
      return zone;
    }
    if (singles.length === 2) {
      if (zone !== this.corner) {
        this.corner = zone;
        this.cornerKeys = singles.includes(this.lastCardinal) ? zone & ~this.lastCardinal : zone;
      }
      return this.cornerKeys;
    }
    this.corner = 0;
    return 0;
  }

  private emit() {
    let keys = this.dpadKeys;
    for (const bits of this.buttons.values()) keys |= bits;
    if (keys !== this.held) {
      this.held = keys;
      this.onChange(keys);
    }
  }
}
