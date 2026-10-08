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

export type ToWorker =
  | { type: "config"; rewindSeconds: number }
  | { type: "load"; rom: ArrayBuffer; save: ArrayBuffer | null; bios: ArrayBuffer | null; unixSeconds: number }
  | { type: "run" }
  | { type: "pause" }
  | { type: "keys"; keys: number }
  | { type: "fastForward"; enabled: boolean }
  | { type: "rewind"; enabled: boolean }
  | { type: "requestSave" }
  | { type: "saveState"; slot: number }
  | { type: "loadState"; data: ArrayBuffer }
  | { type: "returnFrame"; pixels: ArrayBuffer };

export type FromWorker =
  | { type: "loaded"; title: string; gameCode: string }
  | { type: "frame"; pixels: ArrayBuffer; audio: ArrayBuffer; fps: number }
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

export interface Settings {
  volume: number;
  /** Integer pixel scaling instead of filling the available space. */
  integerScale: boolean;
  /** Show touch controls on devices with a mouse as well. */
  alwaysShowTouch: boolean;
  /** Seconds of gameplay kept for rewinding (0 disables it). */
  rewindSeconds: number;
}

export const DEFAULT_SETTINGS: Settings = {
  volume: 0.8,
  integerScale: false,
  alwaysShowTouch: false,
  rewindSeconds: 4,
};

/** Number of save-state slots offered in the UI. */
export const STATE_SLOTS = 3;
