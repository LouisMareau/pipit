// Frame pacing locked to the display.
//
// The GBA draws 59.7275 frames per second; a 60 Hz display refreshes 60 times.
// Pacing the emulator by the wall clock therefore shows one frame twice every
// ~3.7 s: a periodic judder. Instead, `requestAnimationFrame` drives emulation:
// one emulated frame per refresh on a 60 Hz display (every second refresh on
// 120 Hz, and so on), which runs the game 0.46 % fast — a difference the audio
// output absorbs by resampling (see `public/audio-worklet.js`). Displays whose
// rate is not a multiple of ~60 Hz fall back to time-based pacing.

const FRAME_MS = 1000 / (16_777_216 / 280_896); // 59.7275 Hz

export class FramePacer {
  private rafId: number | null = null;
  private lastTime = 0;
  private intervals: number[] = [];
  private refreshes = 0;
  private accumulated = 0;
  /** Called once per emulated frame that is due. */
  onFrame: () => void = () => {};

  start() {
    if (this.rafId !== null) return;
    this.lastTime = 0;
    this.accumulated = 0;
    this.refreshes = 0;
    this.rafId = requestAnimationFrame(this.loop);
  }

  stop() {
    if (this.rafId !== null) cancelAnimationFrame(this.rafId);
    this.rafId = null;
  }

  private loop = (time: number) => {
    this.rafId = requestAnimationFrame(this.loop);
    if (!this.lastTime) {
      this.lastTime = time;
      this.onFrame();
      return;
    }
    const dt = time - this.lastTime;
    this.lastTime = time;
    if (dt <= 0) return;
    if (dt < 200) {
      this.intervals.push(dt);
      if (this.intervals.length > 90) this.intervals.shift();
    }

    const refresh = this.medianInterval();
    const ratio = FRAME_MS / refresh; // 1.005 on 60 Hz, 2.009 on 120 Hz
    const n = Math.round(ratio);
    if (this.intervals.length >= 30 && n >= 1 && Math.abs(ratio - n) < 0.02 * n) {
      // Display-locked: exactly one emulated frame every n refreshes.
      this.accumulated = 0;
      if (++this.refreshes >= n) {
        this.refreshes = 0;
        this.onFrame();
      }
      return;
    }
    // Free-running display: emulate by elapsed time, never sprinting after a stall.
    this.accumulated = Math.min(this.accumulated + dt, FRAME_MS * 2);
    while (this.accumulated >= FRAME_MS) {
      this.accumulated -= FRAME_MS;
      this.onFrame();
    }
  };

  private medianInterval(): number {
    if (this.intervals.length === 0) return 1000 / 60;
    const sorted = [...this.intervals].sort((a, b) => a - b);
    return sorted[sorted.length >> 1]!;
  }
}
