// Messages between the UI thread and the emulator worker, plus shared types.

/** GBA key bits, matching `pipit_gba::Keys`. */
export const Key = {
  A: 1 << 0,
  B: 1 << 1,
  Select: 1 << 2,
  Start: 1 << 3,
  Right: 1 << 4,
  Left: 1 << 5,
  Up: 1 << 6,
  Down: 1 << 7,
  R: 1 << 8,
  L: 1 << 9,
} as const;

export type KeyName = keyof typeof Key;

export const SCREEN_WIDTH = 240;
export const SCREEN_HEIGHT = 160;

/** Several consoles on a link cable, all running the loaded ROM (see `platform/netplay.ts`). */
export interface LinkLoad {
  players: number;
  /** The console this player sees and controls. */
  local: number;
  /** Each console's save data, in player order. */
  saves: (ArrayBuffer | null)[];
}

export type ToWorker =
  | { type: "load"; rom: ArrayBuffer; save: ArrayBuffer | null; bios: ArrayBuffer | null; unixSeconds: number; link?: LinkLoad }
  | { type: "run" }
  | { type: "pause" }
  /**
   * The UI's display loop asks for one emulated frame (see `platform/pacer.ts`).
   * On a link, `keys` holds every player's keys for that frame.
   */
  | { type: "frame"; keys?: number[] }
  | { type: "keys"; keys: number }
  | { type: "fastForward"; enabled: boolean }
  | { type: "colors"; mode: number; strength: number }
  | { type: "requestSave" }
  | { type: "saveState"; slot: number }
  | { type: "loadState"; data: ArrayBuffer }
  | { type: "returnFrame"; pixels: ArrayBuffer };

export type FromWorker =
  | { type: "loaded"; title: string; gameCode: string }
  /** `maxGapMs`: the longest pause between frames in the last second (stutter diagnostics). */
  | { type: "frame"; pixels: ArrayBuffer; audio: ArrayBuffer; fps: number; maxGapMs: number }
  /** On a link: a digest of every console's state after `frame` frames (see `netplay.ts`). */
  | { type: "hash"; frame: number; hash: number }
  | { type: "save"; data: ArrayBuffer }
  | { type: "state"; slot: number; data: ArrayBuffer }
  | { type: "error"; message: string };

export interface RomEntry {
  id: string;
  name: string;
  title: string;
  gameCode: string;
  size: number;
  addedAt: number;
  lastPlayed: number;
}

/** GBA keys in the order the remapping screens list them. */
export const KEY_NAMES: KeyName[] = ["A", "B", "L", "R", "Start", "Select", "Up", "Down", "Left", "Right"];

/** Emulator actions that keys and controller buttons can drive besides GBA keys. */
export type EmulatorAction = "FastForward" | "Pause";
export const EMULATOR_ACTIONS: EmulatorAction[] = ["FastForward", "Pause"];

/** Everything a controller button can be bound to. */
export type ControllerAction = KeyName | EmulatorAction;
export const CONTROLLER_ACTIONS: ControllerAction[] = [...KEY_NAMES, ...EMULATOR_ACTIONS];

/** Which controller button (Gamepad API index) drives each action. */
export interface ControllerMapping {
  buttons: Partial<Record<ControllerAction, number>>;
  /** The left stick also works as the D-pad. */
  stickDpad: boolean;
}

/** Standard-mapping defaults (Xbox / PlayStation / Switch Pro all follow it). */
export const DEFAULT_MAPPING: ControllerMapping = {
  buttons: {
    A: 0,
    B: 1,
    L: 4,
    R: 5,
    Select: 8,
    Start: 9,
    Up: 12,
    Down: 13,
    Left: 14,
    Right: 15,
    FastForward: 7, // RT / R2
  },
  stickDpad: true,
};

/** Human names for standard-mapping button indices. */
export const GAMEPAD_BUTTON_NAMES: Record<number, string> = {
  0: "A / ✕",
  1: "B / ○",
  2: "X / □",
  3: "Y / △",
  4: "LB / L1",
  5: "RB / R1",
  6: "LT / L2",
  7: "RT / R2",
  8: "Back / Share / −",
  9: "Start / Options / +",
  10: "Left stick",
  11: "Right stick",
  12: "D-pad up",
  13: "D-pad down",
  14: "D-pad left",
  15: "D-pad right",
  16: "Home",
};

/** Everything a keyboard key can be bound to: GBA keys plus emulator actions. */
export type KeyboardAction = KeyName | EmulatorAction;

/** Keyboard binding per action, as `KeyboardEvent.code` values ("" = unbound). */
export type KeyboardMapping = Record<KeyboardAction, string>;

export const KEYBOARD_ACTIONS: KeyboardAction[] = [...KEY_NAMES, ...EMULATOR_ACTIONS];

export const DEFAULT_KEYBOARD: KeyboardMapping = {
  A: "KeyZ",
  B: "KeyX",
  L: "KeyA",
  R: "KeyS",
  Start: "Enter",
  Select: "Backspace",
  Up: "ArrowUp",
  Down: "ArrowDown",
  Left: "ArrowLeft",
  Right: "ArrowRight",
  FastForward: "Space",
  Pause: "KeyP",
};

export type TouchLayout = "auto" | "gba" | "gbasp";
/** A touch layout after "auto" has been resolved from the orientation. */
export type ResolvedTouchLayout = "gba" | "gbasp";

/** Everything the layout editor can move and resize. */
export type ControlId = "screen" | "dpad" | "abpad" | "l" | "r" | "select" | "start";
export const CONTROL_IDS: ControlId[] = ["screen", "dpad", "abpad", "l", "r", "select", "start"];

/** Position and size as fractions (0-1) of the stage, the player minus the toolbar. */
export interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

export type CustomLayout = Record<ControlId, Box>;

/** "gba" reproduces the muted colours of the original LCD; "off" shows raw palettes. */
export type ColorCorrection = "off" | "gba";

export interface Settings {
  volume: number;
  /** Integer pixel scaling instead of filling the available space. */
  integerScale: boolean;
  /** Show touch controls on devices with a mouse as well. */
  alwaysShowTouch: boolean;
  /** Where the touch controls go: beside the screen (GBA) or below it (GBA SP). */
  touchLayout: TouchLayout;
  /** Layouts arranged in the editor, per touch layout; absent = the stock layout. */
  touchLayouts: Partial<Record<ResolvedTouchLayout, CustomLayout>>;
  colorCorrection: ColorCorrection;
  /** How far towards the LCD look, 0-100. */
  colorStrength: number;
  /** Button mappings, keyed by controller id. */
  controllerMappings: Record<string, ControllerMapping>;
  keyboardMapping: KeyboardMapping;
}

export const DEFAULT_SETTINGS: Settings = {
  volume: 0.8,
  integerScale: false,
  alwaysShowTouch: false,
  touchLayout: "auto",
  touchLayouts: {},
  colorCorrection: "gba",
  colorStrength: 100,
  controllerMappings: {},
  keyboardMapping: DEFAULT_KEYBOARD,
};

/** Number of save-state slots offered in the UI. */
export const STATE_SLOTS = 3;
