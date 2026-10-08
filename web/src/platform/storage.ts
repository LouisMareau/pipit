// IndexedDB storage. Everything stays on the device: ROMs, saves and settings.

import type { RomEntry, Settings } from "../types";
import { DEFAULT_SETTINGS } from "../types";

const DB_NAME = "pipit";
const DB_VERSION = 1;

let dbPromise: Promise<IDBDatabase> | null = null;

function open(): Promise<IDBDatabase> {
  if (dbPromise) return dbPromise;
  dbPromise = new Promise((resolve, reject) => {
    const request = indexedDB.open(DB_NAME, DB_VERSION);
    request.onupgradeneeded = () => {
      const db = request.result;
      db.createObjectStore("roms", { keyPath: "id" });
      db.createObjectStore("romData", { keyPath: "id" });
      db.createObjectStore("saves", { keyPath: "id" });
      db.createObjectStore("settings", { keyPath: "key" });
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
  const entry: RomEntry = {
    id,
    name: file.name.replace(/\.gba$/i, ""),
    title: readHeaderString(data, 0xa0, 12),
    gameCode: readHeaderString(data, 0xac, 4),
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

export async function deleteRom(id: string) {
  await tx("roms", "readwrite", (s) => s.delete(id));
  await tx("romData", "readwrite", (s) => s.delete(id));
  await tx("saves", "readwrite", (s) => s.delete(id));
}

export async function getSave(id: string): Promise<ArrayBuffer | null> {
  const row = await tx<{ id: string; data: ArrayBuffer } | undefined>("saves", "readonly", (s) => s.get(id));
  return row?.data ?? null;
}

export async function putSave(id: string, data: ArrayBuffer) {
  await tx("saves", "readwrite", (s) => s.put({ id, data, updatedAt: Date.now() }));
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
