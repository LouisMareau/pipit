// Touch layout editor: drag and resize the screen and each on-screen control.
//
// A layout is a box per element as fractions of the stage (the player minus the
// toolbar), saved per touch layout (GBA / GBA SP). Applying one positions the
// elements absolutely; without one the CSS layouts apply. While editing, a
// dashed handle sits over every element: drag it to move, drag its corner grip
// to resize. The screen keeps its 3:2 shape and the discs stay round.

import type { Box, ControlId, CustomLayout } from "../types";
import { CONTROL_IDS } from "../types";

const SELECTORS: Record<ControlId, string> = {
  screen: ".screen-box",
  dpad: ".dpad",
  abpad: ".abpad",
  l: ".touch-left .tbtn-shoulder",
  r: ".touch-right .tbtn-shoulder",
  select: ".tbtn-select",
  start: ".tbtn-start",
};

/** For measuring the stock layout, the screen is the picture, not its column. */
const MEASURE_SELECTORS: Record<ControlId, string> = { ...SELECTORS, screen: ".screen" };

/** Handle labels; elements that already show their own name get none. */
const LABELS: Partial<Record<ControlId, string>> = {
  screen: "Screen",
  dpad: "D-pad",
  abpad: "A / B",
};

/** Width ÷ height to keep while resizing. */
const ASPECT: Partial<Record<ControlId, number>> = { screen: 1.5, dpad: 1, abpad: 1 };
const MIN_PX = 40;
const FAB_SIZE = 44;

const clamp = (v: number, lo: number, hi: number) => Math.min(Math.max(v, lo), hi);

export class LayoutEditor {
  private layer: HTMLDivElement;
  private bar: HTMLDivElement;
  private layout: CustomLayout | null = null;
  private boxes = new Map<ControlId, HTMLDivElement>();
  /** Called after every apply, so the app can refit the screen canvas. */
  onApplied: () => void = () => {};
  /** Width ÷ height of the console's picture (3:2 for the Advance, 10:9 for the Game Boy). */
  private screenAspect = 1.5;
  onDone: (layout: CustomLayout) => void = () => {};
  onCancel: () => void = () => {};
  onReset: () => void = () => {};

  constructor(private stage: HTMLElement) {
    this.bar = document.createElement("div");
    this.bar.className = "editor-bar hidden";
    this.bar.innerHTML = `
      <span class="muted small editor-hint">Drag to move · corner to resize</span>
      <button class="btn" data-edit="reset">Reset</button>
      <button class="btn" data-edit="cancel">Cancel</button>
      <button class="btn btn-primary" data-edit="done">Done</button>`;
    this.layer = document.createElement("div");
    this.layer.className = "editor-layer hidden";
    stage.append(this.layer, this.bar);
    this.bar.addEventListener("click", (e) => {
      const button = (e.target as HTMLElement).closest<HTMLElement>("[data-edit]");
      if (!button || !this.layout) return;
      const layout = this.layout;
      this.close();
      switch (button.dataset["edit"]) {
        case "done":
          this.onDone(layout);
          break;
        case "cancel":
          this.onCancel();
          break;
        case "reset":
          this.onReset();
          break;
      }
    });
  }

  get isOpen(): boolean {
    return this.layout !== null;
  }

  setScreenAspect(aspect: number) {
    this.screenAspect = aspect;
  }

  private aspectOf(id: ControlId): number | undefined {
    return id === "screen" ? this.screenAspect : ASPECT[id];
  }

  private element(id: ControlId, selectors = SELECTORS): HTMLElement | null {
    return this.stage.querySelector<HTMLElement>(selectors[id]);
  }

  /** Where everything is right now, as fractions of the stage. */
  measure(): CustomLayout {
    const s = this.stage.getBoundingClientRect();
    const out = {} as CustomLayout;
    for (const id of CONTROL_IDS) {
      const el = this.element(id, MEASURE_SELECTORS) ?? this.element(id);
      const r = el?.getBoundingClientRect() ?? s;
      out[id] = {
        x: (r.left - s.left) / s.width,
        y: (r.top - s.top) / s.height,
        w: r.width / s.width,
        h: r.height / s.height,
      };
    }
    return out;
  }

  /** Positions every element from `layout` (in pixels, from the stage's current size). */
  apply(layout: CustomLayout) {
    const W = this.stage.clientWidth;
    const H = this.stage.clientHeight;
    for (const id of CONTROL_IDS) {
      const el = this.element(id);
      if (!el) continue;
      const b = layout[id];
      el.style.left = `${b.x * W}px`;
      el.style.top = `${b.y * H}px`;
      el.style.width = `${b.w * W}px`;
      el.style.height = `${b.h * H}px`;
      // The discs scale their buttons and labels from this.
      if (id === "dpad" || id === "abpad") el.style.setProperty("--pad-size", `${b.w * W}px`);
    }
    // The menu disc follows the screen's top-right corner.
    const fab = this.stage.querySelector<HTMLElement>(".menu-fab");
    if (fab) {
      const sc = layout.screen;
      fab.style.left = `${(sc.x + sc.w) * W - FAB_SIZE - 8}px`;
      fab.style.top = `${sc.y * H + 8}px`;
    }
    this.onApplied();
  }

  /** Removes the inline positions so the CSS layouts take over again. */
  clear() {
    const reset = (el: HTMLElement | null) => {
      if (!el) return;
      for (const prop of ["left", "top", "width", "height"]) el.style.removeProperty(prop);
      el.style.removeProperty("--pad-size");
    };
    for (const id of CONTROL_IDS) reset(this.element(id));
    reset(this.stage.querySelector<HTMLElement>(".menu-fab"));
  }

  open(initial: CustomLayout) {
    this.layout = structuredClone(initial);
    this.layer.replaceChildren();
    this.boxes.clear();
    for (const id of CONTROL_IDS) {
      const box = document.createElement("div");
      box.className = "edit-box";
      box.dataset["id"] = id;
      if (ASPECT[id] === 1) box.classList.add("round");
      box.innerHTML = `<span class="edit-label"></span><span class="edit-grip" aria-hidden="true"></span>`;
      box.querySelector(".edit-label")!.textContent = LABELS[id] ?? "";
      this.attach(id, box);
      this.layer.append(box);
      this.boxes.set(id, box);
    }
    this.layer.classList.remove("hidden");
    this.bar.classList.remove("hidden");
    this.reapply();
  }

  /** Closes the editor without touching the elements' positions. */
  close() {
    this.layout = null;
    this.layer.classList.add("hidden");
    this.bar.classList.add("hidden");
    this.layer.replaceChildren();
    this.boxes.clear();
  }

  /** Re-applies the working layout (after a drag or a resize) and moves the handles. */
  reapply() {
    if (!this.layout) return;
    this.apply(this.layout);
    const W = this.stage.clientWidth;
    const H = this.stage.clientHeight;
    for (const [id, box] of this.boxes) {
      const b = this.layout[id];
      box.style.left = `${b.x * W}px`;
      box.style.top = `${b.y * H}px`;
      box.style.width = `${b.w * W}px`;
      box.style.height = `${b.h * H}px`;
    }
  }

  private attach(id: ControlId, box: HTMLDivElement) {
    const grip = box.querySelector<HTMLElement>(".edit-grip")!;
    let mode: "move" | "resize" | null = null;
    let startX = 0;
    let startY = 0;
    let start: Box = { x: 0, y: 0, w: 0, h: 0 };

    const down = (e: PointerEvent, m: "move" | "resize") => {
      if (!this.layout) return;
      e.preventDefault();
      e.stopPropagation();
      mode = m;
      startX = e.clientX;
      startY = e.clientY;
      start = { ...this.layout[id] };
      box.setPointerCapture(e.pointerId);
      box.classList.add("dragging");
    };
    box.addEventListener("pointerdown", (e) => down(e, "move"));
    grip.addEventListener("pointerdown", (e) => down(e, "resize"));

    box.addEventListener("pointermove", (e) => {
      if (!mode || !this.layout) return;
      const W = this.stage.clientWidth;
      const H = this.stage.clientHeight;
      const dx = (e.clientX - startX) / W;
      const dy = (e.clientY - startY) / H;
      const b = { ...start };
      if (mode === "move") {
        b.x = clamp(start.x + dx, 0, 1 - b.w);
        b.y = clamp(start.y + dy, 0, 1 - b.h);
      } else {
        const aspect = this.aspectOf(id);
        let w = Math.max(MIN_PX / W, start.w + dx);
        let h = aspect ? (w * W) / aspect / H : Math.max(MIN_PX / H, start.h + dy);
        w = Math.min(w, 1 - b.x);
        h = Math.min(h, 1 - b.y);
        if (aspect) {
          // Clamping one side must not break the shape: fit to the tighter one.
          const wFromH = (h * H * aspect) / W;
          if (wFromH < w) w = wFromH;
          else h = (w * W) / aspect / H;
        }
        b.w = w;
        b.h = h;
      }
      this.layout[id] = b;
      this.reapply();
    });

    const up = () => {
      mode = null;
      box.classList.remove("dragging");
    };
    box.addEventListener("pointerup", up);
    box.addEventListener("pointercancel", up);
    box.addEventListener("lostpointercapture", up);
    box.addEventListener("contextmenu", (e) => e.preventDefault());
  }
}
