// The emulator runs here, off the UI thread. Frames are requested by the UI's
// display loop (`platform/pacer.ts`), so emulation stays locked to the screen's
// refresh; only fast-forward runs free. Every finished frame (pixels + audio)
// is posted to the UI, and save data whenever the game writes its backup memory.
//
// Nothing on the per-frame path allocates beyond the small audio buffer: frame
// buffers are recycled with the UI thread. (Large per-frame allocations cause
// periodic garbage-collection pauses that show up as stutter.)

import init, { Emulator } from "@wasm/pipit_wasm.js";
import type { FromWorker, LinkLoad, ToWorker } from "../types";

const SAVE_CHECK_FRAMES = 60;
/** On a link, how often the state digest is reported for comparison with the partner's. */
const HASH_FRAMES = 60;

let emulator: Emulator | null = null;
let memory: WebAssembly.Memory | null = null;
let running = false;
let fastForward = false;
let fastForwardTimer: ReturnType<typeof setTimeout> | null = null;
let keys = 0;
/** Several consoles on a link: keys arrive with each frame request, fast-forward is off. */
let linked = false;
/** Rollback: states kept before frames that ran on guessed keys, by frame number. */
const states = new Map<number, Uint8Array>();
const MAX_STATES = 16;
let colorMode = 1;
let colorStrength = 1;
let frameCounter = 0;
const spareBuffers: ArrayBuffer[] = [];

// Frame-rate statistics for the toolbar and for stutter diagnostics.
let fpsWindowStart = 0;
let fpsWindowFrames = 0;
let fps = 0;
let lastFrameAt = 0;
let windowMaxGap = 0;
let maxGapMs = 0;

const post = (msg: FromWorker, transfer: Transferable[] = []) => self.postMessage(msg, transfer);

self.onmessage = async (event: MessageEvent<ToWorker>) => {
  const msg = event.data;
  switch (msg.type) {
    case "load":
      await load(msg.rom, msg.save, msg.bios, msg.unixSeconds, msg.link);
      break;
    case "run":
      running = true;
      lastFrameAt = 0;
      if (fastForward) startFastForward();
      break;
    case "pause":
      running = false;
      stopFastForward();
      break;
    case "frame":
      if (!running || fastForward || !emulator) break;
      if (msg.snapshot && msg.frame !== undefined) keep(msg.frame);
      if (msg.keys) msg.keys.forEach((k, i) => emulator?.set_player_keys(i, k));
      step();
      break;
    case "rollback":
      rollback(msg.toFrame, msg.inputs, msg.snapshots);
      break;
    case "confirm":
      for (const frame of states.keys()) if (frame <= msg.frame) states.delete(frame);
      break;
    case "view":
      emulator?.set_view(msg.player);
      postFrame();
      break;
    case "keys":
      keys = msg.keys;
      if (!linked) emulator?.set_keys(keys);
      break;
    case "fastForward":
      if (linked) break;
      fastForward = msg.enabled;
      if (fastForward && running) startFastForward();
      else stopFastForward();
      break;
    case "colors":
      colorMode = msg.mode;
      colorStrength = msg.strength;
      if (emulator) {
        emulator.set_color_correction(colorMode, colorStrength);
        postFrame();
      }
      break;
    case "requestSave":
      sendSave();
      break;
    case "saveState": {
      const data = emulator?.save_state();
      if (data) post({ type: "state", slot: msg.slot, data: data.buffer as ArrayBuffer }, [data.buffer as ArrayBuffer]);
      break;
    }
    case "loadState":
      try {
        emulator?.load_state(new Uint8Array(msg.data));
        emulator?.refresh_frame();
        postFrame();
      } catch (error) {
        post({ type: "error", message: String(error) });
      }
      break;
    case "returnFrame":
      if (spareBuffers.length < 3) spareBuffers.push(msg.pixels);
      break;
  }
};

async function load(rom: ArrayBuffer, save: ArrayBuffer | null, bios: ArrayBuffer | null, unixSeconds: number, link?: LinkLoad) {
  try {
    if (!memory) {
      const wasm = await init();
      memory = wasm.memory;
    }
    emulator?.free();
    const biosBytes = bios ? new Uint8Array(bios) : undefined;
    linked = link !== undefined && link.players > 1;
    if (link && linked) {
      emulator = Emulator.linked(new Uint8Array(rom), link.players, link.local, biosBytes);
      link.saves.forEach((data, i) => {
        if (data) emulator?.load_player_save_data(i, new Uint8Array(data));
      });
    } else {
      emulator = new Emulator(new Uint8Array(rom), biosBytes);
      if (save) emulator.load_save_data(new Uint8Array(save));
    }
    emulator.set_time(unixSeconds);
    emulator.set_keys(keys);
    emulator.set_color_correction(colorMode, colorStrength);
    frameCounter = 0;
    post({ type: "loaded", title: emulator.title(), gameCode: emulator.game_code() });
  } catch (error) {
    post({ type: "error", message: `Could not start the game: ${String(error)}` });
  }
}

/** Fast-forward: run as many frames as fit in a few milliseconds, then yield. */
function startFastForward() {
  if (fastForwardTimer !== null) return;
  const burst = () => {
    fastForwardTimer = null;
    if (!running || !fastForward || !emulator) return;
    const deadline = performance.now() + 12;
    let n = 0;
    while (performance.now() < deadline && n < 8) {
      step();
      n++;
    }
    fastForwardTimer = setTimeout(burst, 0);
  };
  fastForwardTimer = setTimeout(burst, 0);
}

function stopFastForward() {
  if (fastForwardTimer !== null) clearTimeout(fastForwardTimer);
  fastForwardTimer = null;
}

function step() {
  if (!emulator) return;
  emulator.run_frame();
  frameCounter++;

  const now = performance.now();
  if (lastFrameAt) windowMaxGap = Math.max(windowMaxGap, now - lastFrameAt);
  lastFrameAt = now;
  fpsWindowFrames++;
  if (now - fpsWindowStart >= 1000) {
    fps = Math.round((fpsWindowFrames * 1000) / (now - fpsWindowStart));
    maxGapMs = Math.round(windowMaxGap);
    windowMaxGap = 0;
    fpsWindowStart = now;
    fpsWindowFrames = 0;
  }

  postFrame();
  if (frameCounter % SAVE_CHECK_FRAMES === 0 && emulator.take_save_dirty()) sendSave();
  // No digest while some keys are still guesses: it could be corrected later.
  if (linked && frameCounter % HASH_FRAMES === 0 && states.size === 0) {
    post({ type: "hash", frame: frameCounter, hash: emulator.state_hash() });
  }
}

/** Keeps the current state under `frame` (the number of frames run so far). */
function keep(frame: number) {
  if (!emulator) return;
  states.set(frame, emulator.save_link_state());
  while (states.size > MAX_STATES) states.delete(states.keys().next().value!);
}

/**
 * Goes back to the state kept under `toFrame` and re-runs the frames from
 * there, silently, with the given keys; then shows where that leads.
 */
function rollback(toFrame: number, inputs: number[][], snapshots: boolean[]) {
  const state = states.get(toFrame);
  if (!emulator || !state) {
    post({ type: "error", message: `Cannot roll back to frame ${toFrame}` });
    return;
  }
  emulator.load_link_state(state);
  for (const frame of states.keys()) if (frame >= toFrame) states.delete(frame);
  frameCounter = toFrame;
  inputs.forEach((row, i) => {
    if (snapshots[i]) keep(frameCounter);
    row.forEach((k, player) => emulator?.set_player_keys(player, k));
    emulator?.run_frame_silent();
    frameCounter++;
  });
  emulator.refresh_frame();
  postFrame();
}

function postFrame() {
  if (!emulator || !memory) return;
  const len = emulator.frame_len();
  const src = new Uint8Array(memory.buffer, emulator.frame_ptr(), len);
  const pixels = spareBuffers.pop() ?? new ArrayBuffer(len);
  new Uint8Array(pixels).set(src);
  const audio = emulator.drain_audio().buffer as ArrayBuffer;
  post({ type: "frame", pixels, audio, fps, maxGapMs }, [pixels, audio]);
}

function sendSave() {
  const data = emulator?.save_data()?.buffer as ArrayBuffer | undefined;
  if (data) post({ type: "save", data }, [data]);
}
