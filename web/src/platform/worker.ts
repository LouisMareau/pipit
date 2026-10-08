// The emulator runs here, off the UI thread. It paces itself against wall-clock
// time, posts every finished frame (pixels + audio) to the UI, and reports save
// data whenever the game writes to its backup memory.

import init, { Emulator } from "@wasm/pipit_wasm.js";
import type { FromWorker, ToWorker } from "../types";

const FRAME_MS = 1000 / (16_777_216 / 280_896); // 59.7275 Hz
const MAX_CATCH_UP = 3;
const SAVE_CHECK_FRAMES = 60;

let emulator: Emulator | null = null;
let memory: WebAssembly.Memory | null = null;
let running = false;
let fastForward = false;
let keys = 0;
let nextFrameAt = 0;
let frameCounter = 0;
let fpsWindowStart = 0;
let fpsWindowFrames = 0;
let fps = 0;
let timer: ReturnType<typeof setTimeout> | null = null;
const spareBuffers: ArrayBuffer[] = [];

const post = (msg: FromWorker, transfer: Transferable[] = []) => self.postMessage(msg, transfer);

self.onmessage = async (event: MessageEvent<ToWorker>) => {
  const msg = event.data;
  switch (msg.type) {
    case "load":
      await load(msg.rom, msg.save, msg.bios, msg.unixSeconds);
      break;
    case "run":
      if (!running && emulator) {
        running = true;
        nextFrameAt = performance.now();
        schedule(0);
      }
      break;
    case "pause":
      running = false;
      if (timer !== null) clearTimeout(timer);
      timer = null;
      break;
    case "keys":
      keys = msg.keys;
      emulator?.set_keys(keys);
      break;
    case "fastForward":
      fastForward = msg.enabled;
      break;
    case "requestSave":
      sendSave();
      break;
    case "returnFrame":
      if (spareBuffers.length < 3) spareBuffers.push(msg.pixels);
      break;
  }
};

async function load(rom: ArrayBuffer, save: ArrayBuffer | null, bios: ArrayBuffer | null, unixSeconds: number) {
  try {
    if (!memory) {
      const wasm = await init();
      memory = wasm.memory;
    }
    emulator?.free();
    emulator = new Emulator(new Uint8Array(rom), bios ? new Uint8Array(bios) : undefined);
    if (save) emulator.load_save_data(new Uint8Array(save));
    emulator.set_time(unixSeconds);
    emulator.set_keys(keys);
    frameCounter = 0;
    post({ type: "loaded", title: emulator.title(), gameCode: emulator.game_code() });
  } catch (error) {
    post({ type: "error", message: `Could not start the game: ${String(error)}` });
  }
}

function schedule(delay: number) {
  timer = setTimeout(tick, delay);
}

function tick() {
  if (!running || !emulator) return;
  const now = performance.now();
  if (fastForward) {
    // Run as many frames as fit in a few milliseconds, then yield.
    const deadline = now + 12;
    let n = 0;
    while (performance.now() < deadline && n < 8) {
      step();
      n++;
    }
    nextFrameAt = performance.now();
    schedule(0);
    return;
  }
  let frames = 0;
  while (now >= nextFrameAt && frames < MAX_CATCH_UP) {
    step();
    nextFrameAt += FRAME_MS;
    frames++;
  }
  // Too far behind (tab was hidden): resynchronise rather than sprinting.
  if (now - nextFrameAt > FRAME_MS * MAX_CATCH_UP) nextFrameAt = now;
  schedule(Math.max(0, nextFrameAt - performance.now()));
}

function step() {
  if (!emulator || !memory) return;
  emulator.run_frame();
  frameCounter++;

  const now = performance.now();
  fpsWindowFrames++;
  if (now - fpsWindowStart >= 1000) {
    fps = Math.round((fpsWindowFrames * 1000) / (now - fpsWindowStart));
    fpsWindowStart = now;
    fpsWindowFrames = 0;
  }

  const len = emulator.frame_len();
  const src = new Uint8Array(memory.buffer, emulator.frame_ptr(), len);
  const pixels = spareBuffers.pop() ?? new ArrayBuffer(len);
  new Uint8Array(pixels).set(src);
  const audio = emulator.drain_audio().buffer as ArrayBuffer;
  post({ type: "frame", pixels, audio, fps }, [pixels, audio]);

  if (frameCounter % SAVE_CHECK_FRAMES === 0 && emulator.take_save_dirty()) sendSave();
}

function sendSave() {
  const data = emulator?.save_data()?.buffer as ArrayBuffer | undefined;
  if (data) post({ type: "save", data }, [data]);
}
