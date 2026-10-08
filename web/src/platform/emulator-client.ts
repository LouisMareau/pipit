// UI-thread handle on the emulator worker: a small typed event emitter.

import type { FromWorker, ToWorker } from "../types";

type Handlers = {
  frame: (pixels: ArrayBuffer, audio: ArrayBuffer, fps: number) => void;
  loaded: (title: string, gameCode: string) => void;
  save: (data: ArrayBuffer) => void;
  state: (slot: number, data: ArrayBuffer) => void;
  error: (message: string) => void;
};

export class EmulatorClient {
  private worker: Worker;
  private handlers: Partial<Handlers> = {};

  constructor(rewindSeconds: number) {
    // Vite only bundles workers it can see statically, so this exact form matters.
    this.worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
    this.send({ type: "config", rewindSeconds });
    this.worker.onmessage = (event: MessageEvent<FromWorker>) => {
      const msg = event.data;
      switch (msg.type) {
        case "frame":
          this.handlers.frame?.(msg.pixels, msg.audio, msg.fps);
          break;
        case "loaded":
          this.handlers.loaded?.(msg.title, msg.gameCode);
          break;
        case "save":
          this.handlers.save?.(msg.data);
          break;
        case "state":
          this.handlers.state?.(msg.slot, msg.data);
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

  load(rom: ArrayBuffer, save: ArrayBuffer | null, bios: ArrayBuffer | null) {
    const unixSeconds = Math.floor(Date.now() / 1000);
    this.send({ type: "load", rom, save, bios, unixSeconds }, [rom]);
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

  setRewind(enabled: boolean) {
    this.send({ type: "rewind", enabled });
  }

  setRewindSeconds(seconds: number) {
    this.send({ type: "config", rewindSeconds: seconds });
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
