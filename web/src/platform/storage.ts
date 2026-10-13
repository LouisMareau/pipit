// IndexedDB storage. Everything stays on the device: ROMs, saves and settings.

import type { RomEntry, Settings, System } from "../types";
import { DEFAULT_SETTINGS } from "../types";

const DB_NAME = "pipit";
const DB_VERSION = 2;

let dbPromise: Promise<IDBDatabase> | null = null;

function open(): Promise<IDBDatabase> {
  if (dbPromise) return dbPromise;
  dbPromise = new Promise((resolve, reject) => {
    const request = indexedDB.open(DB_NAME, DB_VERSION);
    request.onupgradeneeded = () => {
      const db = request.result;
      for (const [name, keyPath] of [
        ["roms", "id"],
        ["romData", "id"],
        ["saves", "id"],
        ["settings", "key"],
        ["states", "key"],
      ] as const) {
        if (!db.objectStoreNames.contains(name)) db.createObjectStore(name, { keyPath });
      }
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
  return dbPromise;
}

function tx<T>(store: string, mode: IDBTransactionMode, run: (s: IDBObjectStore) => IDBRequest<T>): Promise<T> {
  return open().then(
    (db) =>
      new Promise<T>((resolve, reject) => {
        const request = run(db.transaction(store, mode).objectStore(store));
        request.onsuccess = () => resolve(request.result);
        request.onerror = () => reject(request.error);
      }),
  );
}

export async function listRoms(): Promise<RomEntry[]> {
  const entries = await tx<RomEntry[]>("roms", "readonly", (s) => s.getAll());
  return entries.sort((a, b) => b.lastPlayed - a.lastPlayed || b.addedAt - a.addedAt);
}

export async function addRom(file: File): Promise<RomEntry> {
  const data = await file.arrayBuffer();
  const id = await hash(data);
  const system: System = /.gbc?$/i.test(file.name) ? "gbc" : /.nds$/i.test(file.name) ? "nds" : "gba";
  // The DS header starts with the title and game code; the Advance's are at A0h.
  const header = system === "nds" ? 0 : 0xa0;
  const entry: RomEntry = {
    id,
    name: file.name.replace(/.(gba|gbc|gb|nds)$/i, ""),
    system,
    title: readHeaderString(data, header, 12),
    gameCode: readHeaderString(data, header + 0xc, 4),
    size: data.byteLength,
    addedAt: Date.now(),
    lastPlayed: 0,
  };
  await tx("romData", "readwrite", (s) => s.put({ id, data }));
  await tx("roms", "readwrite", (s) => s.put(entry));
  return entry;
}

export async function getRomData(id: string): Promise<ArrayBuffer | null> {
  const row = await tx<{ id: string; data: ArrayBuffer } | undefined>("romData", "readonly", (s) => s.get(id));
  return row?.data ?? null;
}

export async function touchRom(id: string) {
  const entry = await tx<RomEntry | undefined>("roms", "readonly", (s) => s.get(id));
  if (entry) await tx("roms", "readwrite", (s) => s.put({ ...entry, lastPlayed: Date.now() }));
}

/** Changes some of a library entry's fields. */
export async function updateRom(id: string, patch: Partial<RomEntry>) {
  const entry = await tx<RomEntry | undefined>("roms", "readonly", (s) => s.get(id));
  if (entry) await tx("roms", "readwrite", (s) => s.put({ ...entry, ...patch }));
}

export async function deleteRom(id: string) {
  await tx("roms", "readwrite", (s) => s.delete(id));
  await tx("romData", "readwrite", (s) => s.delete(id));
  await tx("saves", "readwrite", (s) => s.delete(id));
  await tx("states", "readwrite", (s) => s.delete(IDBKeyRange.bound(`${id}:`, `${id}:￿`)));
}

export async function getSave(id: string): Promise<ArrayBuffer | null> {
  const row = await tx<{ id: string; data: ArrayBuffer } | undefined>("saves", "readonly", (s) => s.get(id));
  return row?.data ?? null;
}

/** Keeps a game's save, and notes on its entry when it last changed (see `RomEntry.savedAt`). */
export async function putSave(id: string, data: ArrayBuffer) {
  const now = Date.now();
  await tx("saves", "readwrite", (s) => s.put({ id, data, updatedAt: now }));
  await updateRom(id, { savedAt: now });
}

/** A save's size and age, without its data. */
export async function getSaveMeta(id: string): Promise<{ size: number; updatedAt: number } | null> {
  const row = await tx<{ id: string; data: ArrayBuffer; updatedAt?: number } | undefined>("saves", "readonly", (s) => s.get(id));
  return row ? { size: row.data.byteLength, updatedAt: row.updatedAt ?? 0 } : null;
}

/**
 * A newer build of a game took the old one's place: its save, states and
 * settings move over to the new entry, and the old entry goes.
 */
export async function replaceRom(oldId: string, newId: string) {
  const old = await tx<RomEntry | undefined>("roms", "readonly", (s) => s.get(oldId));
  const entry = await tx<RomEntry | undefined>("roms", "readonly", (s) => s.get(newId));
  if (!old || !entry || oldId === newId) return;
  const save = await tx<{ id: string; data: ArrayBuffer; updatedAt?: number } | undefined>("saves", "readonly", (s) => s.get(oldId));
  if (save) await tx("saves", "readwrite", (s) => s.put({ ...save, id: newId }));
  const states = await tx<StateEntry[]>("states", "readonly", (s) => s.getAll(IDBKeyRange.bound(`${oldId}:`, `${oldId}:￿`)));
  for (const state of states) {
    await tx("states", "readwrite", (s) => s.put({ ...state, key: `${newId}:${state.slot}`, romId: newId }));
  }
  const { id: _id, title: _title, gameCode: _code, size: _size, ...kept } = old;
  await tx("roms", "readwrite", (s) => s.put({ ...entry, ...kept, id: newId }));
  await deleteRom(oldId);
}

export interface StateEntry {
  key: string;
  romId: string;
  slot: number;
  data: ArrayBuffer;
  savedAt: number;
}

export async function putState(romId: string, slot: number, data: ArrayBuffer) {
  const entry: StateEntry = { key: `${romId}:${slot}`, romId, slot, data, savedAt: Date.now() };
  await tx("states", "readwrite", (s) => s.put(entry));
}

export async function getState(romId: string, slot: number): Promise<StateEntry | null> {
  const row = await tx<StateEntry | undefined>("states", "readonly", (s) => s.get(`${romId}:${slot}`));
  return row ?? null;
}

/** Every saved state of a game, without their data. */
export async function listStates(romId: string): Promise<{ slot: number; savedAt: number }[]> {
  const rows = await tx<StateEntry[]>("states", "readonly", (s) =>
    s.getAll(IDBKeyRange.bound(`${romId}:`, `${romId}:￿`)),
  );
  return rows.map((r) => ({ slot: r.slot, savedAt: r.savedAt }));
}

export async function loadSettings(): Promise<Settings> {
  const row = await tx<{ key: string; value: Partial<Settings> } | undefined>("settings", "readonly", (s) =>
    s.get("settings"),
  );
  return { ...DEFAULT_SETTINGS, ...(row?.value ?? {}) };
}

export async function saveSettings(settings: Settings) {
  await tx("settings", "readwrite", (s) => s.put({ key: "settings", value: settings }));
}

async function hash(data: ArrayBuffer): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-1", data);
  return Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, "0")).join("");
}

function readHeaderString(data: ArrayBuffer, offset: number, length: number): string {
  const bytes = new Uint8Array(data, offset, Math.min(length, Math.max(0, data.byteLength - offset)));
  let out = "";
  for (const b of bytes) {
    if (b === 0) break;
    out += String.fromCharCode(b);
  }
  return out;
}

/** A file the app keeps on the device, such as the DS firmware image. */
export async function getFile(key: string): Promise<ArrayBuffer | null> {
  const row = await tx<{ key: string; data: ArrayBuffer } | undefined>("settings", "readonly", (s) => s.get(`file:${key}`));
  return row?.data ?? null;
}

export async function putFile(key: string, data: ArrayBuffer) {
  await tx("settings", "readwrite", (s) => s.put({ key: `file:${key}`, data }));
}

export async function deleteFile(key: string) {
  await tx("settings", "readwrite", (s) => s.delete(`file:${key}`));
}
