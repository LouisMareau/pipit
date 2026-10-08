// AudioWorklet processor: plays the emulator's 32768 Hz stereo samples.
//
// Samples arrive through the port as Int16Array chunks and sit in a ring buffer.
// The processor resamples linearly to the context rate and fades to silence when
// the buffer runs dry, so stalls click as little as possible.
// Plain JS on purpose: worklet modules are loaded by URL, outside the bundler.

const SOURCE_RATE = 32768;
const CAPACITY = SOURCE_RATE / 4; // 250 ms of stereo frames

class PipitProcessor extends AudioWorkletProcessor {
  constructor() {
    super();
    this.left = new Float32Array(CAPACITY);
    this.right = new Float32Array(CAPACITY);
    this.readPos = 0; // fractional read index into the ring
    this.writePos = 0;
    this.available = 0;
    this.volume = 1;
    this.lastL = 0;
    this.lastR = 0;
    this.port.onmessage = (event) => {
      const msg = event.data;
      if (msg.type === "samples") this.push(msg.samples);
      else if (msg.type === "volume") this.volume = msg.volume;
      else if (msg.type === "clear") this.available = 0;
    };
  }

  push(samples) {
    const frames = samples.length >> 1;
    // Drop the oldest audio if the emulator runs ahead of playback.
    if (this.available + frames > CAPACITY) {
      const drop = this.available + frames - CAPACITY;
      this.readPos = (this.readPos + drop) % CAPACITY;
      this.available -= drop;
    }
    for (let i = 0; i < frames; i++) {
      this.left[this.writePos] = samples[2 * i] / 32768;
      this.right[this.writePos] = samples[2 * i + 1] / 32768;
      this.writePos = (this.writePos + 1) % CAPACITY;
    }
    this.available += frames;
    this.port.postMessage({ type: "level", frames: this.available });
  }

  process(_inputs, outputs) {
    const out = outputs[0];
    const outL = out[0];
    const outR = out[1] ?? out[0];
    const step = SOURCE_RATE / sampleRate;
    for (let i = 0; i < outL.length; i++) {
      if (this.available >= 2) {
        const idx = Math.floor(this.readPos);
        const frac = this.readPos - idx;
        const next = (idx + 1) % CAPACITY;
        this.lastL = (this.left[idx] * (1 - frac) + this.left[next] * frac) * this.volume;
        this.lastR = (this.right[idx] * (1 - frac) + this.right[next] * frac) * this.volume;
        this.readPos += step;
        const consumed = Math.floor(this.readPos) - idx;
        this.available -= consumed;
        if (this.readPos >= CAPACITY) this.readPos -= CAPACITY;
      } else {
        // Underrun: decay towards silence instead of repeating a sample.
        this.lastL *= 0.98;
        this.lastR *= 0.98;
      }
      outL[i] = this.lastL;
      outR[i] = this.lastR;
    }
    return true;
  }
}

registerProcessor("pipit-audio", PipitProcessor);
