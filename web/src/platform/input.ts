// Keyboard and gamepad input, merged into one GBA key bit set.

import { Key } from "../types";

const KEYBOARD: Record<string, number> = {
  KeyZ: Key.A,
  KeyX: Key.B,
  Enter: Key.Start,
  Backspace: Key.Select,
  ShiftRight: Key.Select,
  ArrowUp: Key.Up,
  ArrowDown: Key.Down,
  ArrowLeft: Key.Left,
  ArrowRight: Key.Right,
  KeyA: Key.L,
  KeyS: Key.R,
  KeyQ: Key.L,
  KeyW: Key.R,
};

/** Standard gamepad mapping: button index → GBA key. */
const GAMEPAD_BUTTONS: Record<number, number> = {
  0: Key.A, // A / Cross
  1: Key.B, // B / Circle
  2: Key.B, // X / Square (also B, handy on Nintendo-style pads)
  3: Key.A,
  4: Key.L,
  5: Key.R,
  6: Key.L,
  7: Key.R,
  8: Key.Select,
  9: Key.Start,
  12: Key.Up,
  13: Key.Down,
  14: Key.Left,
  15: Key.Right,
};

export class Input {
  private keyboard = 0;
  private touch = 0;
  private gamepad = 0;
  private last = -1;
  private pollTimer: number | null = null;
  readonly fastForwardKey = "Space";
  onChange: (keys: number) => void = () => {};
  onFastForward: (held: boolean) => void = () => {};

  attach(target: Window) {
    target.addEventListener("keydown", (e) => {
      if (e.code === this.fastForwardKey) {
        this.onFastForward(true);
        e.preventDefault();
        return;
      }
      const bit = KEYBOARD[e.code];
      if (bit === undefined) return;
      e.preventDefault();
      this.keyboard |= bit;
      this.emit();
    });
    target.addEventListener("keyup", (e) => {
      if (e.code === this.fastForwardKey) {
        this.onFastForward(false);
        return;
      }
      const bit = KEYBOARD[e.code];
      if (bit === undefined) return;
      this.keyboard &= ~bit;
      this.emit();
    });
    target.addEventListener("blur", () => {
      this.keyboard = 0;
      this.emit();
    });
    target.addEventListener("gamepadconnected", () => this.startPolling());
    if (navigator.getGamepads?.().some(Boolean)) this.startPolling();
  }

  setTouch(keys: number) {
    this.touch = keys;
    this.emit();
  }

  private startPolling() {
    if (this.pollTimer !== null) return;
    const poll = () => {
      let keys = 0;
      for (const pad of navigator.getGamepads()) {
        if (!pad) continue;
        pad.buttons.forEach((button, i) => {
          const bit = GAMEPAD_BUTTONS[i];
          if (bit !== undefined && button.pressed) keys |= bit;
        });
        const [x = 0, y = 0] = pad.axes;
        if (x < -0.5) keys |= Key.Left;
        if (x > 0.5) keys |= Key.Right;
        if (y < -0.5) keys |= Key.Up;
        if (y > 0.5) keys |= Key.Down;
      }
      if (keys !== this.gamepad) {
        this.gamepad = keys;
        this.emit();
      }
      this.pollTimer = requestAnimationFrame(poll);
    };
    this.pollTimer = requestAnimationFrame(poll);
  }

  private emit() {
    const keys = (this.keyboard | this.touch | this.gamepad) & 0x3ff;
    if (keys !== this.last) {
      this.last = keys;
      this.onChange(keys);
    }
  }
}
