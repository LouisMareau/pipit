// Keyboard, controller and touch input, merged into one GBA key bit set.
//
// Keyboard: every action (GBA key, fast-forward, rewind, pause) is bound to one
// `KeyboardEvent.code`; the user can rebind them in the settings.
//
// Controllers: the browser reports them through the Gamepad API (Chrome only
// exposes a pad after a button is pressed on it). One pad is *active* at a time
// and contributes only while controller input is enabled; the keyboard always
// works. The first pad detected switches controller input on automatically.
// Each pad reads through a mapping (GBA key → button index) that the user can
// change in the controller settings.

import type { ControllerMapping, KeyboardAction, KeyboardMapping, KeyName } from "../types";
import { DEFAULT_KEYBOARD, DEFAULT_MAPPING, Key } from "../types";

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

/** A pending "press something" request from a remapping screen. */
export interface Capture<T> {
  promise: Promise<T | null>;
  cancel(): void;
}

/** Human-readable name for a `KeyboardEvent.code`. */
export function keyLabel(code: string): string {
  if (!code) return "—";
  const special: Record<string, string> = {
    Space: "Space",
    Enter: "Enter",
    Backspace: "Backspace",
    Tab: "Tab",
    Escape: "Esc",
    ArrowUp: "↑",
    ArrowDown: "↓",
    ArrowLeft: "←",
    ArrowRight: "→",
    ShiftLeft: "Left Shift",
    ShiftRight: "Right Shift",
    ControlLeft: "Left Ctrl",
    ControlRight: "Right Ctrl",
    AltLeft: "Left Alt",
    AltRight: "Right Alt",
    CapsLock: "Caps Lock",
    Backquote: "`",
    Minus: "-",
    Equal: "=",
    BracketLeft: "[",
    BracketRight: "]",
    Backslash: "\\",
    Semicolon: ";",
    Quote: "'",
    Comma: ",",
    Period: ".",
    Slash: "/",
  };
  if (special[code]) return special[code];
  const simple = /^(Key|Digit|Numpad)(.+)$/.exec(code);
  if (simple) return simple[1] === "Numpad" ? `Numpad ${simple[2]}` : simple[2]!;
  return code;
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
  private keyboardMap = new Map<string, KeyboardAction>();
  private buttonCapture: { resolve: (button: number | null) => void; previous: boolean[] } | null = null;
  private keyCapture: ((code: string | null) => void) | null = null;
  onChange: (keys: number) => void = () => {};
  onFastForward: (held: boolean) => void = () => {};
  onRewind: (held: boolean) => void = () => {};
  onPause: () => void = () => {};
  onGamepads: (state: GamepadState) => void = () => {};

  constructor() {
    this.setKeyboardMapping(DEFAULT_KEYBOARD);
  }

  attach(target: Window) {
    target.addEventListener("keydown", (e) => {
      if (this.keyCapture) {
        e.preventDefault();
        const resolve = this.keyCapture;
        this.keyCapture = null;
        resolve(e.code === "Escape" ? null : e.code);
        return;
      }
      const action = this.keyboardMap.get(e.code);
      if (!action) return;
      e.preventDefault();
      switch (action) {
        case "FastForward":
          if (!e.repeat) this.onFastForward(true);
          break;
        case "Rewind":
          if (!e.repeat) this.onRewind(true);
          break;
        case "Pause":
          if (!e.repeat) this.onPause();
          break;
        default:
          this.keyboard |= Key[action];
          this.emit();
      }
    });
    target.addEventListener("keyup", (e) => {
      const action = this.keyboardMap.get(e.code);
      if (!action) return;
      switch (action) {
        case "FastForward":
          this.onFastForward(false);
          break;
        case "Rewind":
          this.onRewind(false);
          break;
        case "Pause":
          break;
        default:
          this.keyboard &= ~Key[action];
          this.emit();
      }
    });
    target.addEventListener("blur", () => {
      this.keyboard = 0;
      this.emit();
      this.onFastForward(false);
      this.onRewind(false);
    });
    target.addEventListener("gamepadconnected", () => this.refreshGamepads());
    target.addEventListener("gamepaddisconnected", () => this.refreshGamepads());
    this.refreshGamepads();
  }

  setTouch(keys: number) {
    this.touch = keys;
    this.emit();
  }

  setKeyboardMapping(mapping: KeyboardMapping) {
    this.keyboardMap.clear();
    for (const [action, code] of Object.entries(mapping) as [KeyboardAction, string][]) {
      if (code) this.keyboardMap.set(code, action);
    }
    this.keyboard = 0;
    this.emit();
  }

  /** Waits for the next key press (Escape cancels). Game input is suspended meanwhile. */
  captureKey(): Capture<string> {
    this.keyCapture?.(null);
    let resolve!: (code: string | null) => void;
    const promise = new Promise<string | null>((r) => (resolve = r));
    this.keyCapture = resolve;
    this.keyboard = 0;
    this.emit();
    return {
      promise,
      cancel: () => {
        if (this.keyCapture === resolve) {
          this.keyCapture = null;
          resolve(null);
        }
      },
    };
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
  captureButton(): Capture<number> {
    this.buttonCapture?.resolve(null);
    const pad = this.active !== null ? navigator.getGamepads()[this.active] : null;
    const previous = pad ? pad.buttons.map((b) => b.pressed) : [];
    let resolve!: (button: number | null) => void;
    const promise = new Promise<number | null>((r) => (resolve = r));
    this.buttonCapture = { resolve, previous };
    this.gamepad = 0;
    this.emit();
    return {
      promise,
      cancel: () => {
        if (this.buttonCapture?.resolve === resolve) {
          this.buttonCapture = null;
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

      if (this.buttonCapture) {
        const pressed = pad.buttons.findIndex((b, i) => b.pressed && !this.buttonCapture!.previous[i]);
        this.buttonCapture.previous = pad.buttons.map((b) => b.pressed);
        if (pressed >= 0) {
          const { resolve } = this.buttonCapture;
          this.buttonCapture = null;
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
