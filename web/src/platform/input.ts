// Keyboard, controller and touch input, merged into one GBA key bit set.
//
// Controllers: the browser reports them through the Gamepad API (Chrome only
// exposes a pad after a button is pressed on it). One pad is *active* at a time
// and contributes only while controller input is enabled; the keyboard always
// works. The first pad detected switches controller input on automatically.
// Each pad reads through a mapping (GBA key → button index) that the user can
// change in the controller settings.

import type { ControllerMapping, KeyName } from "../types";
import { DEFAULT_MAPPING, Key } from "../types";

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

export interface GamepadInfo {
  index: number;
  id: string;
}

export interface GamepadState {
  /** Controllers the browser currently reports. */
  detected: GamepadInfo[];
  /** Index (into the Gamepad API) of the controller being read, if any. */
  active: number | null;
  /** Whether the active controller's input is used at all. */
  enabled: boolean;
}

/** A pending "press a button" request from the remapping screen. */
export interface ButtonCapture {
  promise: Promise<number | null>;
  cancel(): void;
}

export class Input {
  private keyboard = 0;
  private touch = 0;
  private gamepad = 0;
  private last = -1;
  private pollTimer: number | null = null;
  private pollCount = 0;
  private detected: GamepadInfo[] = [];
  private active: number | null = null;
  private enabled = false;
  private mapping: ControllerMapping = DEFAULT_MAPPING;
  private capture: { resolve: (button: number | null) => void; previous: boolean[] } | null = null;
  readonly fastForwardKey = "Space";
  readonly rewindKey = "KeyR";
  onChange: (keys: number) => void = () => {};
  onFastForward: (held: boolean) => void = () => {};
  onRewind: (held: boolean) => void = () => {};
  onGamepads: (state: GamepadState) => void = () => {};

  attach(target: Window) {
    target.addEventListener("keydown", (e) => {
      if (e.code === this.fastForwardKey) {
        this.onFastForward(true);
        e.preventDefault();
        return;
      }
      if (e.code === this.rewindKey) {
        if (!e.repeat) this.onRewind(true);
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
      if (e.code === this.rewindKey) {
        this.onRewind(false);
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
    target.addEventListener("gamepadconnected", () => this.refreshGamepads());
    target.addEventListener("gamepaddisconnected", () => this.refreshGamepads());
    this.refreshGamepads();
  }

  setTouch(keys: number) {
    this.touch = keys;
    this.emit();
  }

  gamepadState(): GamepadState {
    return { detected: [...this.detected], active: this.active, enabled: this.enabled };
  }

  /** The id of the active controller, used to look up its mapping. */
  activeGamepadId(): string | null {
    return this.detected.find((p) => p.index === this.active)?.id ?? null;
  }

  setMapping(mapping: ControllerMapping) {
    this.mapping = mapping;
    this.gamepad = 0;
    this.emit();
  }

  /** Switches controller input on or off without forgetting the active pad. */
  setGamepadEnabled(enabled: boolean) {
    this.enabled = enabled && this.active !== null;
    this.gamepad = 0;
    this.emit();
    this.onGamepads(this.gamepadState());
  }

  /** Makes the given controller the one being read, and keeps input enabled. */
  setActiveGamepad(index: number) {
    if (!this.detected.some((p) => p.index === index)) return;
    this.active = index;
    this.enabled = true;
    this.gamepad = 0;
    this.emit();
    this.onGamepads(this.gamepadState());
  }

  /**
   * Waits for the next button pressed on the active controller. Game input from
   * the controller is suspended until the capture ends.
   */
  captureButton(): ButtonCapture {
    this.capture?.resolve(null);
    const pad = this.active !== null ? navigator.getGamepads()[this.active] : null;
    const previous = pad ? pad.buttons.map((b) => b.pressed) : [];
    let resolve!: (button: number | null) => void;
    const promise = new Promise<number | null>((r) => (resolve = r));
    this.capture = { resolve, previous };
    this.gamepad = 0;
    this.emit();
    return {
      promise,
      cancel: () => {
        if (this.capture?.resolve === resolve) {
          this.capture = null;
          resolve(null);
        }
      },
    };
  }

  /** Re-reads the list of controllers and keeps the active choice consistent. */
  private refreshGamepads() {
    const pads = navigator.getGamepads?.() ?? [];
    const detected: GamepadInfo[] = [];
    for (const pad of pads) if (pad) detected.push({ index: pad.index, id: pad.id });

    const same =
      detected.length === this.detected.length &&
      detected.every((p, i) => p.index === this.detected[i]?.index && p.id === this.detected[i]?.id);
    if (same) return;
    this.detected = detected;

    if (this.active !== null && !detected.some((p) => p.index === this.active)) {
      this.active = null;
    }
    if (this.active === null && detected.length > 0) {
      // A controller appeared: it becomes the input device.
      this.active = detected[0]!.index;
      this.enabled = true;
    }
    if (detected.length === 0) {
      this.enabled = false;
      this.gamepad = 0;
      this.emit();
      this.stopPolling();
    } else {
      this.startPolling();
    }
    this.onGamepads(this.gamepadState());
  }

  private startPolling() {
    if (this.pollTimer !== null) return;
    const poll = () => {
      this.pollTimer = requestAnimationFrame(poll);
      // Connection changes are not always announced; look every half second.
      if (++this.pollCount % 30 === 0) this.refreshGamepads();
      const pad = this.active !== null ? navigator.getGamepads()[this.active] : null;
      if (!pad) return;

      if (this.capture) {
        const pressed = pad.buttons.findIndex((b, i) => b.pressed && !this.capture!.previous[i]);
        this.capture.previous = pad.buttons.map((b) => b.pressed);
        if (pressed >= 0) {
          const { resolve } = this.capture;
          this.capture = null;
          resolve(pressed);
        }
        return;
      }

      let keys = 0;
      if (this.enabled) {
        for (const [name, index] of Object.entries(this.mapping.buttons) as [KeyName, number][]) {
          if (pad.buttons[index]?.pressed) keys |= Key[name];
        }
        if (this.mapping.stickDpad) {
          const [x = 0, y = 0] = pad.axes;
          if (x < -0.5) keys |= Key.Left;
          if (x > 0.5) keys |= Key.Right;
          if (y < -0.5) keys |= Key.Up;
          if (y > 0.5) keys |= Key.Down;
        }
      }
      if (keys !== this.gamepad) {
        this.gamepad = keys;
        this.emit();
      }
    };
    this.pollTimer = requestAnimationFrame(poll);
  }

  private stopPolling() {
    if (this.pollTimer !== null) cancelAnimationFrame(this.pollTimer);
    this.pollTimer = null;
  }

  private emit() {
    const keys = (this.keyboard | this.touch | this.gamepad) & 0x3ff;
    if (keys !== this.last) {
      this.last = keys;
      this.onChange(keys);
    }
  }
}
