// On-screen controls for phones and tablets. Pointer events handle multi-touch;
// the D-pad resolves eight directions from the touch position so diagonals work.

import { Key } from "../types";

export class TouchControls {
  readonly element: HTMLDivElement;
  private held = 0;
  private dpadKeys = 0;
  private buttons = new Map<number, number>(); // pointerId → key bits
  onChange: (keys: number) => void = () => {};
  onRewind: (held: boolean) => void = () => {};

  constructor() {
    this.element = document.createElement("div");
    this.element.className = "touch";
    this.element.innerHTML = `
      <div class="touch-left">
        <div class="dpad" data-dpad>
          <span class="dpad-arm dpad-up"></span>
          <span class="dpad-arm dpad-down"></span>
          <span class="dpad-arm dpad-left"></span>
          <span class="dpad-arm dpad-right"></span>
          <span class="dpad-center"></span>
        </div>
      </div>
      <div class="touch-right">
        <button class="tbtn tbtn-b" data-key="${Key.B}">B</button>
        <button class="tbtn tbtn-a" data-key="${Key.A}">A</button>
      </div>
      <div class="touch-shoulders">
        <button class="tbtn tbtn-shoulder" data-key="${Key.L}">L</button>
        <button class="tbtn tbtn-shoulder" data-key="${Key.R}">R</button>
      </div>
      <div class="touch-system">
        <button class="tbtn tbtn-pill" data-key="${Key.Select}">select</button>
        <button class="tbtn tbtn-pill tbtn-rewind" data-hold="rewind" title="Hold to rewind">⟲</button>
        <button class="tbtn tbtn-pill" data-key="${Key.Start}">start</button>
      </div>`;

    const rewind = this.element.querySelector<HTMLButtonElement>("[data-hold=rewind]")!;
    const holdOn = (e: PointerEvent) => {
      e.preventDefault();
      rewind.setPointerCapture(e.pointerId);
      rewind.classList.add("active");
      this.onRewind(true);
    };
    const holdOff = () => {
      if (!rewind.classList.contains("active")) return;
      rewind.classList.remove("active");
      this.onRewind(false);
    };
    rewind.addEventListener("pointerdown", holdOn);
    rewind.addEventListener("pointerup", holdOff);
    rewind.addEventListener("pointercancel", holdOff);
    rewind.addEventListener("lostpointercapture", holdOff);
    rewind.addEventListener("contextmenu", (e) => e.preventDefault());

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
      const dead = 0.12;
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
        keys = table[sector] ?? 0;
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
      dpad.setPointerCapture(e.pointerId);
      update(e);
    });
    dpad.addEventListener("pointermove", (e) => {
      if (e.pointerId === dpadPointer) update(e);
    });
    const end = (e: PointerEvent) => {
      if (e.pointerId !== dpadPointer) return;
      dpadPointer = null;
      this.dpadKeys = 0;
      dpad.dataset["dir"] = "0";
      this.emit();
    };
    dpad.addEventListener("pointerup", end);
    dpad.addEventListener("pointercancel", end);
    dpad.addEventListener("contextmenu", (e) => e.preventDefault());
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
