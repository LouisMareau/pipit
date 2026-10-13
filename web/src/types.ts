// Messages between the UI thread and the emulator worker, plus shared types.

/** Key bits, matching `pipit_gba::Keys`; X and Y are the DS's (`pipit_nds::Keys`). */
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
  X: 1 << 10,
  Y: 1 << 11,
} as const;

export type KeyName = keyof typeof Key;

/**
 * Playing together on DSs, each frame's input number carries the pen as well
 * as the keys: bit 30 says it does, bit 28 that the pen is down, bits 12–19
 * and 20–27 where it is (x, y).
 */
export const PEN_INCLUDED = 1 << 30;
export const PEN_DOWN = 1 << 28;

export const SCREEN_WIDTH = 240;
export const SCREEN_HEIGHT = 160;

/** Several consoles on a link cable, all running the loaded ROM (see `platform/netplay.ts`). */
export interface LinkLoad {
  players: number;
  /** The console this player sees and controls. */
  local: number;
  /** Each console's save data, in player order. */
  saves: (ArrayBuffer | null)[];
  /** The consoles carry Wireless Adapters instead of a link cable (GBA only). */
  wireless?: boolean;
}

/**
 * A DS console's own settings, as games show them: the nickname (up to ten
 * characters), the favourite colour (0–15), the birthday, and the language
 * (0 Japanese, 1 English, 2 French, 3 German, 4 Italian, 5 Spanish).
 */
export interface DsUser {
  nickname: string;
  color: number;
  month: number;
  day: number;
  language: number;
}

/** What goes in a DS's GBA slot: nothing, the Rumble Pak, or a GBA cartridge with its save. */
export interface Slot2Load {
  kind: "empty" | "rumble" | "cartridge";
  rom?: ArrayBuffer;
  save?: ArrayBuffer | null;
}

/** Which console a game is for; `.gb` and `.gbc` files are Game Boy games, `.nds` DS games. */
export type System = "gba" | "gbc" | "nds";

export const SYSTEM_NAMES: Record<System, string> = { gba: "GBA", gbc: "GBC", nds: "DS" };

export type ToWorker =
  | {
      type: "load";
      rom: ArrayBuffer;
      save: ArrayBuffer | null;
      bios: ArrayBuffer | null;
      unixSeconds: number;
      system: System;
      link?: LinkLoad;
      /** For a DS: what is in its GBA slot (nothing when absent). */
      slot2?: Slot2Load;
      /** For a DS: the console's firmware image (Pipit's own when absent). */
      firmware?: ArrayBuffer | null;
      /** For a DS: the card's save chip by name, when the player named it ("auto" otherwise). */
      saveType?: string;
    }
  | { type: "run" }
  | { type: "pause" }
  /**
   * The UI's display loop asks for one emulated frame (see `platform/pacer.ts`).
   * On a link, `keys` holds every player's keys for that frame (with the pen
   * packed in, on DSs: see `PEN_INCLUDED`); `snapshot` asks
   * for the state before it to be kept (some keys were guessed) under `frame`.
   */
  | { type: "frame"; keys?: number[]; frame?: number; snapshot?: boolean }
  /**
   * A guess was wrong: go back to the state kept under `toFrame` and re-run the
   * frames from there with `inputs` (one key set per player per frame), keeping
   * the state before each frame whose `snapshots` flag is set.
   */
  | { type: "rollback"; toFrame: number; inputs: number[][]; snapshots: boolean[] }
  /** Guesses up to this frame proved right: their kept states can go. */
  | { type: "confirm"; frame: number }
  /** Show and play another console of the link. */
  | { type: "view"; player: number }
  /** Also send the other consoles' pictures with every frame (shown beside the player's). */
  | { type: "sideScreens"; enabled: boolean }
  | { type: "keys"; keys: number }
  /** The pen on the DS's bottom screen (x 0–255, y 0–191); negative lifts it. */
  | { type: "touch"; x: number; y: number }
  /** Changes what is in the DS's GBA slot while it runs. */
  | { type: "slot2"; slot2: Slot2Load }
  /** Names the DS card's save chip while the game runs (its save is reloaded into it). */
  | { type: "saveType"; name: string }
  /** Asks what user settings a DS firmware image holds (Pipit's own when null); answered with `firmwareUser`. */
  | { type: "firmwareUser"; image: ArrayBuffer | null }
  /** Writes user settings into a DS firmware image (Pipit's own when null); answered with `firmware`. */
  | { type: "setFirmwareUser"; image: ArrayBuffer | null; user: DsUser }
  | { type: "fastForward"; enabled: boolean }
  | { type: "colors"; mode: number; strength: number }
  | { type: "requestSave" }
  | { type: "saveState"; slot: number }
  | { type: "loadState"; data: ArrayBuffer }
  | { type: "returnFrame"; pixels: ArrayBuffer };

export type FromWorker =
  | { type: "loaded"; title: string; gameCode: string; width: number; height: number }
  /** `maxGapMs`: the longest pause between frames in the last second (stutter diagnostics). */
  | {
      type: "frame";
      pixels: ArrayBuffer;
      audio: ArrayBuffer;
      fps: number;
      maxGapMs: number;
      /** On a link with side screens on: the other consoles' pictures. */
      sides?: { player: number; pixels: ArrayBuffer }[];
      /** On a link: how much the in-game link has done so far (see `Emulator.link_activity`). */
      activity?: number;
      /** On a DS: the Rumble Pak moved during this frame. */
      rumble?: boolean;
    }
  /** On a link: a digest of every console's state after `frame` frames (see `netplay.ts`). */
  | { type: "hash"; frame: number; hash: number }
  | { type: "save"; data: ArrayBuffer }
  /** The GBA cartridge in a DS's slot changed its save memory. */
  | { type: "slot2Save"; data: ArrayBuffer }
  /** A DS firmware image to keep: the console wrote to it, or it carries new user settings. */
  | { type: "firmware"; data: ArrayBuffer }
  | { type: "firmwareUser"; user: DsUser }
  | { type: "state"; slot: number; data: ArrayBuffer }
  | { type: "error"; message: string };

export interface RomEntry {
  id: string;
  name: string;
  /** Absent in entries from before the Game Boy core: those are Advance games. */
  system?: System;
  title: string;
  gameCode: string;
  size: number;
  addedAt: number;
  lastPlayed: number;
  /** A DS game's GBA slot: "empty", "rumble" (the Rumble Pak, the default) or a GBA game's id. */
  slot2?: string;
  /** A DS game's save chip, named by the player ("eeprom4k" … "flash64m"); absent or "auto" lets Pipit tell. */
  saveType?: string;
}

/** Keys in the order the remapping screens list them (X and Y are the DS's). */
export const KEY_NAMES: KeyName[] = ["A", "B", "X", "Y", "L", "R", "Start", "Select", "Up", "Down", "Left", "Right"];

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
    X: 2,
    Y: 3,
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
  X: "KeyW",
  Y: "KeyQ",
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
  /** Playing together: show the other players' screens beside your own. */
  sideScreens: boolean;
  /** Button mappings, keyed by controller id. */
  controllerMappings: Record<string, ControllerMapping>;
  keyboardMapping: KeyboardMapping;
  /** Playing together: where players find each other, and an optional relay. */
  connection: ConnectionSettings;
}

/**
 * `server`: a PeerJS introduction server as a URL (empty = the public one).
 * `relayUrl` and its credentials: a TURN relay for players behind strict NATs
 * (empty = none; the hosting guide explains running one).
 */
export interface ConnectionSettings {
  server: string;
  relayUrl: string;
  relayUsername: string;
  relayCredential: string;
}

export const DEFAULT_CONNECTION: ConnectionSettings = { server: "", relayUrl: "", relayUsername: "", relayCredential: "" };

export const DEFAULT_SETTINGS: Settings = {
  volume: 0.8,
  integerScale: false,
  alwaysShowTouch: false,
  touchLayout: "auto",
  touchLayouts: {},
  colorCorrection: "gba",
  colorStrength: 100,
  sideScreens: false,
  controllerMappings: {},
  keyboardMapping: DEFAULT_KEYBOARD,
  connection: DEFAULT_CONNECTION,
};

/** Number of save-state slots offered in the UI. */
export const STATE_SLOTS = 3;

/** The DS card save chips a player can name, with their labels (see `RomEntry.saveType`). */
export const SAVE_TYPES: [string, string][] = [
  ["auto", "Auto (detected)"],
  ["eeprom4k", "EEPROM 4 Kbit (512 B)"],
  ["eeprom64k", "EEPROM 64 Kbit (8 KB)"],
  ["eeprom512k", "EEPROM 512 Kbit (64 KB)"],
  ["fram256k", "FRAM 256 Kbit (32 KB)"],
  ["eeprom1m", "EEPROM 1 Mbit (128 KB)"],
  ["flash2m", "Flash 2 Mbit (256 KB)"],
  ["flash4m", "Flash 4 Mbit (512 KB)"],
  ["flash8m", "Flash 8 Mbit (1 MB)"],
  ["flash16m", "Flash 16 Mbit (2 MB)"],
  ["flash32m", "Flash 32 Mbit (4 MB)"],
  ["flash64m", "Flash 64 Mbit (8 MB)"],
];
