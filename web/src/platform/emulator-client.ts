// UI-thread handle on the emulator worker: a small typed event emitter.

import type { FromWorker, LinkLoad, System, ToWorker } from "../types";

type Handlers = {
  frame: (pixels: ArrayBuffer, audio: ArrayBuffer, fps: number, maxGapMs: number) => void;
  loaded: (title: string, gameCode: string, width: number, height: number) => void;
  save: (data: ArrayBuffer) => void;
  state: (slot: number, data: ArrayBuffer) => void;
  hash: (frame: number, hash: number) => void;
  error: (message: string) => void;
};

export class EmulatorClient {
  private worker: Worker;
  private handlers: Partial<Handlers> = {};

  constructor() {
    // Vite only bundles workers it can see statically, so this exact form matters.
    this.worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
    this.worker.onmessage = (event: MessageEvent<FromWorker>) => {
      const msg = event.data;
      switch (msg.type) {
        case "frame":
          this.handlers.frame?.(msg.pixels, msg.audio, msg.fps, msg.maxGapMs);
          break;
        case "loaded":
          this.handlers.loaded?.(msg.title, msg.gameCode, msg.width, msg.height);
          break;
        case "save":
          this.handlers.save?.(msg.data);
          break;
        case "state":
          this.handlers.state?.(msg.slot, msg.data);
          break;
        case "hash":
          this.handlers.hash?.(msg.frame, msg.hash);
          break;
        case "error":
          this.handlers.error?.(msg.message);
          break;
      }
    };
    this.worker.onerror = (event) => this.handlers.error?.(event.message);
  }

  on<K extends keyof Handlers>(event: K, handler: Handlers[K]) {
    this.handlers[event] = handler;
  }

  private send(msg: ToWorker, transfer: Transferable[] = []) {
    this.worker.postMessage(msg, transfer);
  }

  /**
   * Loads a game. On a link, `link` names every console's save and the
   * `unixSeconds` both players agreed on, so the cartridge clocks match.
   */
  load(
    rom: ArrayBuffer,
    save: ArrayBuffer | null,
    bios: ArrayBuffer | null,
    system: System = "gba",
    link?: LinkLoad,
    unixSeconds = Math.floor(Date.now() / 1000),
  ) {
    this.send({ type: "load", rom, save, bios, unixSeconds, system, link }, [rom]);
  }

  run() {
    this.send({ type: "run" });
  }

  pause() {
    this.send({ type: "pause" });
  }

  setKeys(keys: number) {
    this.send({ type: "keys", keys });
  }

  setFastForward(enabled: boolean) {
    this.send({ type: "fastForward", enabled });
  }

  /**
   * Asks the worker to emulate one frame (called from the display loop); on a
   * link, with every player's keys, and whether to keep the state before it.
   */
  requestFrame(keys?: number[], frame?: number, snapshot?: boolean) {
    this.send({ type: "frame", keys, frame, snapshot });
  }

  /** Undoes guessed frames: back to `toFrame`, then forward again with `inputs`. */
  rollback(toFrame: number, inputs: number[][], snapshots: boolean[]) {
    this.send({ type: "rollback", toFrame, inputs, snapshots });
  }

  /** Frames up to this one are final; their kept states can go. */
  confirm(frame: number) {
    this.send({ type: "confirm", frame });
  }

  /** Shows and plays another console of the link. */
  setView(player: number) {
    this.send({ type: "view", player });
  }

  /** 0 = raw colours, 1 = GBA LCD look at `strength` 0-1. */
  setColorCorrection(mode: number, strength: number) {
    this.send({ type: "colors", mode, strength });
  }

  requestSave() {
    this.send({ type: "requestSave" });
  }

  /** Asks for a save state; it arrives through the `state` handler with the slot. */
  saveState(slot: number) {
    this.send({ type: "saveState", slot });
  }

  loadState(data: ArrayBuffer) {
    this.send({ type: "loadState", data }, [data]);
  }

  /** Hands a drawn frame buffer back to the worker for reuse. */
  returnFrame(pixels: ArrayBuffer) {
    this.send({ type: "returnFrame", pixels }, [pixels]);
  }
}
